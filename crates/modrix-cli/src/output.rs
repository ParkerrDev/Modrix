// SPDX-License-Identifier: GPL-2.0-only
//! Human-readable formatting for engine results.
//!
//! Output is written to a caller-provided [`Write`] (a locked stdout handle),
//! never the `print!`/`println!` macros, so a broken pipe surfaces as an error
//! rather than a panic - and so the formatting is unit-testable.

use std::io::Write;

use anyhow::Result;
use modrix_core::{Conflict, DeployPlan, DeployReport, FileStatus, Paths, VerifyReport};

/// Emit `value` for machines (`--json`: one `{"ok":true,"data":…}` envelope
/// per invocation - the stable contract agents parse) or via the
/// human-formatting closure otherwise.
///
/// # Errors
///
/// Returns any serialization or write failure.
pub fn emit<T: serde::Serialize>(
    out: &mut dyn Write,
    json: bool,
    value: &T,
    human: impl FnOnce(&mut dyn Write, &T) -> Result<()>,
) -> Result<()> {
    if json {
        let data = serde_json::to_string(value)?;
        writeln!(out, "{{\"ok\":true,\"data\":{data}}}")?;
        return Ok(());
    }
    human(out, value)
}

/// Acknowledge a mutating command: a plain line, or the JSON envelope with a
/// `message` payload.
///
/// # Errors
///
/// Returns any write failure.
pub fn ack(out: &mut dyn Write, json: bool, message: &str) -> Result<()> {
    if json {
        let quoted = serde_json::to_string(message)?;
        writeln!(out, "{{\"ok\":true,\"data\":{{\"message\":{quoted}}}}}")?;
        return Ok(());
    }
    writeln!(out, "{message}")?;
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Agents parse this envelope; its shape is a contract.
    #[test]
    fn json_envelope_is_stable() {
        let mut buf = Vec::new();
        emit(&mut buf, true, &vec![1, 2, 3], |_, _| {
            panic!("human path must not run in json mode")
        })
        .unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "{\"ok\":true,\"data\":[1,2,3]}\n"
        );

        let mut buf = Vec::new();
        ack(&mut buf, true, "done \"quoted\"").unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "{\"ok\":true,\"data\":{\"message\":\"done \\\"quoted\\\"\"}}\n"
        );
    }

    #[test]
    fn human_mode_uses_the_closure() {
        let mut buf = Vec::new();
        emit(&mut buf, false, &42, |out, v| {
            writeln!(out, "value {v}")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "value 42\n");
    }
}
