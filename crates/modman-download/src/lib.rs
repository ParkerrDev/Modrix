// SPDX-License-Identifier: GPL-2.0-only
//! The ModManager download engine.
//!
//! A generic, segmented, resumable download manager - a clean-room Rust
//! reimplementation of the aria2/Motrix download *engine* (no aria2 code or
//! binary), built on ModManager's own GPLv2-clean hyper + rustls client. It is
//! fed by the browser extension's hand-off (see [`HandoffJob`]) rather than any
//! site API: the browser holds the session and mints the signed download URL, we
//! just capture and fetch it.
//!
//! - [`DownloadManager`] schedules and runs downloads (segmented, multi-
//!   connection, resumable, checksum-verified).
//! - [`HandoffJob`] is the browser → engine contract; [`HandoffJob::into_request`]
//!   validates it at the trust boundary and yields a [`DownloadRequest`].
//! - [`NxmUri`] is retained purely as an identity parser (game/mod/file), not a
//!   download mechanism.
//!
//! The HTTP stack is pure-Rust and GPLv2-clean - hyper + rustls with the
//! RustCrypto crypto provider and the OS trust store, never reqwest/ring (see
//! `docs/ARCHITECTURE.md` §11). Held to the Power of Ten reliability standard:
//! panic-free on fallible paths, bounded loops, and offset writes via safe
//! seek-once file handles (no `unsafe`, no `nix`).

mod bits;
mod checksum;
mod control;
mod error;
mod http;
mod manager;
mod nxm;
mod request;
mod segment;
mod task;
mod types;

pub use error::{Error, NxmError, Result};
pub use manager::DownloadManager;
pub use nxm::NxmUri;
pub use request::{GameHint, HandoffJob};
pub use types::{
    Checksum, DownloadEvent, DownloadId, DownloadRequest, DownloadState, DownloadStatus,
    SegmentLimits,
};
