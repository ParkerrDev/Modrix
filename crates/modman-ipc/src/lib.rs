// SPDX-License-Identifier: GPL-2.0-only
//! Single-instance guard and loopback IPC.
//!
//! The loopback HTTP listener *is* the single-instance mechanism: binding
//! `127.0.0.1:<port>` succeeds only for the primary; a second launch forwards
//! its request to the primary and exits. Loopback-only, per-session token.
//!
//! Implemented in Phase 2; this is the crate skeleton.
