// SPDX-License-Identifier: GPL-2.0-only
//! SQLite connection setup and the forward-only migration runner.
//!
//! The database is opened in WAL mode with foreign keys enforced. Schema
//! evolution is a numbered, append-only list of migrations tracked by SQLite's
//! `user_version` pragma, applied in order on [`open`]. Migrations are bounded
//! (a fixed compile-time slice) and never run backwards.

use std::path::Path;

use rusqlite::Connection;

use crate::error::Result;

/// One schema migration: a target `user_version` and the SQL that reaches it.
struct Migration {
    /// The `user_version` the database has after this migration applies.
    version: i64,
    /// DDL executed as a single batch inside a transaction.
    sql: &'static str,
}

/// The ordered, append-only migration list. Never edit a shipped migration;
/// append a new one. Versions must be strictly increasing and contiguous.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("../migrations/0002_plugins.sql"),
    },
];

/// Open (creating if needed) the database at `path` and bring its schema up to
/// date.
///
/// # Errors
///
/// Returns [`crate::Error::Database`] if the connection cannot be opened,
/// pragmas cannot be set, or a migration fails to apply.
pub(crate) fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Apply the durable connection pragmas: WAL journaling and foreign-key
/// enforcement. Kept separate so tests and in-memory connections share it.
fn configure(conn: &Connection) -> Result<()> {
    // WAL gives us concurrent readers and crash-safe commits for the manifest.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Durability for the manifest without the full cost of FULL on every write.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

/// Run every migration whose target version exceeds the database's current
/// `user_version`, each in its own transaction, in order.
fn migrate(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for migration in MIGRATIONS {
        if migration.version <= current {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(migration.sql)?;
        // `user_version` cannot be bound as a parameter; the value is an
        // internal constant, never user input, so formatting it is safe.
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_version(conn: &Connection) -> Result<i64> {
        Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    #[test]
    fn migrations_are_contiguous_and_increasing() {
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            let expected = i64::try_from(index).ok().and_then(|i| i.checked_add(1));
            assert_eq!(
                Some(migration.version),
                expected,
                "migration list must be 1-based and contiguous"
            );
        }
    }

    #[test]
    fn open_in_memory_reaches_latest_version() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrate(&conn)?;
        let latest = MIGRATIONS.last().map_or(0, |m| m.version);
        assert_eq!(schema_version(&conn)?, latest);
        Ok(())
    }

    #[test]
    fn migrate_is_idempotent() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrate(&conn)?;
        // Running again applies nothing and leaves the version untouched.
        migrate(&conn)?;
        let latest = MIGRATIONS.last().map_or(0, |m| m.version);
        assert_eq!(schema_version(&conn)?, latest);
        Ok(())
    }

    #[test]
    fn expected_tables_exist() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrate(&conn)?;
        for table in [
            "games",
            "profiles",
            "mods",
            "profile_mods",
            "deployed_files",
            "downloads",
        ] {
            let count: i64 = conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )?;
            assert_eq!(count, 1, "table `{table}` should exist");
        }
        Ok(())
    }
}
