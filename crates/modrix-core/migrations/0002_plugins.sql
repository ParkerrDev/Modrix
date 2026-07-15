-- SPDX-License-Identifier: GPL-2.0-only
-- Per-profile plugin (.esp/.esm/.esl) load order and activation. Plugins are
-- discovered from enabled mods at read time; this table only persists the
-- user's ordering and enable choices, keyed by plugin filename.
CREATE TABLE profile_plugins (
    profile_id INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    plugin     TEXT    NOT NULL COLLATE NOCASE,
    position   INTEGER NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    PRIMARY KEY (profile_id, plugin)
) STRICT;

CREATE INDEX idx_profile_plugins_order ON profile_plugins (profile_id, position);
