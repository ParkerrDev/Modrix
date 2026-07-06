// SPDX-License-Identifier: GPL-2.0-only
//! Nexus Mods integration.
//!
//! - [`NxmUri`]: the pure, network-free `nxm://` link parser (validated at the
//!   trust boundary).
//! - [`NexusClient`]: the authenticated API client that resolves a link to a CDN
//!   URL via `download_link.json`, tracking rate limits.
//! - The download engine (resumable, checksummed) and the `ModSource` trait,
//!   which keep a second site a new impl rather than a rewrite.
//!
//! The HTTP stack is pure-Rust and GPLv2-clean - hyper + rustls with the
//! RustCrypto crypto provider and the OS trust store, never reqwest/ring (see
//! `docs/ARCHITECTURE.md` §11).

mod client;
mod download;
mod error;
mod http;
mod nxm;

pub use client::{DownloadTarget, NexusClient, RateLimit};
pub use download::Progress;
pub use error::{Error, NxmError, Result};
pub use nxm::NxmUri;
