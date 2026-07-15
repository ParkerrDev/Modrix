// SPDX-License-Identifier: GPL-2.0-only
//! Modrix engine core.
//!
//! This crate is the correctness-critical heart of Modrix: the domain
//! model, the transactional deployment engine, the deployment manifest, and the
//! SQLite-backed storage index. It links no UI, no networking-UI, and no
//! site-specific code - frontends depend on it, never the reverse.
//!
//! It is held to the Power of Ten reliability standard (see
//! `docs/ARCHITECTURE.md` §9.3): `#![forbid(unsafe_code)]`, panic-free on
//! fallible paths, bounded loops, and validation at every trust boundary.

// `modrix-core` touches users' game files, so arithmetic is held to the
// strictest tier here: every `+`/`-`/`*` on a count or size must be a
// `checked_*`/`saturating_*` call, enforced at deny level (the workspace sets
// this to warn; core promotes it).
#![deny(clippy::arithmetic_side_effects)]

mod db;
mod deploy;
pub mod detect;
mod engine;
mod error;
pub mod esp;
pub mod external;
mod gamedef;
pub mod health;
mod id;
mod model;
pub mod naming;
mod paths;
pub mod plugins;
pub mod progress;
pub mod rules;
mod store;

pub use deploy::{Conflict, DeployPlan, DeployReport, FileStatus, VerifyIssue, VerifyReport};
pub use engine::Engine;
pub use error::{Error, Result};
pub use external::{ExternalKind, ExternalMod};
pub use gamedef::GameDef;
pub use health::{Issue, Severity};
pub use id::{GameId, ModId, ProfileId};
pub use model::{Game, LinkType, Mod, Profile};
pub use paths::Paths;
pub use progress::{Progress, ProgressSnapshot};
pub use rules::{ConflictFile, ModConflict, ModRule};
