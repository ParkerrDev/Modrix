// SPDX-License-Identifier: GPL-2.0-only
//! Errors from building the interop model.

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Why an interop model could not be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The game declares more of something than this build will walk. IL2CPP
    /// metadata is read from a file the user supplies, so every count that
    /// drives an allocation is bounded rather than trusted.
    #[error("{what} exceeded the bound of {limit}")]
    TooLarge {
        /// What was being counted.
        what: &'static str,
        /// The ceiling.
        limit: usize,
    },

    /// The underlying IL2CPP metadata or binary could not be read.
    #[error("reading IL2CPP metadata failed: {0}")]
    Il2Cpp(#[from] modrix_il2cpp::Error),

    /// The ECMA-335 writer rejected the model.
    #[error("emitting the reference assembly failed: {0}")]
    Ecma(#[from] modrix_ecma335::Error),

    /// An assembly index does not name an assembly of this game.
    #[error("no assembly at index {index}")]
    NoSuchAssembly {
        /// The offending index.
        index: usize,
    },
}
