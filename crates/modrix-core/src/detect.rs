// SPDX-License-Identifier: GPL-2.0-only
//! Install detection: turn a `game.toml`'s declared probe strategies into an
//! install path by reading on-disk store metadata.
//!
//! Tier-1 games carry `install_probe = ["steam", …]` (see `gamedef.rs`); this
//! module implements those strategies so a frontend can offer "this supported
//! game is installed - add it" without the user hunting for the directory. It
//! is best-effort and side-effect-free: a miss returns `None`, never an error,
//! and nothing here writes or networks. Only the `steam` strategy is
//! implemented today; `registry`/`path-hint` fall through cleanly.
//!
//! The Steam path is pure filesystem reading of Valve's `KeyValues` files
//! (`libraryfolders.vdf`, `appmanifest_<id>.acf`) - no Steam API, no network.
//! Every read is size-capped and every scan bounded (Power of Ten §9.3): a
//! malformed or hostile metadata file can waste no more than a bounded effort.

use std::path::{Path, PathBuf};

use crate::gamedef::GameDef;

/// Cap on bytes read from any Valve metadata file. These are a few KB in the
/// wild; the bound stops a huge or hostile file from being read whole.
const MAX_VDF_BYTES: u64 = 1 << 20; // 1 MiB

/// Cap on library roots scanned per Steam install (a heavy setup has a handful).
const MAX_LIBRARIES: usize = 64;

/// Cap on lines scanned in one metadata file.
const MAX_VDF_LINES: usize = 100_000;

/// Cap on quoted tokens pulled from one line (real lines have two).
const MAX_TOKENS_PER_LINE: usize = 8;

/// Locate a game's install directory using its declared probe strategies, in
/// declared order. Returns the first existing directory found, or `None` when
/// nothing detects (the caller should then offer manual entry).
#[must_use]
pub fn detect_install(def: &GameDef) -> Option<PathBuf> {
    for strategy in &def.install_probe {
        let found = match strategy.as_str() {
            "steam" => def.steam_appid.and_then(steam_install),
            // `registry` and `path-hint` are not implemented yet; skip cleanly
            // so a later implemented strategy in the list still gets its turn.
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Find where Steam installed the app with this id, if anywhere on this box.
#[must_use]
pub fn steam_install(appid: i64) -> Option<PathBuf> {
    detect_in_roots(&steam_roots(), appid)
}

/// The install-dir search over an explicit set of Steam roots - the testable
/// core of [`steam_install`], with the environment factored out.
fn detect_in_roots(roots: &[PathBuf], appid: i64) -> Option<PathBuf> {
    for root in roots {
        for library in libraries(root).into_iter().take(MAX_LIBRARIES) {
            if let Some(dir) = app_install_dir(&library, appid) {
                return Some(dir);
            }
        }
    }
    None
}

/// Existing, de-duplicated Steam root directories for this platform. Paths are
/// canonicalized so the `~/.steam/steam` symlink and its `~/.local/share/Steam`
/// target collapse to one entry (and are scanned once). Public so frontends
/// can locate Steam's local artwork cache (`appcache/librarycache/<appid>`).
pub fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        // Linux native installs (both the historical symlink names) …
        push_dir(&mut roots, &home.join(".steam/steam"));
        push_dir(&mut roots, &home.join(".steam/root"));
        push_dir(&mut roots, &home.join(".local/share/Steam"));
        // … Flatpak Steam …
        push_dir(
            &mut roots,
            &home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
        );
        // … and macOS.
        push_dir(&mut roots, &home.join("Library/Application Support/Steam"));
    }
    // Windows default install locations (registry probe not implemented).
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
            push_dir(&mut roots, &base.join("Steam"));
        }
    }
    roots
}

/// Add `p`'s canonical form to `roots` if it is an existing directory not
/// already present. `canonicalize` fails for a missing path, so this also
/// filters out candidates that do not exist.
fn push_dir(roots: &mut Vec<PathBuf>, p: &Path) {
    if let Ok(real) = p.canonicalize()
        && real.is_dir()
        && !roots.contains(&real)
    {
        roots.push(real);
    }
}

/// The library roots registered with a Steam install: the root itself plus
/// every `path` in its `libraryfolders.vdf` (games may live on other drives).
fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![root.to_path_buf()];
    // Current Steam keeps the index under `steamapps/`; older builds under
    // `config/`. Read whichever exists (both, if both do).
    for rel in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
        let Some(text) = read_capped(&root.join(rel)) else {
            continue;
        };
        for value in vdf_values(&text, "path") {
            let p = PathBuf::from(value);
            if p.is_dir() && !libs.contains(&p) {
                libs.push(p);
            }
        }
    }
    libs
}

/// If `library` holds the app, resolve its install directory from the app
/// manifest's `installdir` (relative to `steamapps/common/`).
fn app_install_dir(library: &Path, appid: i64) -> Option<PathBuf> {
    let manifest = library.join(format!("steamapps/appmanifest_{appid}.acf"));
    let text = read_capped(&manifest)?;
    let installdir = vdf_values(&text, "installdir").into_iter().next()?;
    // `installdir` is a bare directory name; reject a value that tries to
    // escape `common/` (defence-in-depth against a doctored manifest).
    if installdir.is_empty() || Path::new(&installdir).components().count() != 1 {
        return None;
    }
    let dir = library.join("steamapps/common").join(installdir);
    dir.is_dir().then_some(dir)
}

/// Read a small metadata file, bounded to [`MAX_VDF_BYTES`]. Returns `None` on
/// any error, a missing file, or an implausibly large one.
fn read_capped(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_VDF_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Every scalar value whose key equals `key`, in file order. A VDF scalar entry
/// is `"key" "value"` on one line; block headers (`"apps"` then `{`) have no
/// second token and are skipped, so a flat line scan suffices for the two keys
/// we read and never confuses a nested block for a value.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().take(MAX_VDF_LINES) {
        let tokens = quoted_tokens(line);
        if tokens.first().map(String::as_str) == Some(key)
            && let Some(value) = tokens.get(1)
        {
            out.push(value.clone());
        }
    }
    out
}

/// The double-quoted tokens on one line, unescaped, capped at
/// [`MAX_TOKENS_PER_LINE`] so a pathological line cannot allocate unboundedly.
fn quoted_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut token = String::new();
        let mut escaped = false;
        for ch in chars.by_ref() {
            match (escaped, ch) {
                (true, other) => {
                    token.push(unescape(other));
                    escaped = false;
                }
                (false, '\\') => escaped = true,
                (false, '"') => break,
                (false, other) => token.push(other),
            }
        }
        tokens.push(token);
        if tokens.len() >= MAX_TOKENS_PER_LINE {
            break;
        }
    }
    tokens
}

/// Resolve a VDF backslash escape. Notably `\\` -> `\`, which un-doubles the
/// separators in a Windows library path; unknown escapes keep the raw char.
fn unescape(ch: char) -> char {
    match ch {
        'n' => '\n',
        't' => '\t',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    /// A filesystem path as Steam writes it into a VDF: backslashes escaped
    /// (`\` -> `\\`). On Unix this is a no-op; on Windows it produces the same
    /// escaped form the real parser round-trips through.
    fn vdf_path(path: &Path) -> String {
        path.display().to_string().replace('\\', "\\\\")
    }

    #[test]
    fn vdf_values_pulls_scalar_pairs_and_skips_block_headers() {
        let text = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/games/lib\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"264710\"\t\t\"7\"\n\t\t}\n\t}\n}\n";
        assert_eq!(vdf_values(text, "path"), vec!["/games/lib".to_owned()]);
        // `apps` and `libraryfolders` are block headers: no scalar value.
        assert!(vdf_values(text, "apps").is_empty());
    }

    #[test]
    fn vdf_unescapes_windows_backslashes() {
        let text = "\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n";
        assert_eq!(
            vdf_values(text, "path"),
            vec!["D:\\SteamLibrary".to_owned()]
        );
    }

    #[test]
    fn detects_a_game_in_the_root_library() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write(
            &root.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                vdf_path(&root)
            ),
        );
        write(
            &root.join("steamapps/appmanifest_264710.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"264710\"\n\t\"installdir\"\t\t\"Subnautica\"\n}\n",
        );
        std::fs::create_dir_all(root.join("steamapps/common/Subnautica")).unwrap();

        let got = detect_in_roots(std::slice::from_ref(&root), 264_710);
        assert_eq!(got, Some(root.join("steamapps/common/Subnautica")));
    }

    #[test]
    fn detects_a_game_in_a_secondary_library() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("steam");
        let lib = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&lib).unwrap();
        write(
            &root.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                vdf_path(&root),
                vdf_path(&lib)
            ),
        );
        // The manifest lives only in the secondary library.
        write(
            &lib.join("steamapps/appmanifest_489830.acf"),
            "\"AppState\"\n{\n\t\"installdir\"\t\t\"Skyrim Special Edition\"\n}\n",
        );
        std::fs::create_dir_all(lib.join("steamapps/common/Skyrim Special Edition")).unwrap();

        let got = detect_in_roots(&[root], 489_830);
        assert_eq!(
            got,
            Some(lib.join("steamapps/common/Skyrim Special Edition"))
        );
    }

    #[test]
    fn missing_app_yields_none() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write(
            &root.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                vdf_path(&root)
            ),
        );
        assert_eq!(detect_in_roots(&[root], 999_999), None);
    }

    #[test]
    fn manifest_without_the_install_dir_on_disk_is_not_reported() {
        // A manifest can linger after a move/uninstall; only report a real dir.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write(
            &root.join("steamapps/appmanifest_264710.acf"),
            "\"AppState\"\n{\n\t\"installdir\"\t\t\"Subnautica\"\n}\n",
        );
        // No steamapps/common/Subnautica directory created.
        assert_eq!(detect_in_roots(&[root], 264_710), None);
    }

    #[test]
    fn a_manifest_installdir_cannot_escape_common() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write(
            &root.join("steamapps/appmanifest_1.acf"),
            "\"AppState\"\n{\n\t\"installdir\"\t\t\"../../../../etc\"\n}\n",
        );
        assert_eq!(detect_in_roots(&[root], 1), None);
    }
}
