// SPDX-License-Identifier: GPL-2.0-only
//! Primitive encoders shared by the heaps, the signature blobs and the tables.
//!
//! Everything here is little-endian except the compressed integers, which
//! ECMA-335 II.23.2 defines big-endian so that the leading bits act as a
//! self-describing length prefix.

use crate::error::{Error, Result};

/// Largest value ECMA-335's compressed unsigned integer encoding can hold.
pub const MAX_COMPRESSED_UNSIGNED: u32 = 0x1FFF_FFFF;

/// Appends `value` to `out` in ECMA-335 II.23.2 compressed unsigned form.
///
/// # Errors
///
/// Returns [`Error::CompressedRange`] if `value` exceeds
/// [`MAX_COMPRESSED_UNSIGNED`]; the encoding steals the top three bits for the
/// length tag, so larger values are not representable at all.
pub fn compress_unsigned(out: &mut Vec<u8>, value: u32) -> Result<()> {
    // Selecting big-endian bytes rather than shifting into `u8` keeps every
    // narrowing conversion provably lossless.
    let b = value.to_be_bytes();
    if value < 0x80 {
        out.push(b[3]);
    } else if value < 0x4000 {
        out.push(0x80 | b[2]);
        out.push(b[3]);
    } else if value <= MAX_COMPRESSED_UNSIGNED {
        out.push(0xC0 | b[0]);
        out.push(b[1]);
        out.push(b[2]);
        out.push(b[3]);
    } else {
        return Err(Error::CompressedRange { value });
    }
    Ok(())
}

/// Appends `value` to `out` in ECMA-335 II.23.2 compressed *signed* form.
///
/// The sign bit is rotated into bit 0 so small negatives stay one byte. Only
/// `ArrayShape.LoBounds` needs this encoding.
///
/// # Errors
///
/// Returns [`Error::CompressedSignedRange`] if `value` falls outside
/// `-2^28 ..= 2^28 - 1`.
pub fn compress_signed(out: &mut Vec<u8>, value: i32) -> Result<()> {
    const B6: i32 = (1 << 6) - 1;
    const B13: i32 = (1 << 13) - 1;
    const B28: i32 = (1 << 28) - 1;

    // An arithmetic right shift gives all-ones for negatives and all-zeros
    // otherwise, which is exactly the sign bit that belongs in bit 0.
    let sign = value >> 31;
    if (value & !B6) == (sign & !B6) {
        let n = (((value & B6) << 1) | (sign & 1)).cast_unsigned();
        out.push(n.to_be_bytes()[3]);
    } else if (value & !B13) == (sign & !B13) {
        let n = (0x8000 | ((value & B13) << 1) | (sign & 1)).cast_unsigned();
        let b = n.to_be_bytes();
        out.push(b[2]);
        out.push(b[3]);
    } else if (value & !B28) == (sign & !B28) {
        let n = 0xC000_0000_u32 | (((value & B28) << 1) | (sign & 1)).cast_unsigned();
        out.extend_from_slice(&n.to_be_bytes());
    } else {
        return Err(Error::CompressedSignedRange { value });
    }
    Ok(())
}

/// Appends a little-endian `u16`.
pub(crate) fn write_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends a little-endian `u32`.
pub(crate) fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends a little-endian `u64`.
pub(crate) fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends a metadata index that occupies `width` bytes (2 or 4).
///
/// # Errors
///
/// Returns [`Error::IndexTooWide`] if a 2-byte column was asked to hold a
/// value above `u16::MAX`. That can only happen if a width was computed before
/// the row counts were final, which is the single most damaging bug class in
/// a metadata writer, so it is checked rather than truncated.
pub(crate) fn write_index(out: &mut Vec<u8>, width: u8, value: u32) -> Result<()> {
    if width == 2 {
        let narrow = u16::try_from(value).map_err(|_| Error::IndexTooWide { value })?;
        write_u16(out, narrow);
    } else {
        write_u32(out, value);
    }
    Ok(())
}

/// Rounds `value` up to a multiple of `alignment`, or `None` on overflow.
pub(crate) fn align_up(value: usize, alignment: usize) -> Option<usize> {
    let bump = value.checked_add(alignment.checked_sub(1)?)?;
    bump.checked_div(alignment)?.checked_mul(alignment)
}

/// Zero-pads `buf` so its length is a multiple of 4.
///
/// Every metadata stream is 4-byte aligned and the padding counts towards the
/// size recorded in its stream header.
pub(crate) fn pad_to_four(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::{align_up, compress_signed, compress_unsigned};

    fn cu(value: u32) -> Vec<u8> {
        let mut out = Vec::new();
        compress_unsigned(&mut out, value).unwrap();
        out
    }

    fn cs(value: i32) -> Vec<u8> {
        let mut out = Vec::new();
        compress_signed(&mut out, value).unwrap();
        out
    }

    #[test]
    fn compressed_unsigned_matches_spec_examples() {
        assert_eq!(cu(0x00), vec![0x00]);
        assert_eq!(cu(0x03), vec![0x03]);
        assert_eq!(cu(0x7F), vec![0x7F]);
        assert_eq!(cu(0x80), vec![0x80, 0x80]);
        assert_eq!(cu(0x2E57), vec![0xAE, 0x57]);
        assert_eq!(cu(0x3FFF), vec![0xBF, 0xFF]);
        assert_eq!(cu(0x4000), vec![0xC0, 0x00, 0x40, 0x00]);
        assert_eq!(cu(0x1FFF_FFFF), vec![0xDF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn compressed_unsigned_rejects_out_of_range() {
        let mut out = Vec::new();
        assert!(compress_unsigned(&mut out, 0x2000_0000).is_err());
    }

    #[test]
    fn compressed_signed_matches_spec_examples() {
        assert_eq!(cs(3), vec![0x06]);
        assert_eq!(cs(-3), vec![0x7B]);
        assert_eq!(cs(64), vec![0x80, 0x80]);
        assert_eq!(cs(-64), vec![0x01]);
        assert_eq!(cs(8192), vec![0xC0, 0x00, 0x40, 0x00]);
        assert_eq!(cs(-8192), vec![0x80, 0x01]);
        assert_eq!(cs(268_435_455), vec![0xDF, 0xFF, 0xFF, 0xFE]);
        assert_eq!(cs(-268_435_456), vec![0xC0, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn align_up_rounds_and_detects_overflow() {
        assert_eq!(align_up(0, 512), Some(0));
        assert_eq!(align_up(1, 512), Some(512));
        assert_eq!(align_up(512, 512), Some(512));
        assert_eq!(align_up(513, 512), Some(1024));
        assert_eq!(align_up(usize::MAX, 512), None);
    }
}
