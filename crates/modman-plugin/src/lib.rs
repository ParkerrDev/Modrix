// SPDX-License-Identifier: GPL-2.0-only
//! ModManager plugin host.
//!
//! Two tiers: declarative `game.toml` game definitions (the ~80% case, no code)
//! and - later - `game.lua` scripts under a locked-down sandbox for games that
//! need custom install logic. Hosts the [`fomod`] installer engine: parsing
//! `ModuleConfig.xml`, computing default selections, and materializing chosen
//! options into a staged tree.

pub mod fomod;

/// Plugin-host errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A FOMOD installer could not be parsed or applied.
    #[error("fomod {path}: {message}")]
    Fomod {
        /// The offending file or tree.
        path: std::path::PathBuf,
        /// What went wrong.
        message: String,
    },
}

/// Plugin-host result.
pub type Result<T> = std::result::Result<T, Error>;
