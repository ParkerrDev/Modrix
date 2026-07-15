// SPDX-License-Identifier: GPL-2.0-only
//! The crate-wide error type.
//!
//! Libraries use typed errors (`thiserror`); only binary edges reach for
//! `anyhow`. Every fallible boundary in the engine returns [`Result`].

use std::path::PathBuf;

/// Errors produced by the Modrix engine.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A filesystem operation failed; carries the path it was attempted on.
    #[error("i/o error at `{path}`: {source}")]
    Io {
        /// The path the failing operation targeted.
        path: PathBuf,
        /// The underlying OS error.
        source: std::io::Error,
    },

    /// The platform data/config/cache directories could not be determined.
    #[error("could not determine platform directories for this user")]
    NoProjectDirs,

    /// A database operation failed.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// A requested record does not exist.
    #[error("{kind} not found: {key}")]
    NotFound {
        /// The kind of record (e.g. `game`, `profile`, `mod`).
        kind: &'static str,
        /// The identifier that was looked up.
        key: String,
    },

    /// A bounded loop hit its ceiling - the input is too large or adversarial
    /// (Power of Ten: every loop has a visible, enforced bound).
    #[error("{what} exceeded its bound of {limit}")]
    BoundExceeded {
        /// What was being counted (e.g. `mod files`, `directory depth`).
        what: &'static str,
        /// The limit that was reached.
        limit: usize,
    },

    /// A staged file resolved to a path outside the game's deploy root - a mod
    /// tried to escape its target (e.g. via `..`). Never deployed.
    #[error("mod file path escapes the deploy root: {path}")]
    PathEscape {
        /// The offending resolved path.
        path: PathBuf,
    },

    /// The deployment journal could not be read, written, or parsed.
    #[error("deployment journal: {0}")]
    Journal(String),

    /// The on-disk manifest and reality disagree in a way the engine will not
    /// silently paper over.
    #[error("manifest inconsistency: {0}")]
    Manifest(String),

    /// A game definition file (`game.toml`) was invalid.
    #[error("invalid game definition `{path}`: {message}")]
    GameDef {
        /// The definition file at fault.
        path: PathBuf,
        /// What was wrong.
        message: String,
    },

    /// A mod archive could not be read or extracted.
    #[error("archive `{path}`: {message}")]
    Archive {
        /// The archive at fault.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },

    /// Deployment refused to run: unresolved conflicts, missing dependencies,
    /// or a conflict-rule cycle must be fixed first.
    #[error("deploy blocked: {0}")]
    DeployBlocked(String),
}

impl Error {
    /// Wrap a [`std::io::Error`] with the path it occurred on.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// The engine's result alias.
pub type Result<T> = std::result::Result<T, Error>;
