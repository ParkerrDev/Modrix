-- SPDX-License-Identifier: GPL-2.0-only
-- Persist the full game definition a game was registered with, so its
-- capabilities (load-order strategy, content dirs, external scans, health
-- checks) survive without re-reading definition files. NULL for rows that
-- predate this migration; the engine rehydrates those from the definition
-- catalog by plugin_id, falling back to a minimal definition synthesized
-- from the row's own columns.
ALTER TABLE games ADD COLUMN def_json TEXT;
