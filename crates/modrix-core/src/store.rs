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
            "no archive extractor found - install 7-Zip (`7z`) or libarchive (`bsdtar`)".to_owned()
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
        command
            .arg("x")
            .arg("-y")
            .arg("-bd")
            .arg(out_flag)
            .arg(archive);
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
/// Stamp every file under `root` with the current time.
///
/// Bethesda's engine prefers a BSA over a loose file whose timestamp is
/// older than the archive's, so a mod extracted with preserved (often
/// years-old) mtimes deploys cleanly yet silently loses to the game's own
/// BSAs - `7z` and `bsdtar` both preserve archive mtimes. Refreshing the
/// staged tree makes every deployed hardlink/copy strictly newer than any
/// game archive. (Found the hard way: CBBE, 2017 mtimes, inert in-game.)
///
/// # Errors
///
/// Returns [`Error::Io`] if a timestamp cannot be written or
/// [`Error::BoundExceeded`] past the fixed tree limits.
pub(crate) fn refresh_mtimes(root: &Path) -> Result<()> {
    let now = std::time::SystemTime::now();
    let mut stack = vec![(root.to_path_buf(), 0_usize)];
    let mut stamped: usize = 0;
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
            if path.is_dir() {
                stack.push((path, depth.saturating_add(1)));
            } else {
                stamped = bounded_inc(stamped, "mod files", MAX_FILES)?;
                stamp_now(&path, now)?;
            }
        }
    }
    Ok(())
}

/// Set one file's modification time, granting the owner write permission
/// first if the archive shipped the member read-only.
fn stamp_now(path: &Path, now: std::time::SystemTime) -> Result<()> {
    let opened = match File::options().write(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            make_owner_writable(path)?;
            File::options()
                .write(true)
                .open(path)
                .map_err(|retry| Error::io(path, retry))?
        }
        Err(e) => return Err(Error::io(path, e)),
    };
    opened.set_modified(now).map_err(|e| Error::io(path, e))
}

#[cfg(unix)]
fn make_owner_writable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| Error::io(path, e))?
        .permissions();
    perms.set_mode(perms.mode() | 0o200);
    fs::set_permissions(path, perms).map_err(|e| Error::io(path, e))
}

#[cfg(not(unix))]
fn make_owner_writable(path: &Path) -> Result<()> {
    let mut perms = fs::metadata(path)
        .map_err(|e| Error::io(path, e))?
        .permissions();
    #[expect(
        clippy::permissions_set_readonly_false,
        reason = "Windows has no per-user write bits; clearing read-only is the only lever"
    )]
    perms.set_readonly(false);
    fs::set_permissions(path, perms).map_err(|e| Error::io(path, e))
}

pub(crate) fn normalize_staged(dest: &Path, mod_root: &str, content_dirs: &[String]) -> Result<()> {
    let ctx = ContentCtx {
        mod_root,
        content_dirs,
    };
    // Bounded: a wrapper-in-wrapper-in-wrapper is the deepest seen in the wild.
    for _ in 0_u8..3 {
        if hoist_single_root(dest, &ctx)? {
            continue;
        }
        if !hoist_lone_content_dir(dest, &ctx)? {
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
        // A single-component mod root (Bethesda's `Data`) sits beside the game
        // binary, so a top-level .exe/.dll is a loader that belongs at the
        // install root. A *nested* mod root (`BepInEx/plugins`) IS the plugin
        // container: a top-level DLL there is the mod itself, and parking it
        // would deploy the plugin to the game root where nothing loads it.
        if !mod_root.contains('/') {
            // Park loader binaries first - the mod-root remap would otherwise
            // sweep them into `.unmanaged/` as generic extras.
            park_root_binaries(dest)?;
        }
        remap_mod_root(dest, mod_root)?;
    }
    Ok(())
}

/// Move top-level executables/libraries into `.root/`: they belong next to
/// the game binary (SKSE loaders, preloader DLLs), never inside the mod
/// root - the deployer places `.root/` contents at the game install root.
fn park_root_binaries(dest: &Path) -> Result<()> {
    for entry in top_entries(dest)? {
        let name = file_name_of(&entry).into_owned();
        let root_worthy = entry
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("dll"));
        if entry.is_dir() || name.starts_with('.') || !root_worthy {
            continue;
        }
        let park = dest.join(".root");
        fs::create_dir_all(&park).map_err(|e| Error::io(&park, e))?;
        let target = park.join(&name);
        fs::rename(&entry, &target).map_err(|e| Error::io(&entry, e))?;
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

/// What counts as mod content during normalization: the game's mod root plus
/// its definition's `content_dirs` list (or the built-in default when the
/// definition declares none - v1 compatibility).
struct ContentCtx<'a> {
    mod_root: &'a str,
    content_dirs: &'a [String],
}

/// The default content-dir list applied when a definition declares none.
/// Kept for v1 definitions; v2 definitions ship their own list (see
/// `games/*/game.toml`), so this table no longer needs to grow.
const DEFAULT_CONTENT_DIRS: [&str; 18] = [
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
    "shaders",
    "mcm",
    "bepinex",
    "qmods",
];

impl ContentCtx<'_> {
    /// Whether a directory name IS mod content, never a wrapper to hoist. A
    /// lone `meshes/` is a mod that ships meshes; a lone `SkyUI_5_2/` is
    /// packaging.
    fn is_content_dir(&self, name: &str) -> bool {
        if name.eq_ignore_ascii_case(self.mod_root) {
            return true;
        }
        if self.content_dirs.is_empty() {
            DEFAULT_CONTENT_DIRS
                .iter()
                .any(|c| name.eq_ignore_ascii_case(c))
        } else {
            self.content_dirs
                .iter()
                .any(|c| name.eq_ignore_ascii_case(c))
        }
    }
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
fn hoist_single_root(dest: &Path, ctx: &ContentCtx<'_>) -> Result<bool> {
    let entries = top_entries(dest)?;
    let [only] = entries.as_slice() else {
        return Ok(false);
    };
    let name = file_name_of(only).into_owned();
    if !only.is_dir() || name.starts_with('.') || ctx.is_content_dir(&name) {
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

/// A loose file the game itself reads - if one sits at the staged root, the
/// layout is already deployable and must not be second-guessed.
fn is_game_file(name: &str) -> bool {
    const EXTS: [&str; 6] = ["esp", "esm", "esl", "bsa", "ba2", "ini"];
    Path::new(name)
        .extension()
        .is_some_and(|e| EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Whether `dir`'s top level carries recognizable game content: the mod
/// root (`Data/`), a known content directory, or a loose game file.
fn contains_game_content(dir: &Path, ctx: &ContentCtx<'_>) -> Result<bool> {
    Ok(top_entries(dir)?.iter().any(|p| {
        let name = file_name_of(p).into_owned();
        if p.is_dir() {
            ctx.is_content_dir(&name)
        } else {
            is_game_file(&name)
        }
    }))
}

/// An archive that packs its real tree in one folder next to loose extras
/// (screenshots, readmes) - common on older Nexus uploads. If the only
/// top-level directory carries game content and none of the loose files do,
/// park the extras under `.unmanaged/` and hoist the directory. Returns
/// whether it hoisted.
fn hoist_lone_content_dir(dest: &Path, ctx: &ContentCtx<'_>) -> Result<bool> {
    let mut dirs = Vec::new();
    let mut extras = Vec::new();
    for entry in top_entries(dest)? {
        let name = file_name_of(&entry).into_owned();
        if name.starts_with('.') {
            continue;
        }
        if entry.is_dir() {
            dirs.push((entry, name));
        } else {
            extras.push((entry, name));
        }
    }
    let [(dir, name)] = dirs.as_slice() else {
        return Ok(false);
    };
    // A bare wrapper is hoist_single_root's job; a content dir stays put; a
    // loose game file means the root is already the intended layout.
    if extras.is_empty()
        || ctx.is_content_dir(name)
        || extras.iter().any(|(_, n)| is_game_file(n))
        || !contains_game_content(dir, ctx)?
    {
        return Ok(false);
    }
    let park = dest.join(".unmanaged");
    fs::create_dir_all(&park).map_err(|e| Error::io(&park, e))?;
    for (path, extra) in &extras {
        let target = park.join(extra);
        fs::rename(path, &target).map_err(|e| Error::io(path, e))?;
    }
    let tmp = dest.join(".mm-hoist");
    fs::rename(dir, &tmp).map_err(|e| Error::io(dir, e))?;
    for child in top_entries(&tmp)? {
        let target = dest.join(file_name_of(&child).as_ref());
        fs::rename(&child, &target).map_err(|e| Error::io(&child, e))?;
    }
    fs::remove_dir(&tmp).map_err(|e| Error::io(&tmp, e))?;
    Ok(true)
}

/// Strip a redundant `<mod_root>` prefix the archive already carries, so the
/// deployer's own mod-root prefix cannot double it.
///
/// A single component (`Data`) is the classic Bethesda case. A nested root
/// (`BepInEx/plugins`) is the Unity case: some archives ship the full
/// `BepInEx/plugins/<Mod>/…` path while most ship the mod's files bare, and
/// both must land in the same place.
fn remap_mod_root(dest: &Path, mod_root: &str) -> Result<()> {
    let components: Vec<&str> = mod_root.split('/').filter(|c| !c.is_empty()).collect();
    match components.as_slice() {
        [] => Ok(()),
        [single] => remap_single_root(dest, single),
        nested => remap_nested_root(dest, nested),
    }
}

/// The nested-root remap. Only strips when every level below the first holds
/// nothing but the next component - a mod that also ships, say,
/// `BepInEx/config/` is left untouched rather than have that silently dropped.
fn remap_nested_root(dest: &Path, components: &[&str]) -> Result<()> {
    // Verify the whole chain before touching anything.
    let Some(first) = components.first() else {
        return Ok(());
    };
    let Some(top) = find_child_dir_ci(dest, first)? else {
        return Ok(());
    };
    let mut current = top.clone();
    for want in components.iter().skip(1) {
        let visible: Vec<PathBuf> = top_entries(&current)?
            .into_iter()
            .filter(|p| !file_name_of(p).starts_with('.'))
            .collect();
        let [only] = visible.as_slice() else {
            return Ok(());
        };
        if !only.is_dir() || !file_name_of(only).eq_ignore_ascii_case(want) {
            return Ok(());
        }
        current.clone_from(only);
    }
    // Park the wrapper's siblings (readmes and such), as the single-root remap does.
    let park = dest.join(".unmanaged");
    for entry in top_entries(dest)? {
        let name = file_name_of(&entry).into_owned();
        if entry == top || name.starts_with('.') {
            continue;
        }
        fs::create_dir_all(&park).map_err(|e| Error::io(&park, e))?;
        let target = park.join(&name);
        fs::rename(&entry, &target).map_err(|e| Error::io(&entry, e))?;
    }
    // Move the deepest directory out, drop the now-empty chain, then make its
    // children the staged root.
    let tmp = dest.join(".mm-remap");
    fs::rename(&current, &tmp).map_err(|e| Error::io(&current, e))?;
    fs::remove_dir_all(&top).map_err(|e| Error::io(&top, e))?;
    for child in top_entries(&tmp)? {
        let target = dest.join(file_name_of(&child).as_ref());
        fs::rename(&child, &target).map_err(|e| Error::io(&child, e))?;
    }
    fs::remove_dir(&tmp).map_err(|e| Error::io(&tmp, e))?;
    Ok(())
}

/// The child directory of `dir` whose name matches `want` case-insensitively.
fn find_child_dir_ci(dir: &Path, want: &str) -> Result<Option<PathBuf>> {
    Ok(top_entries(dir)?
        .into_iter()
        .find(|p| p.is_dir() && file_name_of(p).eq_ignore_ascii_case(want)))
}

/// If the tree carries a `<mod_root>` directory (case-insensitive), make its
/// contents the staged root and park the rest under `.unmanaged/`.
fn remap_single_root(dest: &Path, mod_root: &str) -> Result<()> {
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
        let name = file_name_of(entry).into_owned();
        if *entry == root_dir || name.starts_with('.') {
            continue;
        }
        let target = park.join(&name);
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
/// Canonicalize a target's directory components to lowercase.
///
/// On a case-sensitive filesystem (Linux/Proton), mods that package the same
/// logical tree with different casing (`Meshes/Actors/` vs `meshes/actors/`)
/// deploy as PARALLEL trees - and Wine resolves each requested path into
/// exactly one variant with no backtracking, so files in the other trees are
/// invisible to the game (found the hard way: XP32's `Meshes/Actors/…`
/// captured the body-mesh lookup while CBBE's bodies sat unreachable in
/// `meshes/actors/…`). Folding directory components lands every mod in one
/// canonical tree; Wine's case-insensitive fallback then always finds it.
/// Basenames keep their case - a lone file never forks the directory walk,
/// and root-level names (`SKSE loaders`, `.esp`s) stay recognizable.
fn fold_target_dirs(rel: &str) -> String {
    match rel.rfind('/') {
        Some(split) => {
            let (dirs, base) = rel.split_at(split);
            let mut folded = dirs.to_lowercase();
            folded.push_str(base);
            folded
        }
        None => rel.to_owned(),
    }
}

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
                    target_rel: fold_target_dirs(&rel),
                    source: path,
                });
            }
        }
    }
    // Game-root files (SKSE loaders, preloader DLLs) live under `.root/` and
    // deploy above the mod root; the planner rewrites the marker.
    let root_dir = staged_root.join(".root");
    if root_dir.is_dir() {
        let mut stack = vec![(root_dir.clone(), 0_usize)];
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
                if path.is_dir() {
                    stack.push((path, depth.saturating_add(1)));
                } else {
                    let _ = bounded_inc(files.len(), "mod files", MAX_FILES)?;
                    let rel = relative_target(&root_dir, &path)?;
                    files.push(ResolvedFile {
                        target_rel: format!("<root>/{rel}"),
                        source: path,
                    });
                }
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
                let part = part.to_string_lossy();
                // `<`/`>` are invalid in Windows names and reserved here for
                // the engine's own path markers (`<up>`) - never trusted from
                // an archive.
                if part.contains('<') || part.contains('>') {
                    return Err(Error::PathEscape {
                        path: path.to_path_buf(),
                    });
                }
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(&part);
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
    fn resolve_files_folds_directory_case_into_one_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("m");
        write(&src.join("Meshes/Actors/Character/FemaleBody_1.nif"), b"n");
        write(&src.join("Textures/skin.dds"), b"t");
        write(&src.join("CBBE.esp"), b"p");
        let files = resolve_files(&src).unwrap();
        let rels: Vec<_> = files.iter().map(|f| f.target_rel.clone()).collect();
        assert_eq!(
            rels,
            vec![
                "CBBE.esp".to_owned(), // basename case survives
                "meshes/actors/character/FemaleBody_1.nif".to_owned(),
                "textures/skin.dds".to_owned(),
            ]
        );
    }

    #[test]
    fn refresh_mtimes_stamps_every_file_fresh() {
        use std::time::{Duration, SystemTime};
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("staged");
        write(&staged.join("meshes/body.nif"), b"n");
        write(&staged.join("a.esp"), b"a");
        // Backdate both files ~9 years, the way 7z/bsdtar preserve archive
        // mtimes (the CBBE case: loose files older than the game's BSAs).
        let old = SystemTime::now()
            .checked_sub(Duration::from_hours(78_840))
            .unwrap();
        for rel in ["meshes/body.nif", "a.esp"] {
            File::options()
                .write(true)
                .open(staged.join(rel))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let floor = SystemTime::now()
            .checked_sub(Duration::from_mins(1))
            .unwrap();
        refresh_mtimes(&staged).unwrap();
        for rel in ["meshes/body.nif", "a.esp"] {
            let mtime = fs::metadata(staged.join(rel)).unwrap().modified().unwrap();
            assert!(mtime > floor, "{rel} kept its stale archive mtime");
        }
    }

    #[test]
    fn refresh_mtimes_handles_read_only_members() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("staged");
        let locked = staged.join("textures/skin.dds");
        write(&locked, b"d");
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&locked, perms).unwrap();

        refresh_mtimes(&staged).unwrap();
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
    fn normalize_hoists_a_content_dir_packed_beside_screenshots() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        // The One Ring Redux layout: loose screenshots + one real tree.
        write(&dest.join("First Person.jpg"), b"jpg");
        write(&dest.join("Alchemy Lab.jpg"), b"jpg");
        write(&dest.join("The One Ring Redux/Readme.txt"), b"txt");
        write(&dest.join("The One Ring Redux/Data/TheOneRing.esp"), b"esp");
        write(
            &dest.join("The One Ring Redux/Data/Scripts/OneRingScript.pex"),
            b"pex",
        );
        normalize_staged(&dest, "Data", &[]).unwrap();

        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(
            rels,
            vec![
                "TheOneRing.esp".to_owned(),
                "scripts/OneRingScript.pex".to_owned(),
            ]
        );
        // The extras survive on disk but never deploy.
        assert!(dest.join(".unmanaged/First Person.jpg").is_file());
        assert!(dest.join(".unmanaged/Readme.txt").is_file());
    }

    #[test]
    fn normalize_keeps_a_lone_shaders_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        // A Community Shaders feature pack is nothing but a `Shaders/` tree;
        // hoisting it would strip the directory and deploy the feature's ini
        // to the Data root, where Community Shaders never finds it.
        write(&dest.join("Shaders/Features/CloudShadows.ini"), b"ini");
        write(
            &dest.join("Shaders/CloudShadows/CloudShadows.hlsli"),
            b"hlsl",
        );
        normalize_staged(&dest, "Data", &[]).unwrap();

        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(
            rels,
            vec![
                "shaders/cloudshadows/CloudShadows.hlsli".to_owned(),
                "shaders/features/CloudShadows.ini".to_owned(),
            ]
        );
    }

    #[test]
    fn normalize_keeps_a_lone_bepinex_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        // A Subnautica BepInEx mod is nothing but a `BepInEx/plugins/<Mod>/`
        // tree deployed at the install root (empty mod_root). Hoisting the lone
        // `BepInEx/` wrapper would strip it and drop the plugin at the game
        // root, where the loader scans only `BepInEx/plugins/` and never sees
        // it.
        write(&dest.join("BepInEx/plugins/Tweaks/Tweaks.dll"), b"dll");
        write(&dest.join("BepInEx/plugins/Tweaks/config.json"), b"json");
        normalize_staged(&dest, "", &[]).unwrap();

        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(
            rels,
            vec![
                "bepinex/plugins/tweaks/Tweaks.dll".to_owned(),
                "bepinex/plugins/tweaks/config.json".to_owned(),
            ]
        );
    }

    #[test]
    fn nested_mod_root_strips_a_full_bepinex_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        // A Subnautica mod that ships the whole path (The Red Plague does this).
        // The deployer already prefixes `BepInEx/plugins`, so the archive's own
        // copy must be stripped or the mod lands in a doubled, unreadable path.
        write(
            &dest.join("BepInEx/plugins/TheRedPlague/TheRedPlague.dll"),
            b"dll",
        );
        write(
            &dest.join("BepInEx/plugins/TheRedPlague/assets/bundle"),
            b"a",
        );
        write(&dest.join("README.md"), b"readme");
        normalize_staged(&dest, "BepInEx/plugins", &[]).unwrap();

        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(
            rels,
            vec![
                "theredplague/TheRedPlague.dll".to_owned(),
                "theredplague/assets/bundle".to_owned(),
            ]
        );
        assert!(dest.join(".unmanaged/README.md").is_file());
    }

    #[test]
    fn nested_mod_root_keeps_a_bare_plugin_at_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        // Most Subnautica mods ship bare (ECC Library, Kallie's Prop Pack): the
        // DLL is the plugin itself. It must NOT be parked as a game-root loader
        // - that deployed it beside the .exe where BepInEx never loads it.
        write(&dest.join("ECCLibrary.dll"), b"dll");
        write(&dest.join("Assets/bundle"), b"a");
        normalize_staged(&dest, "BepInEx/plugins", &[]).unwrap();

        assert!(
            !dest.join(".root").exists(),
            "a bare plugin must not be parked"
        );
        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(
            rels,
            vec!["ECCLibrary.dll".to_owned(), "assets/bundle".to_owned()]
        );
    }

    #[test]
    fn nested_mod_root_leaves_a_multi_target_tree_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        // Ships both plugins/ and config/: stripping `BepInEx/plugins` would
        // silently drop the config, so the tree is left as-is.
        write(&dest.join("BepInEx/plugins/Mod/mod.dll"), b"dll");
        write(&dest.join("BepInEx/config/Mod.cfg"), b"cfg");
        normalize_staged(&dest, "BepInEx/plugins", &[]).unwrap();
        assert!(dest.join("BepInEx/plugins/Mod/mod.dll").is_file());
        assert!(dest.join("BepInEx/config/Mod.cfg").is_file());
    }

    #[test]
    fn normalize_leaves_a_deployable_root_with_a_side_dir_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        // A loose plugin at the root: the layout is already deployable, the
        // side directory (whatever it holds) must not be hoisted over it.
        write(&dest.join("Mod.esp"), b"esp");
        write(&dest.join("Extras/Data/Optional.esp"), b"esp");
        normalize_staged(&dest, "Data", &[]).unwrap();
        assert!(dest.join("Mod.esp").is_file());
        assert!(dest.join("Extras/Data/Optional.esp").is_file());
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
        normalize_staged(&dest, "Data", &[]).unwrap();

        // Scripts became the mod root; loaders/src parked, never deployed.
        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(
            rels,
            vec![
                "<root>/skse64_1_6_1170.dll".to_owned(),
                "<root>/skse64_loader.exe".to_owned(),
                "scripts/Actor.pex".to_owned(),
            ]
        );
        assert!(dest.join(".root/skse64_loader.exe").is_file());
        assert!(dest.join(".unmanaged/src/skse64/main.cpp").is_file());
    }

    #[test]
    fn normalize_leaves_plain_mods_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        write(&dest.join("SkyUI.esp"), b"esp");
        write(&dest.join("textures/ui.dds"), b"dds");
        normalize_staged(&dest, "Data", &[]).unwrap();
        let rels: Vec<_> = resolve_files(&dest)
            .unwrap()
            .iter()
            .map(|f| f.target_rel.clone())
            .collect();
        assert_eq!(
            rels,
            vec!["SkyUI.esp".to_owned(), "textures/ui.dds".to_owned()]
        );
    }

    #[test]
    fn normalize_hoists_a_single_wrapper_without_mod_root() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("staged");
        write(&dest.join("MyMod-1.0/meshes/m.nif"), b"m");
        write(&dest.join("MyMod-1.0/MyMod.esp"), b"e");
        normalize_staged(&dest, "Data", &[]).unwrap();
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
        normalize_staged(&dest, "Data", &[]).unwrap();
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
        let err =
            extract_with_system(&tmp.path().join("nope.7z"), &tmp.path().join("out")).unwrap_err();
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
        normalize_staged(&dest, "Data", &[]).unwrap();
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
