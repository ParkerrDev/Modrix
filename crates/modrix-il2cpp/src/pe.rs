// SPDX-License-Identifier: GPL-2.0-only
//! A minimal, bounded PE32+ reader.
//!
//! `GameAssembly.dll` is a 100 MB native image. We do not load or execute any
//! of it - we only need to translate the virtual addresses stored inside it
//! into file offsets so the IL2CPP registration tables can be read. That is a
//! pure parsing job, so it stays safe Rust with no `unsafe` and no FFI.
//!
//! Only the parts of the format we actually use are decoded: the DOS stub's
//! `e_lfanew`, the COFF header's section count, the optional header's image
//! base, and the section table. Everything else is skipped.

use crate::error::{Error, Result};
use crate::reader::Reader;

/// `PE\0\0`.
const PE_SIGNATURE: u32 = 0x0000_4550;
/// `IMAGE_FILE_MACHINE_AMD64`.
const MACHINE_AMD64: u16 = 0x8664;
/// `IMAGE_NT_OPTIONAL_HDR64_MAGIC`.
const PE32_PLUS: u16 = 0x020B;
/// A PE may not declare more sections than this; guards the section loop.
const MAX_SECTIONS: usize = 96;

/// One mapped section of the image.
#[derive(Debug, Clone)]
pub struct Section {
    /// Section name, trimmed of its NUL padding (`.text`, `.rdata`, `il2cpp`).
    pub name: String,
    /// Address relative to the image base once loaded.
    pub virtual_address: u32,
    /// Size the section occupies in memory.
    pub virtual_size: u32,
    /// Offset of the section's bytes in the file.
    pub raw_offset: u32,
    /// Number of bytes actually present in the file.
    pub raw_size: u32,
}

impl Section {
    /// The span this section covers once loaded. A section's in-memory size
    /// can exceed its on-disk size (BSS-style padding), and occasionally the
    /// reverse, so the larger of the two is used when testing containment.
    fn span(&self) -> u32 {
        self.virtual_size.max(self.raw_size)
    }
}

/// A parsed PE32+ image: enough to map virtual addresses back to file offsets.
#[derive(Debug)]
pub struct PeImage {
    /// The address the image is linked to load at.
    pub image_base: u64,
    /// The section table.
    pub sections: Vec<Section>,
}

impl PeImage {
    /// Parse the headers of a PE32+ image.
    ///
    /// # Errors
    ///
    /// [`Error::NotPe`] if the file is not a 64-bit PE, or [`Error::Truncated`]
    /// if a header runs past the end of the buffer.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let reader = Reader::new(data);
        // The DOS header stores the PE header's offset at 0x3C.
        let pe_offset = usize::try_from(reader.u32_at(0x3C)?).unwrap_or(0);
        if reader.u32_at(pe_offset)? != PE_SIGNATURE {
            return Err(Error::NotPe {
                detail: "no PE signature".to_owned(),
            });
        }
        let coff = pe_offset.saturating_add(4);
        let machine = reader.u16_at(coff)?;
        if machine != MACHINE_AMD64 {
            return Err(Error::NotPe {
                detail: format!("machine 0x{machine:04X} is not x86-64"),
            });
        }
        let section_count = usize::from(reader.u16_at(coff.saturating_add(2))?);
        if section_count > MAX_SECTIONS {
            return Err(Error::BoundExceeded {
                what: "PE sections",
                limit: MAX_SECTIONS,
            });
        }
        let optional_size = usize::from(reader.u16_at(coff.saturating_add(16))?);
        let optional = coff.saturating_add(20);
        if reader.u16_at(optional)? != PE32_PLUS {
            return Err(Error::NotPe {
                detail: "not PE32+ (a 32-bit game is not supported)".to_owned(),
            });
        }
        // PE32+ places ImageBase at optional header offset 24 as a u64.
        let image_base = reader.u64_at(optional.saturating_add(24))?;

        let table = optional.saturating_add(optional_size);
        let mut sections = Vec::with_capacity(section_count);
        for index in 0..section_count {
            let entry = table.saturating_add(index.saturating_mul(40));
            sections.push(Section {
                name: reader.fixed_str(entry, 8)?,
                virtual_size: reader.u32_at(entry.saturating_add(8))?,
                virtual_address: reader.u32_at(entry.saturating_add(12))?,
                raw_size: reader.u32_at(entry.saturating_add(16))?,
                raw_offset: reader.u32_at(entry.saturating_add(20))?,
            });
        }
        Ok(Self {
            image_base,
            sections,
        })
    }

    /// Translate a relative virtual address to a file offset.
    #[must_use]
    pub fn rva_to_offset(&self, rva: u32) -> Option<usize> {
        let section = self.sections.iter().find(|s| {
            rva >= s.virtual_address && rva < s.virtual_address.saturating_add(s.span())
        })?;
        let delta = rva.checked_sub(section.virtual_address)?;
        usize::try_from(section.raw_offset.checked_add(delta)?).ok()
    }

    /// Translate an absolute virtual address (as stored in the image's own
    /// pointers) to a file offset. Returns `None` for a null or out-of-image
    /// pointer, which is how callers validate a candidate structure.
    #[must_use]
    pub fn va_to_offset(&self, va: u64) -> Option<usize> {
        let rva = va.checked_sub(self.image_base)?;
        self.rva_to_offset(u32::try_from(rva).ok()?)
    }

    /// Whether `va` points somewhere inside this image. Used to sanity-check a
    /// candidate registration struct before trusting it.
    #[must_use]
    pub fn is_image_pointer(&self, va: u64) -> bool {
        self.va_to_offset(va).is_some()
    }

    /// Sections that can hold initialised data structures.
    ///
    /// The registration tables are constant data, so only these are scanned -
    /// searching `.text` would waste time on 5 MB of code and risk matching an
    /// immediate operand that happens to equal the anchor value.
    pub fn data_sections(&self) -> impl Iterator<Item = &Section> {
        self.sections
            .iter()
            .filter(|s| matches!(s.name.as_str(), ".rdata" | ".data" | "il2cpp"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_non_pe_buffer() {
        let data = vec![0_u8; 512];
        assert!(matches!(PeImage::parse(&data), Err(Error::NotPe { .. })));
    }

    #[test]
    fn rejects_a_truncated_buffer() {
        let data = [0_u8; 4];
        assert!(PeImage::parse(&data).is_err());
    }

    #[test]
    fn maps_addresses_within_a_section_and_rejects_those_outside() {
        let image = PeImage {
            image_base: 0x1_8000_0000,
            sections: vec![Section {
                name: ".rdata".to_owned(),
                virtual_address: 0x1000,
                virtual_size: 0x2000,
                raw_offset: 0x400,
                raw_size: 0x2000,
            }],
        };
        assert_eq!(image.rva_to_offset(0x1000), Some(0x400));
        assert_eq!(image.rva_to_offset(0x1FFF), Some(0x13FF));
        assert_eq!(image.rva_to_offset(0x3000), None, "past the section end");
        assert_eq!(image.rva_to_offset(0x0FFF), None, "before the section");
        assert_eq!(image.va_to_offset(0x1_8000_1000), Some(0x400));
        assert!(!image.is_image_pointer(0), "null is never an image pointer");
        assert!(
            !image.is_image_pointer(0x1_0000_0000),
            "below the image base"
        );
    }
}
