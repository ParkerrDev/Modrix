// SPDX-License-Identifier: GPL-2.0-only
//! Setup health checks: the problems Vortex surfaces before you launch.
//!
//! Analysis over a [`Snapshot`] of the profile - its plugins, mods, deploy
//! plan, and conflict state. Each issue carries a severity so frontends can
//! colour it, and `blocking` marks the issues a deploy refuses to run over.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::deploy::DeployPlan;
use crate::model::Mod;
use crate::plugins::GamePlugin;
use crate::rules::ModConflict;

/// Most Data-directory entries scanned for foreign files (bounded loop).
const MAX_SCAN: usize = 8192;

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
    /// The game's deploy root (its `Data` directory).
    pub data_dir: &'a Path,
    /// Lowercased target paths the current deployment owns.
    pub owned: &'a HashSet<String>,
    /// The game's Steam app id, when known (drives the vanilla whitelist).
    pub steam_appid: Option<i64>,
}

/// Analyse a profile for setup problems, worst first.
#[must_use]
pub fn check(snapshot: &Snapshot<'_>) -> Vec<Issue> {
    let mut issues = Vec::new();
    rule_cycle(snapshot.rule_cycle, &mut issues);
    missing_masters(snapshot.plugins, &mut issues);
    unresolved_conflicts(snapshot.conflicts, snapshot.plan, &mut issues);
    skse_loader(snapshot.mods, &mut issues);
    engine_fixes_pair(snapshot.mods, &mut issues);
    foreign_files(
        snapshot.data_dir,
        snapshot.owned,
        snapshot.steam_appid,
        &mut issues,
    );
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

/// SKSE plugins (`SKSE/Plugins/*.dll`) need the SKSE loader; without it they
/// silently do nothing (the CommunityShaders/TerrainHelper class of failure).
fn skse_loader(mods: &[Mod], issues: &mut Vec<Issue>) {
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
    if has_skse_plugins && !has_loader {
        issues.push(Issue {
            severity: Severity::Error,
            message: "SKSE plugins are installed but the SKSE loader is missing - \
                      install Skyrim Script Extender"
                .to_owned(),
            blocking: false,
        });
    }
}

/// SSE Engine Fixes ships as two parts: the SKSE plugin and a root preloader
/// (`d3dx9_42.dll`). One without the other fails at launch.
fn engine_fixes_pair(mods: &[Mod], issues: &mut Vec<Issue>) {
    let has_plugin = mods.iter().any(|m| {
        m.staged_path.join("SKSE/Plugins/EngineFixes.dll").exists()
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
            blocking: false,
        });
    }
}

/// Vanilla plugins of the base game (lowercased), by Steam app id. Creation
/// Club content (`cc*`) is matched by prefix.
pub(crate) fn base_plugins(steam_appid: Option<i64>) -> &'static [&'static str] {
    match steam_appid {
        Some(489_830) => &[
            "skyrim.esm",
            "update.esm",
            "dawnguard.esm",
            "hearthfires.esm",
            "dragonborn.esm",
            "_resourcepack.esl",
        ],
        Some(377_160) => &[
            "fallout4.esm",
            "dlcrobot.esm",
            "dlcworkshop01.esm",
            "dlccoast.esm",
            "dlcworkshop02.esm",
            "dlcworkshop03.esm",
            "dlcnukaworld.esm",
            "dlcultrahighresolution.esm",
        ],
        _ => &[],
    }
}

/// Files in the game the deployment does not own and the base game does not
/// ship: leftovers from a previous manager or hand-copied installs. These
/// survive Steam uninstall/reinstall (Steam removes only its own files) and
/// cause "ghost mod" behaviour - plugins and SKSE DLLs that keep loading
/// after their mod was deleted here.
fn foreign_files(
    data_dir: &Path,
    owned: &HashSet<String>,
    steam_appid: Option<i64>,
    issues: &mut Vec<Issue>,
) {
    let base = base_plugins(steam_appid);
    let mut foreign: Vec<String> = Vec::new();
    // Loose plugins in Data that are neither deployed nor vanilla.
    if let Ok(entries) = std::fs::read_dir(data_dir) {
        for entry in entries.flatten().take(MAX_SCAN) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let key = name.to_ascii_lowercase();
            if crate::esp::is_plugin_name(&name)
                && !owned.contains(&key)
                && !base.contains(&key.as_str())
                && !key.starts_with("cc")
            {
                foreign.push(name);
            }
        }
    }
    // The base game ships no SKSE directory: anything unowned there is foreign.
    if let Ok(entries) = std::fs::read_dir(data_dir.join("SKSE/Plugins")) {
        for entry in entries.flatten().take(MAX_SCAN) {
            if !entry.path().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let key = format!("skse/plugins/{}", name.to_ascii_lowercase());
            if !owned.contains(&key) {
                foreign.push(format!("SKSE/Plugins/{name}"));
            }
        }
    }
    if foreign.is_empty() {
        return;
    }
    foreign.sort();
    let mut shown = foreign
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if foreign.len() > 3 {
        shown.push_str(", …");
    }
    issues.push(Issue {
        severity: Severity::Warning,
        message: format!(
            "{} file(s) in the game folder are not managed by Modrix \
             ({shown}) - leftovers from installs outside Modrix; \
             delete them or reinstall the mod here",
            foreign.len(),
        ),
        blocking: false,
    });
}
