// SPDX-License-Identifier: GPL-2.0-only
//! The transactional applier: turns a [`DeployPlan`] into filesystem reality
//! without ever leaving a game directory half-deployed.
//!
//! The sequence, and its ordering, is the whole safety argument:
//!
//! 1. **Snapshot.** Back up every pristine foreign original we are about to
//!    overwrite into the content-addressed store, and record each target's
//!    pre-state. No game file is mutated in this phase.
//! 2. **Journal.** Write the pre-state durably ([`journal::write`]) before any
//!    mutation.
//! 3. **Mutate.** Remove departing files (only if they still match the
//!    manifest - I3) and place new ones atomically (hardlink → symlink → copy).
//! 4. **Commit.** Write the commit marker ([`journal::write_commit`]) - the
//!    atomic point of no return - then flip the SQLite manifest, then clear the
//!    journal.
//!
//! A crash before the commit marker rolls fully back; after it, fully forward
//! (see [`journal`]). The [`Faults`] hook lets tests fail the Nth operation to
//! prove there is no interleaving that corrupts the tree (invariant I4).

use std::cell::Cell;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::deploy::journal::{self, Commit, Journal, JournalEntry, PreState};
use crate::deploy::manifest::{self, DeployedRow};
use crate::deploy::plan::{DeployPlan, PlannedAdd};
use crate::deploy::{DeployReport, fsops};
use crate::error::{Error, Result};
use crate::id::ProfileId;

/// A fault-injection checkpoint. In production [`Faults::none`] never fails; in
/// tests [`Faults::failing_at`] makes the Nth checkpoint return an error so we
/// can assert recovery is total at every crash point (invariant I4).
pub(crate) struct Faults {
    fail_at: Option<usize>,
    seen: Cell<usize>,
    fired: Cell<bool>,
}

impl Faults {
    /// Never inject a fault (production).
    pub(crate) fn none() -> Self {
        Self {
            fail_at: None,
            seen: Cell::new(0),
            fired: Cell::new(false),
        }
    }

    /// Fail the `n`th checkpoint (1-based).
    #[cfg(test)]
    pub(crate) fn failing_at(n: usize) -> Self {
        Self {
            fail_at: Some(n),
            seen: Cell::new(0),
            fired: Cell::new(false),
        }
    }

    /// Whether the injected fault actually fired.
    #[cfg(test)]
    pub(crate) fn fired(&self) -> bool {
        self.fired.get()
    }

    /// Advance the counter; error out if this is the chosen failure point.
    fn checkpoint(&self) -> Result<()> {
        let next = self.seen.get().saturating_add(1);
        self.seen.set(next);
        if self.fail_at == Some(next) {
            self.fired.set(true);
            return Err(Error::io(
                Path::new("<injected-fault>"),
                io::Error::other("injected fault"),
            ));
        }
        Ok(())
    }
}

fn bump(counter: &mut usize) {
    *counter = counter.saturating_add(1);
}

/// Current unix time as a seconds string for `deployed_at`.
fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or_else(|_| "0".to_owned(), |d| d.as_secs().to_string())
}

/// A progress sink plus the operation label it reports under.
pub(crate) struct Reporter<'a> {
    /// The shared sink.
    pub progress: &'a crate::Progress,
    /// "Deploying" / "Purging".
    pub label: &'a str,
}

impl Reporter<'_> {
    fn advance(&self, what: &str) {
        self.progress
            .advance_with(1, &format!("{} · {what}", self.label));
    }
}

/// Apply `plan` with no fault injection - the production entry point.
/// Reports live per-file progress through `reporter`.
pub(crate) fn run(
    conn: &Connection,
    paths: &crate::paths::Paths,
    plan: &DeployPlan,
    profile: ProfileId,
    reporter: &Reporter<'_>,
) -> Result<DeployReport> {
    let total = plan.adds.len().saturating_add(plan.removes.len());
    reporter
        .progress
        .begin(reporter.label, u64::try_from(total).unwrap_or(u64::MAX));
    let ctx = ApplyCtx {
        faults: &Faults::none(),
        reporter,
    };
    let outcome = apply(conn, paths, plan, profile, &ctx);
    reporter.progress.finish();
    outcome
}

/// Apply `plan`, recording the result under `profile`. Transactional and
/// journalled: on success the game tree and manifest both reflect the plan; on
/// any failure the caller (or the next engine open) recovers to a clean state.
pub(crate) struct ApplyCtx<'a> {
    /// Fault injection (tests only; `Faults::none()` in production).
    pub faults: &'a Faults,
    /// Live progress reporting.
    pub reporter: &'a Reporter<'a>,
}

pub(crate) fn apply(
    conn: &Connection,
    paths: &crate::paths::Paths,
    plan: &DeployPlan,
    profile: ProfileId,
    ctx: &ApplyCtx<'_>,
) -> Result<DeployReport> {
    let (faults, reporter) = (ctx.faults, ctx.reporter);
    reporter
        .progress
        .set_message(&format!("{} · preparing (hashing + backups)", reporter.label));
    let snapshot = snapshot(plan, faults)?;

    faults.checkpoint()?;
    let journal = Journal {
        game: plan.game,
        target_root: plan.target_root.clone(),
        backup_root: plan.backup_root.clone(),
        entries: snapshot.entries,
    };
    journal::write(paths, &journal)?;

    let (new_rows, mut report) = mutate(plan, &snapshot.add_backups, faults, reporter)?;

    faults.checkpoint()?;
    let deployed_at = now_string();
    journal::write_commit(
        paths,
        &Commit {
            game: plan.game,
            profile,
            deployed_at: deployed_at.clone(),
            rows: new_rows.clone(),
        },
    )?;

    faults.checkpoint()?;
    let tx = conn.unchecked_transaction()?;
    manifest::replace_manifest(&tx, plan.game, profile, &new_rows, &deployed_at)?;
    tx.commit()?;

    journal::clear(paths)?;

    report.set_summary(plan.unchanged(), plan.conflicts().to_vec());
    Ok(report)
}

/// The result of the snapshot phase.
struct Snapshot {
    entries: Vec<JournalEntry>,
    /// Backup path to carry onto each add's new manifest row, aligned to
    /// `plan.adds`.
    add_backups: Vec<Option<std::path::PathBuf>>,
}

/// Compute pre-state for every touched target and back up foreign originals
/// before anything is mutated.
fn snapshot(plan: &DeployPlan, faults: &Faults) -> Result<Snapshot> {
    let mut entries = Vec::new();
    let mut add_backups = Vec::with_capacity(plan.adds.len());

    for add in &plan.adds {
        let (entry, backup) = snapshot_add(plan, add, faults)?;
        entries.push(entry);
        add_backups.push(backup);
    }
    for remove in &plan.removes {
        entries.push(JournalEntry {
            target_rel: remove.row.target_rel.clone(),
            pre: PreState::Ours {
                source: remove.row.source.clone(),
            },
        });
    }
    Ok(Snapshot {
        entries,
        add_backups,
    })
}

fn snapshot_add(
    plan: &DeployPlan,
    add: &PlannedAdd,
    faults: &Faults,
) -> Result<(JournalEntry, Option<std::path::PathBuf>)> {
    let target = fsops::rel_to_abs(&plan.target_root, &add.target_rel);
    if let Some(prior) = &add.prior {
        // We already own this target; carry its displaced-original backup.
        let entry = JournalEntry {
            target_rel: add.target_rel.clone(),
            pre: PreState::Ours {
                source: prior.source.clone(),
            },
        };
        return Ok((entry, prior.backup_path.clone()));
    }
    if target.symlink_metadata().is_ok() {
        // A pristine foreign file lives here - back it up before we overwrite.
        faults.checkpoint()?;
        let (hash, backup_path) = fsops::backup_into_store(&target, &plan.backup_root)?;
        let entry = JournalEntry {
            target_rel: add.target_rel.clone(),
            pre: PreState::Foreign { backup_hash: hash },
        };
        Ok((entry, Some(backup_path)))
    } else {
        Ok((
            JournalEntry {
                target_rel: add.target_rel.clone(),
                pre: PreState::Absent,
            },
            None,
        ))
    }
}

/// Perform the removes and adds, returning the new manifest rows and a report.
fn mutate(
    plan: &DeployPlan,
    add_backups: &[Option<std::path::PathBuf>],
    faults: &Faults,
    reporter: &Reporter<'_>,
) -> Result<(Vec<DeployedRow>, DeployReport)> {
    let mut report = DeployReport::default();

    for remove in &plan.removes {
        faults.checkpoint()?;
        apply_remove(&plan.target_root, &remove.row, &mut report)?;
        reporter.advance(&remove.row.target_rel);
    }

    let mut rows = plan.keep.clone();
    for (add, carried) in plan.adds.iter().zip(add_backups) {
        faults.checkpoint()?;
        rows.push(apply_add(
            &plan.target_root,
            add,
            carried.clone(),
            &mut report,
        )?);
        reporter.advance(&add.target_rel);
    }
    Ok((rows, report))
}

/// Remove a departing file - but only if the on-disk copy still matches what we
/// deployed. A user-modified file is left untouched (invariant I3).
fn apply_remove(target_root: &Path, row: &DeployedRow, report: &mut DeployReport) -> Result<()> {
    let target = fsops::rel_to_abs(target_root, &row.target_rel);
    let present = target.symlink_metadata().is_ok();
    if present && !file_matches(&target, &row.source_hash)? {
        // The user changed our deployed file; do not clobber it.
        bump(&mut report.skipped_modified);
        return Ok(());
    }
    fsops::remove_if_present(&target)?;
    match &row.backup_path {
        Some(backup) => fsops::restore_from_store(backup, &target)?,
        None => fsops::remove_empty_ancestors(&target, target_root),
    }
    bump(&mut report.removed);
    Ok(())
}

/// Place one file and produce its manifest row.
fn apply_add(
    target_root: &Path,
    add: &PlannedAdd,
    carried_backup: Option<std::path::PathBuf>,
    report: &mut DeployReport,
) -> Result<DeployedRow> {
    let target = fsops::rel_to_abs(target_root, &add.target_rel);
    fsops::ensure_parent_dirs(&target)?;
    let link_type = fsops::place(&add.source, &target)?;
    let source_hash = fsops::hash_file(&add.source)?;
    match link_type {
        crate::model::LinkType::Hardlink => bump(&mut report.hardlinks),
        crate::model::LinkType::Symlink => bump(&mut report.symlinks),
        crate::model::LinkType::Copy => bump(&mut report.copies),
    }
    bump(&mut report.added);
    Ok(DeployedRow {
        mod_id: add.mod_id,
        target_rel: add.target_rel.clone(),
        source: add.source.clone(),
        link_type,
        source_hash,
        backup_path: carried_backup,
    })
}

/// Whether the file at `target` still hashes to `expected` (i.e. is unchanged
/// since we deployed it). Following a link and hashing covers hardlink, symlink,
/// and copy uniformly.
fn file_matches(target: &Path, expected: &str) -> Result<bool> {
    match fsops::hash_file(target) {
        Ok(hash) => Ok(hash == expected),
        // A dangling link or vanished file simply does not match.
        Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod apply_tests;
