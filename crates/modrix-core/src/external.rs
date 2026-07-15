// SPDX-License-Identifier: GPL-2.0-only
//! External mods: content already installed in the game that Modrix did
//! **not** deploy.
//!
//! A user who adopts Modrix for an existing setup - or who hand-installs a
//! mod alongside it - has files in the game directory the deployment manifest
//! knows nothing about. The engine must never touch those (they are not ours to
//! enable, reorder, or delete), but a frontend should still *show* them so the
//! picture of what is installed is honest. This module reports them, read-only.
//!
//! Detection is "present but not owned and not vanilla", grouped into nameable
//! mod units. **What** to look for comes from the game definition's
//! `[[external_scan]]` entries (see [`crate::gamedef::ExternalScanDef`]) - a
//! `file` scan reports matching loose files (Bethesda plugins, SKSE DLLs), a
//! `folder` scan reports subdirectories as units (BepInEx plugins). Core has
//! no per-game layout knowledge of its own; a v1 definition that declares
//! nothing falls back to [`v1_default_scans`]. Every scan is bounded (Power
//! of Ten §9.3): no unbounded directory walk, no recursion.

use std::collections::HashSet;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};

use crate::gamedef::ExternalScanDef;

/// Most directory entries any single scan level will consider (bounded loop).
const MAX_SCAN: usize = 8192;

/// Deepest a folder file-count walk descends.
const MAX_DEPTH: u32 = 32;

/// Cap on files counted for one external folder (stops a huge tree from being
/// walked whole just to show a number).
const MAX_COUNT: usize = 100_000;

/// One mod present in the game directory but outside Modrix's management.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExternalMod {
    /// Display name (the folder or file name).
    pub name: String,
    /// The detector's human label (e.g. `plugin`, `SKSE plugin`,
    /// `BepInEx plugin`), from the game definition.
    pub label: String,
    /// Absolute path to its main artifact (the folder, or the file).
    pub path: PathBuf,
    /// Number of files it contributes (1 for a single-file plugin/DLL).
    pub files: usize,
}

/// Scan a game's deploy root for mods it holds that the deployment does not
/// own (`owned` = lowercased deployed target paths, relative to the deploy
/// root) and the base game does not ship (`base_files`, lowercased; trailing
/// `*` = prefix match). Results are sorted by label then name for a stable
/// display.
#[must_use]
pub fn scan<S: BuildHasher>(
    deploy_root: &Path,
    owned: &HashSet<String, S>,
    scans: &[ExternalScanDef],
    base_files: &[String],
) -> Vec<ExternalMod> {
    let mut out = Vec::new();
    for def in scans {
        match def.kind.as_str() {
            "file" => file_scan(deploy_root, owned, def, base_files, &mut out),
            "folder" => folder_scan(deploy_root, owned, def, &mut out),
            _ => {}
        }
    }
    out.sort_by(|a, b| {
        (a.label.as_str(), a.name.to_ascii_lowercase())
            .cmp(&(b.label.as_str(), b.name.to_ascii_lowercase()))
    });
    out
}

/// The detectors applied to a v1 definition that declares no `external_scan`:
/// exactly the layouts the pre-v2 hard-coded scanner knew. New definitions
/// declare their own instead.
#[must_use]
pub fn v1_default_scans() -> Vec<ExternalScanDef> {
    vec![
        ExternalScanDef {
            kind: "file".to_owned(),
            label: "plugin".to_owned(),
            dir: String::new(),
            exts: vec!["esp".to_owned(), "esm".to_owned(), "esl".to_owned()],
            skip_base: true,
        },
        ExternalScanDef {
            kind: "file".to_owned(),
            label: "SKSE plugin".to_owned(),
            dir: "SKSE/Plugins".to_owned(),
            exts: vec!["dll".to_owned()],
            skip_base: false,
        },
        ExternalScanDef {
            kind: "folder".to_owned(),
            label: "BepInEx plugin".to_owned(),
            dir: "BepInEx/plugins".to_owned(),
            exts: Vec::new(),
            skip_base: false,
        },
    ]
}

/// Whether a lowercased file name matches an entry in `base_files`
/// (exact, or prefix when the entry ends with `*`).
fn is_base_file(key: &str, base_files: &[String]) -> bool {
    base_files.iter().any(|b| match b.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => key == b,
    })
}

/// Loose files matching the scan's extensions that are neither deployed nor
/// shipped by the base game.
fn file_scan<S: BuildHasher>(
    deploy_root: &Path,
    owned: &HashSet<String, S>,
    def: &ExternalScanDef,
    base_files: &[String],
    out: &mut Vec<ExternalMod>,
) {
    let Some((dir, prefix)) = scan_dir(deploy_root, &def.dir) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten().take(MAX_SCAN) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let key = name.to_ascii_lowercase();
        let matches_ext = def
            .exts
            .iter()
            .any(|ext| key.rsplit('.').next() == Some(ext.as_str()) && key.contains('.'));
        if !matches_ext {
            continue;
        }
        if owned.contains(&format!("{prefix}{key}")) {
            continue;
        }
        if def.skip_base && is_base_file(&key, base_files) {
            continue;
        }
        out.push(ExternalMod {
            name,
            label: def.label.clone(),
            path,
            files: 1,
        });
    }
}

/// Subdirectories of the scan dir that no deployed target lives beneath -
/// each is one external mod (the BepInEx layout).
fn folder_scan<S: BuildHasher>(
    deploy_root: &Path,
    owned: &HashSet<String, S>,
    def: &ExternalScanDef,
    out: &mut Vec<ExternalMod>,
) {
    let Some((dir, prefix)) = scan_dir(deploy_root, &def.dir) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten().take(MAX_SCAN) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !path.is_dir() || name.starts_with('.') {
            continue;
        }
        let owned_prefix = format!("{prefix}{}/", name.to_ascii_lowercase());
        if owned.iter().any(|k| k.starts_with(&owned_prefix)) {
            continue;
        }
        let files = count_files(&path);
        out.push(ExternalMod {
            name,
            label: def.label.clone(),
            path,
            files,
        });
    }
}

/// Resolve a scan's directory under the deploy root (case-insensitively) and
/// the lowercased manifest prefix addressing entries inside it. An empty
/// `dir` is the deploy root itself (bare manifest paths).
fn scan_dir(deploy_root: &Path, dir: &str) -> Option<(PathBuf, String)> {
    if dir.is_empty() {
        return Some((deploy_root.to_path_buf(), String::new()));
    }
    let chain: Vec<&str> = dir.split('/').filter(|c| !c.is_empty()).collect();
    let resolved = resolve_ci(deploy_root, &chain)?;
    let mut prefix = chain.join("/").to_ascii_lowercase();
    prefix.push('/');
    Some((resolved, prefix))
}

/// Resolve a chain of child names under `root`, matching each component
/// case-insensitively (the on-disk `BepInEx` may differ in case from our
/// folded target paths). Returns the resolved path, or `None` if any step is
/// missing.
fn resolve_ci(root: &Path, chain: &[&str]) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for want in chain {
        let entries = std::fs::read_dir(&cur).ok()?;
        let mut next = None;
        for entry in entries.flatten().take(MAX_SCAN) {
            if entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(want)
            {
                next = Some(entry.path());
                break;
            }
        }
        cur = next?;
    }
    Some(cur)
}

/// Count files under `dir` with an explicit worklist (no recursion), bounded by
/// depth and total count.
fn count_files(dir: &Path) -> usize {
    let mut count = 0_usize;
    let mut stack = vec![(dir.to_path_buf(), 0_u32)];
    while let Some((d, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten().take(MAX_SCAN) {
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth.saturating_add(1)));
            } else {
                count = count.saturating_add(1);
                if count >= MAX_COUNT {
                    return count;
                }
            }
        }
    }
    count
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

    fn owned(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    fn skyrim_base() -> Vec<String> {
        ["skyrim.esm", "cc*"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn bepinex_folders_are_external_unless_deployed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("BepInEx/plugins/CharacterForge/forge.dll"), "x");
        write(
            &root.join("BepInEx/plugins/CharacterForge/config.json"),
            "x",
        );
        write(&root.join("BepInEx/plugins/Managed/managed.dll"), "x");
        // `Managed` was deployed by us; `CharacterForge` was hand-installed.
        let got = scan(
            root,
            &owned(&["bepinex/plugins/managed/managed.dll"]),
            &v1_default_scans(),
            &[],
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "CharacterForge");
        assert_eq!(got[0].label, "BepInEx plugin");
        assert_eq!(got[0].files, 2);
    }

    #[test]
    fn bepinex_folders_are_found_when_the_deploy_root_is_the_container() {
        let tmp = tempfile::tempdir().unwrap();
        // Subnautica's mod_root IS `BepInEx/plugins`, so the deploy root is the
        // container itself and manifest paths are bare. Its v2 definition
        // declares a folder scan with `dir = ""`.
        let root = tmp.path().join("Subnautica/BepInEx/plugins");
        write(&root.join("HandInstalled/mod.dll"), "x");
        write(&root.join("Nautilus/Nautilus.dll"), "x");
        write(&root.join("Deployed/mod.dll"), "x");
        let scans = vec![ExternalScanDef {
            kind: "folder".to_owned(),
            label: "BepInEx plugin".to_owned(),
            dir: String::new(),
            exts: Vec::new(),
            skip_base: false,
        }];
        let got = scan(&root, &owned(&["deployed/mod.dll"]), &scans, &[]);

        let names: Vec<_> = got.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["HandInstalled", "Nautilus"]);
        assert_eq!(got[0].label, "BepInEx plugin");
    }

    #[test]
    fn loose_plugins_skip_vanilla_cc_and_managed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for f in [
            "CBBE.esp",        // external
            "Skyrim.esm",      // vanilla (base_files)
            "ccBGSSSE037.esl", // Creation Club (cc* prefix)
            "Managed.esp",     // deployed by us
        ] {
            write(&root.join(f), "x");
        }
        let got = scan(
            root,
            &owned(&["managed.esp"]),
            &v1_default_scans(),
            &skyrim_base(),
        );
        let names: Vec<_> = got.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["CBBE.esp"]);
        assert_eq!(got[0].label, "plugin");
    }

    #[test]
    fn skse_dlls_are_external_unless_deployed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("SKSE/Plugins/hand_installed.dll"), "x");
        write(&root.join("SKSE/Plugins/deployed.dll"), "x");
        write(&root.join("SKSE/Plugins/readme.txt"), "x"); // not a DLL
        let got = scan(
            root,
            &owned(&["skse/plugins/deployed.dll"]),
            &v1_default_scans(),
            &skyrim_base(),
        );
        let names: Vec<_> = got.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["hand_installed.dll"]);
        assert_eq!(got[0].label, "SKSE plugin");
    }

    #[test]
    fn a_fully_managed_or_vanilla_root_reports_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("Skyrim.esm"), "x");
        write(&root.join("Managed.esp"), "x");
        assert!(
            scan(
                root,
                &owned(&["managed.esp"]),
                &v1_default_scans(),
                &skyrim_base()
            )
            .is_empty()
        );
    }

    #[test]
    fn a_definition_with_no_scans_reports_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("Something.esp"), "x");
        assert!(scan(tmp.path(), &owned(&[]), &[], &[]).is_empty());
    }
}
