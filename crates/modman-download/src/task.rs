// SPDX-License-Identifier: GPL-2.0-only
//! One download, driven to completion.
//!
//! Probe → (segmented ‖ single-stream) → verify → atomic rename. In segmented
//! mode the file is preallocated and up to N connections each fetch a `Range` of
//! contiguous pieces, writing to disjoint offsets on their own file handle (no
//! shared cursor, no locking on the hot path, no `unsafe`). Piece completion is
//! persisted to a `.mmdl` control file for byte-... well, piece-precise resume.
//! A finished file is checksum-verified (when a hash is known) and only then
//! renamed into place - a partial or bad file is never accepted.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use http_body_util::BodyExt as _;
use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};

use crate::checksum;
use crate::control::Control;
use crate::error::{Error, Result};
use crate::http::{HttpClient, Response};
use crate::segment::SegmentMap;
use crate::types::{Checksum, DownloadRequest};

/// Total per-job segment failures tolerated before the download is abandoned.
const MAX_FAILURES: u32 = 30;
/// How often the control file is persisted and progress refreshed.
const SAVE_INTERVAL: Duration = Duration::from_secs(5);

/// Live, atomically-updated progress for one running download (read by the
/// manager to answer `status`, written by the task).
#[derive(Debug, Default)]
pub(crate) struct DownloadHandle {
    /// Bytes on disk.
    pub done: AtomicU64,
    /// Total bytes (0 = unknown).
    pub total: AtomicU64,
    /// Live connection count.
    pub connections: AtomicU8,
    /// Set to request a graceful stop.
    pub cancel: AtomicBool,
}

/// The result of the cheap range probe.
struct Probe {
    total: Option<u64>,
    ranges: bool,
    validator: String,
}

/// Download `req`, updating `handle`, returning the byte count on success.
pub(crate) async fn run(
    http: &HttpClient,
    req: &DownloadRequest,
    handle: &Arc<DownloadHandle>,
) -> Result<u64> {
    let dest = req.dest();
    let part = part_path(&dest);
    let control = control_path(&dest);
    tokio::fs::create_dir_all(&req.dir)
        .await
        .map_err(|e| Error::io(&req.dir, e))?;

    let probe = probe(http, &req.url, &req.headers).await?;
    match (probe.ranges, probe.total) {
        (true, Some(total)) if total > 0 => {
            segmented(
                http,
                req,
                total,
                probe.validator,
                &part,
                &dest,
                &control,
                handle,
            )
            .await
        }
        _ => single_stream(http, req, &part, &dest, handle).await,
    }
}

/// A cheap `Range: bytes=0-0` probe: 206 ⇒ ranged (total from `Content-Range`),
/// 200 ⇒ single-stream (total from `Content-Length`).
async fn probe(http: &HttpClient, url: &str, base: &[(String, String)]) -> Result<Probe> {
    let mut headers = borrow(base);
    headers.push(("range", "bytes=0-0".to_owned()));
    let response = http.get(url, &headers).await?;
    let validator = response
        .header("etag")
        .or_else(|| response.header("last-modified"))
        .unwrap_or("")
        .to_owned();
    match response.status {
        206 => Ok(Probe {
            total: content_range_total(&response),
            ranges: true,
            validator,
        }),
        200 => Ok(Probe {
            total: content_length(&response),
            ranges: false,
            validator,
        }),
        403 | 410 => Err(Error::Expired),
        other => Err(Error::Http(format!("probe returned HTTP {other}"))),
    }
    // response body is dropped unread - the connection closes.
}

/// Shared state for the segment workers of one download.
struct Shared {
    http: HttpClient,
    url: String,
    base_headers: Vec<(String, String)>,
    part: PathBuf,
    piece_len: u64,
    total: u64,
    map: Mutex<SegmentMap>,
    failures: AtomicU32,
    handle: Arc<DownloadHandle>,
}

impl Shared {
    fn lock_map(&self) -> Result<std::sync::MutexGuard<'_, SegmentMap>> {
        self.map
            .lock()
            .map_err(|_| Error::Http("segment map poisoned".to_owned()))
    }
}

/// Multi-connection segmented download with resume.
#[expect(
    clippy::too_many_arguments,
    reason = "one cohesive download; grouping into a struct adds no clarity"
)]
async fn segmented(
    http: &HttpClient,
    req: &DownloadRequest,
    total: u64,
    validator: String,
    part: &Path,
    dest: &Path,
    control_path: &Path,
    handle: &Arc<DownloadHandle>,
) -> Result<u64> {
    let piece_len = req.limits.piece_len;
    let piece = u64::from(piece_len.max(1));
    // Resume from a matching control file, else start fresh.
    let control = match Control::load(control_path).await? {
        Some(c) if c.matches(piece_len, total, &validator) => c,
        _ => Control::fresh(piece_len, total, validator.clone(), false)?,
    };
    preallocate(part, total).await?;
    handle.total.store(total, Ordering::Relaxed);
    handle.done.store(control.bytes_done(), Ordering::Relaxed);
    // A resumed download that is already fully present just needs finalizing.
    if control.is_complete() {
        return finalize(
            part,
            dest,
            control_path,
            req.checksum.as_ref(),
            total,
            handle,
        )
        .await;
    }
    let done = control.done;

    let shared = Arc::new(Shared {
        http: http.clone(),
        url: req.url.clone(),
        base_headers: req.headers.clone(),
        part: part.to_path_buf(),
        piece_len: piece,
        total,
        map: Mutex::new(SegmentMap::new(done, req.limits.connections)),
        failures: AtomicU32::new(0),
        handle: Arc::clone(handle),
    });

    let saver = tokio::spawn(save_loop(
        Arc::clone(&shared),
        control_path.to_path_buf(),
        validator.clone(),
    ));

    let first_error = spawn_and_join(&shared, req.limits.connections).await;
    saver.abort();

    let complete = shared.lock_map()?.is_complete();
    if !complete {
        // Persist progress for a later resume, then report why we stopped.
        let snapshot = shared.lock_map()?.snapshot();
        persist(control_path, piece_len, total, &validator, snapshot).await?;
        if shared.handle.cancel.load(Ordering::Relaxed) {
            return Err(Error::Http("download cancelled".to_owned()));
        }
        return Err(first_error.unwrap_or_else(|| Error::Http("download incomplete".to_owned())));
    }

    finalize(
        part,
        dest,
        control_path,
        req.checksum.as_ref(),
        total,
        handle,
    )
    .await
}

/// Verify (when a hash is known), drop the control file, and atomically rename
/// `<dest>.part` into place - the only way a completed download is accepted.
#[expect(
    clippy::too_many_arguments,
    reason = "the finalize step's inputs are all distinct and cohesive"
)]
async fn finalize(
    part: &Path,
    dest: &Path,
    control_path: &Path,
    checksum: Option<&Checksum>,
    total: u64,
    handle: &Arc<DownloadHandle>,
) -> Result<u64> {
    if let Some(check) = checksum {
        checksum::verify(part, check).await?;
    }
    Control::remove(control_path).await?;
    tokio::fs::rename(part, dest)
        .await
        .map_err(|e| Error::io(dest, e))?;
    handle.done.store(total, Ordering::Relaxed);
    Ok(total)
}

/// Spawn `connections` workers, await them all, and return the first error.
async fn spawn_and_join(shared: &Arc<Shared>, connections: u8) -> Option<Error> {
    let mut workers = Vec::new();
    for _ in 0..u32::from(connections.max(1)) {
        let worker_shared = Arc::clone(shared);
        workers.push(tokio::spawn(async move { worker(worker_shared).await }));
    }
    let mut first_error = None;
    for worker in workers {
        match worker.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                first_error.get_or_insert(e);
            }
            Err(_join) => {
                first_error.get_or_insert(Error::Http("worker panicked".to_owned()));
            }
        }
    }
    first_error
}

/// Pull runs and fetch them until the file is complete or the job fails.
async fn worker(shared: Arc<Shared>) -> Result<()> {
    loop {
        if shared.handle.cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let run = shared.lock_map()?.next_run();
        let Some((start, end)) = run else {
            return Ok(()); // no more work
        };
        shared.handle.connections.fetch_add(1, Ordering::Relaxed);
        let result = download_run(&shared, start, end).await;
        shared.handle.connections.fetch_sub(1, Ordering::Relaxed);
        if let Err(error) = result {
            shared.lock_map()?.release(start, end);
            let fails = shared
                .failures
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1);
            if matches!(error, Error::Expired) || fails > MAX_FAILURES {
                shared.handle.cancel.store(true, Ordering::Relaxed);
                return Err(error);
            }
            tokio::time::sleep(backoff(fails)).await;
        }
    }
}

/// Fetch pieces `[start, end)` with one `Range` request, marking each piece done
/// as its bytes fully land.
async fn download_run(shared: &Shared, start: usize, end: usize) -> Result<()> {
    let byte_start = as_u64(start).saturating_mul(shared.piece_len);
    let byte_end_excl = as_u64(end)
        .saturating_mul(shared.piece_len)
        .min(shared.total);
    if byte_start >= byte_end_excl {
        return Ok(());
    }
    let mut file = open_at(&shared.part, byte_start).await?;
    let mut headers = borrow(&shared.base_headers);
    headers.push((
        "range",
        format!("bytes={byte_start}-{}", byte_end_excl.saturating_sub(1)),
    ));
    let response = shared.http.get(&shared.url, &headers).await?;
    match response.status {
        206 => {}
        403 | 410 => return Err(Error::Expired),
        other => return Err(Error::Http(format!("segment returned HTTP {other}"))),
    }

    let mut cursor = byte_start;
    let mut next_piece = start;
    let mut body = response.body;
    while let Some(frame) = body.frame().await {
        if shared.handle.cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let frame = frame.map_err(|e| Error::Http(e.to_string()))?;
        if let Ok(chunk) = frame.into_data() {
            file.write_all(&chunk)
                .await
                .map_err(|e| Error::io(&shared.part, e))?;
            cursor = cursor.saturating_add(as_u64(chunk.len()));
            mark_pieces(shared, &mut next_piece, end, cursor)?;
        }
    }
    file.flush().await.map_err(|e| Error::io(&shared.part, e))?;
    file.sync_all()
        .await
        .map_err(|e| Error::io(&shared.part, e))?;
    mark_pieces(shared, &mut next_piece, end, cursor)?;
    Ok(())
}

/// Mark every piece in `[*next, end)` whose end offset the cursor has reached.
fn mark_pieces(shared: &Shared, next: &mut usize, end: usize, cursor: u64) -> Result<()> {
    let mut map = shared.lock_map()?;
    while *next < end {
        let piece_end = as_u64(*next)
            .saturating_add(1)
            .saturating_mul(shared.piece_len)
            .min(shared.total);
        if cursor >= piece_end {
            map.mark_done(*next);
            *next = next.saturating_add(1);
        } else {
            break;
        }
    }
    Ok(())
}

/// Periodically refresh progress and persist the control file, until aborted.
async fn save_loop(shared: Arc<Shared>, control_path: PathBuf, validator: String) {
    loop {
        tokio::time::sleep(SAVE_INTERVAL).await;
        let snapshot = match shared.lock_map() {
            Ok(map) => {
                let done = as_u64(map.done_count())
                    .saturating_mul(shared.piece_len)
                    .min(shared.total);
                shared.handle.done.store(done, Ordering::Relaxed);
                map.snapshot()
            }
            Err(_) => return,
        };
        // Persist for resume; ignore transient write errors on this heartbeat.
        let piece_len = u32::try_from(shared.piece_len).unwrap_or(u32::MAX);
        let _ = persist(&control_path, piece_len, shared.total, &validator, snapshot)
            .await
            .map_err(|e| tracing::debug!(%e, "control save skipped"));
    }
}

/// Write a control snapshot to disk.
async fn persist(
    control_path: &Path,
    piece_len: u32,
    total: u64,
    validator: &str,
    done: Vec<bool>,
) -> Result<()> {
    let control = Control {
        piece_len,
        total,
        validator: validator.to_owned(),
        single_stream: false,
        done,
    };
    control.save(control_path).await
}

/// Single-connection streaming download (server ignored `Range`). No resume.
async fn single_stream(
    http: &HttpClient,
    req: &DownloadRequest,
    part: &Path,
    dest: &Path,
    handle: &Arc<DownloadHandle>,
) -> Result<u64> {
    let headers = borrow(&req.headers);
    let response = http.get(&req.url, &headers).await?;
    if response.status != 200 {
        return Err(Error::Http(format!(
            "download returned HTTP {}",
            response.status
        )));
    }
    if let Some(total) = content_length(&response) {
        handle.total.store(total, Ordering::Relaxed);
    }
    let mut file = tokio::fs::File::create(part)
        .await
        .map_err(|e| Error::io(part, e))?;
    let mut done = 0_u64;
    let mut body = response.body;
    while let Some(frame) = body.frame().await {
        if handle.cancel.load(Ordering::Relaxed) {
            return Err(Error::Http("download cancelled".to_owned()));
        }
        let frame = frame.map_err(|e| Error::Http(e.to_string()))?;
        if let Ok(chunk) = frame.into_data() {
            file.write_all(&chunk)
                .await
                .map_err(|e| Error::io(part, e))?;
            done = done.saturating_add(as_u64(chunk.len()));
            handle.done.store(done, Ordering::Relaxed);
        }
    }
    file.flush().await.map_err(|e| Error::io(part, e))?;
    file.sync_all().await.map_err(|e| Error::io(part, e))?;
    if let Some(check) = &req.checksum {
        checksum::verify(part, check).await?;
    }
    tokio::fs::rename(part, dest)
        .await
        .map_err(|e| Error::io(dest, e))?;
    Ok(done)
}

// --- small helpers ---------------------------------------------------------

async fn preallocate(part: &Path, total: u64) -> Result<()> {
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(part)
        .await
        .map_err(|e| Error::io(part, e))?;
    file.set_len(total).await.map_err(|e| Error::io(part, e))?;
    Ok(())
}

async fn open_at(part: &Path, offset: u64) -> Result<tokio::fs::File> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(part)
        .await
        .map_err(|e| Error::io(part, e))?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| Error::io(part, e))?;
    Ok(file)
}

fn borrow(headers: &[(String, String)]) -> Vec<(&str, String)> {
    headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect()
}

fn content_length(response: &Response) -> Option<u64> {
    response
        .header("content-length")
        .and_then(|v| v.parse().ok())
}

fn content_range_total(response: &Response) -> Option<u64> {
    response
        .header("content-range")
        .and_then(|v| v.rsplit('/').next().map(str::trim))
        .and_then(|total| total.parse().ok())
}

fn part_path(dest: &Path) -> PathBuf {
    with_suffix(dest, ".part")
}

fn control_path(dest: &Path) -> PathBuf {
    with_suffix(dest, ".mmdl")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn as_u64(x: usize) -> u64 {
    u64::try_from(x).unwrap_or(u64::MAX)
}

fn backoff(attempt: u32) -> Duration {
    // 200ms, 400, 800, … capped at 5s.
    let ms = 200_u64.saturating_mul(2_u64.saturating_pow(attempt.min(5)));
    Duration::from_millis(ms.min(5_000))
}

#[cfg(test)]
#[path = "task_tests.rs"]
mod task_tests;
