// SPDX-License-Identifier: GPL-2.0-only
//! Setup health checks: the problems Vortex surfaces before you launch.
//!
//! Pure analysis over the profile's plugins, mods, and deploy plan - no I/O
//! of its own. Each issue carries a severity so frontends can colour it.

use crate::deploy::DeployPlan;
use crate::model::Mod;
use crate::plugins::GamePlugin;

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
}

/// Analyse a profile for setup problems.
#[must_use]
pub fn check(plugins: &[GamePlugin], mods: &[Mod], plan: &DeployPlan) -> Vec<Issue> {
    let mut issues = Vec::new();
    missing_masters(plugins, &mut issues);
    skse_loader(mods, plugins, &mut issues);
    engine_fixes_pair(mods, &mut issues);
    file_conflicts(plan, &mut issues);
    issues
}

/// One issue per *missing file*, listing who needs it - sixteen Lux patches
/// missing the same Resources pack is one problem, not sixteen.
fn missing_masters(plugins: &[GamePlugin], issues: &mut Vec<Issue>) {
    let mut by_master: std::collections::BTreeMap<String, Vec<&str>> =
        std::collections::BTreeMap::new();
    for plugin in plugins {
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
        });
    }
}

/// SKSE plugins (`SKSE/Plugins/*.dll`) need the SKSE loader; without it they
/// silently do nothing (the CommunityShaders/TerrainHelper class of failure).
fn skse_loader(mods: &[Mod], plugins: &[GamePlugin], issues: &mut Vec<Issue>) {
    let has_skse_plugins = mods.iter().any(|m| {
        let dir = m.staged_path.join("SKSE").join("Plugins");
        std::fs::read_dir(&dir).is_ok_and(|mut e| e.any(|f| f.is_ok()))
    });
    let has_loader = mods.iter().any(|m| {
        let root = m.staged_path.join(".root");
        std::fs::read_dir(&root).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with("skse")
            })
        })
    });
    let _ = plugins;
    if has_skse_plugins && !has_loader {
        issues.push(Issue {
            severity: Severity::Error,
            message: "SKSE plugins are installed but the SKSE loader is missing - \
                      install Skyrim Script Extender"
                .to_owned(),
        });
    }
}

/// SSE Engine Fixes ships as two parts: the SKSE plugin and a root preloader
/// (`d3dx9_42.dll`). One without the other fails at launch.
fn engine_fixes_pair(mods: &[Mod], issues: &mut Vec<Issue>) {
    let has_plugin = mods.iter().any(|m| {
        m.staged_path
            .join("SKSE/Plugins/EngineFixes.dll")
            .exists()
            || m.name.to_ascii_lowercase().contains("engine fixes")
    });
    let has_preloader = mods
        .iter()
        .any(|m| m.staged_path.join(".root/d3dx9_42.dll").exists());
    if has_plugin && !has_preloader {
        issues.push(Issue {
            severity: Severity::Error,
            message: "Engine Fixes is installed without its Part 2 preloader \
                      (d3dx9_42.dll) - download the Part 2 archive"
                .to_owned(),
        });
    }
}

fn file_conflicts(plan: &DeployPlan, issues: &mut Vec<Issue>) {
    let count = plan.conflicts().len();
    if count > 0 {
        issues.push(Issue {
            severity: Severity::Warning,
            message: format!(
                "{count} file conflict(s) resolved by load order - review Load Order"
            ),
        });
    }
}
