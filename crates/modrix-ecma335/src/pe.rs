// SPDX-License-Identifier: GPL-2.0-only
//! The PE32+ container: DOS stub, COFF header, optional header, one `.text`
//! section holding the CLI header and the metadata root.
//!
//! A reference assembly needs no import table, no IAT, no `.reloc` section, no
//! debug directory and no entry point - the file is only ever *read* by a
//! compiler, never loaded by the OS. That reduces the whole container to a
//! fixed 0x200-byte header block plus one section.

use crate::encode::{align_up, write_u16, write_u32, write_u64};
use crate::error::{Error, Result};
use crate::materialize::Image;

/// PE file alignment: the granularity of raw section data.
const FILE_ALIGNMENT: u32 = 0x200;
/// PE section alignment: the granularity of section RVAs in memory.
const SECTION_ALIGNMENT: u32 = 0x2000;
/// RVA of the single `.text` section.
const TEXT_RVA: u32 = 0x2000;
/// Size of `IMAGE_COR20_HEADER`.
const CLI_HEADER_SIZE: u32 = 72;
/// `COMIMAGE_FLAGS_ILONLY`. `STRONGNAMESIGNED` is deliberately not set: there
/// is no signature blob to back it.
const COR20_FLAGS: u32 = 0x0000_0001;
/// Number of data directories in the optional header.
const DATA_DIRECTORIES: u32 = 16;
/// Index of the CLI header data directory.
const CLI_DIRECTORY: u32 = 14;

/// The 128-byte DOS header and stub. Only `e_lfanew` at 0x3C is load-bearing;
/// the rest is the conventional "cannot be run in DOS mode" stub every real
/// toolchain emits, kept so the file looks unremarkable to other tools.
const DOS_HEADER: [u8; 128] = [
    0x4D, 0x5A, 0x90, 0x00, 0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00,
    0xB8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00,
    0x0E, 0x1F, 0xBA, 0x0E, 0x00, 0xB4, 0x09, 0xCD, 0x21, 0xB8, 0x01, 0x4C, 0xCD, 0x21, 0x54, 0x68,
    0x69, 0x73, 0x20, 0x70, 0x72, 0x6F, 0x67, 0x72, 0x61, 0x6D, 0x20, 0x63, 0x61, 0x6E, 0x6E, 0x6F,
    0x74, 0x20, 0x62, 0x65, 0x20, 0x72, 0x75, 0x6E, 0x20, 0x69, 0x6E, 0x20, 0x44, 0x4F, 0x53, 0x20,
    0x6D, 0x6F, 0x64, 0x65, 0x2E, 0x0D, 0x0D, 0x0A, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Sizes the whole image once so every header field can be filled in.
struct Layout {
    /// Bytes of real content in `.text`.
    text_virtual: u32,
    /// `.text` raw size, rounded to the file alignment.
    text_raw: u32,
    /// Where the headers end and `.text` begins in the file.
    headers: u32,
    /// Total mapped size, rounded to the section alignment.
    image: u32,
    /// Byte length of the optional header.
    optional: u16,
}

/// Emits the finished reference assembly.
///
/// # Errors
///
/// Propagates metadata failures, or returns [`Error::LayoutOverflow`] if the
/// image is too large for the 32-bit PE size fields.
pub(crate) fn write_image(image: &Image) -> Result<Vec<u8>> {
    let metadata = crate::metadata::metadata_root(image)?;
    let metadata_size = u32::try_from(metadata.len()).map_err(|_| Error::LayoutOverflow {
        what: "the metadata size",
    })?;
    let layout = plan(metadata_size)?;
    let mut out = Vec::new();
    out.extend_from_slice(&DOS_HEADER);
    out.extend_from_slice(b"PE\0\0");
    write_coff_header(&mut out, &layout);
    write_optional_header(&mut out, &layout);
    write_section_table(&mut out, &layout);
    resize_to(&mut out, layout.headers)?;
    write_cli_header(&mut out, metadata_size);
    out.extend_from_slice(&metadata);
    let total = layout
        .headers
        .checked_add(layout.text_raw)
        .ok_or(Error::LayoutOverflow {
            what: "the file size",
        })?;
    resize_to(&mut out, total)?;
    Ok(out)
}

fn plan(metadata_size: u32) -> Result<Layout> {
    let overflow = || Error::LayoutOverflow {
        what: "the section layout",
    };
    let optional_size = 24_u32
        .checked_add(88)
        .and_then(|base| base.checked_add(DATA_DIRECTORIES.checked_mul(8)?))
        .ok_or_else(overflow)?;
    let header_bytes = 0x80_u32
        .checked_add(4)
        .and_then(|base| base.checked_add(20))
        .and_then(|base| base.checked_add(optional_size))
        .and_then(|base| base.checked_add(40))
        .ok_or_else(overflow)?;
    let virtual_size = CLI_HEADER_SIZE
        .checked_add(metadata_size)
        .ok_or_else(overflow)?;
    Ok(Layout {
        text_virtual: virtual_size,
        text_raw: align32(virtual_size, FILE_ALIGNMENT).ok_or_else(overflow)?,
        headers: align32(header_bytes, FILE_ALIGNMENT).ok_or_else(overflow)?,
        image: align32(
            TEXT_RVA.checked_add(virtual_size).ok_or_else(overflow)?,
            SECTION_ALIGNMENT,
        )
        .ok_or_else(overflow)?,
        optional: u16::try_from(optional_size).map_err(|_| overflow())?,
    })
}

fn write_coff_header(out: &mut Vec<u8>, layout: &Layout) {
    // AMD64 paired with the PE32+ magic below; Roslyn accepts either that or
    // the I386/PE32 pairing the real reference packs use.
    write_u16(out, 0x8664);
    write_u16(out, 1);
    // Zero timestamp keeps the output reproducible.
    write_u32(out, 0);
    write_u32(out, 0);
    write_u32(out, 0);
    write_u16(out, layout.optional);
    // EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE | DLL.
    write_u16(out, 0x2022);
}

fn write_optional_header(out: &mut Vec<u8>, layout: &Layout) {
    write_u16(out, 0x020B);
    out.push(48);
    out.push(0);
    write_u32(out, layout.text_raw);
    write_u32(out, 0);
    write_u32(out, 0);
    write_u32(out, 0);
    write_u32(out, TEXT_RVA);
    write_u64(out, 0x0000_0001_8000_0000);
    write_u32(out, SECTION_ALIGNMENT);
    write_u32(out, FILE_ALIGNMENT);
    for field in [4_u16, 0, 0, 0, 4, 0] {
        write_u16(out, field);
    }
    write_u32(out, 0);
    write_u32(out, layout.image);
    write_u32(out, layout.headers);
    write_u32(out, 0);
    write_u16(out, 3);
    // HIGH_ENTROPY_VA | DYNAMIC_BASE | NX_COMPAT | NO_SEH | TERMINAL_SERVER_AWARE.
    write_u16(out, 0x8560);
    for field in [0x0040_0000_u64, 0x4000, 0x0010_0000, 0x2000] {
        write_u64(out, field);
    }
    write_u32(out, 0);
    write_u32(out, DATA_DIRECTORIES);
    write_data_directories(out);
}

fn write_data_directories(out: &mut Vec<u8>) {
    for index in 0..DATA_DIRECTORIES {
        if index == CLI_DIRECTORY {
            write_u32(out, TEXT_RVA);
            write_u32(out, CLI_HEADER_SIZE);
        } else {
            write_u32(out, 0);
            write_u32(out, 0);
        }
    }
}

fn write_section_table(out: &mut Vec<u8>, layout: &Layout) {
    out.extend_from_slice(b".text\0\0\0");
    write_u32(out, layout.text_virtual);
    write_u32(out, TEXT_RVA);
    write_u32(out, layout.text_raw);
    write_u32(out, layout.headers);
    write_u32(out, 0);
    write_u32(out, 0);
    write_u16(out, 0);
    write_u16(out, 0);
    // CNT_CODE | MEM_EXECUTE | MEM_READ.
    write_u32(out, 0x6000_0020);
}

fn write_cli_header(out: &mut Vec<u8>, metadata_size: u32) {
    write_u32(out, CLI_HEADER_SIZE);
    write_u16(out, 2);
    write_u16(out, 5);
    write_u32(out, TEXT_RVA.saturating_add(CLI_HEADER_SIZE));
    write_u32(out, metadata_size);
    write_u32(out, COR20_FLAGS);
    // No entry point, no resources, no strong-name signature, and none of the
    // trailing directories: a reference assembly has nothing to put in them.
    for _ in 0..13 {
        write_u32(out, 0);
    }
}

fn align32(value: u32, alignment: u32) -> Option<u32> {
    let aligned = align_up(
        usize::try_from(value).ok()?,
        usize::try_from(alignment).ok()?,
    )?;
    u32::try_from(aligned).ok()
}

fn resize_to(out: &mut Vec<u8>, size: u32) -> Result<()> {
    let target = usize::try_from(size).map_err(|_| Error::LayoutOverflow {
        what: "the file size",
    })?;
    if out.len() < target {
        out.resize(target, 0);
    }
    Ok(())
}
