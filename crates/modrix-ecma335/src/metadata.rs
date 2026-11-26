// SPDX-License-Identifier: GPL-2.0-only
//! The metadata root: the `BSJB` signature, the version string, and the five
//! stream headers followed by the stream bodies.
//!
//! Two details here are load-bearing and neither is obvious:
//!
//! * The root's `Length` field - the size of the version string - **must be a
//!   multiple of 4**. Readers compute the stream-header array's position as
//!   `16 + Length`, so an unpadded 11 for `"v4.0.30319\0"` makes Roslyn abort
//!   with `CS0009: Read out of bounds`.
//! * A stream header's `Offset` is relative to the `B` of `BSJB`, not to the
//!   file or to the section.

use crate::encode::{align_up, pad_to_four, write_u16, write_u32};
use crate::error::{Error, Result};
use crate::materialize::Image;

/// Metadata root signature, `BSJB` in little-endian byte order.
pub(crate) const SIGNATURE: u32 = 0x424A_5342;

/// The version string every .NET assembly since v4 carries, NUL included.
const VERSION_STRING: &[u8] = b"v4.0.30319\0";

/// One `#`-prefixed stream.
struct Stream {
    name: &'static str,
    data: Vec<u8>,
}

/// Builds the metadata root for `image`.
///
/// # Errors
///
/// Propagates table-serialisation failures, or returns
/// [`Error::LayoutOverflow`] if the stream offsets do not fit a `u32`.
pub(crate) fn metadata_root(image: &Image) -> Result<Vec<u8>> {
    let streams = build_streams(image)?;
    let version_length = align_up(VERSION_STRING.len(), 4).ok_or(Error::LayoutOverflow {
        what: "the metadata version string length",
    })?;
    let headers: usize = streams
        .iter()
        .map(|stream| header_size(stream.name))
        .try_fold(0_usize, usize::checked_add)
        .ok_or(Error::LayoutOverflow {
            what: "the stream header block",
        })?;
    let mut root = Vec::new();
    write_root_header(&mut root, version_length, streams.len())?;
    let first = [16_usize, version_length, 4, headers]
        .into_iter()
        .try_fold(0_usize, usize::checked_add)
        .ok_or(Error::LayoutOverflow {
            what: "the first stream offset",
        })?;
    write_stream_headers(&mut root, &streams, first)?;
    for stream in &streams {
        root.extend_from_slice(&stream.data);
    }
    Ok(root)
}

fn write_root_header(root: &mut Vec<u8>, version_length: usize, streams: usize) -> Result<()> {
    write_u32(root, SIGNATURE);
    write_u16(root, 1);
    write_u16(root, 1);
    write_u32(root, 0);
    let length = u32::try_from(version_length).map_err(|_| Error::LayoutOverflow {
        what: "the metadata version string length",
    })?;
    write_u32(root, length);
    root.extend_from_slice(VERSION_STRING);
    root.resize(
        root.len()
            .checked_sub(VERSION_STRING.len())
            .and_then(|base| base.checked_add(version_length))
            .ok_or(Error::LayoutOverflow {
                what: "the metadata version string padding",
            })?,
        0,
    );
    write_u16(root, 0);
    write_u16(
        root,
        u16::try_from(streams).map_err(|_| Error::LayoutOverflow {
            what: "the stream count",
        })?,
    );
    Ok(())
}

fn write_stream_headers(root: &mut Vec<u8>, streams: &[Stream], first: usize) -> Result<()> {
    let mut offset = first;
    for stream in streams {
        let start = u32::try_from(offset).map_err(|_| Error::LayoutOverflow {
            what: "a stream offset",
        })?;
        let size = u32::try_from(stream.data.len()).map_err(|_| Error::LayoutOverflow {
            what: "a stream size",
        })?;
        write_u32(root, start);
        write_u32(root, size);
        root.extend_from_slice(stream.name.as_bytes());
        root.push(0);
        while !root.len().is_multiple_of(4) {
            root.push(0);
        }
        offset = offset
            .checked_add(stream.data.len())
            .ok_or(Error::LayoutOverflow {
                what: "a stream offset",
            })?;
    }
    Ok(())
}

/// A stream header is 8 bytes of offset/size plus a NUL-terminated name padded
/// to a 4-byte boundary.
fn header_size(name: &str) -> usize {
    let name_field = align_up(name.len().saturating_add(1), 4).unwrap_or(4);
    name_field.saturating_add(8)
}

/// Assembles the five streams in Roslyn's order. `#Blob` in particular must be
/// present even when it holds only its leading zero byte: omitting it yields
/// `CS0009: Read out of bounds`.
fn build_streams(image: &Image) -> Result<Vec<Stream>> {
    let mut tables = crate::emit::table_stream(image)?;
    pad_to_four(&mut tables);
    let mut strings = image.strings.bytes().to_vec();
    pad_to_four(&mut strings);
    // A reference assembly holds no IL, so the user-string heap is just its
    // mandatory leading zero byte.
    let mut user_strings = vec![0_u8];
    pad_to_four(&mut user_strings);
    let mut guids = image.guids.bytes().to_vec();
    pad_to_four(&mut guids);
    let mut blobs = image.blobs.bytes().to_vec();
    pad_to_four(&mut blobs);
    Ok(vec![
        Stream {
            name: "#~",
            data: tables,
        },
        Stream {
            name: "#Strings",
            data: strings,
        },
        Stream {
            name: "#US",
            data: user_strings,
        },
        Stream {
            name: "#GUID",
            data: guids,
        },
        Stream {
            name: "#Blob",
            data: blobs,
        },
    ])
}
