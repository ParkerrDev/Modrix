// SPDX-License-Identifier: GPL-2.0-only
//! A bounded little-endian reader over an in-memory metadata blob.
//!
//! Every accessor is fallible and range-checked. `global-metadata.dat` is
//! attacker-reachable in the sense that matters here - it is a 26 MB binary
//! from a game directory Modrix does not control, and a truncated or hostile
//! one must produce an error, never a panic or an out-of-bounds read. Slicing
//! is done with `get`, never `[..]`, so a malformed offset returns `None`
//! rather than aborting the process.

use crate::error::{Error, Result};

/// A read-only cursor over the metadata blob.
#[derive(Debug, Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Wrap a metadata blob.
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Read a little-endian `i32` at `offset`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if four bytes are not available there.
    pub fn i32_at(&self, offset: usize) -> Result<i32> {
        let end = offset.checked_add(4).ok_or(Error::Truncated {
            offset,
            need: 4,
            have: self.data.len(),
        })?;
        let bytes = self
            .data
            .get(offset..end)
            .ok_or(Error::Truncated {
                offset,
                need: 4,
                have: self.data.len(),
            })?
            .try_into()
            .map_err(|_| Error::Truncated {
                offset,
                need: 4,
                have: self.data.len(),
            })?;
        Ok(i32::from_le_bytes(bytes))
    }

    /// Read a little-endian `u32` at `offset`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if four bytes are not available there.
    pub fn u32_at(&self, offset: usize) -> Result<u32> {
        self.i32_at(offset).map(i32::cast_unsigned)
    }

    /// Read a little-endian `u16` at `offset`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if two bytes are not available there.
    pub fn u16_at(&self, offset: usize) -> Result<u16> {
        let end = offset.checked_add(2).ok_or(Error::Truncated {
            offset,
            need: 2,
            have: self.data.len(),
        })?;
        let bytes = self
            .data
            .get(offset..end)
            .ok_or(Error::Truncated {
                offset,
                need: 2,
                have: self.data.len(),
            })?
            .try_into()
            .map_err(|_| Error::Truncated {
                offset,
                need: 2,
                have: self.data.len(),
            })?;
        Ok(u16::from_le_bytes(bytes))
    }

    /// Read a single byte at `offset`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the offset is past the end.
    pub fn u8_at(&self, offset: usize) -> Result<u8> {
        self.data.get(offset).copied().ok_or(Error::Truncated {
            offset,
            need: 1,
            have: self.data.len(),
        })
    }

    /// Read a little-endian `u64` at `offset`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if eight bytes are not available there.
    pub fn u64_at(&self, offset: usize) -> Result<u64> {
        let lo = u64::from(self.u32_at(offset)?);
        let hi = u64::from(self.u32_at(offset.saturating_add(4))?);
        Ok((hi << 32) | lo)
    }

    /// Borrow the `len` bytes at `offset`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if that range is not wholly inside the blob.
    pub fn slice(&self, offset: usize, len: usize) -> Result<&'a [u8]> {
        let end = offset.checked_add(len).ok_or(Error::Truncated {
            offset,
            need: len,
            have: self.data.len(),
        })?;
        self.data.get(offset..end).ok_or(Error::Truncated {
            offset,
            need: len,
            have: self.data.len(),
        })
    }

    /// Read a fixed-width, NUL-padded field as a string (PE section names).
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the field lies outside the blob.
    pub fn fixed_str(&self, offset: usize, len: usize) -> Result<String> {
        let raw = self.slice(offset, len)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Ok(String::from_utf8_lossy(raw.get(..end).unwrap_or(&[])).into_owned())
    }

    /// Read the NUL-terminated UTF-8 string starting at `offset`.
    ///
    /// IL2CPP's string table is a single blob of NUL-terminated names indexed
    /// by byte offset. Names are ordinary identifiers, but a corrupt table
    /// could point mid-sequence, so decoding is lossy rather than fallible -
    /// a mangled name is far better than refusing to read the whole game.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if `offset` is past the end of the blob, or
    /// [`Error::Unterminated`] if no NUL follows within the bound.
    pub fn cstr_at(&self, offset: usize) -> Result<String> {
        let rest = self.data.get(offset..).ok_or(Error::Truncated {
            offset,
            need: 1,
            have: self.data.len(),
        })?;
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::Unterminated { offset })?;
        let bytes = rest.get(..end).unwrap_or(&[]);
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_scalars() {
        let data = [0x01, 0x02, 0x03, 0x04, 0xFF, 0xFF];
        let r = Reader::new(&data);
        assert_eq!(r.i32_at(0).unwrap(), 0x0403_0201);
        assert_eq!(r.u16_at(4).unwrap(), 0xFFFF);
    }

    #[test]
    fn refuses_to_read_past_the_end() {
        let data = [0x01, 0x02];
        let r = Reader::new(&data);
        assert!(matches!(r.i32_at(0), Err(Error::Truncated { .. })));
        assert!(matches!(r.i32_at(usize::MAX), Err(Error::Truncated { .. })));
        assert!(matches!(r.u16_at(1), Err(Error::Truncated { .. })));
    }

    #[test]
    fn reads_nul_terminated_strings() {
        let data = b"System.Object\0Foo\0";
        let r = Reader::new(data);
        assert_eq!(r.cstr_at(0).unwrap(), "System.Object");
        assert_eq!(r.cstr_at(14).unwrap(), "Foo");
    }

    #[test]
    fn rejects_an_unterminated_string() {
        let data = b"no terminator";
        let r = Reader::new(data);
        assert!(matches!(r.cstr_at(0), Err(Error::Unterminated { .. })));
    }

    #[test]
    fn decodes_invalid_utf8_lossily_rather_than_failing() {
        // A corrupt name must not make the whole game unreadable.
        let data = b"bad\xFFname\0";
        let r = Reader::new(data);
        assert!(r.cstr_at(0).unwrap().contains("bad"));
    }
}
