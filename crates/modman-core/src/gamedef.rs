// SPDX-License-Identifier: GPL-2.0-only
//! Declarative game definitions (`game.toml`).
//!
//! Tier 1 of the plugin system (see `docs/ARCHITECTURE.md` §5.1): most games are
//! pure data - an id, a name, where mods deploy - and need no code. This loads
//! and validates that data. The Lua tier (`game.lua`) and plugin discovery come
//! later, in `modman-plugin`; this stays a small, dependency-light parser so the
//! engine can drive a data-defined game without linking the Lua host.

use std::path::Path;

use crate::error::{Error, Result};

/// The `game.toml` schema version this build understands.
pub const SUPPORTED_API_VERSION: u32 = 1;

/// A parsed, validated game definition.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GameDef {
    /// Definition schema version; must be [`SUPPORTED_API_VERSION`].
    pub api_version: u32,
    /// Stable identifier (e.g. `skyrimse`), used as the plugin id.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Steam AppID, when the game is on Steam.
    #[serde(default)]
    pub steam_appid: Option<i64>,
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
    /// Named load-order strategy provided by core (e.g. `plugins_txt`).
    #[serde(default)]
    pub load_order: Option<String>,
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

    fn validate(&self, source: &Path) -> Result<()> {
        let complaint = if self.api_version != SUPPORTED_API_VERSION {
            Some(format!(
                "unsupported api_version {} (this build supports {SUPPORTED_API_VERSION})",
                self.api_version
            ))
        } else if self.id.trim().is_empty() {
            Some("`id` must not be empty".to_owned())
        } else if self.name.trim().is_empty() {
            Some("`name` must not be empty".to_owned())
        } else {
            None
        };
        match complaint {
            Some(message) => Err(Error::GameDef {
                path: source.to_path_buf(),
                message,
            }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn inline() -> PathBuf {
        PathBuf::from("<inline>")
    }

    const SAMPLE: &str = r#"
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
    fn parses_a_valid_definition() {
        let def = GameDef::from_toml_str(SAMPLE, &inline()).unwrap();
        assert_eq!(def.id, "skyrimse");
        assert_eq!(def.steam_appid, Some(489_830));
        assert_eq!(def.mod_root, "Data");
    }

    #[test]
    fn rejects_unsupported_api_version() {
        let text = "api_version = 999\nid = \"x\"\nname = \"X\"\n";
        let err = GameDef::from_toml_str(text, &inline()).unwrap_err();
        assert!(matches!(err, Error::GameDef { .. }));
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
    }
}
