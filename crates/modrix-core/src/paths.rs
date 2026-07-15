// SPDX-License-Identifier: GPL-2.0-only
//! Platform-appropriate config/data/cache locations.
//!
//! All on-disk locations the engine uses are derived from a single [`Paths`]
//! value so tests can point the whole engine at a temp directory and production
//! can use the OS conventions (XDG on Linux, Known Folders on Windows,
//! Application Support on macOS) via the `directories` crate.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::{Error, Result};

/// Resolved on-disk locations for one Modrix installation.
///
/// Clone-cheap: it is three owned paths. The engine keeps a copy so every
/// subsystem derives its files from the same root.
#[derive(Debug, Clone)]
pub struct Paths {
    data: PathBuf,
    config: PathBuf,
    cache: PathBuf,
}

impl Paths {
    /// Resolve the standard per-user locations for this platform.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoProjectDirs`] if the platform directories cannot be
    /// determined (e.g. no `HOME` on Unix).
    pub fn resolve() -> Result<Self> {
        let dirs = ProjectDirs::from("", "Modrix", "Modrix").ok_or(Error::NoProjectDirs)?;
        Ok(Self {
            data: dirs.data_dir().to_path_buf(),
            config: dirs.config_dir().to_path_buf(),
            cache: dirs.cache_dir().to_path_buf(),
        })
    }

    /// Build a [`Paths`] rooted at a single directory.
    ///
    /// Used by tests to confine the entire engine to a temp directory, and
    /// available for a future `--data-dir` override.
    #[must_use]
    pub fn rooted_at(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            data: root.join("data"),
            config: root.join("config"),
            cache: root.join("cache"),
        }
    }

    /// The directory holding the database, the mod staging store, and backups.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// The directory holding user configuration and plugin definitions.
    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// The directory holding disposable cache data (e.g. Nexus metadata).
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    /// The SQLite database file (the relational index and deploy manifest).
    #[must_use]
    pub fn database_file(&self) -> PathBuf {
        self.data.join("modrix.db")
    }

    /// The root under which extracted mods are staged.
    #[must_use]
    pub fn staging_root(&self) -> PathBuf {
        self.data.join("mods")
    }

    /// The root under which displaced original game files are backed up.
    #[must_use]
    pub fn backup_root(&self) -> PathBuf {
        self.data.join("backups")
    }

    /// The deploy journal: pre-state written before any mutation. Its presence
    /// at startup means a deploy was interrupted and must be recovered.
    #[must_use]
    pub fn journal_file(&self) -> PathBuf {
        self.data.join("deploy.journal.json")
    }

    /// The deploy commit marker: written atomically once all disk mutations
    /// succeed. Its presence decides recovery direction (roll forward vs back).
    #[must_use]
    pub fn commit_file(&self) -> PathBuf {
        self.data.join("deploy.commit.json")
    }

    /// The single-instance lockfile holding the loopback port and session token.
    #[must_use]
    pub fn instance_lock(&self) -> PathBuf {
        self.data.join("instance.json")
    }

    /// Create every directory this installation needs, if absent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if any directory cannot be created.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.data,
            &self.config,
            &self.cache,
            &self.staging_root(),
            &self.backup_root(),
        ] {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.clone(), e))?;
        }
        Ok(())
    }
}
