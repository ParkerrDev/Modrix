-- SPDX-License-Identifier: GPL-2.0-only
-- Vortex-style conflict resolution configuration.
--
-- mod_rules: "winner loads after loser" - the winner's files override the
-- loser's wherever they collide. One row per ordered pair per profile.
CREATE TABLE mod_rules (
    profile_id INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    loser_mod  INTEGER NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    winner_mod INTEGER NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    PRIMARY KEY (profile_id, loser_mod, winner_mod),
    CHECK (loser_mod <> winner_mod)
) STRICT;

-- file_overrides: a per-file exception - for this one target the named mod
-- provides the file, regardless of mod rules or load order.
CREATE TABLE file_overrides (
    profile_id INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    target_rel TEXT    NOT NULL COLLATE NOCASE,
    mod_id     INTEGER NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    PRIMARY KEY (profile_id, target_rel)
) STRICT;
