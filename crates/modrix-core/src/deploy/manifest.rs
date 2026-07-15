// SPDX-License-Identifier: GPL-2.0-only
//! The deployment manifest: reading and writing the `deployed_files` rows that
//! record exactly what the engine placed into a game directory.
//!
//! This table is how undeploy, verify, and crash recovery work - it is the
//! authoritative record of "what is ours, and what we displaced." All writes go
//! through [`replace_manifest`], which the applier calls inside one transaction
//! at its commit point.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::id::{GameId, ModId, ProfileId};
use crate::model::LinkType;

/// One deployed file: a row of the manifest and, equivalently, one entry the
/// journal replays. Serializable so the crash-recovery commit record can carry
/// the exact rows to finalize.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DeployedRow {
    /// The mod that owns this file at deploy time.
    pub mod_id: ModId,
    /// Target path relative to the game's deploy root, `/`-separated.
    pub target_rel: String,
    /// Absolute path to the source file in the staging store.
    pub source: PathBuf,
    /// How the file was placed.
    pub link_type: LinkType,
    /// SHA-256 of the source content at deploy time.
    pub source_hash: String,
    /// If this file displaced a pre-existing foreign original, the backup of
    /// that original in the content-addressed store (carried across replaces).
    pub backup_path: Option<PathBuf>,
}

/// Load the game's current deployment (every deployed file), or an empty vec if
/// nothing is deployed.
///
/// Only one profile is ever deployed per game at a time (the applier enforces
/// this); a second profile's rows appearing here is a manifest inconsistency and
/// is reported as such rather than silently merged.
pub(crate) fn current_deployment(conn: &Connection, game: GameId) -> Result<Vec<DeployedRow>> {
    let mut stmt = conn.prepare(
        "SELECT df.profile_id, df.mod_id, df.target_path, df.source_path, \
                df.link_type, df.source_hash, df.backup_path \
         FROM deployed_files df \
         JOIN profiles p ON p.id = df.profile_id \
         WHERE p.game_id = ?1 \
         ORDER BY df.target_path",
    )?;
    let mut profile: Option<ProfileId> = None;
    let mut rows = Vec::new();
    let mut cursor = stmt.query([game.get()])?;
    while let Some(row) = cursor.next()? {
        let profile_id = ProfileId::from_raw(row.get(0)?);
        match profile {
            Some(existing) if existing != profile_id => {
                return Err(Error::Manifest(
                    "more than one profile is deployed for a single game".into(),
                ));
            }
            _ => profile = Some(profile_id),
        }
        rows.push(row_from_sql(row)?);
    }
    Ok(rows)
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> Result<DeployedRow> {
    let link_token: String = row.get(4)?;
    let link_type = LinkType::from_token(&link_token)
        .ok_or_else(|| Error::Manifest(format!("unknown link type `{link_token}`")))?;
    let backup: Option<String> = row.get(6)?;
    Ok(DeployedRow {
        mod_id: ModId::from_raw(row.get(1)?),
        target_rel: row.get(2)?,
        source: PathBuf::from(row.get::<_, String>(3)?),
        link_type,
        source_hash: row.get(5)?,
        backup_path: backup.map(PathBuf::from),
    })
}

/// Atomically replace the game's manifest with `rows` for `profile`.
///
/// Deletes every deployed-file row for the game and inserts the new set. The
/// caller wraps this in the same transaction as its commit so the manifest
/// flips from the old deployment to the new one atomically.
pub(crate) fn replace_manifest(
    conn: &Connection,
    game: GameId,
    profile: ProfileId,
    rows: &[DeployedRow],
    deployed_at: &str,
) -> Result<()> {
    clear_manifest(conn, game)?;
    let mut insert = conn.prepare(
        "INSERT INTO deployed_files \
             (profile_id, mod_id, target_path, source_path, link_type, \
              source_hash, backup_path, deployed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for row in rows {
        insert.execute(rusqlite::params![
            profile.get(),
            row.mod_id.get(),
            row.target_rel,
            path_text(&row.source),
            row.link_type.as_str(),
            row.source_hash,
            row.backup_path.as_deref().map(path_text),
            deployed_at,
        ])?;
    }
    Ok(())
}

/// Delete every deployed-file row for a game (used by undeploy's commit).
pub(crate) fn clear_manifest(conn: &Connection, game: GameId) -> Result<()> {
    conn.execute(
        "DELETE FROM deployed_files WHERE profile_id IN \
             (SELECT id FROM profiles WHERE game_id = ?1)",
        [game.get()],
    )?;
    Ok(())
}

/// Render a path as text for storage. Paths are stored lossily as UTF-8; mod and
/// game paths are overwhelmingly ASCII/UTF-8 in practice.
fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
