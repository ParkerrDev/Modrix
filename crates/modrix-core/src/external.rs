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
//! mod units per ecosystem: loose game plugins and `SKSE/Plugins` DLLs (Bethesda)
//! and `BepInEx/plugins/<Mod>/` folders (Unity/BepInEx). Each detector is a
//! no-op when its layout is absent, so one scan serves every game. Every scan is
//! bounded (Power of Ten §9.3): no unbounded directory walk, no recursion.

use std::collections::HashSet;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};

/// Most directory entries any single scan level will consider (bounded loop).
const MAX_SCAN: usize = 8192;

/// Deepest a folder file-count walk descends.
const MAX_DEPTH: u32 = 32;

/// Cap on files counted for one external folder (stops a huge tree from being
/// walked whole just to show a number).
const MAX_COUNT: usize = 100_000;

/// What kind of external content this is - drives the frontend's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalKind {
    /// A loose game plugin (`.esp`/`.esl`/`.esm`) not deployed by Modrix.
    Plugin,
    /// An SKSE plugin DLL under `SKSE/Plugins/`.
    SksePlugin,
    /// A BepInEx plugin folder under `BepInEx/plugins/`.
    BepInExPlugin,
}

impl ExternalKind {
    /// A short human label for the kind.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::SksePlugin => "SKSE plugin",
            Self::BepInExPlugin => "BepInEx plugin",
        }
    }
}

/// One mod present in the game directory but outside Modrix's management.
#[derive(Debug, Clone)]
pub struct ExternalMod {
    /// Display name (the folder or plugin file name).
    pub name: String,
    /// Which ecosystem/layout it was found in.
    pub kind: ExternalKind,
    /// Absolute path to its main artifact (the folder, or the file).
    pub path: PathBuf,
    /// Number of files it contributes (1 for a single-file plugin/DLL).
    pub files: usize,
}

/// Scan a game's deploy root for mods it holds that the deployment does not own
/// (`owned` = lowercased deployed target paths) and the base game does not ship.
/// Results are sorted by kind then name for a stable display.
#[must_use]
pub fn scan<S: BuildHasher>(
    mod_root: &Path,
    owned: &HashSet<String, S>,
    steam_appid: Option<i64>,
) -> Vec<ExternalMod> {
    let mut out = Vec::new();
    loose_plugins(mod_root, owned, steam_appid, &mut out);
    skse_plugins(mod_root, owned, &mut out);
    bepinex_plugins(mod_root, owned, &mut out);
    out.sort_by(|a, b| {
        (a.kind.label(), a.name.to_ascii_lowercase())
            .cmp(&(b.kind.label(), b.name.to_ascii_lowercase()))
    });
    out
}

/// Loose plugins at the deploy root that are neither deployed nor vanilla.
fn loose_plugins<S: BuildHasher>(
    mod_root: &Path,
    owned: &HashSet<String, S>,
    steam_appid: Option<i64>,
    out: &mut Vec<ExternalMod>,
) {
    let base = crate::health::base_plugins(steam_appid);
    let Ok(entries) = std::fs::read_dir(mod_root) else {
        return;
    };
    for entry in entries.flatten().take(MAX_SCAN) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let key = name.to_ascii_lowercase();
        if crate::esp::is_plugin_name(&name)
            && !owned.contains(&key)
            && !base.contains(&key.as_str())
            && !key.starts_with("cc")
        {
            out.push(ExternalMod {
                name,
                kind: ExternalKind::Plugin,
                path,
                files: 1,
            });
        }
    }
}

/// SKSE plugin DLLs under `SKSE/Plugins/` we did not deploy. The base game
/// ships no such directory, so any unowned DLL there is external.
fn skse_plugins<S: BuildHasher>(
    mod_root: &Path,
    owned: &HashSet<String, S>,
    out: &mut Vec<ExternalMod>,
) {
    let Some(dir) = resolve_ci(mod_root, &["SKSE", "Plugins"]) else {
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
        if !name.to_ascii_lowercase().ends_with(".dll") {
            continue;
        }
        let key = format!("skse/plugins/{}", name.to_ascii_lowercase());
        if owned.contains(&key) {
            continue;
        }
        out.push(ExternalMod {
            name,
            kind: ExternalKind::SksePlugin,
            path,
            files: 1,
        });
    }
}

/// BepInEx plugin folders we did not deploy. A folder is ours if any deployed
/// target lives beneath it; the base game ships no BepInEx tree, so every other
/// folder is an external mod.
fn bepinex_plugins<S: BuildHasher>(
    mod_root: &Path,
    owned: &HashSet<String, S>,
    out: &mut Vec<ExternalMod>,
) {
    let Some((plugins, prefix)) = bepinex_container(mod_root) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&plugins) else {
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
            kind: ExternalKind::BepInExPlugin,
            path,
            files,
        });
    }
}

/// The BepInEx plugin container under the deploy root, paired with the manifest
/// prefix that addresses entries inside it.
///
/// Manifest paths are relative to the game's mod root, so where the container
/// sits decides how a deployed plugin is addressed. When the mod root points
/// straight at the container (Subnautica's `BepInEx/plugins`) the deploy root
/// *is* the container and its entries are addressed bare. When the mod root is
/// the install root the container sits beneath it, and entries carry the
/// `bepinex/plugins/` prefix the deployer's case-folding produced.
fn bepinex_container(deploy_root: &Path) -> Option<(PathBuf, String)> {
    if is_plugins_dir(deploy_root) {
        return Some((deploy_root.to_path_buf(), String::new()));
    }
    let dir = resolve_ci(deploy_root, &["BepInEx", "plugins"])?;
    Some((dir, "bepinex/plugins/".to_owned()))
}

/// Whether `path` is itself a `BepInEx/plugins` directory.
fn is_plugins_dir(path: &Path) -> bool {
    let named = |name: Option<&std::ffi::OsStr>, want: &str| {
        name.is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(want))
    };
    named(path.file_name(), "plugins") && named(path.parent().and_then(Path::file_name), "BepInEx")
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
        let got = scan(root, &owned(&["bepinex/plugins/managed/managed.dll"]), None);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "CharacterForge");
        assert_eq!(got[0].kind, ExternalKind::BepInExPlugin);
        assert_eq!(got[0].files, 2);
    }

    #[test]
    fn bepinex_folders_are_found_when_the_deploy_root_is_the_container() {
        let tmp = tempfile::tempdir().unwrap();
        // Subnautica's mod_root IS `BepInEx/plugins`, so the deploy root is the
        // container itself and manifest paths are bare. Regression: the scan
        // used to descend into `<root>/BepInEx/plugins` unconditionally, which
        // under this mod_root looked for `BepInEx/plugins/BepInEx/plugins` and
        // silently reported nothing installed.
        let root = tmp.path().join("Subnautica/BepInEx/plugins");
        write(&root.join("HandInstalled/mod.dll"), "x");
        write(&root.join("Nautilus/Nautilus.dll"), "x");
        write(&root.join("Deployed/mod.dll"), "x");
        let got = scan(&root, &owned(&["deployed/mod.dll"]), Some(264_710));

        let names: Vec<_> = got.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["HandInstalled", "Nautilus"]);
        assert_eq!(got[0].kind, ExternalKind::BepInExPlugin);
    }

    #[test]
    fn loose_plugins_skip_vanilla_cc_and_managed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for f in [
            "CBBE.esp",        // external
            "Skyrim.esm",      // vanilla
            "ccBGSSSE037.esl", // Creation Club
            "Managed.esp",     // deployed by us
        ] {
            write(&root.join(f), "x");
        }
        let got = scan(root, &owned(&["managed.esp"]), Some(489_830));
        let names: Vec<_> = got.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["CBBE.esp"]);
        assert_eq!(got[0].kind, ExternalKind::Plugin);
    }

    #[test]
    fn skse_dlls_are_external_unless_deployed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("SKSE/Plugins/hand_installed.dll"), "x");
        write(&root.join("SKSE/Plugins/deployed.dll"), "x");
        write(&root.join("SKSE/Plugins/readme.txt"), "x"); // not a DLL
        let got = scan(root, &owned(&["skse/plugins/deployed.dll"]), Some(489_830));
        let names: Vec<_> = got.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["hand_installed.dll"]);
        assert_eq!(got[0].kind, ExternalKind::SksePlugin);
    }

    #[test]
    fn a_fully_managed_or_vanilla_root_reports_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("Skyrim.esm"), "x");
        write(&root.join("Managed.esp"), "x");
        assert!(scan(root, &owned(&["managed.esp"]), Some(489_830)).is_empty());
    }
}
