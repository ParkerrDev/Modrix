// SPDX-License-Identifier: GPL-2.0-only
//! ModManager engine core.
//!
//! This crate is the correctness-critical heart of ModManager: the domain
//! model, the transactional deployment engine, the deployment manifest, and the
//! SQLite-backed storage index. It links no UI, no networking-UI, and no
//! site-specific code - frontends depend on it, never the reverse.
//!
//! It is held to the Power of Ten reliability standard (see
//! `docs/ARCHITECTURE.md` §9.3): `#![forbid(unsafe_code)]`, panic-free on
//! fallible paths, bounded loops, and validation at every trust boundary.

mod db;
mod engine;
mod error;
mod paths;

pub use engine::Engine;
pub use error::{Error, Result};
pub use paths::Paths;
