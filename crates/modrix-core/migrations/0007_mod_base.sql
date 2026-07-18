-- SPDX-License-Identifier: GPL-2.0-only
-- Anchor for a game's mod_root: `install` (the game directory, the default and
-- the only pre-existing behavior), `documents`, `local_appdata`, or
-- `roaming_appdata` (the user's profile folder - some games, e.g. The Sims,
-- Dragon Age, Baldur's Gate 3, deploy mods there rather than under the install).
-- NULL for rows that predate this migration = `install`, so every existing game
-- keeps deploying byte-identically.
ALTER TABLE games ADD COLUMN mod_base TEXT;
