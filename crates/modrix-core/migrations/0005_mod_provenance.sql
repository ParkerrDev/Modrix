-- SPDX-License-Identifier: GPL-2.0-only
-- Mod provenance: when a mod was staged and the content hash of its source
-- archive. The hash lets frontends detect "you already installed this exact
-- archive" and offer replace/cancel instead of silently staging a copy; the
-- timestamp gives a real most-recently-installed sort. Both are NULL for
-- rows that predate this migration (sorts fall back to rowid order) and the
-- hash is NULL when a mod was staged from a directory rather than an archive.
ALTER TABLE mods ADD COLUMN created_at INTEGER;
ALTER TABLE mods ADD COLUMN archive_sha256 TEXT;

CREATE INDEX idx_mods_archive_hash ON mods (game_id, archive_sha256)
    WHERE archive_sha256 IS NOT NULL;
