// SPDX-License-Identifier: GPL-2.0-only
//! The seam between the engine and Tier-2 game logic (Lua plugins).
//!
//! Core defines the [`GameLogic`] trait and the validated [`StagePlan`]
//! currency, but hosts no scripting itself - `modrix-plugin` implements the
//! trait over a sandboxed Lua VM and frontends register instances with the
//! engine at boot. Plugins **return plans; they never touch files**: every
//! plan is validated here (relative paths only, bounded size) and applied by
//! core's own bounded file operations, so a misbehaving plugin cannot write
//! outside the staging tree.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// Upper bound on entries in one stage plan.
pub const MAX_PLAN_ENTRIES: usize = 100_000;

/// One planned placement: a file in the extracted archive tree → its staged
/// location. Both paths are relative; validation rejects anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageEntry {
    /// Source path, relative to the extracted archive root.
    pub src: String,
    /// Destination path, relative to the staged mod root.
    pub dest: String,
}

/// A plugin's answer to "how should this archive be staged?".
#[derive(Debug, Clone, Default)]
pub struct StagePlan {
    /// Planned placements, applied in order (later wins on collision).
    pub entries: Vec<StageEntry>,
}

impl StagePlan {
    /// Validate every entry: relative, no `..`/root components, non-empty,
    /// and within the entry cap.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PathEscape`] for an escaping path or
    /// [`Error::BoundExceeded`] past [`MAX_PLAN_ENTRIES`].
    pub fn validate(&self) -> Result<()> {
        if self.entries.len() > MAX_PLAN_ENTRIES {
            return Err(Error::BoundExceeded {
                what: "stage plan entries",
                limit: MAX_PLAN_ENTRIES,
            });
        }
        for entry in &self.entries {
            validate_rel(&entry.src)?;
            validate_rel(&entry.dest)?;
        }
        Ok(())
    }
}

/// Reject absolute paths, parent traversal, and empty paths.
fn validate_rel(path: &str) -> Result<()> {
    let p = Path::new(path);
    let escapes = path.is_empty()
        || p.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
    if escapes {
        return Err(Error::PathEscape {
            path: PathBuf::from(path),
        });
    }
    Ok(())
}

/// Tier-2 game logic, implemented by the Lua plugin host. Every method has a
/// "not handled" default so a plugin only defines the callbacks it needs;
/// `None` means "fall back to the engine's data-driven behavior".
pub trait GameLogic: Send + Sync {
    /// Locate the game's install directory (beyond the declarative probes).
    ///
    /// # Errors
    ///
    /// Returns a plugin-side failure (script error, budget exhausted).
    fn detect(&self) -> Result<Option<PathBuf>> {
        Ok(None)
    }

    /// Compute a dynamic mod root for an install.
    ///
    /// # Errors
    ///
    /// As for [`GameLogic::detect`].
    fn mod_root(&self, _install: &Path) -> Result<Option<String>> {
        Ok(None)
    }

    /// Plan how an extracted archive should be staged. `None` = use the
    /// engine's normalization.
    ///
    /// # Errors
    ///
    /// As for [`GameLogic::detect`].
    fn install(&self, _archive_root: &Path) -> Result<Option<StagePlan>> {
        Ok(None)
    }

    /// Reorder a plugin list (game-specific sort rules). `None` = keep the
    /// engine's ordering.
    ///
    /// # Errors
    ///
    /// As for [`GameLogic::detect`].
    fn load_order(&self, _plugins: &[String]) -> Result<Option<Vec<String>>> {
        Ok(None)
    }
}

/// Rearrange an extracted tree according to a validated plan. Sources are
/// moved (not copied) into a temporary sibling, leftovers are removed, and
/// the result replaces the tree - so a plan that lists a subset produces
/// exactly that subset.
///
/// # Errors
///
/// Returns the plan's validation error, or [`Error::Io`] on a move failure.
pub(crate) fn apply_plan(root: &Path, plan: &StagePlan) -> Result<()> {
    plan.validate()?;
    let tmp = root.with_extension("mm-plan");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| Error::io(&tmp, e))?;
    for entry in &plan.entries {
        let src = root.join(&entry.src);
        if !src.is_file() {
            continue; // A planned source the archive lacks is a no-op.
        }
        let dest = tmp.join(&entry.dest);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        std::fs::rename(&src, &dest).map_err(|e| Error::io(&src, e))?;
    }
    std::fs::remove_dir_all(root).map_err(|e| Error::io(root, e))?;
    std::fs::rename(&tmp, root).map_err(|e| Error::io(&tmp, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(src: &str, dest: &str) -> StageEntry {
        StageEntry {
            src: src.to_owned(),
            dest: dest.to_owned(),
        }
    }

    #[test]
    fn plans_reject_escapes_and_absolutes() {
        for bad in ["../evil.dll", "/etc/passwd", "", "a/../../b"] {
            let plan = StagePlan {
                entries: vec![entry(bad, "ok.txt")],
            };
            assert!(plan.validate().is_err(), "src {bad:?} must be rejected");
            let plan = StagePlan {
                entries: vec![entry("ok.txt", bad)],
            };
            assert!(plan.validate().is_err(), "dest {bad:?} must be rejected");
        }
    }

    #[test]
    fn apply_plan_rearranges_and_drops_unplanned_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("staged");
        std::fs::create_dir_all(root.join("wrapper")).unwrap();
        std::fs::write(root.join("wrapper/mod.dll"), b"m").unwrap();
        std::fs::write(root.join("readme.txt"), b"r").unwrap();
        let plan = StagePlan {
            entries: vec![entry("wrapper/mod.dll", "Plugins/MyMod/mod.dll")],
        };
        apply_plan(&root, &plan).unwrap();
        assert!(root.join("Plugins/MyMod/mod.dll").is_file());
        assert!(!root.join("readme.txt").exists(), "unplanned files drop");
        assert!(!root.join("wrapper").exists());
    }
}
