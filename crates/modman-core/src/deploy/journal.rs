// SPDX-License-Identifier: GPL-2.0-only
//! The deploy journal: what makes an interrupted deploy fully recoverable.
//!
//! A deploy writes two files, in this strict order:
//!
//! 1. **The journal** ([`Journal`]) - the pre-state of every target it is about
//!    to touch - written and fsync'd *before* any game file is mutated, and
//!    *after* every displaced foreign original is safely in the backup store.
//! 2. **The commit marker** ([`Commit`]): the exact manifest rows to record,
//!    written atomically *after* every disk mutation has succeeded.
//!
//! The commit marker is the linearization point. On the next [`recover`]:
//!
//! * **No commit marker** → the deploy did not finish; every target is restored
//!   to its journalled pre-state (roll **back**). The SQLite manifest was never
//!   touched, so disk and manifest agree again.
//! * **Commit marker present** → all disk mutations completed; finish the
//!   bookkeeping by writing the recorded rows into the manifest (roll
//!   **forward**).
//!
//! Every restore step is idempotent, so recovery can itself be interrupted and
//! re-run. All loops are bounded by the (finite) journal.

use std::path::PathBuf;

use rusqlite::Connection;

use crate::deploy::fsops;
use crate::deploy::manifest::{self, DeployedRow};
use crate::error::{Error, Result};
use crate::id::{GameId, ProfileId};
use crate::paths::Paths;

/// The state a target was in before the deploy touched it - everything needed
/// to put it back exactly.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum PreState {
    /// Nothing was there; on rollback, remove what we added.
    Absent,
    /// We already owned a file here; on rollback, recreate it from this source.
    Ours { source: PathBuf },
    /// A pristine foreign original was here; on rollback, restore it from the
    /// content-addressed backup store.
    Foreign { backup_hash: String },
}

/// One journalled target and how to restore it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct JournalEntry {
    /// Target path relative to `target_root`, `/`-separated.
    pub target_rel: String,
    /// The pre-deploy state to roll back to.
    pub pre: PreState,
}

/// The pre-mutation journal for one deploy.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Journal {
    /// The game being deployed.
    pub game: GameId,
    /// Absolute deploy root; targets are resolved relative to it.
    pub target_root: PathBuf,
    /// Root of the content-addressed backup store.
    pub backup_root: PathBuf,
    /// Every target this deploy will touch, with its pre-state.
    pub entries: Vec<JournalEntry>,
}

/// The commit marker: the manifest rows to record once disk work is done.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Commit {
    /// The game deployed.
    pub game: GameId,
    /// The profile now deployed.
    pub profile: ProfileId,
    /// Timestamp string recorded on each row.
    pub deployed_at: String,
    /// The exact rows that constitute the new manifest.
    pub rows: Vec<DeployedRow>,
}

/// What a recovery pass did (for logging).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Recovered {
    /// No journal was present; nothing to do.
    Nothing,
    /// An unfinished deploy was rolled back to its pre-state.
    RolledBack { targets: usize },
    /// A committed-but-unfinalized deploy was rolled forward.
    RolledForward { rows: usize },
}

/// Persist the journal durably before any mutation begins.
pub(crate) fn write(paths: &Paths, journal: &Journal) -> Result<()> {
    let bytes = serde_json::to_vec(journal).map_err(|e| Error::Journal(e.to_string()))?;
    fsops::write_atomic(&paths.journal_file(), &bytes)
}

/// Persist the commit marker durably - the atomic point of no return.
pub(crate) fn write_commit(paths: &Paths, commit: &Commit) -> Result<()> {
    let bytes = serde_json::to_vec(commit).map_err(|e| Error::Journal(e.to_string()))?;
    fsops::write_atomic(&paths.commit_file(), &bytes)
}

/// Delete both journal files after a successful deploy or completed recovery.
pub(crate) fn clear(paths: &Paths) -> Result<()> {
    fsops::remove_if_present(&paths.commit_file())?;
    fsops::remove_if_present(&paths.journal_file())?;
    Ok(())
}

/// Recover any interrupted deploy. Call this on engine open, before anything
/// else touches game files.
pub(crate) fn recover(conn: &Connection, paths: &Paths) -> Result<Recovered> {
    let journal_path = paths.journal_file();
    if !journal_path.exists() {
        return Ok(Recovered::Nothing);
    }
    let journal = read_journal(paths)?;
    if paths.commit_file().exists() {
        let rows = roll_forward(conn, paths)?;
        Ok(Recovered::RolledForward { rows })
    } else {
        let targets = roll_back(paths, &journal)?;
        Ok(Recovered::RolledBack { targets })
    }
}

fn read_journal(paths: &Paths) -> Result<Journal> {
    let bytes =
        std::fs::read(paths.journal_file()).map_err(|e| Error::io(paths.journal_file(), e))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Journal(e.to_string()))
}

fn read_commit(paths: &Paths) -> Result<Commit> {
    let bytes =
        std::fs::read(paths.commit_file()).map_err(|e| Error::io(paths.commit_file(), e))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Journal(e.to_string()))
}

/// Finish a committed deploy: write the recorded rows into the manifest.
fn roll_forward(conn: &Connection, paths: &Paths) -> Result<usize> {
    let commit = read_commit(paths)?;
    let tx = conn.unchecked_transaction()?;
    manifest::replace_manifest(
        &tx,
        commit.game,
        commit.profile,
        &commit.rows,
        &commit.deployed_at,
    )?;
    tx.commit()?;
    clear(paths)?;
    Ok(commit.rows.len())
}

/// Roll an unfinished deploy fully back: restore every target's pre-state.
pub(crate) fn roll_back(paths: &Paths, journal: &Journal) -> Result<usize> {
    for entry in &journal.entries {
        restore_entry(journal, entry)?;
    }
    clear(paths)?;
    Ok(journal.entries.len())
}

/// Restore one target to its recorded pre-state. Idempotent.
fn restore_entry(journal: &Journal, entry: &JournalEntry) -> Result<()> {
    let target = fsops::rel_to_abs(&journal.target_root, &entry.target_rel);
    match &entry.pre {
        PreState::Absent => {
            fsops::remove_if_present(&target)?;
            fsops::remove_empty_ancestors(&target, &journal.target_root);
        }
        PreState::Ours { source } => {
            fsops::ensure_parent_dirs(&target)?;
            fsops::place(source, &target)?;
        }
        PreState::Foreign { backup_hash } => {
            let backup = journal.backup_root.join(backup_hash);
            fsops::restore_from_store(&backup, &target)?;
        }
    }
    Ok(())
}
