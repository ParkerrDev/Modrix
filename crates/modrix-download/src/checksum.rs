// SPDX-License-Identifier: GPL-2.0-only
//! Streaming integrity verification (MD5 / SHA-256, both RustCrypto).
//!
//! A finished download is hashed and compared against the expected digest before
//! it is renamed into place. A bad file is never accepted (the same guarantee the
//! single-connection downloader gave).

use std::fmt::Write as _;
use std::path::Path;

use md5::Md5;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::error::{Error, Result};
use crate::types::Checksum;

/// Read buffer for streaming hashing - O(1) memory regardless of file size.
const HASH_BUF_LEN: usize = 64 * 1024;

/// Verify `path` against `expected`, returning [`Error::Checksum`] on mismatch.
///
/// # Errors
///
/// Returns [`Error::Io`] if the file cannot be read, or [`Error::Checksum`] if
/// the digest differs.
pub(crate) async fn verify(path: &Path, expected: &Checksum) -> Result<()> {
    let (actual, want) = match expected {
        Checksum::Md5(want) => (hash_file::<Md5>(path).await?, want),
        Checksum::Sha256(want) => (hash_file::<Sha256>(path).await?, want),
    };
    if actual.eq_ignore_ascii_case(want) {
        Ok(())
    } else {
        Err(Error::Checksum {
            expected: want.clone(),
            actual,
        })
    }
}

/// Stream-hash a file with digest `D`, returning lowercase hex.
async fn hash_file<D: Digest>(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| Error::io(path, e))?;
    let mut hasher = D::new();
    let mut buf = vec![0_u8; HASH_BUF_LEN];
    loop {
        let n = file.read(&mut buf).await.map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(buf.get(..n).unwrap_or_default());
    }
    Ok(hex(hasher.finalize().as_slice()))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn write(path: &Path, bytes: &[u8]) {
        tokio::fs::write(path, bytes).await.unwrap();
    }

    #[tokio::test]
    async fn md5_and_sha256_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        write(&f, b"hello world").await;
        // Known digests of "hello world".
        verify(
            &f,
            &Checksum::Md5("5eb63bbbe01eeed093cb22bb8f5acdc3".into()),
        )
        .await
        .unwrap();
        verify(
            &f,
            &Checksum::Sha256(
                "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".into(),
            ),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn mismatch_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        write(&f, b"hello world").await;
        let err = verify(&f, &Checksum::Md5("deadbeef".into()))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Checksum { .. }));
    }

    #[tokio::test]
    async fn is_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        write(&f, b"hello world").await;
        verify(
            &f,
            &Checksum::Md5("5EB63BBBE01EEED093CB22BB8F5ACDC3".into()),
        )
        .await
        .unwrap();
    }
}
