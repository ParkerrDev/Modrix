// SPDX-License-Identifier: GPL-2.0-only
//! The crate-wide error type.
//!
//! Libraries use typed errors (`thiserror`); only binary edges reach for
//! `anyhow`. Every fallible boundary in the engine returns [`Result`].

use std::path::PathBuf;

/// Errors produced by the ModManager engine.
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
