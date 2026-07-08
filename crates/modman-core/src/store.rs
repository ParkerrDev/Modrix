// SPDX-License-Identifier: GPL-2.0-only
//! The mod staging store.
//!
//! A mod is an extracted archive living under the per-game staging root. This
//! module stages an extracted directory (or a zip archive) into the store and
//! enumerates a staged mod's files for the planner. Every tree walk is a bounded
//! explicit worklist, never recursion over untrusted input (Power of Ten), and
//! every archive/relative path is validated against directory-escape ("zip
//! slip") before it is trusted.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use crate::deploy::plan::ResolvedFile;
use crate::error::{Error, Result};

/// Upper bound on files in a single mod. Guards the walk and archive loops.
const MAX_FILES: usize = 200_000;
/// Upper bound on directory nesting within a mod.
const MAX_DEPTH: usize = 64;

/// Copy an already-extracted directory tree into `dest` (the mod's staged path).
///
/// # Errors
///
/// Returns [`Error::Io`] on any copy failure or [`Error::BoundExceeded`] if the
/// tree is larger or deeper than the fixed limits.
pub(crate) fn stage_extracted(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;
    let mut stack = vec![(src.to_path_buf(), 0_usize)];
    let mut copied: usize = 0;
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(Error::BoundExceeded {
                what: "mod directory depth",
                limit: MAX_DEPTH,
            });
        }
        let entries = fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::io(&dir, e))?;
            let path = entry.path();
            let rel = path.strip_prefix(src).unwrap_or(&path);
            let out = dest.join(rel);
            if path.is_dir() {
                fs::create_dir_all(&out).map_err(|e| Error::io(&out, e))?;
                stack.push((path, depth.saturating_add(1)));
            } else {
                copied = bounded_inc(copied, "mod files", MAX_FILES)?;
                if let Some(parent) = out.parent() {
                    fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                }
                fs::copy(&path, &out).map_err(|e| Error::io(&out, e))?;
            }
        }
    }
    Ok(())
}

/// Extract a zip archive into `dest`, rejecting any entry that would escape it.
///
/// # Errors
///
/// Returns [`Error::Archive`] for a malformed archive, [`Error::PathEscape`] for
/// a directory-escaping entry, or [`Error::BoundExceeded`] past the file limit.
pub(crate) fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive).map_err(|e| Error::io(archive, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Archive {
        path: archive.to_path_buf(),
        message: e.to_string(),
    })?;
    fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;

    let count = zip.len();
    if count > MAX_FILES {
        return Err(Error::BoundExceeded {
            what: "archive entries",
            limit: MAX_FILES,
        });
    }
    for index in 0..count {
        let mut entry = zip.by_index(index).map_err(|e| Error::Archive {
            path: archive.to_path_buf(),
            message: e.to_string(),
        })?;
        // `enclosed_name` returns None for any name that escapes the root.
        let Some(rel) = entry.enclosed_name() else {
            return Err(Error::PathEscape {
                path: PathBuf::from(entry.name()),
            });
        };
        let out = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| Error::io(&out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let mut sink = File::create(&out).map_err(|e| Error::io(&out, e))?;
        std::io::copy(&mut entry, &mut sink).map_err(|e| Error::io(&out, e))?;
    }
    Ok(())
}

/// Extract an archive `zip` cannot read (`.7z`, `.rar`, `.tar.*`) by
/// delegating to a system extractor. The tools run as **separate processes**
/// (nothing is linked), so the GPLv2 license gate is unaffected. Tools are
/// tried in order; extraction is validated (bounds, no symlinks) afterwards.
///
/// # Errors
///
/// Returns [`Error::Archive`] if no extractor is installed or every installed
/// one fails, [`Error::PathEscape`] if the archive smuggled a symlink, or
/// [`Error::BoundExceeded`] past the file limits.
pub(crate) fn extract_with_system(archive: &Path, dest: &Path) -> Result<()> {
    let mut last_failure: Option<String> = None;
    for bin in ["7zz", "7z", "7za", "bsdtar"] {
        // Re-create `dest` so a half-failed attempt never leaks into the next.
        let _ = fs::remove_dir_all(dest);
        fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;
        match run_extractor(bin, archive, dest) {
            Ok(ExtractorRun::Success) => return validate_extracted(dest),
            Ok(ExtractorRun::NotInstalled) => {}
            Ok(ExtractorRun::Failed(message)) => last_failure = Some(message),
            Err(error) => return Err(error),
        }
    }
    Err(Error::Archive {
        path: archive.to_path_buf(),
        message: last_failure.unwrap_or_else(|| {
            "no archive extractor found - install 7-Zip (`7z`) or libarchive (`bsdtar`)"
                .to_owned()
        }),
    })
}

/// What happened when one extractor binary was tried.
enum ExtractorRun {
    Success,
    NotInstalled,
    Failed(String),
}

fn run_extractor(bin: &str, archive: &Path, dest: &Path) -> Result<ExtractorRun> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(bin);
    if bin == "bsdtar" {
        command.arg("-xf").arg(archive).arg("-C").arg(dest);
    } else {
        // 7-Zip's `-o<dir>` takes the directory glued to the flag.
        let mut out_flag = std::ffi::OsString::from("-o");
        out_flag.push(dest);
        command.arg("x").arg("-y").arg("-bd").arg(out_flag).arg(archive);
    }
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match output {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ExtractorRun::NotInstalled),
        Err(e) => Err(Error::io(archive, e)),
        Ok(out) if out.status.success() => Ok(ExtractorRun::Success),
        Ok(out) => Ok(ExtractorRun::Failed(format!(
            "`{bin}` could not extract it: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// Validate an externally-extracted tree: bounded size/depth, no symlinks (a
/// malicious archive must not plant links that later deploy outside the game).
fn validate_extracted(root: &Path) -> Result<()> {
    let mut seen: usize = 0;
    let mut stack = vec![(root.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(Error::BoundExceeded {
                what: "archive directory depth",
                limit: MAX_DEPTH,
            });
        }
        let entries = fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::io(&dir, e))?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| Error::io(&path, e))?;
            if meta.file_type().is_symlink() {
                return Err(Error::PathEscape { path });
            }
            if meta.is_dir() {
                stack.push((path, depth.saturating_add(1)));
            } else {
                seen = bounded_inc(seen, "archive files", MAX_FILES)?;
            }
        }
    }
    Ok(())
}

/// Normalize a freshly staged tree so it deploys the way the author intended:
///
/// 1. Hoist a single wrapping directory (`skse64_2_02_06/…` → `…`), up to
///    three levels.
/// 2. If the tree then carries its own copy of the game's mod root (e.g. a
///    `Data/` directory), that directory's contents *become* the staged root
///    and everything else is parked under `.unmanaged/` - kept on disk for
///    the user (SKSE's loader binaries, `src/`), never deployed.
///
/// # Errors
///
/// Returns [`Error::Io`] on any rename failure.
pub(crate) fn normalize_staged(dest: &Path, mod_root: &str) -> Result<()> {
    // Bounded: a wrapper-in-wrapper-in-wrapper is the deepest seen in the wild.
    for _ in 0_u8..3 {
        if !hoist_single_root(dest, mod_root)? {
            break;
        }
    }
    // A FOMOD-packaged tree stays pristine (past wrapper hoisting): its
    // installer references sources relative to this root, and the FOMOD
    // engine decides the final layout - remapping would break both.
    if has_fomod(dest) {
        return Ok(());
    }
    if !mod_root.is_empty() {
        remap_mod_root(dest, mod_root)?;
    }
    Ok(())
}

/// Whether the tree root carries a `fomod/ModuleConfig.xml` (any case).
fn has_fomod(dest: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dest) else {
        return false;
    };
    for entry in entries.flatten().take(MAX_FILES) {
        let path = entry.path();
        if path.is_dir()
            && file_name_of(&path).eq_ignore_ascii_case("fomod")
            && fs::read_dir(&path).is_ok_and(|mut inner| {
                inner.any(|e| {
                    e.is_ok_and(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .eq_ignore_ascii_case("ModuleConfig.xml")
                    })
                })
            })
        {
            return true;
        }
    }
    false
}

/// Directory names that ARE mod content, never a wrapper to hoist. A lone
/// `meshes/` is a mod that ships meshes; a lone `SkyUI_5_2/` is packaging.
/// (Conservative, Bethesda-centric for now; game plugins can extend later.)
const CONTENT_DIRS: [&str; 14] = [
    "meshes",
    "textures",
    "scripts",
    "interface",
    "sound",
    "music",
    "strings",
    "seq",
    "grass",
    "shadersfx",
    "lodsettings",
    "skse",
    "fomod",
    "source",
];

fn is_content_dir(name: &str, mod_root: &str) -> bool {
    name.eq_ignore_ascii_case(mod_root)
        || CONTENT_DIRS.iter().any(|c| name.eq_ignore_ascii_case(c))
}

/// The visible top-level entries of `dir`.
fn top_entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        out.push(entry.path());
    }
    Ok(out)
}

/// If `dest` holds exactly one *wrapper* directory and nothing else, replace
/// `dest`'s contents with that directory's contents. Returns whether it
/// hoisted. Content directories (the mod root, `meshes/`, …) never hoist.
fn hoist_single_root(dest: &Path, mod_root: &str) -> Result<bool> {
    let entries = top_entries(dest)?;
    let [only] = entries.as_slice() else {
        return Ok(false);
    };
    let name = file_name_of(only).into_owned();
    if !only.is_dir() || name.starts_with('.') || is_content_dir(&name, mod_root) {
        return Ok(false);
    }
    // Rename aside first so a child sharing the wrapper's name cannot collide.
    let tmp = dest.join(".mm-hoist");
    fs::rename(only, &tmp).map_err(|e| Error::io(only, e))?;
    for child in top_entries(&tmp)? {
        let target = dest.join(file_name_of(&child).as_ref());
        fs::rename(&child, &target).map_err(|e| Error::io(&child, e))?;
    }
    fs::remove_dir(&tmp).map_err(|e| Error::io(&tmp, e))?;
    Ok(true)
}

/// If the tree carries a `<mod_root>` directory (case-insensitive), make its
/// contents the staged root and park the rest under `.unmanaged/`.
fn remap_mod_root(dest: &Path, mod_root: &str) -> Result<()> {
    let entries = top_entries(dest)?;
    let Some(root_dir) = entries
        .iter()
        .find(|p| p.is_dir() && file_name_of(p).eq_ignore_ascii_case(mod_root))
        .cloned()
    else {
        return Ok(());
    };
    let park = dest.join(".unmanaged");
    fs::create_dir_all(&park).map_err(|e| Error::io(&park, e))?;
    for entry in &entries {
        if *entry == root_dir || file_name_of(entry).starts_with('.') {
            continue;
        }
        let target = park.join(file_name_of(entry).as_ref());
        fs::rename(entry, &target).map_err(|e| Error::io(entry, e))?;
    }
    let tmp = dest.join(".mm-remap");
    fs::rename(&root_dir, &tmp).map_err(|e| Error::io(&root_dir, e))?;
    for child in top_entries(&tmp)? {
        let target = dest.join(file_name_of(&child).as_ref());
        fs::rename(&child, &target).map_err(|e| Error::io(&child, e))?;
    }
    fs::remove_dir(&tmp).map_err(|e| Error::io(&tmp, e))?;
    Ok(())
}

/// A path's final component as a lossy string (empty when absent).
fn file_name_of(path: &Path) -> std::borrow::Cow<'_, str> {
    path.file_name()
        .map_or(std::borrow::Cow::Borrowed(""), |n| n.to_string_lossy())
}

/// Enumerate a staged mod's files as planner input, in a stable sorted order.
/// Entries whose name starts with `.` are skipped - `.unmanaged/` parking,
/// `.git/`, `.DS_Store` and friends never deploy.
///
/// # Errors
///
/// Returns [`Error::Io`] on a read failure, [`Error::BoundExceeded`] past the
/// limits, or [`Error::PathEscape`] if a path cannot be expressed safely.
pub(crate) fn resolve_files(staged_root: &Path) -> Result<Vec<ResolvedFile>> {
    let mut files = Vec::new();
    let mut stack = vec![(staged_root.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(Error::BoundExceeded {
                what: "mod directory depth",
                limit: MAX_DEPTH,
            });
        }
        let entries = fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::io(&dir, e))?;
            let path = entry.path();
            if file_name_of(&path).starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push((path, depth.saturating_add(1)));
            } else {
                let _ = bounded_inc(files.len(), "mod files", MAX_FILES)?;
                let rel = relative_target(staged_root, &path)?;
                files.push(ResolvedFile {
                    target_rel: rel,
                    source: path,
                });
            }
        }
    }
    // Sorted output keeps planning and reports deterministic regardless of the
    // order the filesystem hands back directory entries.
    files.sort_by(|a, b| a.target_rel.cmp(&b.target_rel));
    Ok(files)
}

/// Turn an absolute staged path into a `/`-separated relative target, rejecting
/// anything that escapes the staging root.
fn relative_target(root: &Path, path: &Path) -> Result<String> {
    let rel = path.strip_prefix(root).map_err(|_| Error::PathEscape {
        path: path.to_path_buf(),
    })?;
    let mut out = String::new();
    for component in rel.components() {
        match component {
            std::path::Component::Normal(part) => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(&part.to_string_lossy());
            }
            // Anything other than a plain name (`..`, a root, a prefix) is an escape.
            _ => {
                return Err(Error::PathEscape {
                    path: path.to_path_buf(),
                });
            }
        }
    }
    Ok(out)
}

/// Increment a bounded counter, erroring if it would reach `limit`.
fn bounded_inc(current: usize, what: &'static str, limit: usize) -> Result<usize> {
    let next = current.checked_add(1).filter(|n| *n <= limit);
    next.ok_or(Error::BoundExceeded { what, limit })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn stage_and_resolve_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("extracted");
        write(&src.join("a.esp"), b"a");
        write(&src.join("meshes/x.nif"), b"x");
        let dest = tmp.path().join("staged");
        stage_extracted(&src, &dest).unwrap();

        let files = resolve_files(&dest).unwrap();
        let rels: Vec<_> = files.iter().map(|f| f.target_rel.clone()).collect();
        assert_eq!(rels, vec!["a.esp".to_owned(), "meshes/x.nif".to_owned()]);
        assert_eq!(fs::read(&files[0].source).unwrap(), b"a");
    }

    #[test]
    fn normalize_hoists_wrapper_and_remaps_data_like_skse() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        // The exact SKSE layout: one version dir wrapping loaders + Data + src.
        write(&dest.join("skse64_2_02_06/skse64_loader.exe"), b"exe");
        write(&dest.join("skse64_2_02_06/skse64_1_6_1170.dll"), b"dll");
        write(&dest.join("skse64_2_02_06/Data/Scripts/Actor.pex"), b"pex");
        write(&dest.join("skse64_2_02_06/src/skse64/main.cpp"), b"cpp");
        normalize_staged(&dest, "Data").unwrap();

        // Scripts became the mod root; loaders/src parked, never deployed.
        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(rels, vec!["Scripts/Actor.pex".to_owned()]);
        assert!(dest.join(".unmanaged/skse64_loader.exe").is_file());
        assert!(dest.join(".unmanaged/src/skse64/main.cpp").is_file());
    }

    #[test]
    fn normalize_leaves_plain_mods_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        write(&dest.join("SkyUI.esp"), b"esp");
        write(&dest.join("textures/ui.dds"), b"dds");
        normalize_staged(&dest, "Data").unwrap();
        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(rels, vec!["SkyUI.esp".to_owned(), "textures/ui.dds".to_owned()]);
    }

    #[test]
    fn normalize_hoists_a_single_wrapper_without_mod_root() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        write(&dest.join("MyMod-1.0/meshes/m.nif"), b"m");
        write(&dest.join("MyMod-1.0/MyMod.esp"), b"e");
        normalize_staged(&dest, "Data").unwrap();
        assert!(dest.join("MyMod.esp").is_file());
        assert!(dest.join("meshes/m.nif").is_file());
        assert!(!dest.join("MyMod-1.0").exists());
    }

    #[test]
    fn normalize_never_dissolves_a_lone_content_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        // A mod that IS just meshes must stay under meshes/.
        write(&dest.join("meshes/armor/a.nif"), b"m");
        normalize_staged(&dest, "Data").unwrap();
        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(rels, vec!["meshes/armor/a.nif".to_owned()]);
    }

    #[test]
    fn resolve_files_skips_hidden_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("m");
        write(&src.join("a.esp"), b"a");
        write(&src.join(".unmanaged/loader.exe"), b"x");
        write(&src.join(".DS_Store"), b"junk");
        let rels: Vec<_> = resolve_files(&src)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(rels, vec!["a.esp".to_owned()]);
    }

    #[test]
    fn system_extraction_reports_missing_archive_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let err = extract_with_system(&tmp.path().join("nope.7z"), &tmp.path().join("out"))
            .unwrap_err();
        assert!(matches!(err, Error::Archive { .. }));
    }

    #[test]
    fn system_extraction_roundtrips_a_tar_when_a_tool_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("payload");
        write(&src.join("inner/Data/Scripts/s.pex"), b"pex");
        // Build the archive with whichever tool exists; skip the test if none.
        let archive = tmp.path().join("mod.tar");
        let built = std::process::Command::new("bsdtar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg(".")
            .status()
            .is_ok_and(|s| s.success());
        if !built {
            // No bsdtar on this machine - nothing to exercise.
            return;
        }
        let dest = tmp.path().join("staged");
        extract_with_system(&archive, &dest).unwrap();
        assert!(dest.join("inner/Data/Scripts/s.pex").is_file());
        normalize_staged(&dest, "Data").unwrap();
        assert!(dest.join("Scripts/s.pex").is_file());
    }

    #[test]
    fn resolve_files_is_sorted_and_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("m");
        for name in ["z.esp", "a.esp", "m/b.esp"] {
            write(&src.join(name), b"x");
        }
        let files = resolve_files(&src).unwrap();
        let rels: Vec<_> = files.iter().map(|f| f.target_rel.clone()).collect();
        assert_eq!(
            rels,
            vec!["a.esp".to_owned(), "m/b.esp".to_owned(), "z.esp".to_owned()]
        );
    }
}
