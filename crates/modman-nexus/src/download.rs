// SPDX-License-Identifier: GPL-2.0-only
//! Resumable, checksum-verified file downloads.
//!
//! Downloads stream to a `<dest>.part` file and are renamed into place only on
//! success, so a partial file is never mistaken for a complete one. If a
//! `.part` already exists the transfer resumes with an HTTP `Range` request; a
//! server that ignores the range (200 instead of 206) restarts cleanly. When an
//! expected MD5 is supplied (Nexus publishes them) the finished file is hashed
//! and compared before it is accepted.

use std::path::{Path, PathBuf};

use http_body_util::BodyExt as _;
use md5::{Digest, Md5};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{Error, Result};
use crate::http::{HttpClient, Response};

/// Progress update: bytes downloaded so far and the total when known.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    /// Bytes written so far.
    pub done: u64,
    /// Total expected bytes, if the server reported them.
    pub total: Option<u64>,
}

/// Download `url` to `dest`, resuming a prior `.part` if present, then verify
/// `expected_md5` if given. `on_progress` is called as bytes arrive.
///
/// # Errors
///
/// Returns [`Error::Http`]/[`Error::Api`] on transport failures, [`Error::Io`]
/// on write failures, or [`Error::Checksum`] if the finished file's MD5 differs.
pub(crate) async fn download_to<P>(
    http: &HttpClient,
    url: &str,
    dest: &Path,
    expected_md5: Option<&str>,
    mut on_progress: P,
) -> Result<u64>
where
    P: FnMut(Progress),
{
    let part = part_path(dest);
    let resumed = existing_len(&part).await;
    let range = (resumed > 0).then(|| ("range", format!("bytes={resumed}-")));
    let headers: Vec<(&str, String)> = range.into_iter().collect();

    let response = http.get(url, &headers).await?;
    let (mut file, mut done, total) = open_target(&part, resumed, &response).await?;

    let mut body = response.body;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| Error::Http(e.to_string()))?;
        if let Ok(chunk) = frame.into_data() {
            file.write_all(&chunk)
                .await
                .map_err(|e| Error::io(&part, e))?;
            done = done.saturating_add(chunk.len() as u64);
            on_progress(Progress { done, total });
        }
    }
    file.flush().await.map_err(|e| Error::io(&part, e))?;
    file.sync_all().await.map_err(|e| Error::io(&part, e))?;
    drop(file);

    if let Some(expected) = expected_md5 {
        verify_md5(&part, expected).await?;
    }
    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| Error::io(dest, e))?;
    Ok(done)
}

/// Open the `.part` for the chosen strategy, returning `(file, already_done,
/// total)`. `206` appends and resumes; `200` restarts from scratch.
async fn open_target(
    part: &Path,
    resumed: u64,
    response: &Response,
) -> Result<(tokio::fs::File, u64, Option<u64>)> {
    match response.status {
        206 => {
            let total = content_range_total(response)
                .or_else(|| content_length(response).and_then(|len| resumed.checked_add(len)));
            let file = tokio::fs::OpenOptions::new()
                .append(true)
                .open(part)
                .await
                .map_err(|e| Error::io(part, e))?;
            Ok((file, resumed, total))
        }
        200 => {
            let file = tokio::fs::File::create(part)
                .await
                .map_err(|e| Error::io(part, e))?;
            Ok((file, 0, content_length(response)))
        }
        other => Err(Error::Api(format!("download returned HTTP {other}"))),
    }
}

async fn existing_len(part: &Path) -> u64 {
    tokio::fs::metadata(part).await.map_or(0, |m| m.len())
}

fn content_length(response: &Response) -> Option<u64> {
    response
        .header("content-length")
        .and_then(|v| v.parse().ok())
}

/// Parse the total size out of a `Content-Range: bytes a-b/total` header.
fn content_range_total(response: &Response) -> Option<u64> {
    response
        .header("content-range")
        .and_then(|value| value.rsplit('/').next().map(str::trim))
        .and_then(|total| total.parse().ok())
}

/// Hash the finished file and compare against the expected lowercase-hex MD5.
async fn verify_md5(path: &Path, expected: &str) -> Result<()> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| Error::io(path, e))?;
    let mut hasher = Md5::new();
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await.map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(buf.get(..n).unwrap_or_default());
    }
    let actual = hex(&hasher.finalize());
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::Checksum {
            expected: expected.to_owned(),
            actual,
        })
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path as pathm};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client() -> HttpClient {
        HttpClient::new().unwrap()
    }

    // MD5 of "hello world" is 5eb63bbbe01eeed093cb22bb8f5acdc3.
    const PAYLOAD: &[u8] = b"hello world";
    const PAYLOAD_MD5: &str = "5eb63bbbe01eeed093cb22bb8f5acdc3";

    #[tokio::test]
    async fn full_download_verifies_md5_and_renames() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(pathm("/file.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(PAYLOAD))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        let url = format!("{}/file.bin", server.uri());

        let bytes = download_to(&client(), &url, &dest, Some(PAYLOAD_MD5), |_| {})
            .await
            .unwrap();
        assert_eq!(bytes, PAYLOAD.len() as u64);
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), PAYLOAD);
        assert!(
            !part_path(&dest).exists(),
            ".part must be gone after success"
        );
    }

    #[tokio::test]
    async fn bad_md5_is_rejected_and_not_renamed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(PAYLOAD))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f");
        let url = format!("{}/f", server.uri());
        let err = download_to(&client(), &url, &dest, Some("deadbeef"), |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Checksum { .. }));
        assert!(!dest.exists(), "a bad download must not be accepted");
    }

    #[tokio::test]
    async fn resumes_from_a_partial_part_file() {
        // Server serves the tail when a Range is requested.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 6-10/11")
                    .set_body_bytes(&PAYLOAD[6..]),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        // Pre-seed the first 6 bytes as an interrupted transfer.
        tokio::fs::write(part_path(&dest), &PAYLOAD[..6])
            .await
            .unwrap();
        let url = format!("{}/file.bin", server.uri());

        download_to(&client(), &url, &dest, Some(PAYLOAD_MD5), |_| {})
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), PAYLOAD);
    }
}
