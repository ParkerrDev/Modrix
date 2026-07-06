// SPDX-License-Identifier: GPL-2.0-only
//! `nxm://` protocol forwarder.
//!
//! The OS launches this tiny binary for `nxm://` links. It forwards the URL to
//! the running ModManager instance over the loopback IPC seam (starting a
//! headless engine if none is running), then exits.
//!
//! Implemented in Phase 2; this is the binary skeleton.

fn main() {}
