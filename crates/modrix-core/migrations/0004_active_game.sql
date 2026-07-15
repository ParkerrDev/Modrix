-- SPDX-License-Identifier: GPL-2.0-only
-- Remember which game the user last worked on, so a frontend reopens on it
-- instead of falling back to whichever game happened to be registered first.
--
-- Mirrors `profiles.is_active`, but scoped globally rather than per-game:
-- exactly one game is active at a time. Existing rows default to 0 (nothing
-- active), which reads as "no preference yet" and leaves the old
-- first-registered fallback in place until the user picks a game.
ALTER TABLE games ADD COLUMN is_active INTEGER NOT NULL DEFAULT 0;

-- At most one active game (the partial-index trick used for profiles).
CREATE UNIQUE INDEX idx_games_one_active ON games (is_active) WHERE is_active = 1;
