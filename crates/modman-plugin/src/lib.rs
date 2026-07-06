// SPDX-License-Identifier: GPL-2.0-only
//! ModManager plugin host.
//!
//! Two tiers: declarative `game.toml` game definitions (the ~80% case, no code)
//! and `game.lua` scripts run under a locked-down `mlua` sandbox for games that
//! need custom install logic. Also hosts the FOMOD installer engine. Every
//! filesystem effect a plugin requests is returned as a plan for `modman-core`
//! to apply transactionally - plugins never write files directly.
//!
//! Implemented in Phase 3; this is the crate skeleton.
