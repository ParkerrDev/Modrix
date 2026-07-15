// SPDX-License-Identifier: GPL-2.0-only
//! Human-readable formatting for engine results.
//!
//! Output is written to a caller-provided [`Write`] (a locked stdout handle),
//! never the `print!`/`println!` macros, so a broken pipe surfaces as an error
//! rather than a panic - and so the formatting is unit-testable.

use std::io::Write;

use anyhow::Result;
use modrix_core::{Conflict, DeployPlan, DeployReport, FileStatus, Paths, VerifyReport};

/// Report the resolved locations and database path.
pub fn paths(paths: &Paths, out: &mut dyn Write) -> Result<()> {
    writeln!(out, "config:   {}", paths.config_dir().display())?;
    writeln!(out, "data:     {}", paths.data_dir().display())?;
    writeln!(out, "cache:    {}", paths.cache_dir().display())?;
    writeln!(out, "database: {}", paths.database_file().display())?;
    Ok(())
}

/// Summarise a dry-run plan.
pub fn plan(plan: &DeployPlan, out: &mut dyn Write) -> Result<()> {
    writeln!(
        out,
        "plan: {} to add, {} to remove, {} unchanged",
        plan.to_add(),
        plan.to_remove(),
        plan.unchanged()
    )?;
    conflicts(plan.conflicts(), out)
}

/// Summarise the outcome of a deploy.
pub fn report(report: &DeployReport, out: &mut dyn Write) -> Result<()> {
    let (hard, sym, copy) = report.link_breakdown();
    writeln!(
        out,
        "deployed: {} added, {} removed, {} unchanged, {} left as-modified",
        report.added(),
        report.removed(),
        report.unchanged(),
        report.skipped_modified()
    )?;
    writeln!(out, "links: {hard} hardlink, {sym} symlink, {copy} copy")?;
    conflicts(report.conflicts(), out)
}

/// Report the outcome of a verify pass.
pub fn verify(report: &VerifyReport, out: &mut dyn Write) -> Result<()> {
    if report.is_clean() {
        writeln!(out, "verify: clean ({} files checked)", report.checked())?;
        return Ok(());
    }
    writeln!(
        out,
        "verify: {} of {} files have issues",
        report.issues().len(),
        report.checked()
    )?;
    for issue in report.issues() {
        writeln!(out, "  {}\t{}", status_label(issue.status), issue.target)?;
    }
    Ok(())
}

fn conflicts(conflicts: &[Conflict], out: &mut dyn Write) -> Result<()> {
    for conflict in conflicts {
        let shadowed = conflict
            .shadowed
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            out,
            "conflict: {} -> won by mod {} (shadows {shadowed})",
            conflict.target, conflict.winner
        )?;
    }
    Ok(())
}

fn status_label(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Ok => "ok",
        FileStatus::Missing => "missing",
        FileStatus::Modified => "modified",
    }
}
