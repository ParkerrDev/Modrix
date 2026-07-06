// SPDX-License-Identifier: GPL-2.0-only
//! The deployment engine - ModManager's crown jewel.
//!
//! Split into a **pure planner** ([`plan`], no I/O, exhaustively unit-testable)
//! and a **transactional applier** ([`apply`], journalled and crash-recoverable).
//! Every file operation is atomic, hash-checked, backed up, and reversible via
//! the manifest and journal. The five invariants it upholds - reversibility
//! (I1), idempotence (I2), no-silent-clobber (I3), crash-safety (I4), and
//! determinism (I5) - are pinned by the tests in `apply.rs` and `plan.rs`.

pub(crate) mod apply;
mod fsops;
pub(crate) mod journal;
pub(crate) mod manifest;
pub(crate) mod plan;
pub(crate) mod verify;

pub use plan::{Conflict, DeployPlan};
pub use verify::{FileStatus, VerifyIssue, VerifyReport};

/// The outcome of applying a [`DeployPlan`]: what changed and how.
#[derive(Debug, Default, Clone)]
pub struct DeployReport {
    added: usize,
    removed: usize,
    unchanged: usize,
    skipped_modified: usize,
    hardlinks: usize,
    symlinks: usize,
    copies: usize,
    conflicts: Vec<Conflict>,
}

impl DeployReport {
    /// Fill in the summary fields the applier knows only from the plan.
    pub(crate) fn set_summary(&mut self, unchanged: usize, conflicts: Vec<Conflict>) {
        self.unchanged = unchanged;
        self.conflicts = conflicts;
    }

    /// Files newly placed (added or replaced).
    #[must_use]
    pub fn added(&self) -> usize {
        self.added
    }

    /// Files removed.
    #[must_use]
    pub fn removed(&self) -> usize {
        self.removed
    }

    /// Files already correct and left untouched.
    #[must_use]
    pub fn unchanged(&self) -> usize {
        self.unchanged
    }

    /// Files left in place because the on-disk copy no longer matched the
    /// manifest - a user had modified them, so the engine refused to clobber
    /// them (invariant I3).
    #[must_use]
    pub fn skipped_modified(&self) -> usize {
        self.skipped_modified
    }

    /// How the placed files broke down by link strategy: `(hardlinks, symlinks,
    /// copies)`.
    #[must_use]
    pub fn link_breakdown(&self) -> (usize, usize, usize) {
        (self.hardlinks, self.symlinks, self.copies)
    }

    /// The conflicts observed while resolving the deployment.
    #[must_use]
    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }
}
