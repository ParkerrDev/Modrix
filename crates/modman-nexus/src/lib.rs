// SPDX-License-Identifier: GPL-2.0-only
//! Nexus Mods integration.
//!
//! Implements the Nexus source: the `nxm://` link parser (this module tree),
//! and - added in the rest of Phase 2 - the authenticated API client, the
//! resumable download queue, and rate-limit handling, all behind a `ModSource`
//! abstraction so a second site is a new impl rather than a rewrite.
//!
//! The `nxm://` parser is pure and network-free; it validates every field of an
//! untrusted URI before anything reaches the network.

mod error;
mod nxm;

pub use error::NxmError;
pub use nxm::NxmUri;
