// SPDX-License-Identifier: GPL-2.0-only
//! The four metadata heaps: `#Strings`, `#Blob`, `#GUID` and `#US`.
//!
//! Each heap is append-only and deduplicating. The two byte-addressed heaps
//! (`#Strings`, `#Blob`) both begin with a single `0x00` byte so that index 0
//! means "the empty string" / "the empty blob" - metadata uses index 0 as the
//! neutral value for `Culture`, `PublicKey`, `HashValue` and a nested type's
//! namespace, so that first byte is load-bearing, not padding.

use std::collections::HashMap;

use crate::encode::compress_unsigned;
use crate::error::{Error, Result};

/// Ceiling on any one heap. Well under `u32::MAX` so every offset stays a
/// valid 4-byte index with room to spare.
const HEAP_LIMIT: usize = 0x1000_0000;

/// The `#Strings` heap: NUL-terminated UTF-8, indexed by byte offset.
#[derive(Debug)]
pub(crate) struct StringHeap {
    data: Vec<u8>,
    seen: HashMap<String, u32>,
}

impl StringHeap {
    /// Creates a heap whose index 0 is already the empty string.
    pub(crate) fn new() -> Self {
        let mut seen = HashMap::new();
        seen.insert(String::new(), 0_u32);
        Self {
            data: vec![0],
            seen,
        }
    }

    /// Interns `value`, returning its byte offset.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NulInString`] if `value` contains an interior NUL (the
    /// heap is NUL-terminated, so it would be silently truncated) or
    /// [`Error::HeapOverflow`] if the heap would exceed its ceiling.
    pub(crate) fn intern(&mut self, value: &str) -> Result<u32> {
        if let Some(offset) = self.seen.get(value) {
            return Ok(*offset);
        }
        if value.as_bytes().contains(&0) {
            return Err(Error::NulInString {
                name: value.escape_default().to_string(),
            });
        }
        let offset = u32::try_from(self.data.len()).map_err(|_| Error::HeapOverflow {
            heap: "#Strings",
            size: self.data.len(),
            limit: HEAP_LIMIT,
        })?;
        self.data.extend_from_slice(value.as_bytes());
        self.data.push(0);
        self.check_limit("#Strings")?;
        self.seen.insert(value.to_owned(), offset);
        Ok(offset)
    }

    fn check_limit(&self, heap: &'static str) -> Result<()> {
        if self.data.len() > HEAP_LIMIT {
            return Err(Error::HeapOverflow {
                heap,
                size: self.data.len(),
                limit: HEAP_LIMIT,
            });
        }
        Ok(())
    }

    /// The raw heap bytes, before stream padding.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.data
    }
}

/// The `#Blob` heap: each entry is a compressed length followed by its bytes,
/// indexed by the byte offset of the *length prefix*, not of the data.
#[derive(Debug)]
pub(crate) struct BlobHeap {
    data: Vec<u8>,
    seen: HashMap<Vec<u8>, u32>,
}

impl BlobHeap {
    /// Creates a heap whose index 0 is already the empty blob.
    pub(crate) fn new() -> Self {
        let mut seen = HashMap::new();
        seen.insert(Vec::new(), 0_u32);
        Self {
            data: vec![0],
            seen,
        }
    }

    /// Interns `value`, returning the offset of its length prefix.
    ///
    /// # Errors
    ///
    /// Returns [`Error::HeapOverflow`] if the heap would exceed its ceiling,
    /// or [`Error::CompressedRange`] if the blob is longer than a compressed
    /// integer can describe.
    pub(crate) fn intern(&mut self, value: &[u8]) -> Result<u32> {
        if let Some(offset) = self.seen.get(value) {
            return Ok(*offset);
        }
        let offset = u32::try_from(self.data.len()).map_err(|_| Error::HeapOverflow {
            heap: "#Blob",
            size: self.data.len(),
            limit: HEAP_LIMIT,
        })?;
        let length = u32::try_from(value.len()).map_err(|_| Error::HeapOverflow {
            heap: "#Blob",
            size: value.len(),
            limit: HEAP_LIMIT,
        })?;
        compress_unsigned(&mut self.data, length)?;
        self.data.extend_from_slice(value);
        if self.data.len() > HEAP_LIMIT {
            return Err(Error::HeapOverflow {
                heap: "#Blob",
                size: self.data.len(),
                limit: HEAP_LIMIT,
            });
        }
        self.seen.insert(value.to_vec(), offset);
        Ok(offset)
    }

    /// The raw heap bytes, before stream padding.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.data
    }
}

/// The `#GUID` heap: a bare array of 16-byte GUIDs indexed by *1-based
/// ordinal*, not by byte offset. Index 0 means "no GUID".
#[derive(Debug, Default)]
pub(crate) struct GuidHeap {
    data: Vec<u8>,
}

impl GuidHeap {
    /// Appends a GUID and returns its 1-based ordinal.
    ///
    /// # Errors
    ///
    /// Returns [`Error::HeapOverflow`] if the heap would exceed its ceiling.
    pub(crate) fn add(&mut self, guid: [u8; 16]) -> Result<u32> {
        self.data.extend_from_slice(&guid);
        if self.data.len() > HEAP_LIMIT {
            return Err(Error::HeapOverflow {
                heap: "#GUID",
                size: self.data.len(),
                limit: HEAP_LIMIT,
            });
        }
        u32::try_from(self.count()).map_err(|_| Error::HeapOverflow {
            heap: "#GUID",
            size: self.data.len(),
            limit: HEAP_LIMIT,
        })
    }

    /// How many GUIDs the heap holds. The `HeapSizes` bit for `#GUID` keys off
    /// this count, not off the byte length.
    pub(crate) fn count(&self) -> usize {
        self.data.len().checked_div(16).unwrap_or(0)
    }

    /// The raw heap bytes, before stream padding.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::{BlobHeap, GuidHeap, StringHeap};

    #[test]
    fn string_heap_starts_with_the_empty_string_and_dedupes() {
        let mut heap = StringHeap::new();
        assert_eq!(heap.intern("").unwrap(), 0);
        let first = heap.intern("MyLib").unwrap();
        assert_eq!(first, 1);
        assert_eq!(heap.intern("MyLib").unwrap(), first);
        assert_eq!(heap.bytes(), b"\0MyLib\0");
    }

    #[test]
    fn string_heap_rejects_interior_nul() {
        let mut heap = StringHeap::new();
        assert!(heap.intern("bad\0name").is_err());
    }

    #[test]
    fn blob_heap_indexes_the_length_prefix() {
        let mut heap = BlobHeap::new();
        assert_eq!(heap.intern(&[]).unwrap(), 0);
        let sig = heap.intern(&[0x06, 0x08]).unwrap();
        assert_eq!(sig, 1);
        assert_eq!(heap.bytes(), &[0x00, 0x02, 0x06, 0x08]);
        assert_eq!(heap.intern(&[0x06, 0x08]).unwrap(), sig);
    }

    #[test]
    fn guid_heap_is_one_based_by_ordinal() {
        let mut heap = GuidHeap::default();
        assert_eq!(heap.count(), 0);
        assert_eq!(heap.add([7; 16]).unwrap(), 1);
        assert_eq!(heap.add([9; 16]).unwrap(), 2);
        assert_eq!(heap.count(), 2);
        assert_eq!(heap.bytes().len(), 32);
    }
}
