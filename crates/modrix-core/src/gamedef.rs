// SPDX-License-Identifier: GPL-2.0-only
//! Declarative game definitions (`game.toml`).
//!
//! Tier 1 of the plugin system (see `docs/ARCHITECTURE.md` §5.1): most games
//! are pure data - an id, a name, where mods deploy, and (since `api_version` 2)
//! the game's *capabilities*: its load-order strategy, which directories count
//! as mod content when normalizing archives, how to find hand-installed mods,
//! and game-specific health checks. Core dispatches on this data and carries
//! **no game-specific logic of its own**; anything a definition does not
//! declare simply does not run. The Lua tier (`game.lua`) adds logic on top in
//! `modrix-plugin`; this stays a small, dependency-light parser so the engine
//! can drive a data-defined game without linking the Lua host.

use std::path::Path;

use crate::error::{Error, Result};

/// The oldest `game.toml` schema version this build understands.
pub const MIN_API_VERSION: u32 = 1;
/// The newest `game.toml` schema version this build understands.
pub const MAX_API_VERSION: u32 = 2;

/// A parsed, validated game definition.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GameDef {
    /// Definition schema version; must be within
    /// [`MIN_API_VERSION`]..=[`MAX_API_VERSION`].
    pub api_version: u32,
    /// Stable identifier (e.g. `skyrimse`), used as the plugin id.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Steam AppID, when the game is on Steam.
    #[serde(default)]
    pub steam_appid: Option<i64>,
    /// The Nexus Mods game domain (e.g. `skyrimspecialedition`), used to route
    /// incoming `nxm://` links to this game.
    #[serde(default)]
    pub nexus_domain: Option<String>,
    /// Where mods deploy, relative to the install path (e.g. `Data`). May be
    /// empty to deploy at the install root.
    #[serde(default)]
    pub mod_root: String,
    /// Deploy strategy hint (`link` or `copy`); the applier always falls back
    /// link → copy, so this only forces copy when a game cannot tolerate links.
    #[serde(default)]
    pub deploy: Option<String>,
    /// Ordered install-probe strategies (`steam`, `registry`, `path-hint`).
    #[serde(default)]
    pub install_probe: Vec<String>,
    /// The game's load-order strategy, when it has one. v1 defs use a bare
    /// name (`"plugins_txt"`, parameters from a legacy preset); v2 defs use a
    /// table with explicit parameters. Absent = the game has no load-order
    /// file (e.g. BepInEx orders by declared dependencies).
    #[serde(default)]
    pub load_order: Option<LoadOrderDef>,
    /// Directory names that ARE mod content at the root of an archive, never
    /// a packaging wrapper to hoist away (e.g. Bethesda's `meshes`,
    /// `textures`; BepInEx's `bepinex`). Compared case-insensitively. Empty =
    /// use the conservative built-in default list (v1 compatibility).
    #[serde(default)]
    pub content_dirs: Vec<String>,
    /// Lowercased file names the base game ships in its deploy root (vanilla
    /// plugins, DLC). A trailing `*` matches a prefix (`cc*` for Creation
    /// Club). Used to classify unowned files as vanilla rather than foreign.
    #[serde(default)]
    pub base_files: Vec<String>,
    /// How to find hand-installed (external) mods in the game directory.
    /// Empty on a v1 def = the built-in default detectors; empty on a v2 def
    /// = the game declares no external-mod layout and nothing is scanned.
    #[serde(default)]
    pub external_scan: Vec<ExternalScanDef>,
    /// Game-specific health checks (script-extender loader, known mod
    /// pairings). Absent = none run.
    #[serde(default)]
    pub health: Option<HealthDef>,
}

/// A load-order strategy declaration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum LoadOrderDef {
    /// v1 shorthand: a strategy name; parameters come from the legacy preset
    /// table in `loadorder.rs` (kept only for pre-v2 definitions).
    Named(String),
    /// v2: an explicit strategy table.
    PluginsTxt(PluginsTxtDef),
}

/// Parameters of the `plugins_txt` strategy (Bethesda's activation file).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginsTxtDef {
    /// Strategy discriminator; must be `"plugins_txt"`.
    pub strategy: String,
    /// The game's folder under the user's local app data (e.g.
    /// `Skyrim Special Edition`), where `Plugins.txt` lives.
    pub appdata_dir: String,
}

/// One external-mod detector: how hand-installed content appears on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExternalScanDef {
    /// `"file"` (each matching file is one external mod) or `"folder"` (each
    /// subdirectory is one external mod).
    pub kind: String,
    /// Human label for the kind (e.g. `SKSE plugin`, `BepInEx plugin`).
    pub label: String,
    /// Directory to scan, relative to the deploy root (matched
    /// case-insensitively). Empty = the deploy root itself.
    #[serde(default)]
    pub dir: String,
    /// For `kind = "file"`: extensions to report (lowercase, no dot).
    #[serde(default)]
    pub exts: Vec<String>,
    /// For `kind = "file"`: skip names in the definition's `base_files`
    /// (vanilla content is not an external mod).
    #[serde(default)]
    pub skip_base: bool,
}

/// Game-specific health checks, all data-driven.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthDef {
    /// A script-extender-style loader requirement.
    #[serde(default)]
    pub loader: Option<LoaderCheckDef>,
    /// Known "part A needs part B" mod pairings.
    #[serde(default)]
    pub recommended: Vec<RecommendDef>,
}

/// "Mods ship `<plugins_dir>` content, so a loader binary must be present."
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoaderCheckDef {
    /// Staged-tree-relative directory whose presence in any mod means the
    /// loader is required (e.g. `SKSE/Plugins`).
    pub plugins_dir: String,
    /// A root-parked binary (`.root/<name>`) starting with this prefix
    /// (case-insensitive) satisfies the requirement (e.g. `skse`).
    pub root_prefix: String,
    /// The issue text shown when the loader is missing.
    pub message: String,
}

/// "If a mod matches, it also needs this root-parked file."
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecommendDef {
    /// Trigger: a staged file at this relative path in any mod.
    #[serde(default)]
    pub if_file: Option<String>,
    /// Trigger: a mod whose name contains this (case-insensitive).
    #[serde(default)]
    pub if_name_contains: Option<String>,
    /// Required: `.root/<name>` present in any mod.
    pub requires_root_file: String,
    /// The issue text shown when the requirement is missing.
    pub message: String,
}

impl GameDef {
    /// Parse and validate a `game.toml` from its text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GameDef`] if the TOML is malformed, the `api_version` is
    /// unsupported, or required fields are empty.
    pub fn from_toml_str(text: &str, source: &Path) -> Result<Self> {
        let def: Self = toml::from_str(text).map_err(|e| Error::GameDef {
            path: source.to_path_buf(),
            message: e.to_string(),
        })?;
        def.validate(source)?;
        Ok(def)
    }

    /// Load and validate a `game.toml` from disk.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the file cannot be read, or [`Error::GameDef`]
    /// as for [`GameDef::from_toml_str`].
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        Self::from_toml_str(&text, path)
    }

    /// Serialize back to JSON (how the engine persists the def it registered
    /// a game with, so capabilities survive without re-reading files).
    ///
    /// # Errors
    ///
    /// Returns [`Error::GameDef`] if serialization fails (practically never).
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| Error::GameDef {
            path: std::path::PathBuf::from("<def>"),
            message: e.to_string(),
        })
    }

    /// Parse a def previously persisted with [`GameDef::to_json`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::GameDef`] on malformed JSON.
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).map_err(|e| Error::GameDef {
            path: std::path::PathBuf::from("<def-json>"),
            message: e.to_string(),
        })
    }

    fn validate(&self, source: &Path) -> Result<()> {
        let complaint = if !(MIN_API_VERSION..=MAX_API_VERSION).contains(&self.api_version) {
            Some(format!(
                "unsupported api_version {} (this build supports {MIN_API_VERSION}..={MAX_API_VERSION})",
                self.api_version
            ))
        } else if self.id.trim().is_empty() {
            Some("`id` must not be empty".to_owned())
        } else if self.name.trim().is_empty() {
            Some("`name` must not be empty".to_owned())
        } else {
            self.validate_details()
        };
        match complaint {
            Some(message) => Err(Error::GameDef {
                path: source.to_path_buf(),
                message,
            }),
            None => Ok(()),
        }
    }

    fn validate_details(&self) -> Option<String> {
        if let Some(LoadOrderDef::PluginsTxt(t)) = &self.load_order {
            if t.strategy != "plugins_txt" {
                return Some(format!("unknown load_order strategy `{}`", t.strategy));
            }
            if t.appdata_dir.trim().is_empty() {
                return Some("load_order.appdata_dir must not be empty".to_owned());
            }
        }
        for scan in &self.external_scan {
            if scan.kind != "file" && scan.kind != "folder" {
                return Some(format!(
                    "external_scan kind `{}` is not `file` or `folder`",
                    scan.kind
                ));
            }
            if scan.kind == "file" && scan.exts.is_empty() {
                return Some("a `file` external_scan needs `exts`".to_owned());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn inline() -> PathBuf {
        PathBuf::from("<inline>")
    }

    const SAMPLE_V1: &str = r#"
        api_version = 1
        id = "skyrimse"
        name = "Skyrim Special Edition"
        steam_appid = 489_830
        mod_root = "Data"
        deploy = "link"
        install_probe = ["steam", "registry"]
        load_order = "plugins_txt"
    "#;

    #[test]
    fn parses_a_valid_v1_definition() {
        let def = GameDef::from_toml_str(SAMPLE_V1, &inline()).unwrap();
        assert_eq!(def.id, "skyrimse");
        assert_eq!(def.steam_appid, Some(489_830));
        assert_eq!(def.mod_root, "Data");
        assert!(matches!(
            def.load_order,
            Some(LoadOrderDef::Named(ref n)) if n == "plugins_txt"
        ));
    }

    #[test]
    fn parses_a_v2_definition_with_capabilities() {
        let text = r#"
            api_version = 2
            id = "testgame"
            name = "Test"
            mod_root = "Data"
            content_dirs = ["meshes", "textures"]
            base_files = ["base.esm", "cc*"]

            [load_order]
            strategy = "plugins_txt"
            appdata_dir = "Test Game"

            [[external_scan]]
            kind = "file"
            label = "plugin"
            exts = ["esp", "esm"]
            skip_base = true

            [[external_scan]]
            kind = "folder"
            label = "BepInEx plugin"
            dir = "BepInEx/plugins"

            [health.loader]
            plugins_dir = "SKSE/Plugins"
            root_prefix = "skse"
            message = "loader missing"

            [[health.recommended]]
            if_file = "SKSE/Plugins/EngineFixes.dll"
            requires_root_file = "d3dx9_42.dll"
            message = "part 2 missing"
        "#;
        let def = GameDef::from_toml_str(text, &inline()).unwrap();
        assert!(matches!(
            def.load_order,
            Some(LoadOrderDef::PluginsTxt(ref t)) if t.appdata_dir == "Test Game"
        ));
        assert_eq!(def.content_dirs, vec!["meshes", "textures"]);
        assert_eq!(def.external_scan.len(), 2);
        assert_eq!(def.external_scan[1].kind, "folder");
        let health = def.health.unwrap();
        assert_eq!(health.loader.unwrap().root_prefix, "skse");
        assert_eq!(health.recommended.len(), 1);
    }

    #[test]
    fn shipped_definitions_are_valid_v2() {
        for (path, text) in [
            (
                "games/skyrimse/game.toml",
                include_str!("../../../games/skyrimse/game.toml"),
            ),
            (
                "games/subnautica/game.toml",
                include_str!("../../../games/subnautica/game.toml"),
            ),
        ] {
            let def = GameDef::from_toml_str(text, std::path::Path::new(path)).unwrap();
            assert_eq!(def.api_version, 2, "{path} should be api_version 2");
        }
    }

    #[test]
    fn shipped_subnautica_definition_declares_bepinex() {
        // Subnautica is a Unity/BepInEx game: mods deploy into the plugin
        // container (a nested mod root) and there is no load-order strategy -
        // BepInEx orders plugins by their declared dependencies.
        let text = include_str!("../../../games/subnautica/game.toml");
        let def = GameDef::from_toml_str(text, std::path::Path::new("games/subnautica/game.toml"))
            .unwrap();
        assert_eq!(def.id, "subnautica");
        assert_eq!(def.steam_appid, Some(264_710));
        assert_eq!(def.mod_root, "BepInEx/plugins");
        assert!(def.load_order.is_none());
        assert!(def.external_scan.iter().any(|s| s.kind == "folder"));
    }

    #[test]
    fn def_round_trips_through_json() {
        let def = GameDef::from_toml_str(SAMPLE_V1, &inline()).unwrap();
        let json = def.to_json().unwrap();
        let back = GameDef::from_json(&json).unwrap();
        assert_eq!(back.id, def.id);
        assert!(matches!(back.load_order, Some(LoadOrderDef::Named(_))));
    }

    #[test]
    fn rejects_unsupported_api_version() {
        let text = "api_version = 999\nid = \"x\"\nname = \"X\"\n";
        let err = GameDef::from_toml_str(text, &inline()).unwrap_err();
        assert!(matches!(err, Error::GameDef { .. }));
    }

    #[test]
    fn rejects_a_bad_scan_kind() {
        let text = "api_version = 2\nid = \"x\"\nname = \"X\"\n\
                    [[external_scan]]\nkind = \"weird\"\nlabel = \"x\"\n";
        assert!(GameDef::from_toml_str(text, &inline()).is_err());
    }

    #[test]
    fn rejects_empty_id() {
        let text = "api_version = 1\nid = \"\"\nname = \"X\"\n";
        assert!(GameDef::from_toml_str(text, &inline()).is_err());
    }

    #[test]
    fn defaults_optional_fields() {
        let text = "api_version = 1\nid = \"x\"\nname = \"X\"\n";
        let def = GameDef::from_toml_str(text, &inline()).unwrap();
        assert_eq!(def.mod_root, "");
        assert!(def.steam_appid.is_none());
        assert!(def.install_probe.is_empty());
        assert!(def.content_dirs.is_empty());
        assert!(def.external_scan.is_empty());
        assert!(def.health.is_none());
    }
}
