// SPDX-License-Identifier: GPL-2.0-only
//! Bethesda plugin (`.esp`/`.esm`/`.esl`) header parsing.
//!
//! Reads just the `TES4` header record: the ESM/ESL flags and the `MAST`
//! subrecords - a plugin's *hard dependencies*, which the game requires to be
//! present and loaded earlier. Everything is bounded and panic-free; a file
//! that is not a valid plugin parses as [`None`] rather than erroring, so one
//! corrupt file never breaks load-order management.

use std::io::Read;
use std::path::Path;

/// Read at most this much of a plugin file (the TES4 header is tiny; the
/// bound guards against a hostile size field).
const MAX_HEADER: usize = 1024 * 1024;
/// The record header length in Skyrim SE-era plugins.
const RECORD_HEADER: usize = 24;
/// Upper bound on subrecords walked (a header holds a handful).
const MAX_SUBRECORDS: usize = 4096;

/// `TES4` record flag: this plugin is a master (loads in the master tier).
const FLAG_ESM: u32 = 0x0000_0001;
/// `TES4` record flag: light plugin (ESL - shares the `FE` load slot space).
const FLAG_LIGHT: u32 = 0x0000_0200;

/// A parsed plugin header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHeader {
    /// Master-tier plugin (ESM flag, or a `.esm`/`.esl` extension).
    pub is_master: bool,
    /// Light plugin (ESL flag or `.esl` extension).
    pub is_light: bool,
    /// The plugins this one requires, in declaration order.
    pub masters: Vec<String>,
}

/// Whether a filename looks like a game plugin.
#[must_use]
pub fn is_plugin_name(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| {
            e.eq_ignore_ascii_case("esp")
                || e.eq_ignore_ascii_case("esm")
                || e.eq_ignore_ascii_case("esl")
        })
}

/// Parse a plugin file's header. Returns `None` for files that are not valid
/// TES4 plugins (unreadable, wrong magic, truncated).
#[must_use]
pub fn parse_header(path: &Path) -> Option<PluginHeader> {
    let bytes = read_bounded(path)?;
    let mut header = parse_bytes(&bytes)?;
    // The extension overrides/augments flags: the game treats every `.esm`
    // as master-tier and every `.esl` as a light master regardless of flags.
    let ext = path
        .extension()
        .map(|e| e.to_ascii_lowercase().to_string_lossy().into_owned())
        .unwrap_or_default();
    if ext == "esm" || ext == "esl" {
        header.is_master = true;
    }
    if ext == "esl" {
        header.is_light = true;
    }
    Some(header)
}

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_HEADER as u64).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Parse the `TES4` record from raw plugin bytes.
fn parse_bytes(bytes: &[u8]) -> Option<PluginHeader> {
    if bytes.get(..4)? != b"TES4" {
        return None;
    }
    let data_size = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let flags = u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?);
    let end = RECORD_HEADER
        .saturating_add(data_size)
        .min(bytes.len())
        .min(MAX_HEADER);
    let mut masters = Vec::new();
    let mut cursor = RECORD_HEADER;
    for _ in 0..MAX_SUBRECORDS {
        let Some(kind) = bytes.get(cursor..cursor.saturating_add(4)) else {
            break;
        };
        if cursor.saturating_add(6) > end {
            break;
        }
        let size = u16::from_le_bytes(
            bytes
                .get(cursor.saturating_add(4)..cursor.saturating_add(6))?
                .try_into()
                .ok()?,
        ) as usize;
        let data_start = cursor.saturating_add(6);
        let data_end = data_start.saturating_add(size).min(end);
        if kind == b"MAST"
            && let Some(raw) = bytes.get(data_start..data_end)
        {
            let name = raw.split(|b| *b == 0).next().unwrap_or_default();
            let name = String::from_utf8_lossy(name).into_owned();
            if !name.is_empty() {
                masters.push(name);
            }
        }
        cursor = data_end;
        if cursor >= end {
            break;
        }
    }
    Some(PluginHeader {
        is_master: flags & FLAG_ESM != 0,
        is_light: flags & FLAG_LIGHT != 0,
        masters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal TES4 header with the given flags and masters.
    fn plugin_bytes(flags: u32, masters: &[&str]) -> Vec<u8> {
        let mut data = Vec::new();
        // HEDR subrecord (12 bytes of stats the parser skips over).
        data.extend_from_slice(b"HEDR");
        data.extend_from_slice(&12u16.to_le_bytes());
        data.extend_from_slice(&[0u8; 12]);
        for master in masters {
            let z: Vec<u8> = master.bytes().chain(std::iter::once(0)).collect();
            data.extend_from_slice(b"MAST");
            data.extend_from_slice(&u16::try_from(z.len()).unwrap().to_le_bytes());
            data.extend_from_slice(&z);
            data.extend_from_slice(b"DATA");
            data.extend_from_slice(&8u16.to_le_bytes());
            data.extend_from_slice(&[0u8; 8]);
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"TES4");
        bytes.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&flags.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 12]); // formid, revision, version, unknown
        bytes.extend_from_slice(&data);
        bytes
    }

    #[test]
    fn parses_flags_and_masters() {
        let bytes = plugin_bytes(FLAG_LIGHT, &["Skyrim.esm", "Update.esm"]);
        let header = parse_bytes(&bytes).unwrap();
        assert!(!header.is_master);
        assert!(header.is_light);
        assert_eq!(header.masters, vec!["Skyrim.esm", "Update.esm"]);
    }

    #[test]
    fn esl_extension_forces_light_master() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("small.esl");
        std::fs::write(&path, plugin_bytes(0, &[])).unwrap();
        let header = parse_header(&path).unwrap();
        assert!(header.is_master);
        assert!(header.is_light);
    }

    #[test]
    fn rejects_non_plugins() {
        assert!(parse_bytes(b"not a plugin at all").is_none());
        assert!(parse_bytes(b"").is_none());
    }

    #[test]
    fn parses_a_real_skyui_style_header() {
        // Byte-for-byte shape of SkyUI_SE.esp's opening (ESL-flagged, one master).
        let bytes = plugin_bytes(FLAG_LIGHT, &["Skyrim.esm"]);
        let header = parse_bytes(&bytes).unwrap();
        assert!(header.is_light);
        assert_eq!(header.masters, vec!["Skyrim.esm"]);
    }

    #[test]
    fn plugin_name_detection() {
        assert!(is_plugin_name("SkyUI_SE.esp"));
        assert!(is_plugin_name("Skyrim.ESM"));
        assert!(is_plugin_name("ccBGSSSE001-Fish.esl"));
        assert!(!is_plugin_name("readme.txt"));
    }
}
