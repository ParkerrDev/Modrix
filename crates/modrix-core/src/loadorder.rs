// SPDX-License-Identifier: GPL-2.0-only
//! Load-order strategies, dispatched on the game definition.
//!
//! A game either declares a load-order strategy in its `game.toml` (v2), or
//! names one (`load_order = "plugins_txt"`, v1 - parameters come from the
//! legacy preset table below), or has none at all (BepInEx-style games).
//! Core dispatches purely on this data; no engine code checks *which game*
//! it is talking to.

use std::path::{Path, PathBuf};

use crate::gamedef::{GameDef, LoadOrderDef, PluginsTxtDef};

/// A resolved load-order strategy for one game.
#[derive(Debug, Clone)]
pub enum LoadOrderStrategy {
    /// Bethesda's `Plugins.txt` activation file in the user's local app data.
    PluginsTxt {
        /// The game's folder under local app data (e.g. `Skyrim Special Edition`).
        appdata_dir: String,
    },
}

impl LoadOrderStrategy {
    /// Resolve the strategy a definition declares, if any. A v1 def naming
    /// `plugins_txt` resolves through the legacy preset table (which knows
    /// exactly the games that shipped before v2); anything unknown resolves
    /// to `None` rather than an error, matching v1 behavior.
    #[must_use]
    pub fn from_def(def: &GameDef) -> Option<Self> {
        match def.load_order.as_ref()? {
            LoadOrderDef::PluginsTxt(PluginsTxtDef {
                strategy,
                appdata_dir,
            }) if strategy == "plugins_txt" => Some(Self::PluginsTxt {
                appdata_dir: appdata_dir.clone(),
            }),
            LoadOrderDef::Named(name) if name == "plugins_txt" => {
                let appdata_dir = preset_appdata_dir(def.steam_appid)?;
                Some(Self::PluginsTxt {
                    appdata_dir: appdata_dir.to_owned(),
                })
            }
            LoadOrderDef::PluginsTxt(_) | LoadOrderDef::Named(_) => None,
        }
    }

    /// The per-game local-appdata directory holding `Plugins.txt`, when it
    /// can be resolved. For Steam installs running under Proton this lives
    /// inside the game's compatdata prefix.
    ///
    /// The game-specific leaf is **created** if the prefix itself is
    /// initialized: after a fresh reinstall the game has not run yet, and
    /// skipping the write would silently deploy mods whose plugins never
    /// activate.
    #[must_use]
    pub fn plugins_dir(&self, install_path: &Path, steam_appid: Option<i64>) -> Option<PathBuf> {
        match self {
            Self::PluginsTxt { appdata_dir } => {
                let local = proton_local_appdata(install_path, steam_appid?)?;
                let dir = local.join(appdata_dir);
                std::fs::create_dir_all(&dir).ok()?;
                Some(dir)
            }
        }
    }
}

/// The Proton prefix's `AppData/Local` for a Steam install, when initialized.
/// `<steamapps>/common/<Game>` → `<steamapps>/compatdata/<appid>/pfx/…`.
fn proton_local_appdata(install_path: &Path, appid: i64) -> Option<PathBuf> {
    let steamapps = install_path.parent()?.parent()?;
    let local = steamapps
        .join("compatdata")
        .join(appid.to_string())
        .join("pfx/drive_c/users/steamuser/AppData/Local");
    // No initialized prefix - the game has never run under Proton here.
    local.is_dir().then_some(local)
}

/// Legacy preset parameters for v1 definitions that name a strategy without
/// parameters. Frozen: contains exactly the games that shipped on
/// `api_version` 1 and never grows - new games declare parameters in their
/// definition instead.
fn preset_appdata_dir(steam_appid: Option<i64>) -> Option<&'static str> {
    match steam_appid? {
        489_830 => Some("Skyrim Special Edition"),
        377_160 => Some("Fallout4"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(toml: &str) -> GameDef {
        GameDef::from_toml_str(toml, Path::new("<test>")).unwrap()
    }

    #[test]
    fn v1_named_strategy_resolves_through_the_preset() {
        let d = def("api_version = 1\nid = \"skyrimse\"\nname = \"S\"\n\
             steam_appid = 489830\nload_order = \"plugins_txt\"\n");
        let Some(LoadOrderStrategy::PluginsTxt { appdata_dir }) = LoadOrderStrategy::from_def(&d)
        else {
            panic!("expected plugins_txt");
        };
        assert_eq!(appdata_dir, "Skyrim Special Edition");
    }

    #[test]
    fn v1_named_strategy_without_a_preset_resolves_to_none() {
        let d = def("api_version = 1\nid = \"x\"\nname = \"X\"\n\
             steam_appid = 999\nload_order = \"plugins_txt\"\n");
        assert!(LoadOrderStrategy::from_def(&d).is_none());
    }

    #[test]
    fn v2_table_strategy_carries_its_own_parameters() {
        let d = def("api_version = 2\nid = \"x\"\nname = \"X\"\n\
             [load_order]\nstrategy = \"plugins_txt\"\nappdata_dir = \"My Game\"\n");
        let Some(LoadOrderStrategy::PluginsTxt { appdata_dir }) = LoadOrderStrategy::from_def(&d)
        else {
            panic!("expected plugins_txt");
        };
        assert_eq!(appdata_dir, "My Game");
    }

    #[test]
    fn no_load_order_means_no_strategy() {
        let d = def("api_version = 2\nid = \"x\"\nname = \"X\"\n");
        assert!(LoadOrderStrategy::from_def(&d).is_none());
    }

    #[test]
    fn plugins_dir_creates_the_leaf_inside_an_initialized_prefix() {
        // Golden equivalence with the pre-v2 hard-coded resolver: same
        // compatdata path, same create-on-demand behavior.
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("steamapps/common/Skyrim Special Edition");
        let local = tmp
            .path()
            .join("steamapps/compatdata/489830/pfx/drive_c/users/steamuser/AppData/Local");
        std::fs::create_dir_all(&install).unwrap();
        let strategy = LoadOrderStrategy::PluginsTxt {
            appdata_dir: "Skyrim Special Edition".to_owned(),
        };
        // No prefix yet → no directory (the game never ran).
        assert_eq!(strategy.plugins_dir(&install, Some(489_830)), None);
        // Initialized prefix → the leaf is created on demand.
        std::fs::create_dir_all(&local).unwrap();
        let dir = strategy.plugins_dir(&install, Some(489_830)).unwrap();
        assert_eq!(dir, local.join("Skyrim Special Edition"));
        assert!(dir.is_dir());
    }
}
