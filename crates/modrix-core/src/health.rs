// SPDX-License-Identifier: GPL-2.0-only
//! Setup health checks: the problems Vortex surfaces before you launch.
//!
//! Analysis over a [`Snapshot`] of the profile - its plugins, mods, deploy
//! plan, conflict state, and external (unmanaged) content. Each issue carries
//! a severity so frontends can colour it, and `blocking` marks the issues a
//! deploy refuses to run over.
//!
//! Game-specific checks (script-extender loaders, known mod pairings) are
//! **data-driven**: they run only when the game definition declares them in
//! its `[health]` block, and their parameters come from that block. Core
//! carries no per-game knowledge.

use std::collections::BTreeMap;

use crate::deploy::DeployPlan;
use crate::external::ExternalMod;
use crate::gamedef::{HealthDef, LoaderCheckDef, RecommendDef};
use crate::model::Mod;
use crate::plugins::GamePlugin;
use crate::rules::ModConflict;

/// How serious a health issue is (drives the notification colour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Will break the game or a mod outright.
    Error,
    /// May cause problems; worth reviewing.
    Warning,
    /// Informational.
    Info,
}

/// One detected problem.
#[derive(Debug, Clone)]
pub struct Issue {
    /// Severity.
    pub severity: Severity,
    /// One-line description.
    pub message: String,
    /// Deploy refuses to run while this issue stands.
    pub blocking: bool,
}

/// Everything the checks look at, gathered by the engine.
pub struct Snapshot<'a> {
    /// The profile's plugin load order.
    pub plugins: &'a [GamePlugin],
    /// All mods of the game.
    pub mods: &'a [Mod],
    /// The would-be deployment.
    pub plan: &'a DeployPlan,
    /// Pairwise mod conflicts with their rule state.
    pub conflicts: &'a [ModConflict],
    /// Mods caught in a conflict-rule cycle (empty when acyclic).
    pub rule_cycle: &'a [String],
    /// External (unmanaged) mods found in the game directory.
    pub externals: &'a [ExternalMod],
    /// The game definition's health block, when it declares one.
    pub health_def: Option<&'a HealthDef>,
}

/// Analyse a profile for setup problems, worst first.
#[must_use]
pub fn check(snapshot: &Snapshot<'_>) -> Vec<Issue> {
    let mut issues = Vec::new();
    rule_cycle(snapshot.rule_cycle, &mut issues);
    missing_masters(snapshot.plugins, &mut issues);
    unresolved_conflicts(snapshot.conflicts, snapshot.plan, &mut issues);
    if let Some(def) = snapshot.health_def {
        if let Some(loader) = &def.loader {
            loader_check(snapshot.mods, loader, &mut issues);
        }
        for rec in &def.recommended {
            recommended_check(snapshot.mods, rec, &mut issues);
        }
    }
    foreign_files(snapshot.externals, &mut issues);
    issues.sort_by_key(|i| match i.severity {
        Severity::Error => 0_u8,
        Severity::Warning => 1,
        Severity::Info => 2,
    });
    issues
}

fn rule_cycle(cycle: &[String], issues: &mut Vec<Issue>) {
    if cycle.is_empty() {
        return;
    }
    issues.push(Issue {
        severity: Severity::Error,
        message: format!(
            "Conflict rules form a cycle ({}) - remove one of the rules",
            cycle.join(" → "),
        ),
        blocking: true,
    });
}

/// One issue per *missing file*, listing who needs it - sixteen Lux patches
/// missing the same Resources pack is one problem, not sixteen. Disabled
/// plugins are skipped: the game will not load them, so their masters are
/// irrelevant.
fn missing_masters(plugins: &[GamePlugin], issues: &mut Vec<Issue>) {
    let mut by_master: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for plugin in plugins.iter().filter(|p| p.enabled) {
        for master in &plugin.missing_masters {
            by_master
                .entry(master.clone())
                .or_default()
                .push(plugin.name.as_str());
        }
    }
    for (master, dependents) in by_master {
        let message = if let [only] = dependents.as_slice() {
            format!("{only} requires {master}, which is not installed")
        } else {
            format!(
                "{master} is not installed - required by {} plugins ({}, …)",
                dependents.len(),
                dependents
                    .iter()
                    .take(3)
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        };
        issues.push(Issue {
            severity: Severity::Error,
            message,
            blocking: true,
        });
    }
}

/// Conflicts with no rule are unresolved (red); fully ruled conflicts are
/// fine and only worth a quiet note.
fn unresolved_conflicts(conflicts: &[ModConflict], plan: &DeployPlan, issues: &mut Vec<Issue>) {
    let unresolved = conflicts.iter().filter(|c| !c.resolved()).count();
    if unresolved > 0 {
        issues.push(Issue {
            severity: Severity::Error,
            message: format!(
                "{unresolved} mod conflict(s) have no rule - resolve them in Conflicts"
            ),
            blocking: true,
        });
    } else if !plan.conflicts().is_empty() {
        issues.push(Issue {
            severity: Severity::Info,
            message: format!(
                "{} file conflict(s), all covered by rules",
                plan.conflicts().len()
            ),
            blocking: false,
        });
    }
}

/// Script-extender loader requirement, parameterized by the definition:
/// mods shipping `<plugins_dir>` content need a `.root/<root_prefix>*`
/// loader binary; without it they silently do nothing (the
/// CommunityShaders/TerrainHelper class of failure).
fn loader_check(mods: &[Mod], def: &LoaderCheckDef, issues: &mut Vec<Issue>) {
    let has_plugins = mods.iter().any(|m| {
        let dir = m.staged_path.join(&def.plugins_dir);
        std::fs::read_dir(&dir).is_ok_and(|mut e| e.any(|f| f.is_ok()))
    });
    let prefix = def.root_prefix.to_ascii_lowercase();
    let has_loader = mods.iter().any(|m| {
        let root = m.staged_path.join(".root");
        std::fs::read_dir(&root).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with(&prefix)
            })
        })
    });
    if has_plugins && !has_loader {
        issues.push(Issue {
            severity: Severity::Error,
            message: def.message.clone(),
            blocking: false,
        });
    }
}

/// A known "part A needs part B" pairing, parameterized by the definition
/// (the SSE Engine Fixes two-part install is the canonical case).
fn recommended_check(mods: &[Mod], def: &RecommendDef, issues: &mut Vec<Issue>) {
    let name_needle = def.if_name_contains.as_deref().map(str::to_ascii_lowercase);
    let triggered = mods.iter().any(|m| {
        let by_file = def
            .if_file
            .as_deref()
            .is_some_and(|f| m.staged_path.join(f).exists());
        let by_name = name_needle
            .as_deref()
            .is_some_and(|n| m.name.to_ascii_lowercase().contains(n));
        by_file || by_name
    });
    let satisfied = mods.iter().any(|m| {
        m.staged_path
            .join(".root")
            .join(&def.requires_root_file)
            .exists()
    });
    if triggered && !satisfied {
        issues.push(Issue {
            severity: Severity::Error,
            message: def.message.clone(),
            blocking: false,
        });
    }
}

/// Files in the game the deployment does not own and the base game does not
/// ship: leftovers from a previous manager or hand-copied installs. These
/// survive Steam uninstall/reinstall (Steam removes only its own files) and
/// cause "ghost mod" behaviour - plugins and script-extender DLLs that keep
/// loading after their mod was deleted here. The detection itself is the
/// definition-driven external scan; this just summarizes it as an issue.
fn foreign_files(externals: &[ExternalMod], issues: &mut Vec<Issue>) {
    if externals.is_empty() {
        return;
    }
    let mut names: Vec<&str> = externals.iter().map(|m| m.name.as_str()).collect();
    names.sort_unstable();
    let mut shown = names.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    if names.len() > 3 {
        shown.push_str(", …");
    }
    issues.push(Issue {
        severity: Severity::Warning,
        message: format!(
            "{} mod(s) in the game folder are not managed by Modrix \
             ({shown}) - installed by hand or by another manager; \
             manage them where you installed them, or reinstall them here",
            externals.len(),
        ),
        blocking: false,
    });
}
