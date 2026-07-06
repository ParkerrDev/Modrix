// SPDX-License-Identifier: GPL-2.0-only
//! Error types for the Nexus source.

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
