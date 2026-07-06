// SPDX-License-Identifier: GPL-2.0-only
//! The public data types of the download manager.

use std::path::PathBuf;

/// Default piece size and connection tuning. Mirrors aria2's model but with sane
/// defaults (aria2's single-connection default is a well-known performance trap).
#[derive(Debug, Clone, Copy)]
pub struct SegmentLimits {
    /// Max concurrent connections to one host.
    pub connections: u8,
    /// Do not split a range whose length is below `2 * min_split`.
    pub min_split: u64,
    /// Fixed piece size (the resume + progress unit).
    pub piece_len: u32,
}

impl Default for SegmentLimits {
    fn default() -> Self {
        Self {
            connections: 16,
            min_split: 1024 * 1024,
            piece_len: 1024 * 1024,
        }
    }
}

/// An integrity check applied before a finished file is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checksum {
    /// Lowercase-hex MD5 (Nexus publishes these).
    Md5(String),
    /// Lowercase-hex SHA-256.
    Sha256(String),
}

/// An opaque, stable handle to a download (aria2's "gid" analog).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DownloadId(u64);

impl DownloadId {
    /// The raw handle value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Build a handle from a raw value (e.g. parsed from a status URL).
    #[must_use]
    pub const fn from_u64(raw: u64) -> Self {
        Self(raw)
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

impl std::fmt::Display for DownloadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The lifecycle state of a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// Waiting behind the concurrency cap.
    Queued,
    /// Actively transferring.
    Active,
    /// Paused by the user.
    Paused,
    /// Finished and verified.
    Complete,
    /// Terminally failed.
    Failed,
}

/// A point-in-time snapshot of a download (the `tellStatus` analog).
#[derive(Debug, Clone)]
pub struct DownloadStatus {
    /// The download's handle.
    pub id: DownloadId,
    /// Its lifecycle state.
    pub state: DownloadState,
    /// Bytes written so far.
    pub done: u64,
    /// Total bytes, when known.
    pub total: Option<u64>,
    /// Live connection count.
    pub connections: u8,
    /// The destination file (`.part` while active).
    pub file: PathBuf,
}

/// A lifecycle event broadcast to subscribers (GUI/TUI/bridge).
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// The download started transferring.
    Started(DownloadId),
    /// A progress snapshot.
    Progress(DownloadStatus),
    /// The download finished and was accepted.
    Complete {
        /// Which download.
        id: DownloadId,
        /// The final file path.
        file: PathBuf,
        /// Total bytes downloaded.
        bytes: u64,
    },
    /// The download failed.
    Failed {
        /// Which download.
        id: DownloadId,
        /// A human-readable reason.
        error: String,
    },
}

/// A generic download request, normalized from a browser hand-off or a CLI verb.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    /// The (already-signed, possibly single-use) URL to fetch.
    pub url: String,
    /// Raw request header lines to replay verbatim (User-Agent, Referer, Cookie…).
    pub headers: Vec<(String, String)>,
    /// Directory to download into (engine-chosen, never from the browser).
    pub dir: PathBuf,
    /// Sanitized output filename - a basename only.
    pub out: String,
    /// Advisory size hint from the browser.
    pub expected_size: Option<u64>,
    /// Optional integrity check applied before acceptance.
    pub checksum: Option<Checksum>,
    /// Segmentation tuning.
    pub limits: SegmentLimits,
}

impl DownloadRequest {
    /// The final destination path (`dir` joined with the sanitized `out`).
    #[must_use]
    pub fn dest(&self) -> PathBuf {
        self.dir.join(&self.out)
    }
}
