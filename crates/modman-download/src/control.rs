// SPDX-License-Identifier: GPL-2.0-only
//! The `<dest>.mmdl` resume control file.
//!
//! Records the piece size, total length, a remote validator (`ETag` or
//! `Last-Modified`), and a packed bitfield of completed pieces. Written atomically
//! (temp + rename) periodically and on stop, and deleted on success. On resume
//! the validator is checked against a fresh probe so a changed remote resource
//! restarts cleanly rather than corrupting the file.
//!
//! It is a small JSON envelope with a hex-packed bitfield - compact enough (one
//! bit per piece) and far simpler to keep correct than a hand-rolled binary
//! format.

use std::path::{Path, PathBuf};

use crate::bits;
use crate::error::{Error, Result};

/// Cap on piece count so an adversarial `Content-Length` cannot make us allocate
/// unboundedly (Power of Ten). 16M pieces × 1 MiB ≈ 16 TiB.
const MAX_PIECES: u64 = 16_000_000;

const MAGIC: &str = "MMDL";
const VERSION: u32 = 1;

/// The live resume state for one download.
#[derive(Debug, Clone)]
pub(crate) struct Control {
    pub piece_len: u32,
    pub total: u64,
    pub validator: String,
    pub single_stream: bool,
    /// One bool per piece; `true` ⇒ that piece is fully on disk.
    pub done: Vec<bool>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OnDisk {
    magic: String,
    version: u32,
    piece_len: u32,
    total: u64,
    validator: String,
    single_stream: bool,
    num_pieces: u64,
    bitfield_hex: String,
}

impl Control {
    /// A fresh control for a download of `total` bytes at `piece_len`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BoundExceeded`] if the piece count exceeds [`MAX_PIECES`].
    pub(crate) fn fresh(
        piece_len: u32,
        total: u64,
        validator: String,
        single_stream: bool,
    ) -> Result<Self> {
        let num_pieces = piece_count(total, piece_len, single_stream)?;
        Ok(Self {
            piece_len,
            total,
            validator,
            single_stream,
            done: vec![false; num_pieces],
        })
    }

    /// Whether every piece is complete.
    pub(crate) fn is_complete(&self) -> bool {
        self.done.iter().all(|&d| d)
    }

    /// Bytes on disk = complete-piece count × piece size (last piece clamped).
    pub(crate) fn bytes_done(&self) -> u64 {
        let piece = u64::from(self.piece_len);
        let mut sum = 0_u64;
        for (index, &is_done) in self.done.iter().enumerate() {
            if is_done {
                sum = sum.saturating_add(self.piece_bytes(index, piece));
            }
        }
        sum
    }

    /// The byte length of piece `index` (the last piece may be short).
    fn piece_bytes(&self, index: usize, piece: u64) -> u64 {
        let start = (index as u64).saturating_mul(piece);
        self.total.saturating_sub(start).min(piece)
    }

    /// Whether a fresh probe matches this control (else resume must restart).
    pub(crate) fn matches(&self, piece_len: u32, total: u64, validator: &str) -> bool {
        self.piece_len == piece_len && self.total == total && self.validator == validator
    }

    /// Load a control file, returning `None` if it does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ControlFile`] if it exists but is malformed.
    pub(crate) async fn load(path: &Path) -> Result<Option<Self>> {
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::io(path, e)),
        };
        let disk: OnDisk =
            serde_json::from_slice(&bytes).map_err(|e| Error::ControlFile(e.to_string()))?;
        if disk.magic != MAGIC || disk.version != VERSION {
            return Err(Error::ControlFile("bad magic or version".to_owned()));
        }
        if disk.num_pieces > MAX_PIECES {
            return Err(Error::BoundExceeded {
                what: "control pieces",
                limit: MAX_PIECES,
            });
        }
        let num_pieces = usize::try_from(disk.num_pieces)
            .map_err(|_| Error::ControlFile("piece count".into()))?;
        let done = bits::unpack_hex(&disk.bitfield_hex, num_pieces)
            .ok_or_else(|| Error::ControlFile("bad bitfield".to_owned()))?;
        Ok(Some(Self {
            piece_len: disk.piece_len,
            total: disk.total,
            validator: disk.validator,
            single_stream: disk.single_stream,
            done,
        }))
    }

    /// Atomically persist the control file (temp + rename).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`]/[`Error::ControlFile`] on write/serialize failure.
    pub(crate) async fn save(&self, path: &Path) -> Result<()> {
        let disk = OnDisk {
            magic: MAGIC.to_owned(),
            version: VERSION,
            piece_len: self.piece_len,
            total: self.total,
            validator: self.validator.clone(),
            single_stream: self.single_stream,
            num_pieces: self.done.len() as u64,
            bitfield_hex: bits::pack_hex(&self.done),
        };
        let bytes = serde_json::to_vec(&disk).map_err(|e| Error::ControlFile(e.to_string()))?;
        let tmp = temp_path(path);
        tokio::fs::write(&tmp, &bytes)
            .await
            .map_err(|e| Error::io(&tmp, e))?;
        tokio::fs::rename(&tmp, path)
            .await
            .map_err(|e| Error::io(path, e))?;
        Ok(())
    }

    /// Delete the control file (called on success). Absent is fine.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on a removal failure other than "not found".
    pub(crate) async fn remove(path: &Path) -> Result<()> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(path, e)),
        }
    }
}

/// The number of pieces for `total`/`piece_len` (0 in single-stream mode).
fn piece_count(total: u64, piece_len: u32, single_stream: bool) -> Result<usize> {
    if single_stream {
        return Ok(0);
    }
    let piece = u64::from(piece_len.max(1));
    let count = total.div_ceil(piece);
    if count > MAX_PIECES {
        return Err(Error::BoundExceeded {
            what: "download pieces",
            limit: MAX_PIECES,
        });
    }
    usize::try_from(count).map_err(|_| Error::BoundExceeded {
        what: "download pieces",
        limit: MAX_PIECES,
    })
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.mmdl");
        let mut c = Control::fresh(1024, 4096, "etag-1".to_owned(), false).unwrap();
        assert_eq!(c.done.len(), 4);
        c.done[0] = true;
        c.done[2] = true;
        c.save(&path).await.unwrap();

        let loaded = Control::load(&path).await.unwrap().unwrap();
        assert_eq!(loaded.done, vec![true, false, true, false]);
        assert!(loaded.matches(1024, 4096, "etag-1"));
        assert!(!loaded.matches(1024, 4096, "etag-2"));
        assert_eq!(loaded.bytes_done(), 2048);
    }

    #[tokio::test]
    async fn absent_control_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Control::load(&dir.path().join("nope.mmdl"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn corrupt_control_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.mmdl");
        tokio::fs::write(&path, b"not json").await.unwrap();
        assert!(matches!(
            Control::load(&path).await,
            Err(Error::ControlFile(_))
        ));
    }

    #[test]
    fn last_piece_is_clamped() {
        // 4100 bytes at 1024 => 5 pieces, last is 4 bytes.
        let mut c = Control::fresh(1024, 4100, String::new(), false).unwrap();
        assert_eq!(c.done.len(), 5);
        for d in &mut c.done {
            *d = true;
        }
        assert_eq!(c.bytes_done(), 4100);
    }
}
