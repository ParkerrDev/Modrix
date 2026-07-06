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

/// Enumerate a staged mod's files as planner input, in a stable sorted order.
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
