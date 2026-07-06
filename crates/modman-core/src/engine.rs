// SPDX-License-Identifier: GPL-2.0-only
//! The [`Engine`]: the single action surface every frontend drives.
//!
//! Frontends (CLI, TUI, GUI) call only `Engine` and the report/plan types they
//! return. They never touch SQLite or the filesystem directly. This keeps all
//! business logic in one place and all three faces honestly equivalent.

use rusqlite::Connection;

use crate::db;
use crate::error::Result;
use crate::paths::Paths;

/// The ModManager engine: an open database plus the resolved on-disk locations.
///
/// Constructed with [`Engine::open`], which ensures the data directories exist
/// and brings the database schema up to date.
pub struct Engine {
    paths: Paths,
    conn: Connection,
}

impl Engine {
    /// Open the engine for the installation described by `paths`, creating the
    /// data directories and database (and applying any pending migrations) if
    /// they do not yet exist.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Io`] if the data directories cannot be created,
    /// or [`crate::Error::Database`] if the database cannot be opened or
    /// migrated.
    pub fn open(paths: &Paths) -> Result<Self> {
        paths.ensure_dirs()?;
        let conn = db::open(&paths.database_file())?;
        Ok(Self {
            paths: paths.clone(),
            conn,
        })
    }

    /// The resolved on-disk locations this engine uses.
    #[must_use]
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Borrow the open database connection (crate-internal; frontends never see
    /// SQLite).
    #[expect(dead_code, reason = "consumed by domain queries added in Phase 1")]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_data_dir_and_database() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| crate::Error::io("tempdir", e))?;
        let paths = Paths::rooted_at(tmp.path());
        let engine = Engine::open(&paths)?;

        assert!(paths.data_dir().is_dir(), "data dir should be created");
        assert!(
            paths.database_file().is_file(),
            "database file should be created"
        );
        assert!(engine.paths().staging_root().is_dir());
        Ok(())
    }

    #[test]
    fn open_is_repeatable() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| crate::Error::io("tempdir", e))?;
        let paths = Paths::rooted_at(tmp.path());
        // Opening twice against the same location must not fail or re-migrate.
        let _first = Engine::open(&paths)?;
        let _second = Engine::open(&paths)?;
        Ok(())
    }
}
