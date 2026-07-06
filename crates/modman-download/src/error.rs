// SPDX-License-Identifier: GPL-2.0-only
//! Error types for the Nexus source.

use std::path::PathBuf;

/// The maximum length of an `nxm://` URI we will even look at, so a hostile
/// handler argument cannot make us allocate or scan unboundedly.
pub(crate) const MAX_NXM_URI_LEN: usize = 8192;

/// Failures parsing or validating an `nxm://` URI.
///
/// Every field of the URI is validated for semantics, not just shape, so junk is
/// rejected at the boundary before it can drive a network request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NxmError {
    /// The input did not start with the `nxm://` scheme.
    #[error("not an nxm:// URI")]
    NotNxm,

    /// The input exceeded [`MAX_NXM_URI_LEN`].
    #[error("nxm URI is too long")]
    TooLong,

    /// The game domain was missing or contained invalid characters.
    #[error("invalid game domain in nxm URI")]
    InvalidDomain,

    /// The path was not `/mods/<mod_id>/files/<file_id>`.
    #[error("malformed nxm path; expected /mods/<id>/files/<id>")]
    BadPath,

    /// A `mod_id` or `file_id` was not a positive integer.
    #[error("invalid numeric id in nxm URI")]
    BadId,

    /// The query string was malformed or a numeric parameter did not parse.
    #[error("malformed nxm query string")]
    BadQuery,
}

/// Failures talking to Nexus, downloading, or verifying a download.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An `nxm://` URI failed to parse.
    #[error("nxm uri: {0}")]
    Nxm(#[from] NxmError),

    /// The TLS layer could not be configured or negotiated.
    #[error("tls error: {0}")]
    Tls(String),

    /// An HTTP transport-level failure.
    #[error("http error: {0}")]
    Http(String),

    /// A signed/single-use URL has expired or is IP-bound to a different host
    /// (the browser must re-mint it and hand off again).
    #[error("the download URL has expired; re-click the download in the browser")]
    Expired,

    /// The deploy/resume control file was missing, malformed, or stale.
    #[error("download control file: {0}")]
    ControlFile(String),

    /// A bounded loop hit its ceiling - an input was too large or adversarial.
    #[error("{what} exceeded its bound of {limit}")]
    BoundExceeded {
        /// What was being counted.
        what: &'static str,
        /// The limit reached.
        limit: u64,
    },

    /// A hand-off job from the browser was malformed or failed validation.
    #[error("invalid download job: {0}")]
    BadJob(String),

    /// A downloaded file did not match its expected checksum.
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    Checksum {
        /// The expected digest.
        expected: String,
        /// The digest actually computed.
        actual: String,
    },

    /// A filesystem operation failed.
    #[error("i/o error at `{path}`: {source}")]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
}

impl Error {
    /// Wrap an I/O error with the path it happened on.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Result alias for the Nexus source.
pub type Result<T> = std::result::Result<T, Error>;
