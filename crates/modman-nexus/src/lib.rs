// SPDX-License-Identifier: GPL-2.0-only
//! Nexus Mods integration.
//!
//! Implements a `ModSource` for Nexus: the authenticated API client, the
//! `nxm://` link resolver (every URI field validated), rate-limit handling
//! (`X-RL-*` headers, 429 backoff), and metadata caching. Kept behind the
//! `ModSource` trait so a second site is a new impl, not a rewrite.
//!
//! Implemented in Phase 2; this is the crate skeleton.
