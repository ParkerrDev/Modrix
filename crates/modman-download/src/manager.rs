// SPDX-License-Identifier: GPL-2.0-only
//! The download manager: a small scheduler over the segmented [`task`] engine.
//!
//! `submit` returns immediately with a [`DownloadId`]; the transfer runs on a
//! spawned task that first acquires one of `max_concurrent` permits (the rest
//! queue). Live status is readable via `status`/`list` and lifecycle events are
//! broadcast so a GUI/TUI/bridge can subscribe instead of polling.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{Semaphore, broadcast};

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::task::{self, DownloadHandle};
use crate::types::{DownloadEvent, DownloadId, DownloadRequest, DownloadState, DownloadStatus};

/// Progress refresh / event cadence for an active download.
const TICK: Duration = Duration::from_millis(750);
/// Bound on the broadcast backlog before lagging receivers drop events.
const EVENT_CAPACITY: usize = 256;

/// A cheap-to-clone handle to the download scheduler.
#[derive(Clone)]
pub struct DownloadManager {
    inner: Arc<Inner>,
}

struct Inner {
    http: HttpClient,
    permits: Arc<Semaphore>,
    next_id: AtomicU64,
    entries: Mutex<HashMap<DownloadId, Arc<Entry>>>,
    events: broadcast::Sender<DownloadEvent>,
}

/// Per-download shared state: the live atomics plus the latest status snapshot.
struct Entry {
    handle: Arc<DownloadHandle>,
    status: Mutex<DownloadStatus>,
}

impl Entry {
    fn set_state(&self, state: DownloadState) {
        if let Ok(mut status) = self.status.lock() {
            status.state = state;
        }
    }

    /// Copy the live atomics into the status snapshot.
    fn refresh(&self) -> Option<DownloadStatus> {
        let mut status = self.status.lock().ok()?;
        status.done = self.handle.done.load(Ordering::Relaxed);
        let total = self.handle.total.load(Ordering::Relaxed);
        status.total = (total > 0).then_some(total);
        status.connections = self.handle.connections.load(Ordering::Relaxed);
        Some(status.clone())
    }

    fn snapshot(&self) -> Option<DownloadStatus> {
        self.refresh()
    }
}

impl DownloadManager {
    /// Build a manager allowing `max_concurrent` simultaneously-active downloads
    /// (the rest queue FIFO).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Tls`] if the shared HTTP client cannot be built.
    pub fn new(max_concurrent: u8) -> Result<Self> {
        let permits = usize::from(max_concurrent.max(1));
        Ok(Self {
            inner: Arc::new(Inner {
                http: HttpClient::new()?,
                permits: Arc::new(Semaphore::new(permits)),
                next_id: AtomicU64::new(1),
                entries: Mutex::new(HashMap::new()),
                events: broadcast::channel(EVENT_CAPACITY).0,
            }),
        })
    }

    /// Enqueue a request; the download runs in the background respecting the
    /// concurrency cap. Returns its id immediately.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Http`] if the internal state lock is poisoned.
    pub fn submit(&self, request: DownloadRequest) -> Result<DownloadId> {
        let id = DownloadId::from_raw(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let entry = Arc::new(Entry {
            handle: Arc::new(DownloadHandle::default()),
            status: Mutex::new(DownloadStatus {
                id,
                state: DownloadState::Queued,
                done: 0,
                total: request.expected_size,
                connections: 0,
                file: request.dest(),
            }),
        });
        lock(&self.inner.entries)?.insert(id, Arc::clone(&entry));
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            drive(inner, id, request, entry).await;
        });
        Ok(id)
    }

    /// The latest status snapshot for `id`, if it exists.
    #[must_use]
    pub fn status(&self, id: DownloadId) -> Option<DownloadStatus> {
        lock(&self.inner.entries).ok()?.get(&id)?.snapshot()
    }

    /// Snapshots of every known download.
    #[must_use]
    pub fn list(&self) -> Vec<DownloadStatus> {
        let Ok(entries) = lock(&self.inner.entries) else {
            return Vec::new();
        };
        entries.values().filter_map(|e| e.snapshot()).collect()
    }

    /// Request a graceful cancel. The partial file and its control file are left
    /// so a later resubmit resumes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`]-style [`Error::BadJob`] if the id is unknown.
    pub fn cancel(&self, id: DownloadId) -> Result<()> {
        let entries = lock(&self.inner.entries)?;
        let entry = entries
            .get(&id)
            .ok_or_else(|| Error::BadJob(format!("no download {id}")))?;
        entry.handle.cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Subscribe to lifecycle events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
        self.inner.events.subscribe()
    }
}

/// Acquire a permit, run the download, and publish its lifecycle.
async fn drive(inner: Arc<Inner>, id: DownloadId, request: DownloadRequest, entry: Arc<Entry>) {
    let _permit = inner.permits.acquire().await;
    entry.set_state(DownloadState::Active);
    let _ = inner.events.send(DownloadEvent::Started(id));

    let ticker = tokio::spawn(tick(Arc::clone(&entry), inner.events.clone()));
    let result = task::run(&inner.http, &request, &entry.handle).await;
    ticker.abort();

    match result {
        Ok(bytes) => {
            entry.set_state(DownloadState::Complete);
            let _ = inner.events.send(DownloadEvent::Complete {
                id,
                file: request.dest(),
                bytes,
            });
        }
        Err(error) => {
            entry.set_state(DownloadState::Failed);
            let _ = inner.events.send(DownloadEvent::Failed {
                id,
                error: error.to_string(),
            });
        }
    }
}

/// Emit periodic progress events while a download is active.
async fn tick(entry: Arc<Entry>, events: broadcast::Sender<DownloadEvent>) {
    loop {
        tokio::time::sleep(TICK).await;
        if let Some(status) = entry.snapshot() {
            let _ = events.send(DownloadEvent::Progress(status));
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| Error::Http("download state lock poisoned".to_owned()))
}
