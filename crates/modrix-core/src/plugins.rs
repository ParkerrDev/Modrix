// SPDX-License-Identifier: GPL-2.0-only
//! Plugin (.esp/.esm/.esl) load-order management.
//!
//! Mods carry *plugins*; the game loads plugins in the order named by its
//! `Plugins.txt`. This module discovers the plugins the enabled mods provide,
//! merges the profile's persisted ordering, detects missing masters, sorts
//! automatically (masters before dependents, master tier first - the LOOT
//! core rules), and renders `Plugins.txt`/`loadorder.txt`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::esp;
use crate::id::ModId;

/// Upper bound on plugins considered (a full load order is ~4096 lights max).
const MAX_PLUGINS: usize = 8192;

/// One plugin in the profile's load order.
#[derive(Debug, Clone)]
pub struct GamePlugin {
    /// The plugin filename (`SkyUI_SE.esp`).
    pub name: String,
    /// The mod that provides it (the override winner).
    pub mod_id: ModId,
    /// The providing mod's display name.
    pub mod_name: String,
    /// Master tier (ESM flag or `.esm`/`.esl` extension) - loads first.
    pub is_master: bool,
    /// Light plugin (ESL).
    pub is_light: bool,
    /// Its declared masters.
    pub masters: Vec<String>,
    /// Masters that are neither vanilla nor provided by an enabled mod.
    pub missing_masters: Vec<String>,
    /// Whether the plugin is activated in `Plugins.txt`.
    pub enabled: bool,
}

/// A plugin file discovered in an enabled mod's staged tree.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    /// Filename.
    pub name: String,
    /// Providing mod.
    pub mod_id: ModId,
    /// Providing mod's name.
    pub mod_name: String,
    /// Absolute path to the staged file.
    pub path: PathBuf,
}

/// Assemble the profile's plugin list: discovered plugins, annotated with
/// parsed headers and missing masters, ordered by (persisted order, then
/// discovery order), with the master tier always ahead of the regular tier.
#[must_use]
pub fn assemble<S: std::hash::BuildHasher>(
    discovered: &[DiscoveredPlugin],
    vanilla: &HashSet<String, S>,
    saved_order: &[(String, bool)],
) -> Vec<GamePlugin> {
    let mut plugins: Vec<GamePlugin> = discovered
        .iter()
        .take(MAX_PLUGINS)
        .map(|d| {
            let header = esp::parse_header(&d.path).unwrap_or(esp::PluginHeader {
                is_master: false,
                is_light: false,
                masters: Vec::new(),
            });
            GamePlugin {
                name: d.name.clone(),
                mod_id: d.mod_id,
                mod_name: d.mod_name.clone(),
                is_master: header.is_master,
                is_light: header.is_light,
                masters: header.masters,
                missing_masters: Vec::new(),
                enabled: true,
            }
        })
        .collect();
    apply_saved(&mut plugins, saved_order);
    tier_partition(&mut plugins);
    annotate_missing(&mut plugins, vanilla);
    plugins
}

/// Order by the persisted (position, enabled) list; unknown plugins keep
/// discovery order after the known ones.
fn apply_saved(plugins: &mut Vec<GamePlugin>, saved: &[(String, bool)]) {
    let rank: HashMap<String, (usize, bool)> = saved
        .iter()
        .enumerate()
        .map(|(i, (name, enabled))| (name.to_ascii_lowercase(), (i, *enabled)))
        .collect();
    for plugin in plugins.iter_mut() {
        if let Some((_, enabled)) = rank.get(&plugin.name.to_ascii_lowercase()) {
            plugin.enabled = *enabled;
        }
    }
    let fallback = saved.len();
    let mut indexed: Vec<(usize, usize, GamePlugin)> = std::mem::take(plugins)
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let key = rank
                .get(&p.name.to_ascii_lowercase())
                .map_or(fallback, |(pos, _)| *pos);
            (key, i, p)
        })
        .collect();
    indexed.sort_by_key(|(key, i, _)| (*key, *i));
    *plugins = indexed.into_iter().map(|(_, _, p)| p).collect();
}

/// The game always loads the master tier first; mirror that visibly.
fn tier_partition(plugins: &mut Vec<GamePlugin>) {
    let (masters, regular): (Vec<GamePlugin>, Vec<GamePlugin>) =
        std::mem::take(plugins).into_iter().partition(|p| p.is_master);
    *plugins = masters;
    plugins.extend(regular);
}

fn annotate_missing<S: std::hash::BuildHasher>(
    plugins: &mut [GamePlugin],
    vanilla: &HashSet<String, S>,
) {
    let present: HashSet<String> = plugins
        .iter()
        .map(|p| p.name.to_ascii_lowercase())
        .collect();
    for plugin in plugins.iter_mut() {
        plugin.missing_masters = plugin
            .masters
            .iter()
            .filter(|m| {
                let key = m.to_ascii_lowercase();
                !present.contains(&key) && !vanilla.contains(&key)
            })
            .cloned()
            .collect();
    }
}

/// Topological auto-sort (the LOOT core rules): masters tier first, and
/// within each tier every plugin loads after its masters. Ties resolve
/// alphabetically, so the result is a pure function of the installed set -
/// two runs always agree, whatever order mods were installed in. Cycles
/// keep their alphabetical order at the end of their tier.
#[must_use]
pub fn auto_sort(plugins: &[GamePlugin]) -> Vec<String> {
    let mut order = Vec::with_capacity(plugins.len());
    for tier in [true, false] {
        let mut members: Vec<&GamePlugin> =
            plugins.iter().filter(|p| p.is_master == tier).collect();
        members.sort_by_key(|p| p.name.to_ascii_lowercase());
        order.extend(sort_tier(&members));
    }
    order
}

fn sort_tier(members: &[&GamePlugin]) -> Vec<String> {
    let index: HashMap<String, usize> = members
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.to_ascii_lowercase(), i))
        .collect();
    // Kahn's algorithm, always emitting the lowest current-position ready
    // node - a stable topological sort.
    let mut indegree: Vec<usize> = vec![0; members.len()];
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); members.len()];
    for (i, plugin) in members.iter().enumerate() {
        for master in &plugin.masters {
            if let Some(&m) = index.get(&master.to_ascii_lowercase())
                && m != i
            {
                if let Some(out) = edges.get_mut(m) {
                    out.push(i);
                }
                if let Some(d) = indegree.get_mut(i) {
                    *d = d.saturating_add(1);
                }
            }
        }
    }
    let mut ready: Vec<usize> = indegree
        .iter()
        .enumerate()
        .filter(|(_, d)| **d == 0)
        .map(|(i, _)| i)
        .collect();
    let mut out = Vec::with_capacity(members.len());
    let mut emitted = vec![false; members.len()];
    while let Some(pos) = ready.iter().enumerate().min_by_key(|(_, i)| **i).map(|(p, _)| p) {
        let node = ready.swap_remove(pos);
        if let Some(e) = emitted.get_mut(node) {
            *e = true;
        }
        if let Some(plugin) = members.get(node) {
            out.push(plugin.name.clone());
        }
        for &next in edges.get(node).map_or(&[][..], Vec::as_slice) {
            if let Some(d) = indegree.get_mut(next) {
                *d = d.saturating_sub(1);
                if *d == 0 {
                    ready.push(next);
                }
            }
        }
    }
    // Cycle members (never became ready) keep their current order at the end.
    for (i, plugin) in members.iter().enumerate() {
        if !emitted.get(i).copied().unwrap_or(true) {
            out.push(plugin.name.clone());
        }
    }
    out
}

/// Render `Plugins.txt` (the game's activation file): one line per managed
/// plugin, `*` marking enabled. Vanilla and Creation Club content is loaded
/// implicitly by the game and must not be listed.
#[must_use]
pub fn render_plugins_txt(plugins: &[GamePlugin]) -> String {
    let mut out = String::from("# Managed by Modrix\n");
    for plugin in plugins {
        if plugin.enabled {
            out.push('*');
        }
        out.push_str(&plugin.name);
        out.push('\n');
    }
    out
}

/// The vanilla plugins of a game install: everything in its Data directory
/// that the deployment manifest does not own (base game, DLC, Creation Club).
#[must_use]
pub fn vanilla_plugins<S: std::hash::BuildHasher>(
    data_dir: &Path,
    managed: &HashSet<String, S>,
) -> HashSet<String> {
    let mut vanilla = HashSet::new();
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return vanilla;
    };
    for entry in entries.flatten().take(MAX_PLUGINS) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let key = name.to_ascii_lowercase();
        if esp::is_plugin_name(&name) && !managed.contains(&key) {
            vanilla.insert(key);
        }
    }
    vanilla
}

/// The per-game local-appdata directory holding `Plugins.txt`, when it can be
/// resolved. For Steam installs running under Proton this lives inside the
/// game's compatdata prefix.
///
/// The game-specific leaf is **created** if the prefix itself is initialized:
/// after a fresh reinstall the game has not run yet, and skipping the write
/// would silently deploy mods whose plugins never activate.
#[must_use]
pub fn plugins_txt_dir(install_path: &Path, steam_appid: Option<i64>) -> Option<PathBuf> {
    let appid = steam_appid?;
    let local_dir = match appid {
        489_830 => "Skyrim Special Edition",
        377_160 => "Fallout4",
        _ => return None,
    };
    // <steamapps>/common/<Game> -> <steamapps>/compatdata/<appid>/pfx/...
    let steamapps = install_path.parent()?.parent()?;
    let local = steamapps
        .join("compatdata")
        .join(appid.to_string())
        .join("pfx/drive_c/users/steamuser/AppData/Local");
    if !local.is_dir() {
        // No initialized prefix - the game has never run under Proton here.
        return None;
    }
    let dir = local.join(local_dir);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(name: &str, is_master: bool, masters: &[&str]) -> GamePlugin {
        GamePlugin {
            name: name.to_owned(),
            mod_id: crate::id::ModId::from_raw(1),
            mod_name: "m".to_owned(),
            is_master,
            is_light: false,
            masters: masters.iter().map(|s| (*s).to_owned()).collect(),
            missing_masters: Vec::new(),
            enabled: true,
        }
    }

    #[test]
    fn auto_sort_puts_masters_before_dependents() {
        let plugins = vec![
            plugin("patch.esp", false, &["base.esp"]),
            plugin("base.esp", false, &["Skyrim.esm"]),
            plugin("core.esm", true, &[]),
        ];
        assert_eq!(auto_sort(&plugins), vec!["core.esm", "base.esp", "patch.esp"]);
    }

    #[test]
    fn auto_sort_is_stable_for_unrelated_plugins() {
        let plugins = vec![
            plugin("a.esp", false, &[]),
            plugin("b.esp", false, &[]),
            plugin("c.esp", false, &[]),
        ];
        assert_eq!(auto_sort(&plugins), vec!["a.esp", "b.esp", "c.esp"]);
    }

    #[test]
    fn auto_sort_survives_cycles() {
        let plugins = vec![
            plugin("x.esp", false, &["y.esp"]),
            plugin("y.esp", false, &["x.esp"]),
            plugin("z.esp", false, &[]),
        ];
        let order = auto_sort(&plugins);
        assert_eq!(order.len(), 3);
        assert!(order.contains(&"z.esp".to_owned()));
    }

    #[test]
    fn missing_masters_ignore_vanilla_and_present() {
        let mut plugins = vec![plugin("p.esp", false, &["Skyrim.esm", "ELE_SSE.esp"])];
        let vanilla: HashSet<String> = ["skyrim.esm".to_owned()].into();
        annotate_missing(&mut plugins, &vanilla);
        assert_eq!(plugins[0].missing_masters, vec!["ELE_SSE.esp"]);
    }

    #[test]
    fn plugins_txt_dir_creates_the_leaf_inside_an_initialized_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("steamapps/common/Skyrim Special Edition");
        let local = tmp
            .path()
            .join("steamapps/compatdata/489830/pfx/drive_c/users/steamuser/AppData/Local");
        std::fs::create_dir_all(&install).unwrap();
        // No prefix yet → no directory (the game never ran).
        assert_eq!(plugins_txt_dir(&install, Some(489_830)), None);
        // Initialized prefix → the leaf is created on demand.
        std::fs::create_dir_all(&local).unwrap();
        let dir = plugins_txt_dir(&install, Some(489_830)).unwrap();
        assert_eq!(dir, local.join("Skyrim Special Edition"));
        assert!(dir.is_dir());
    }

    #[test]
    fn renders_plugins_txt_with_activation_stars() {
        let mut plugins = vec![plugin("a.esp", false, &[]), plugin("b.esp", false, &[])];
        plugins[1].enabled = false;
        let text = render_plugins_txt(&plugins);
        assert_eq!(text, "# Managed by Modrix\n*a.esp\nb.esp\n");
    }
}
