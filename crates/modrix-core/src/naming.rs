// SPDX-License-Identifier: GPL-2.0-only
//! Mod name / version / Nexus-id detection from archive filenames.
//!
//! Nexus web downloads are named `Name-<modid>-<version, dots as dashes>-
//! <unixtime>.<ext>`; some tools name files `Name <modid> <semver> <hash>.<ext>`.
//! [`detect`] recovers the human name, the version, and the mod id from either
//! convention, falling back to the bare stem.

/// What [`detect`] recovered from a filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    /// The human mod name (ids, timestamps, and hashes stripped).
    pub name: String,
    /// The version, when the filename carries one (`6.11`, `2.0.2c`, `Final`).
    pub version: Option<String>,
    /// The Nexus mod id, when the filename carries one.
    pub nexus_mod_id: Option<i64>,
}

/// Archive suffixes stripped before parsing.
const EXTS: [&str; 8] = [
    ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".zip", ".7z", ".rar", ".tar",
];

/// Longest run of trailing version tokens the parser will accept.
const MAX_VERSION_TOKENS: usize = 6;

/// Parse a mod archive filename into name / version / mod id.
#[must_use]
pub fn detect(file_name: &str) -> Detected {
    let stem = strip_ext(file_name);
    if let Some(found) = detect_dashed(stem) {
        return found;
    }
    if let Some(found) = detect_spaced(stem) {
        return found;
    }
    Detected {
        name: tidy(stem),
        version: None,
        nexus_mod_id: None,
    }
}

fn strip_ext(file_name: &str) -> &str {
    let lower = file_name.to_ascii_lowercase();
    for ext in EXTS {
        if lower.ends_with(ext) {
            return file_name
                .get(..file_name.len().saturating_sub(ext.len()))
                .unwrap_or(file_name);
        }
    }
    file_name
}

/// The Nexus convention: `Name-<modid>-<version tokens>[-<timestamp>]`.
fn detect_dashed(stem: &str) -> Option<Detected> {
    let mut tokens: Vec<&str> = stem.split('-').collect();
    if tokens.len() < 3 {
        return None;
    }
    // A trailing 9-11 digit token is the download timestamp - drop it.
    if tokens.last().is_some_and(|t| is_digits(t, 9, 11)) {
        tokens.pop();
    }
    // The mod id is the leftmost 3-7 digit token that only version-ish tokens
    // follow; everything before it is the name, everything after the version.
    let candidates = tokens
        .iter()
        .enumerate()
        .skip(1)
        .take(tokens.len().saturating_sub(1));
    for (i, token) in candidates {
        let rest = tokens.get(i.saturating_add(1)..).unwrap_or_default();
        if is_digits(token, 3, 7)
            && !rest.is_empty()
            && rest.len() <= MAX_VERSION_TOKENS
            && rest.iter().all(|t| is_version_token(t))
        {
            return Some(Detected {
                name: tidy(&tokens.get(..i)?.join("-")),
                version: Some(join_version(rest)),
                nexus_mod_id: token.parse().ok(),
            });
        }
    }
    None
}

/// The tool convention: `Name <modid> <semver> [hash]` (space-separated).
fn detect_spaced(stem: &str) -> Option<Detected> {
    let tokens: Vec<&str> = stem.split_whitespace().collect();
    let (i, id) = tokens
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, t)| is_digits(t, 3, 7))?;
    let version = tokens.get(i.saturating_add(1)).filter(|v| is_semver(v))?;
    Some(Detected {
        name: tidy(&tokens.get(..i)?.join(" ")),
        version: Some((*version).to_owned()),
        nexus_mod_id: id.parse().ok(),
    })
}

fn is_digits(token: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_digit())
}

/// `4`, `08`, `2c`, `v5`, `rc2`, `final`, `beta`, `hotfix` - the pieces a
/// dotted version splits into once dots become dashes.
fn is_version_token(token: &str) -> bool {
    if token.len() > 5 || token.is_empty() {
        return false;
    }
    let lower = token.to_ascii_lowercase();
    if matches!(lower.as_str(), "final" | "beta" | "alpha" | "hotfix") || lower.starts_with("rc") {
        return true;
    }
    let digits = lower.strip_prefix('v').unwrap_or(&lower);
    let letters = digits.trim_start_matches(|c: char| c.is_ascii_digit());
    digits.len() > letters.len()
        && letters.len() <= 2
        && letters.bytes().all(|b| b.is_ascii_lowercase())
}

fn is_semver(token: &str) -> bool {
    token.split('.').count() >= 2
        && token
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn join_version(tokens: &[&str]) -> String {
    let joined = tokens.join(".");
    joined
        .strip_prefix('v')
        .or_else(|| joined.strip_prefix('V'))
        .unwrap_or(&joined)
        .to_owned()
}

/// Trim and collapse runs of whitespace.
fn tidy(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_space = true;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(file: &str, name: &str, version: Option<&str>, id: Option<i64>) {
        let d = detect(file);
        assert_eq!(d.name, name, "name of {file}");
        assert_eq!(d.version.as_deref(), version, "version of {file}");
        assert_eq!(d.nexus_mod_id, id, "mod id of {file}");
    }

    #[test]
    fn parses_nexus_dashed_filenames() {
        check(
            "SkyUI-12604-6-11-1778020881.zip",
            "SkyUI",
            Some("6.11"),
            Some(12604),
        );
        check(
            "Unofficial Skyrim Special Edition Patch-266-4-3-8a-1774132896.7z",
            "Unofficial Skyrim Special Edition Patch",
            Some("4.3.8a"),
            Some(266),
        );
        check(
            "Skyrim Script Extender (SKSE64)-30379-2-2-6-1705522967.7z",
            "Skyrim Script Extender (SKSE64)",
            Some("2.2.6"),
            Some(30379),
        );
        check(
            "DllLoader-3619-1-0-0-4.zip",
            "DllLoader",
            Some("1.0.0.4"),
            Some(3619),
        );
        check(
            "Joy of Perspective-9358-2-0-2c.7z",
            "Joy of Perspective",
            Some("2.0.2c"),
            Some(9358),
        );
        check(
            "High Poly Project-12029-v5-3-1634909383.zip",
            "High Poly Project",
            Some("5.3"),
            Some(12029),
        );
        check(
            "Immersive Patrols (Main)-718-3-0b-1710611172.zip",
            "Immersive Patrols (Main)",
            Some("3.0b"),
            Some(718),
        );
    }

    #[test]
    fn keeps_dashes_and_dots_that_belong_to_the_name() {
        check(
            "SMIM SE 2-08-659-2-08.7z",
            "SMIM SE 2-08",
            Some("2.08"),
            Some(659),
        );
        check(
            "SMIM Quality Addon 1.5-44388-1-5-1700735143.7z",
            "SMIM Quality Addon 1.5",
            Some("1.5"),
            Some(44388),
        );
        check(
            "Achievements Mods Enabler SE-AE-245-1-41-1715217907.zip",
            "Achievements Mods Enabler SE-AE",
            Some("1.41"),
            Some(245),
        );
        check(
            "Relationship Dialogue Overhaul - RDO Final-1187-Final.7z",
            "Relationship Dialogue Overhaul - RDO Final",
            Some("Final"),
            Some(1187),
        );
    }

    #[test]
    fn parses_space_separated_tool_filenames() {
        check(
            "Community Shaders 86492 1.7.3 6Xybdafll.7z",
            "Community Shaders",
            Some("1.7.3"),
            Some(86492),
        );
        check(
            "Assorted Mesh Fixes 32117 0.139.3 s6Og0dhln.7z",
            "Assorted Mesh Fixes",
            Some("0.139.3"),
            Some(32117),
        );
        check(
            "PGPatcher 120946 1.1.4 QRde31YZG.zip",
            "PGPatcher",
            Some("1.1.4"),
            Some(120_946),
        );
    }

    #[test]
    fn falls_back_to_the_bare_stem() {
        check("1j01he.7z", "1j01he", None, None);
        check("hkulqf.zip", "hkulqf", None, None);
        check("plain-mod.zip", "plain-mod", None, None);
    }
}
