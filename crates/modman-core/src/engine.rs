// SPDX-License-Identifier: GPL-2.0-only
//! The [`Engine`]: the single action surface every frontend drives.
//!
//! Frontends (CLI, TUI, GUI) call only `Engine` and the report/plan types it
//! returns. They never touch SQLite or the filesystem directly. This keeps all
//! business logic in one place and all three faces honestly equivalent.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::deploy::plan::{ResolvedFile, plan};
use crate::deploy::{DeployPlan, DeployReport, VerifyReport, apply, journal, manifest, verify};
use crate::error::{Error, Result};
use crate::gamedef::GameDef;
use crate::id::{GameId, ModId, ProfileId};
use crate::model::{Game, Mod, Profile};
use crate::paths::Paths;
use crate::store;
use crate::{db, model};

/// The ModManager engine: an open database plus the resolved on-disk locations.
pub struct Engine {
    paths: Paths,
    conn: Connection,
}

impl Engine {
    /// Open the engine for the installation described by `paths`, creating the
    /// data directories and database (and applying migrations) if needed, and
    /// recovering any deploy that a crash left half-finished.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the data directories cannot be created, or
    /// [`Error::Database`]/[`Error::Journal`] if the database cannot be opened,
    /// migrated, or a pending deploy cannot be recovered.
    pub fn open(paths: &Paths) -> Result<Self> {
        paths.ensure_dirs()?;
        let conn = db::open(&paths.database_file())?;
        // Crash recovery must run before anything else touches game files.
        match journal::recover(&conn, paths)? {
            journal::Recovered::Nothing => {}
            other => tracing::warn!(?other, "recovered an interrupted deploy on open"),
        }
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
}

// --- games -----------------------------------------------------------------

impl Engine {
    /// Register a game install from a definition, creating a default profile.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on insert failure.
    pub fn add_game(&self, def: &GameDef, install_path: &Path, store_kind: &str) -> Result<Game> {
        let staging_root = self.paths.staging_root().join(&def.id);
        std::fs::create_dir_all(&staging_root).map_err(|e| Error::io(&staging_root, e))?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO games \
                 (plugin_id, name, install_path, mod_root, store, steam_appid, nexus_domain, staging_root) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                def.id,
                def.name,
                install_path.to_string_lossy(),
                def.mod_root,
                store_kind,
                def.steam_appid,
                def.nexus_domain,
                staging_root.to_string_lossy(),
            ],
        )?;
        let game = GameId::from_raw(tx.last_insert_rowid());
        tx.execute(
            "INSERT INTO profiles (game_id, name, is_active) VALUES (?1, 'default', 1)",
            [game.get()],
        )?;
        tx.commit()?;
        self.game(game)
    }

    /// All registered games.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on query failure.
    pub fn games(&self) -> Result<Vec<Game>> {
        let mut stmt = self.conn.prepare(GAME_COLUMNS)?;
        let rows = stmt.query_map([], game_from_row)?;
        collect(rows)
    }

    /// Find the game an `nxm://` link's game domain routes to, matching a
    /// game's `nexus_domain` (or, as a fallback, its `plugin_id`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no registered game matches `domain`.
    pub fn game_by_nexus_domain(&self, domain: &str) -> Result<Game> {
        self.games()?
            .into_iter()
            .find(|g| g.nexus_domain.as_deref() == Some(domain) || g.plugin_id == domain)
            .ok_or_else(|| Error::NotFound {
                kind: "game for nexus domain",
                key: domain.to_owned(),
            })
    }

    /// Look up one game by id.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no such game exists.
    pub fn game(&self, id: GameId) -> Result<Game> {
        let sql = format!("{GAME_COLUMNS} WHERE id = ?1");
        self.conn
            .query_row(&sql, [id.get()], game_from_row)
            .optional()?
            .ok_or_else(|| Error::NotFound {
                kind: "game",
                key: id.to_string(),
            })
    }
}

// --- profiles --------------------------------------------------------------

impl Engine {
    /// Create a new profile for a game.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on insert failure (e.g. a duplicate name).
    pub fn create_profile(&self, game: GameId, name: &str) -> Result<Profile> {
        self.conn.execute(
            "INSERT INTO profiles (game_id, name, is_active) VALUES (?1, ?2, 0)",
            rusqlite::params![game.get(), name],
        )?;
        self.profile(ProfileId::from_raw(self.conn.last_insert_rowid()))
    }

    /// All profiles for a game.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on query failure.
    pub fn profiles(&self, game: GameId) -> Result<Vec<Profile>> {
        let sql = format!("{PROFILE_COLUMNS} WHERE game_id = ?1 ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([game.get()], profile_from_row)?;
        collect(rows)
    }

    /// Look up one profile by id.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no such profile exists.
    pub fn profile(&self, id: ProfileId) -> Result<Profile> {
        let sql = format!("{PROFILE_COLUMNS} WHERE id = ?1");
        self.conn
            .query_row(&sql, [id.get()], profile_from_row)
            .optional()?
            .ok_or_else(|| Error::NotFound {
                kind: "profile",
                key: id.to_string(),
            })
    }

    /// The game's active profile.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if the game has no active profile.
    pub fn active_profile(&self, game: GameId) -> Result<Profile> {
        let sql = format!("{PROFILE_COLUMNS} WHERE game_id = ?1 AND is_active = 1");
        self.conn
            .query_row(&sql, [game.get()], profile_from_row)
            .optional()?
            .ok_or_else(|| Error::NotFound {
                kind: "active profile",
                key: game.to_string(),
            })
    }

    /// Make `profile` the game's active profile (exactly one is active).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on update failure.
    pub fn set_active_profile(&self, profile: ProfileId) -> Result<()> {
        let game = self.game_of_profile(profile)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE profiles SET is_active = 0 WHERE game_id = ?1",
            [game.get()],
        )?;
        tx.execute(
            "UPDATE profiles SET is_active = 1 WHERE id = ?1",
            [profile.get()],
        )?;
        tx.commit()?;
        Ok(())
    }
}

// --- mods ------------------------------------------------------------------

impl Engine {
    /// Stage a mod into the store from an extracted directory or a `.zip`
    /// archive, recording it against `game`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`]/[`Error::Archive`]/[`Error::BoundExceeded`] on a
    /// staging failure, or [`Error::Database`] on insert failure.
    pub fn stage(&self, game: GameId, name: &str, path: &Path) -> Result<Mod> {
        let staged = self.paths.staging_root().join(game.to_string()).join(name);
        if path.is_dir() {
            store::stage_extracted(path, &staged)?;
        } else if is_zip(path) {
            store::extract_zip(path, &staged)?;
        } else {
            return Err(Error::Archive {
                path: path.to_path_buf(),
                message: "unsupported source; expected a directory or a .zip".to_owned(),
            });
        }
        self.conn.execute(
            "INSERT INTO mods (game_id, name, source, staged_path) VALUES (?1, ?2, 'local', ?3)",
            rusqlite::params![game.get(), name, staged.to_string_lossy()],
        )?;
        self.get_mod(ModId::from_raw(self.conn.last_insert_rowid()))
    }

    /// All mods staged for a game.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on query failure.
    pub fn mods(&self, game: GameId) -> Result<Vec<Mod>> {
        let sql = format!("{MOD_COLUMNS} WHERE game_id = ?1 ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([game.get()], mod_from_row)?;
        collect(rows)
    }

    /// Look up one mod by id.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no such mod exists.
    pub fn get_mod(&self, id: ModId) -> Result<Mod> {
        let sql = format!("{MOD_COLUMNS} WHERE id = ?1");
        self.conn
            .query_row(&sql, [id.get()], mod_from_row)
            .optional()?
            .ok_or_else(|| Error::NotFound {
                kind: "mod",
                key: id.to_string(),
            })
    }

    /// Enable or disable a mod within a profile. Enabling a mod not yet in the
    /// profile appends it to the end of the load order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on failure.
    pub fn set_enabled(&self, profile: ProfileId, m: ModId, on: bool) -> Result<()> {
        let next = self.next_load_order(profile)?;
        self.conn.execute(
            "INSERT INTO profile_mods (profile_id, mod_id, enabled, load_order) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(profile_id, mod_id) DO UPDATE SET enabled = excluded.enabled",
            rusqlite::params![profile.get(), m.get(), i64::from(on), next],
        )?;
        Ok(())
    }

    /// Set the load order for a profile. Listed mods are ordered as given and
    /// enabled; any unlisted mods keep their relative order after them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on failure.
    pub fn set_load_order(&self, profile: ProfileId, order: &[ModId]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (index, m) in order.iter().enumerate() {
            let position = i64::try_from(index).unwrap_or(i64::MAX);
            tx.execute(
                "INSERT INTO profile_mods (profile_id, mod_id, enabled, load_order) \
                 VALUES (?1, ?2, 1, ?3) \
                 ON CONFLICT(profile_id, mod_id) DO UPDATE SET load_order = excluded.load_order",
                rusqlite::params![profile.get(), m.get(), position],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The profile's enabled mods, in load order - what a deploy would apply.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on query failure.
    pub fn enabled_mods(&self, profile: ProfileId) -> Result<Vec<Mod>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.game_id, m.name, m.version, m.source, m.staged_path \
             FROM profile_mods pm JOIN mods m ON m.id = pm.mod_id \
             WHERE pm.profile_id = ?1 AND pm.enabled = 1 \
             ORDER BY pm.load_order, m.id",
        )?;
        let rows = stmt.query_map([profile.get()], mod_from_row)?;
        collect(rows)
    }
}

// --- deploy ----------------------------------------------------------------

impl Engine {
    /// Compute (but do not apply) the deployment for a profile - a dry run.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if the profile/game is missing, or an I/O
    /// error while enumerating staged files.
    pub fn plan(&self, profile: ProfileId) -> Result<DeployPlan> {
        Ok(self.build_plan(profile)?.1)
    }

    /// Deploy a profile: make the game directory reflect its enabled mods in
    /// load order. Transactional, reversible, and crash-recoverable.
    ///
    /// # Errors
    ///
    /// Returns any planning or apply error; on failure the game directory is
    /// left recoverable via the journal.
    pub fn deploy(&self, profile: ProfileId) -> Result<DeployReport> {
        let (_, plan) = self.build_plan(profile)?;
        let report = apply::run(&self.conn, &self.paths, &plan, profile)?;
        self.set_active_profile(profile)?;
        Ok(report)
    }

    /// Remove everything the game's current deployment placed, restoring
    /// displaced originals - the reverse of [`Engine::deploy`].
    ///
    /// # Errors
    ///
    /// Returns any apply error; on failure the game directory is recoverable.
    pub fn undeploy(&self, profile: ProfileId) -> Result<DeployReport> {
        let game = self.game(self.game_of_profile(profile)?)?;
        let current = self.current_rows(game.id)?;
        let empty: Vec<(ModId, Vec<ResolvedFile>)> = Vec::new();
        let plan = plan(
            game.id,
            game.deploy_target_root(),
            self.paths.backup_root(),
            &empty,
            &current,
        );
        apply::run(&self.conn, &self.paths, &plan, profile)
    }

    /// Verify the game's current deployment against the manifest.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] or an I/O error while hashing files.
    pub fn verify(&self, profile: ProfileId) -> Result<VerifyReport> {
        let game = self.game(self.game_of_profile(profile)?)?;
        verify::verify(&self.conn, game.id, &game.deploy_target_root())
    }
}

// --- internals -------------------------------------------------------------

impl Engine {
    fn build_plan(&self, profile: ProfileId) -> Result<(Game, DeployPlan)> {
        let game = self.game(self.game_of_profile(profile)?)?;
        let ordered = self.enabled_mods_resolved(profile)?;
        let current = self.current_rows(game.id)?;
        let plan = plan(
            game.id,
            game.deploy_target_root(),
            self.paths.backup_root(),
            &ordered,
            &current,
        );
        Ok((game, plan))
    }

    fn current_rows(&self, game: GameId) -> Result<Vec<manifest::DeployedRow>> {
        manifest::current_deployment(&self.conn, game)
    }

    fn game_of_profile(&self, profile: ProfileId) -> Result<GameId> {
        self.conn
            .query_row(
                "SELECT game_id FROM profiles WHERE id = ?1",
                [profile.get()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(GameId::from_raw)
            .ok_or_else(|| Error::NotFound {
                kind: "profile",
                key: profile.to_string(),
            })
    }

    fn next_load_order(&self, profile: ProfileId) -> Result<i64> {
        let max: Option<i64> = self.conn.query_row(
            "SELECT max(load_order) FROM profile_mods WHERE profile_id = ?1",
            [profile.get()],
            |row| row.get(0),
        )?;
        Ok(max.map_or(0, |m| m.saturating_add(1)))
    }

    fn enabled_mods_resolved(&self, profile: ProfileId) -> Result<Vec<(ModId, Vec<ResolvedFile>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.staged_path FROM profile_mods pm \
             JOIN mods m ON m.id = pm.mod_id \
             WHERE pm.profile_id = ?1 AND pm.enabled = 1 \
             ORDER BY pm.load_order, m.id",
        )?;
        let rows = stmt.query_map([profile.get()], |row| {
            Ok((ModId::from_raw(row.get(0)?), row.get::<_, String>(1)?))
        })?;
        let mut ordered = Vec::new();
        for row in rows {
            let (mod_id, staged) = row?;
            ordered.push((mod_id, store::resolve_files(Path::new(&staged))?));
        }
        Ok(ordered)
    }
}

fn is_zip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

// --- row mapping -----------------------------------------------------------

const GAME_COLUMNS: &str = "SELECT id, plugin_id, name, install_path, mod_root, store, \
                            steam_appid, nexus_domain, staging_root FROM games";
const PROFILE_COLUMNS: &str = "SELECT id, game_id, name, is_active FROM profiles";
const MOD_COLUMNS: &str = "SELECT id, game_id, name, version, source, staged_path FROM mods";

fn game_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Game> {
    Ok(Game {
        id: GameId::from_raw(row.get(0)?),
        plugin_id: row.get(1)?,
        name: row.get(2)?,
        install_path: row.get::<_, String>(3)?.into(),
        mod_root: row.get(4)?,
        store: row.get(5)?,
        steam_appid: row.get(6)?,
        nexus_domain: row.get(7)?,
        staging_root: row.get::<_, String>(8)?.into(),
    })
}

fn profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Profile> {
    Ok(Profile {
        id: ProfileId::from_raw(row.get(0)?),
        game_id: GameId::from_raw(row.get(1)?),
        name: row.get(2)?,
        is_active: row.get::<_, i64>(3)? != 0,
    })
}

fn mod_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Mod> {
    Ok(model::Mod {
        id: ModId::from_raw(row.get(0)?),
        game_id: GameId::from_raw(row.get(1)?),
        name: row.get(2)?,
        version: row.get(3)?,
        source: row.get(4)?,
        staged_path: row.get::<_, String>(5)?.into(),
    })
}

/// Collect an iterator of `rusqlite` row results into a `Vec`, mapping the error.
fn collect<T>(rows: impl Iterator<Item = rusqlite::Result<T>>) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> (tempfile::TempDir, Engine) {
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine::open(&Paths::rooted_at(tmp.path())).unwrap();
        (tmp, engine)
    }

    fn sample_def() -> GameDef {
        GameDef::from_toml_str(
            "api_version = 1\nid = \"testgame\"\nname = \"Test\"\nmod_root = \"Data\"\n",
            Path::new("<test>"),
        )
        .unwrap()
    }

    #[test]
    fn add_game_creates_a_default_active_profile() {
        let (_tmp, engine) = engine();
        let install = engine.paths().data_dir().join("install");
        std::fs::create_dir_all(&install).unwrap();
        let game = engine.add_game(&sample_def(), &install, "manual").unwrap();

        assert_eq!(game.plugin_id, "testgame");
        let active = engine.active_profile(game.id).unwrap();
        assert_eq!(active.name, "default");
    }

    #[test]
    fn full_stage_enable_deploy_undeploy_cycle() {
        let (tmp, engine) = engine();
        let install = tmp.path().join("game");
        std::fs::create_dir_all(install.join("Data")).unwrap();
        std::fs::write(install.join("Data/original.esp"), b"ORIGINAL").unwrap();
        let pristine = std::fs::read(install.join("Data/original.esp")).unwrap();

        let game = engine.add_game(&sample_def(), &install, "manual").unwrap();
        let profile = engine.active_profile(game.id).unwrap();

        // A mod that overrides the original and adds a new file.
        let src = tmp.path().join("extracted");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("original.esp"), b"MODDED").unwrap();
        std::fs::write(src.join("new.esp"), b"NEW").unwrap();
        let m = engine.stage(game.id, "mymod", &src).unwrap();
        engine.set_enabled(profile.id, m.id, true).unwrap();

        let report = engine.deploy(profile.id).unwrap();
        assert_eq!(report.added(), 2);
        assert_eq!(
            std::fs::read(install.join("Data/original.esp")).unwrap(),
            b"MODDED"
        );
        assert!(engine.verify(profile.id).unwrap().is_clean());

        engine.undeploy(profile.id).unwrap();
        assert_eq!(
            std::fs::read(install.join("Data/original.esp")).unwrap(),
            pristine
        );
        assert!(!install.join("Data/new.esp").exists());
    }
}
