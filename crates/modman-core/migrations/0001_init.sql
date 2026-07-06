-- SPDX-License-Identifier: GPL-2.0-only
-- Migration 0001: initial schema.
--
-- SQLite (WAL mode) is both the relational index (games x profiles x mods) and
-- the deployment manifest (`deployed_files`). Game/plugin definitions and user
-- config stay as plain files on disk; only relational and transactional state
-- lives here. `id` columns are INTEGER PRIMARY KEY (rowid aliases).

-- A resolved game install: which plugin drives it, where it lives, and where
-- this game's mods are staged.
CREATE TABLE games (
    id           INTEGER PRIMARY KEY,
    plugin_id    TEXT    NOT NULL,
    name         TEXT    NOT NULL,
    install_path TEXT    NOT NULL,
    -- Where mods deploy, relative to install_path (from the game definition).
    mod_root     TEXT    NOT NULL DEFAULT '',
    store        TEXT    NOT NULL DEFAULT 'unknown',
    steam_appid  INTEGER,
    staging_root TEXT    NOT NULL
) STRICT;

-- A named, switchable set of enabled mods + load order for one game.
CREATE TABLE profiles (
    id        INTEGER PRIMARY KEY,
    game_id   INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    name      TEXT    NOT NULL,
    is_active INTEGER NOT NULL DEFAULT 0 CHECK (is_active IN (0, 1)),
    UNIQUE (game_id, name)
) STRICT;

-- Only one active profile per game.
CREATE UNIQUE INDEX idx_profiles_one_active
    ON profiles (game_id) WHERE is_active = 1;

-- A staged mod: an extracted archive in the central store plus its provenance.
CREATE TABLE mods (
    id            INTEGER PRIMARY KEY,
    game_id       INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    version       TEXT,
    source        TEXT    NOT NULL DEFAULT 'local',
    nexus_mod_id  INTEGER,
    nexus_file_id INTEGER,
    archive_path  TEXT,
    staged_path   TEXT    NOT NULL,
    install_state TEXT    NOT NULL DEFAULT 'staged'
) STRICT;

CREATE INDEX idx_mods_game ON mods (game_id);

-- Per-profile membership: whether a mod is enabled and where it sits in the
-- load order. The ordering lives here, not on the mod.
CREATE TABLE profile_mods (
    profile_id INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    mod_id     INTEGER NOT NULL REFERENCES mods(id)     ON DELETE CASCADE,
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    load_order INTEGER NOT NULL,
    PRIMARY KEY (profile_id, mod_id)
) STRICT;

CREATE INDEX idx_profile_mods_order ON profile_mods (profile_id, load_order);

-- The deployment manifest: every file we placed into the game directory, how we
-- placed it, its content hash, and any original file we displaced. This table
-- is how undeploy, verify, and crash recovery work - treat it as sacred.
CREATE TABLE deployed_files (
    id          INTEGER PRIMARY KEY,
    profile_id  INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    mod_id      INTEGER NOT NULL REFERENCES mods(id)     ON DELETE CASCADE,
    target_path TEXT    NOT NULL,
    source_path TEXT    NOT NULL,
    link_type   TEXT    NOT NULL CHECK (link_type IN ('hardlink', 'symlink', 'copy')),
    source_hash TEXT    NOT NULL,
    backup_path TEXT,
    deployed_at TEXT    NOT NULL
) STRICT;

CREATE UNIQUE INDEX idx_deployed_target ON deployed_files (profile_id, target_path);

-- The download queue: resumable, checksummed transfers routed to install.
CREATE TABLE downloads (
    id          INTEGER PRIMARY KEY,
    source      TEXT    NOT NULL,
    url         TEXT,
    nxm_uri     TEXT,
    state       TEXT    NOT NULL DEFAULT 'queued',
    bytes_total INTEGER,
    bytes_done  INTEGER NOT NULL DEFAULT 0
) STRICT;
