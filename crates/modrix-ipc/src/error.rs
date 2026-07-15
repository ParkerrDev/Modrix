// SPDX-License-Identifier: GPL-2.0-only
//! IPC error type.

/// Failures binding, forwarding to, or serving the loopback IPC endpoint.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A socket or filesystem I/O error.
    #[error("ipc i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The OS random source failed while minting a session token.
    #[error("could not generate a session token: {0}")]
    Random(String),

    /// The instance lockfile could not be read/written or was malformed.
    #[error("instance lockfile: {0}")]
    Lockfile(String),

    /// A request or response exceeded a hard size/line bound.
    #[error("ipc message exceeded its {what} bound of {limit} bytes")]
    TooLarge {
        /// What was being bounded (`header`, `body`).
        what: &'static str,
        /// The byte ceiling.
        limit: usize,
    },

    /// A request or response was not well-formed HTTP/1.1 as we expect it.
    #[error("malformed ipc message: {0}")]
    Malformed(&'static str),

    /// The peer took too long; the bounded read timed out.
    #[error("ipc read timed out")]
    Timeout,
}

/// IPC result alias.
pub type Result<T> = std::result::Result<T, Error>;
