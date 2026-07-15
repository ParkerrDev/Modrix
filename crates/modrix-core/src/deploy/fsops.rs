// SPDX-License-Identifier: GPL-2.0-only
//! Low-level, crash-safe filesystem primitives for the deploy engine.
//!
//! Every mutation here lands atomically: content is written to a sibling
//! temporary path, fsync'd, then `rename`d over the destination (an atomic
//! replace on both Unix and Windows for regular files). Hardlink/symlink
//! placement uses the same temp-then-rename dance so a half-created link is
//! never observed at the destination. These properties are what let the applier
//! and its journal roll a crash fully forward or fully back (invariant I4).

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::model::LinkType;

/// Read buffer for streaming copies and hashing. Fixed so memory use is O(1)
/// regardless of file size.
const IO_BUF_LEN: usize = 64 * 1024;

/// Compute the lowercase hex SHA-256 of a file's contents.
///
/// # Errors
///
/// Returns [`Error::Io`] if the file cannot be opened or read.
pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| Error::io(path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; IO_BUF_LEN];
    loop {
        let n = file.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).ok_or_else(|| short_read(path))?;
        hasher.update(chunk);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Format bytes as a lowercase hex string.
fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        // Two lowercase hex digits per byte; `write!` to a String never fails.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Targets whose staged source drifted after deploy (a store-file edit; most
/// editors write-then-rename, which breaks the hardlink). The planner re-places
/// these instead of keeping them - otherwise the edit never reaches the game.
pub(crate) fn dirty_targets(
    target_root: &Path,
    current: &[crate::deploy::manifest::DeployedRow],
) -> std::collections::HashSet<String> {
    current
        .iter()
        .filter(|row| {
            let target = rel_to_abs(target_root, &row.target_rel);
            source_out_of_sync(row.link_type, &row.source, &target, &row.source_hash)
        })
        .map(|row| row.target_rel.clone())
        .collect()
}

/// Whether a kept deployment's staged source has drifted out of sync with the
/// deployed file at `target` - the store file was edited after deploy (most
/// editors write-then-rename, which silently breaks a hardlink), so keeping the
/// row would leave the game running stale content forever.
///
/// Only a *pristine* target is reported stale: if the deployed copy no longer
/// hashes to `recorded_hash` the *user* changed the game-side file, and
/// re-placing it would clobber their edit (invariant I3) - `verify` surfaces
/// that case instead.
pub(crate) fn source_out_of_sync(
    link_type: LinkType,
    source: &Path,
    target: &Path,
    recorded_hash: &str,
) -> bool {
    let drifted = match link_type {
        LinkType::Hardlink => !same_file(source, target),
        LinkType::Symlink => fs::read_link(target).map_or(true, |dest| dest != source),
        LinkType::Copy => hash_file(source).is_ok_and(|h| h != recorded_hash),
    };
    if !drifted {
        return false;
    }
    // The source must still exist (a vanished source is the remove path's
    // business) and the deployed copy must still be exactly what we placed.
    source.exists() && hash_file(target).is_ok_and(|h| h == recorded_hash)
}

/// Whether two paths refer to the same underlying file (an intact hardlink).
#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (a.metadata(), b.metadata()) {
        (Ok(ma), Ok(mb)) => ma.dev() == mb.dev() && ma.ino() == mb.ino(),
        _ => false,
    }
}

/// Windows: `std` exposes no stable file-index API, but two names of one file
/// necessarily share length and modified time, and a store file replaced by an
/// editor's write-rename gets a fresh mtime - so (len, mtime) equality is a
/// faithful intact-hardlink test without hashing gigabytes per deploy.
#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.metadata(), b.metadata()) {
        (Ok(ma), Ok(mb)) => {
            ma.len() == mb.len() && ma.modified().ok() == mb.modified().ok()
        }
        _ => false,
    }
}

/// A temporary sibling of `target` in the same directory (hence same
/// filesystem, so the final `rename` is atomic). Unique per target filename.
fn sibling_temp(target: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".modrix-tmp.");
    match target.file_name() {
        Some(file) => name.push(file),
        None => name.push("unnamed"),
    }
    match target.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// Create `target`'s parent directories, returning the ones that did not exist
/// (outermost first) so a rollback can remove exactly what it created.
///
/// # Errors
///
/// Returns [`Error::Io`] if a directory cannot be created.
pub(crate) fn ensure_parent_dirs(target: &Path) -> Result<Vec<PathBuf>> {
    let Some(parent) = target.parent() else {
        return Ok(Vec::new());
    };
    // Collect the chain of missing ancestors, outermost last.
    let mut missing = Vec::new();
    let mut cursor = Some(parent);
    while let Some(dir) = cursor {
        if dir.as_os_str().is_empty() || dir.exists() {
            break;
        }
        missing.push(dir.to_path_buf());
        cursor = dir.parent();
    }
    missing.reverse();
    for dir in &missing {
        match fs::create_dir(dir) {
            Ok(()) => {}
            // A concurrent create is fine; anything else is a real error.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(Error::io(dir.clone(), e)),
        }
    }
    Ok(missing)
}

/// Place `source` at `target`, trying hardlink → symlink → copy and returning
/// which succeeded. The destination is replaced atomically.
///
/// # Errors
///
/// Returns [`Error::Io`] if even the copy fallback fails, or the rename fails.
pub(crate) fn place(source: &Path, target: &Path) -> Result<LinkType> {
    let tmp = sibling_temp(target);
    remove_if_present(&tmp)?;

    if try_hardlink(source, &tmp) {
        commit_rename(&tmp, target)?;
        return Ok(LinkType::Hardlink);
    }
    remove_if_present(&tmp)?;
    if try_symlink(source, &tmp) {
        commit_rename(&tmp, target)?;
        return Ok(LinkType::Symlink);
    }
    remove_if_present(&tmp)?;
    copy_to(source, &tmp)?;
    commit_rename(&tmp, target)?;
    Ok(LinkType::Copy)
}

fn try_hardlink(source: &Path, tmp: &Path) -> bool {
    fs::hard_link(source, tmp).is_ok()
}

#[cfg(unix)]
fn try_symlink(source: &Path, tmp: &Path) -> bool {
    std::os::unix::fs::symlink(source, tmp).is_ok()
}

#[cfg(windows)]
fn try_symlink(source: &Path, tmp: &Path) -> bool {
    std::os::windows::fs::symlink_file(source, tmp).is_ok()
}

#[cfg(not(any(unix, windows)))]
fn try_symlink(_source: &Path, _tmp: &Path) -> bool {
    false
}

/// Stream-copy `source` to a fresh file at `dest`, fsync'ing the contents.
///
/// # Errors
///
/// Returns [`Error::Io`] on any read/write/sync failure.
pub(crate) fn copy_to(source: &Path, dest: &Path) -> Result<()> {
    let mut input = File::open(source).map_err(|e| Error::io(source, e))?;
    let mut output = File::create(dest).map_err(|e| Error::io(dest, e))?;
    let mut buf = vec![0_u8; IO_BUF_LEN];
    loop {
        let n = input.read(&mut buf).map_err(|e| Error::io(source, e))?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).ok_or_else(|| short_read(source))?;
        output.write_all(chunk).map_err(|e| Error::io(dest, e))?;
    }
    output.sync_all().map_err(|e| Error::io(dest, e))?;
    Ok(())
}

/// Copy `target`'s current contents into the content-addressed backup store,
/// returning `(hash, backup_path)`. Idempotent: identical content dedups to the
/// same path. The caller has not yet modified `target`, so this preserves the
/// pristine original before any overwrite (invariant I3).
///
/// # Errors
///
/// Returns [`Error::Io`] if hashing or copying fails.
pub(crate) fn backup_into_store(target: &Path, backup_root: &Path) -> Result<(String, PathBuf)> {
    let hash = hash_file(target)?;
    let backup_path = backup_root.join(&hash);
    if backup_path.exists() {
        return Ok((hash, backup_path));
    }
    ensure_parent_dirs(&backup_path)?;
    let tmp = sibling_temp(&backup_path);
    remove_if_present(&tmp)?;
    copy_to(target, &tmp)?;
    commit_rename(&tmp, &backup_path)?;
    Ok((hash, backup_path))
}

/// Restore a backed-up original from the store to `target`, atomically.
///
/// # Errors
///
/// Returns [`Error::Io`] if the backup cannot be read or written.
pub(crate) fn restore_from_store(backup_path: &Path, target: &Path) -> Result<()> {
    ensure_parent_dirs(target)?;
    let tmp = sibling_temp(target);
    remove_if_present(&tmp)?;
    copy_to(backup_path, &tmp)?;
    commit_rename(&tmp, target)?;
    Ok(())
}

/// Atomically write `bytes` to `path`: create a sibling temp, fsync it, then
/// rename it into place. Used for the deploy journal and commit marker, where a
/// half-written file must never be observed.
///
/// # Errors
///
/// Returns [`Error::Io`] on any write/sync/rename failure.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_parent_dirs(path)?;
    let tmp = sibling_temp(path);
    remove_if_present(&tmp)?;
    let mut file = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
    file.write_all(bytes).map_err(|e| Error::io(&tmp, e))?;
    file.sync_all().map_err(|e| Error::io(&tmp, e))?;
    drop(file);
    commit_rename(&tmp, path)?;
    Ok(())
}

/// Remove a file or symlink at `path` if it exists. Removing an absent path is
/// success (idempotent), which is what makes rollback re-runnable.
///
/// # Errors
///
/// Returns [`Error::Io`] on a removal failure other than "not found".
pub(crate) fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(path, e)),
    }
}

/// The maximum directory depth the ancestor cleanup walks, so an adversarial
/// path can never spin the loop unbounded (Power of Ten: every loop is bounded).
const MAX_ANCESTOR_DEPTH: usize = 256;

/// Join a `/`-separated relative target onto `root`. Empty components are
/// skipped; the relative part is validated for escapes before it ever reaches
/// here, so this is a pure path join.
pub(crate) fn rel_to_abs(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for component in rel.split('/').filter(|c| !c.is_empty()) {
        // Engine-synthesized marker for game-root files (a deploy root above
        // the mod root, e.g. SKSE loaders next to the game executable).
        // Archive-supplied names can never contain it: `relative_target`
        // rejects `<`/`>` at the staging trust boundary.
        if component == UP_MARKER {
            path.pop();
        } else {
            path.push(component);
        }
    }
    path
}

/// The reserved path component that walks one level above the deploy root.
pub(crate) const UP_MARKER: &str = "<up>";

/// Remove now-empty ancestor directories of `child`, walking upward but never
/// past (or including) `stop_root`. Best-effort: a non-empty directory ends the
/// walk. This undoes exactly the directories a deploy created, so undeploy and
/// rollback leave the game tree byte-identical (invariant I1).
pub(crate) fn remove_empty_ancestors(child: &Path, stop_root: &Path) {
    let mut cursor = child.parent().map(Path::to_path_buf);
    let mut depth: usize = 0;
    while let Some(dir) = cursor {
        if depth >= MAX_ANCESTOR_DEPTH || dir == *stop_root || !dir.starts_with(stop_root) {
            break;
        }
        // Stops the walk as soon as a directory is non-empty (or gone).
        if fs::remove_dir(&dir).is_err() {
            break;
        }
        cursor = dir.parent().map(Path::to_path_buf);
        depth = depth.saturating_add(1);
    }
}

/// Rename `tmp` onto `target`, then best-effort fsync the parent directory so
/// the rename is durable across a crash.
fn commit_rename(tmp: &Path, target: &Path) -> Result<()> {
    fs::rename(tmp, target).map_err(|e| Error::io(target, e))?;
    if let Some(parent) = target.parent() {
        // Directory fsync makes the rename durable on Unix; unsupported on some
        // platforms, so failure here is non-fatal.
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

fn short_read(path: &Path) -> Error {
    Error::io(
        path,
        io::Error::new(io::ErrorKind::UnexpectedEof, "short read"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn hash_is_stable_and_content_addressed() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_file(&a, b"hello world");
        write_file(&b, b"hello world");
        // Known SHA-256 of "hello world".
        assert_eq!(
            hash_file(&a).unwrap(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(hash_file(&a).unwrap(), hash_file(&b).unwrap());
    }

    #[test]
    fn place_prefers_hardlink_within_one_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("store/mod/file.txt");
        let dst = dir.path().join("game/data/file.txt");
        write_file(&src, b"payload");
        ensure_parent_dirs(&dst).unwrap();

        let link = place(&src, &dst).unwrap();
        assert_eq!(link, LinkType::Hardlink);
        assert_eq!(fs::read(&dst).unwrap(), b"payload");
        // Hardlink shares the inode: bumping the link count is observable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(fs::metadata(&src).unwrap().nlink(), 2);
        }
    }

    #[test]
    fn place_overwrites_existing_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write_file(&src, b"new");
        write_file(&dst, b"old");
        place(&src, &dst).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"new");
        // The temporary is cleaned up.
        assert!(!sibling_temp(&dst).exists());
    }

    #[test]
    fn backup_roundtrip_restores_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("backups");
        let target = dir.path().join("game/original.esp");
        write_file(&target, b"pristine game file");

        let (hash, backup) = backup_into_store(&target, &store).unwrap();
        assert!(backup.exists());
        // Now clobber the target, then restore.
        write_file(&target, b"clobbered");
        restore_from_store(&backup, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"pristine game file");
        assert_eq!(hash_file(&target).unwrap(), hash);
    }

    #[test]
    fn backup_dedups_identical_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("backups");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_file(&a, b"same");
        write_file(&b, b"same");
        let (ha, pa) = backup_into_store(&a, &store).unwrap();
        let (hb, pb) = backup_into_store(&b, &store).unwrap();
        assert_eq!(ha, hb);
        assert_eq!(pa, pb);
    }

    #[test]
    fn ensure_parent_dirs_reports_only_created() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a/b/c/file");
        let created = ensure_parent_dirs(&target).unwrap();
        // a, a/b, a/b/c - outermost first.
        assert_eq!(created.len(), 3);
        assert!(created[0].ends_with("a"));
        assert!(created[2].ends_with("a/b/c") || created[2].ends_with("a\\b\\c"));
        // Re-running creates nothing new.
        assert!(ensure_parent_dirs(&target).unwrap().is_empty());
    }

    #[test]
    fn remove_empty_ancestors_stops_at_root_and_non_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("game");
        let deep = root.join("a/b/c/file");
        ensure_parent_dirs(&deep).unwrap();
        // A sibling file keeps `game/a` non-empty.
        write_file(&root.join("a/keep"), b"x");
        write_file(&deep, b"y");

        fs::remove_file(&deep).unwrap();
        remove_empty_ancestors(&deep, &root);
        // c and b are emptied and removed; a survives (has `keep`); root stays.
        assert!(!root.join("a/b").exists());
        assert!(root.join("a").exists());
        assert!(root.exists());
    }

    #[test]
    fn rel_to_abs_joins_slash_separated() {
        let root = Path::new("/game/Data");
        assert_eq!(
            rel_to_abs(root, "meshes/a.nif"),
            PathBuf::from("/game/Data/meshes/a.nif")
        );
        assert_eq!(rel_to_abs(root, ""), PathBuf::from("/game/Data"));
        // The engine-synthesized root marker walks above the deploy root.
        assert_eq!(
            rel_to_abs(root, "<up>/skse64_loader.exe"),
            PathBuf::from("/game/skse64_loader.exe")
        );
    }

    #[test]
    fn remove_if_present_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("gone");
        write_file(&f, b"x");
        remove_if_present(&f).unwrap();
        // Second removal of an absent file still succeeds.
        remove_if_present(&f).unwrap();
    }
}
