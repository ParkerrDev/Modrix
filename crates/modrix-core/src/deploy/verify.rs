// SPDX-License-Identifier: GPL-2.0-only
//! The verify pass: does what the manifest says we deployed still match what is
//! on disk? A first-class, read-only health check for a deployment.

use std::path::Path;

use rusqlite::Connection;

use crate::deploy::{fsops, manifest};
use crate::error::Result;
use crate::id::GameId;

/// The state of one deployed file relative to the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum FileStatus {
    /// Present and content matches - healthy.
    Ok,
    /// The manifest expects a file here, but it is gone.
    Missing,
    /// Present, but its content no longer matches what we deployed (a user or
    /// another tool changed it).
    Modified,
}

/// A single non-healthy file found by [`verify`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifyIssue {
    /// Target path relative to the deploy root.
    pub target: String,
    /// What is wrong with it.
    pub status: FileStatus,
}

/// The result of verifying a deployment.
#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    checked: usize,
    issues: Vec<VerifyIssue>,
}

impl VerifyReport {
    /// Whether every deployed file is present and unmodified.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    /// How many deployed files were checked.
    #[must_use]
    pub fn checked(&self) -> usize {
        self.checked
    }

    /// The files that were missing or modified.
    #[must_use]
    pub fn issues(&self) -> &[VerifyIssue] {
        &self.issues
    }
}

/// Verify the game's current deployment against the manifest.
pub(crate) fn verify(conn: &Connection, game: GameId, target_root: &Path) -> Result<VerifyReport> {
    let rows = manifest::current_deployment(conn, game)?;

    let mut issues = Vec::new();
    for row in &rows {
        let target = fsops::rel_to_abs(target_root, &row.target_rel);
        let status = if target.symlink_metadata().is_err() {
            FileStatus::Missing
        } else if fsops::hash_file(&target)? == row.source_hash {
            FileStatus::Ok
        } else {
            FileStatus::Modified
        };
        if status != FileStatus::Ok {
            issues.push(VerifyIssue {
                target: row.target_rel.clone(),
                status,
            });
        }
    }
    Ok(VerifyReport {
        checked: rows.len(),
        issues,
    })
}
