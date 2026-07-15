// SPDX-License-Identifier: GPL-2.0-only
//! The domain model: the records the engine and frontends exchange.
//!
//! These mirror the SQLite rows (see `migrations/0001_init.sql`) but are the
//! public, storage-agnostic shape frontends see - no `rusqlite` types leak out.

use std::path::PathBuf;

use crate::id::{GameId, ModId, ProfileId};

/// How a deployed file was placed into the game directory.
///
/// The applier tries these in order (`Hardlink` → `Symlink` → `Copy`) and
/// records which one succeeded, so undeploy and verify know how to check and
/// undo it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkType {
    /// A hardlink: same inode as the staged file (same filesystem only).
    Hardlink,
    /// A symbolic link pointing at the staged file.
    Symlink,
    /// A byte-for-byte copy of the staged file.
    Copy,
}

impl LinkType {
    /// The stable lowercase token stored in the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hardlink => "hardlink",
            Self::Symlink => "symlink",
            Self::Copy => "copy",
        }
    }

    /// Parse the database token back into a [`LinkType`].
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "hardlink" => Some(Self::Hardlink),
            "symlink" => Some(Self::Symlink),
            "copy" => Some(Self::Copy),
            _ => None,
        }
    }
}

/// A resolved game install the engine manages mods for.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Game {
    /// Primary key.
    pub id: GameId,
    /// The plugin/definition that drives this game (e.g. `skyrimse`).
    pub plugin_id: String,
    /// Human-readable name.
    pub name: String,
    /// Absolute path to the game's install directory.
    pub install_path: PathBuf,
    /// Where mods deploy, relative to `install_path` (e.g. `Data`; may be empty).
    pub mod_root: String,
    /// The store this install came from (`steam`, `manual`, …).
    pub store: String,
    /// Steam AppID, when known.
    pub steam_appid: Option<i64>,
    /// The Nexus game domain, used to route `nxm://` links to this game.
    pub nexus_domain: Option<String>,
    /// Root under which this game's mods are staged.
    pub staging_root: PathBuf,
}

impl Game {
    /// The absolute directory files deploy into: `install_path` joined with
    /// `mod_root` (which may be empty, in which case it is `install_path`).
    #[must_use]
    pub fn deploy_target_root(&self) -> PathBuf {
        if self.mod_root.is_empty() {
            self.install_path.clone()
        } else {
            self.install_path.join(&self.mod_root)
        }
    }
}

/// A named, switchable set of enabled mods + load order for one game.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Profile {
    /// Primary key.
    pub id: ProfileId,
    /// The game this profile belongs to.
    pub game_id: GameId,
    /// Human-readable name (unique within the game).
    pub name: String,
    /// Whether this is the game's currently selected profile.
    pub is_active: bool,
}

/// A staged mod: an extracted archive in the central store plus provenance.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Mod {
    /// Primary key.
    pub id: ModId,
    /// The game this mod belongs to.
    pub game_id: GameId,
    /// Human-readable name.
    pub name: String,
    /// Version string, when known.
    pub version: Option<String>,
    /// Where the mod came from (`local`, `nexus`, …).
    pub source: String,
    /// Absolute path to the extracted mod tree in the staging store.
    pub staged_path: PathBuf,
    /// Install lifecycle: `staged`, or `fomod` when a FOMOD installer
    /// configured this tree (re-configurable from its parked sources).
    pub install_state: String,
    /// The source archive, kept for reinstall/reconfigure when known.
    pub archive_path: Option<PathBuf>,
    /// The Nexus mod id recovered from the filename, when known.
    pub nexus_mod_id: Option<i64>,
    /// When the mod was staged (unix seconds). `None` for rows that predate
    /// the provenance migration; sorts fall back to insertion (rowid) order.
    pub created_at: Option<i64>,
    /// Lowercase hex SHA-256 of the source archive, used to detect installing
    /// the same archive twice. `None` when staged from a directory.
    pub archive_sha256: Option<String>,
}
