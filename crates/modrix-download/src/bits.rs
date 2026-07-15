// SPDX-License-Identifier: GPL-2.0-only
//! A compact MSB-first packing of a per-piece "done" boolean vector, plus hex.
//!
//! The in-memory representation stays a plain `Vec<bool>` (trivial and
//! lint-clean); this module only packs/unpacks it for the on-disk control file,
//! so the bit-twiddling happens once per save/load, never per byte transferred.
//! All indexing is bounds-checked and all shifts are non-panicking (Power of Ten:
//! no panics, no naked arithmetic).

/// Pack a per-piece done vector into a lowercase-hex, MSB-first bitfield.
pub(crate) fn pack_hex(done: &[bool]) -> String {
    let byte_count = done.len().div_ceil(8);
    let mut bytes = vec![0_u8; byte_count];
    for (index, &is_done) in done.iter().enumerate() {
        if !is_done {
            continue;
        }
        let (byte, mask) = locate(index);
        if let Some(slot) = bytes.get_mut(byte) {
            *slot |= mask;
        }
    }
    to_hex(&bytes)
}

/// Unpack a hex bitfield into a `num_pieces`-long done vector.
pub(crate) fn unpack_hex(hex: &str, num_pieces: usize) -> Option<Vec<bool>> {
    let bytes = from_hex(hex)?;
    let mut done = vec![false; num_pieces];
    for index in 0..num_pieces {
        let (byte, mask) = locate(index);
        let set = bytes.get(byte).is_some_and(|b| b & mask != 0);
        if let Some(slot) = done.get_mut(index) {
            *slot = set;
        }
    }
    Some(done)
}

/// `(byte index, MSB-first mask)` for bit `index`.
fn locate(index: usize) -> (usize, u8) {
    let byte = index.wrapping_shr(3);
    let within = u32::try_from(index & 7).unwrap_or(0);
    (byte, 0x80_u8.wrapping_shr(within))
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(hex.len().wrapping_shr(1));
    let mut index = 0;
    while let (Some(&hi), Some(&lo)) = (bytes.get(index), bytes.get(index.wrapping_add(1))) {
        let high = hex_val(hi)?;
        let low = hex_val(lo)?;
        out.push(high.wrapping_shl(4).wrapping_add(low));
        index = index.wrapping_add(2);
    }
    Some(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b.wrapping_sub(b'0')),
        b'a'..=b'f' => Some(b.wrapping_sub(b'a').wrapping_add(10)),
        b'A'..=b'F' => Some(b.wrapping_sub(b'A').wrapping_add(10)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_arbitrary_done_vectors() {
        for pieces in [0_usize, 1, 7, 8, 9, 63, 64, 1000] {
            let done: Vec<bool> = (0..pieces).map(|i| i % 3 == 0).collect();
            let hex = pack_hex(&done);
            let back = unpack_hex(&hex, pieces).unwrap();
            assert_eq!(done, back, "pieces={pieces}");
        }
    }

    #[test]
    fn rejects_odd_length_hex() {
        assert!(unpack_hex("abc", 4).is_none());
    }

    #[test]
    fn msb_first_layout() {
        // bit 0 set => most-significant bit of byte 0 => 0x80.
        let done = vec![true, false, false, false, false, false, false, false];
        assert_eq!(pack_hex(&done), "80");
    }
}
