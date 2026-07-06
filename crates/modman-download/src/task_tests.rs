// SPDX-License-Identifier: GPL-2.0-only
//! Behavioral tests for the segmented download engine, against a real
//! Range-capable HTTP server (so the concurrent offset writes, resume, and
//! single-stream fallback are all exercised end to end).

use std::sync::Arc;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use super::{DownloadHandle, run};
use crate::http::HttpClient;
use crate::manager::DownloadManager;
use crate::types::{Checksum, DownloadEvent, DownloadRequest, SegmentLimits};

/// A deterministic payload of `n` bytes.
fn payload(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| u8::try_from(i & 0xff).unwrap_or(0))
        .collect()
}

/// Start a loopback HTTP server that serves `body` with (optional) `Range`
/// support. Returns its `http://127.0.0.1:<port>` base URL.
async fn serve(body: Vec<u8>, honor_range: bool) -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = Arc::new(body);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let body = Arc::clone(&body);
            tokio::spawn(async move {
                let _ = handle(stream, &body, honor_range).await;
            });
        }
    });
    format!("http://127.0.0.1:{port}/file.bin")
}

async fn handle(mut stream: TcpStream, body: &[u8], honor_range: bool) -> std::io::Result<()> {
    let request = read_request(&mut stream).await?;
    let total = body.len() as u64;
    let response = match (honor_range, parse_range(&request, total)) {
        (true, Some((start, end))) => partial(body, start, end, total),
        _ => full(body, total),
    };
    stream.write_all(&response).await?;
    stream.flush().await
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn parse_range(request: &str, total: u64) -> Option<(u64, u64)> {
    let line = request
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("range:"))?;
    let spec = line.split_once('=')?.1.trim();
    let (s, e) = spec.split_once('-')?;
    let start: u64 = s.trim().parse().ok()?;
    let last = total.saturating_sub(1);
    let end = if e.trim().is_empty() {
        last
    } else {
        e.trim().parse::<u64>().ok()?.min(last)
    };
    Some((start, end))
}

fn partial(body: &[u8], start: u64, end: u64, total: u64) -> Vec<u8> {
    let s = usize::try_from(start).unwrap_or(0);
    let e = usize::try_from(end).unwrap_or(0);
    let slice = body.get(s..=e).unwrap_or_default();
    let header = format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\n\
         Content-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n",
        slice.len()
    );
    let mut out = header.into_bytes();
    out.extend_from_slice(slice);
    out
}

fn full(body: &[u8], total: u64) -> Vec<u8> {
    let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n");
    let mut out = header.into_bytes();
    out.extend_from_slice(body);
    out
}

fn request(
    url: &str,
    dir: &std::path::Path,
    out: &str,
    checksum: Option<Checksum>,
) -> DownloadRequest {
    DownloadRequest {
        url: url.to_owned(),
        headers: Vec::new(),
        dir: dir.to_path_buf(),
        out: out.to_owned(),
        expected_size: None,
        checksum,
        // Small pieces + a few connections so several segments really run.
        limits: SegmentLimits {
            connections: 4,
            min_split: 4096,
            piece_len: 8192,
        },
    }
}

// Computed in the test so we compare against the real digest, not a constant.
fn md5_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(bytes);
    let mut out = String::new();
    for b in h.finalize() {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[tokio::test]
async fn full_segmented_download_reassembles_exactly() {
    let data = payload(200_000); // ~25 pieces at 8 KiB
    let url = serve(data.clone(), true).await;
    let dir = tempfile::tempdir().unwrap();
    let handle = Arc::new(DownloadHandle::default());
    let bytes = run(
        &HttpClient::new().unwrap(),
        &request(&url, dir.path(), "file.bin", None),
        &handle,
    )
    .await
    .unwrap();

    assert_eq!(bytes, data.len() as u64);
    assert_eq!(
        tokio::fs::read(dir.path().join("file.bin")).await.unwrap(),
        data
    );
    assert!(!dir.path().join("file.bin.part").exists());
    assert!(!dir.path().join("file.bin.mmdl").exists());
}

#[tokio::test]
async fn checksum_is_verified_and_bad_ones_rejected() {
    let data = payload(100_000);
    let url = serve(data.clone(), true).await;
    let dir = tempfile::tempdir().unwrap();
    let handle = Arc::new(DownloadHandle::default());

    // Correct MD5 accepts.
    run(
        &HttpClient::new().unwrap(),
        &request(
            &url,
            dir.path(),
            "ok.bin",
            Some(Checksum::Md5(md5_hex(&data))),
        ),
        &handle,
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::fs::read(dir.path().join("ok.bin")).await.unwrap(),
        data
    );

    // Wrong MD5 fails, and the file is not accepted.
    let err = run(
        &HttpClient::new().unwrap(),
        &request(
            &url,
            dir.path(),
            "bad.bin",
            Some(Checksum::Md5("deadbeef".into())),
        ),
        &Arc::new(DownloadHandle::default()),
    )
    .await;
    assert!(err.is_err());
    assert!(!dir.path().join("bad.bin").exists());
}

#[tokio::test]
async fn resumes_from_a_partial_part_and_control_file() {
    let data = payload(160_000);
    let url = serve(data.clone(), true).await;
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("r.bin.part");
    let control = dir.path().join("r.bin.mmdl");

    // Seed the first half as an interrupted transfer: a full-length .part with
    // only the leading pieces written, and a control file marking them.
    let piece_len = 8192_u32;
    let mut seed = data.clone();
    for byte in seed.iter_mut().skip(80_000) {
        *byte = 0; // second half not yet downloaded
    }
    tokio::fs::write(&part, &seed).await.unwrap();
    let mut ctl =
        crate::control::Control::fresh(piece_len, data.len() as u64, "\"v1\"".into(), false)
            .unwrap();
    let done_pieces = 80_000_usize.div_euclid(8192); // whole pieces fully present
    for slot in ctl.done.iter_mut().take(done_pieces) {
        *slot = true;
    }
    ctl.save(&control).await.unwrap();

    let mut req = request(&url, dir.path(), "r.bin", None);
    req.limits.piece_len = piece_len;
    run(
        &HttpClient::new().unwrap(),
        &req,
        &Arc::new(DownloadHandle::default()),
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::fs::read(dir.path().join("r.bin")).await.unwrap(),
        data
    );
}

#[tokio::test]
async fn falls_back_to_single_stream_when_range_ignored() {
    let data = payload(50_000);
    let url = serve(data.clone(), false).await; // server ignores Range → 200
    let dir = tempfile::tempdir().unwrap();
    run(
        &HttpClient::new().unwrap(),
        &request(&url, dir.path(), "s.bin", None),
        &Arc::new(DownloadHandle::default()),
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::fs::read(dir.path().join("s.bin")).await.unwrap(),
        data
    );
}

#[tokio::test]
async fn manager_submits_and_completes() {
    let data = payload(120_000);
    let url = serve(data.clone(), true).await;
    let dir = tempfile::tempdir().unwrap();
    let manager = DownloadManager::new(2).unwrap();
    let mut events = manager.subscribe();

    let id = manager
        .submit(request(&url, dir.path(), "m.bin", None))
        .unwrap();

    // Wait for this download's Complete event.
    let completed = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            if let Ok(DownloadEvent::Complete { id: cid, .. }) = events.recv().await
                && cid == id
            {
                return;
            }
        }
    })
    .await;
    assert!(completed.is_ok(), "download did not complete in time");
    assert_eq!(
        tokio::fs::read(dir.path().join("m.bin")).await.unwrap(),
        data
    );
}
