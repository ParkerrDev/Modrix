// SPDX-License-Identifier: GPL-2.0-only
//! The authenticated Nexus Mods API client.
//!
//! Resolves an `nxm://` link to a concrete CDN download URL via Nexus's
//! `download_link.json` endpoint, authenticating with the user's personal API
//! key and (for free accounts) the `key`/`expires` pair carried by the link.
//! Rate-limit headers (`X-RL-*`) are parsed on every response so callers can
//! back off before Nexus starts returning 429s.

use std::path::Path;

use crate::download::{self, Progress};
use crate::error::{Error, Result};
use crate::http::{self, HttpClient};
use crate::nxm::NxmUri;

/// The default Nexus API base URL.
const DEFAULT_BASE: &str = "https://api.nexusmods.com/";

/// A resolved, ready-to-download file.
#[derive(Debug, Clone)]
pub struct DownloadTarget {
    /// The CDN URL to fetch.
    pub url: String,
    /// A suggested on-disk file name derived from the URL.
    pub file_name: String,
}

/// A snapshot of Nexus's rate-limit headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimit {
    /// Requests remaining in the hourly window.
    pub hourly_remaining: Option<i64>,
    /// Requests remaining in the daily window.
    pub daily_remaining: Option<i64>,
    /// Seconds to wait when throttled (from `Retry-After`), if given.
    pub retry_after: Option<u64>,
}

impl RateLimit {
    fn from_response(response: &http::Response) -> Self {
        Self {
            hourly_remaining: response
                .header("x-rl-hourly-remaining")
                .and_then(|v| v.parse().ok()),
            daily_remaining: response
                .header("x-rl-daily-remaining")
                .and_then(|v| v.parse().ok()),
            retry_after: response.header("retry-after").and_then(|v| v.parse().ok()),
        }
    }

    /// Whether the caller should stop and wait before issuing more requests.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.hourly_remaining == Some(0) || self.daily_remaining == Some(0)
    }
}

/// One entry of a `download_link.json` response.
#[derive(serde::Deserialize)]
struct CdnLink {
    #[serde(rename = "URI")]
    uri: String,
}

/// The Nexus API client.
#[derive(Clone)]
pub struct NexusClient {
    http: HttpClient,
    api_key: String,
    base: String,
    last_rate_limit: std::sync::Arc<std::sync::Mutex<RateLimit>>,
}

impl NexusClient {
    /// Create a client for the live Nexus API.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Tls`] if the TLS stack cannot be built.
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        Self::with_base(api_key, DEFAULT_BASE)
    }

    /// Create a client against an explicit base URL (used by tests to point at a
    /// mock server). `base` must end in `/`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Tls`] if the TLS stack cannot be built.
    pub fn with_base(api_key: impl Into<String>, base: impl Into<String>) -> Result<Self> {
        Ok(Self {
            http: HttpClient::new()?,
            api_key: api_key.into(),
            base: base.into(),
            last_rate_limit: std::sync::Arc::new(std::sync::Mutex::new(RateLimit::default())),
        })
    }

    /// The most recently observed rate-limit snapshot.
    #[must_use]
    pub fn rate_limit(&self) -> RateLimit {
        self.last_rate_limit
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Resolve an `nxm://` link to a concrete CDN download URL.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RateLimited`] on 429, [`Error::Api`] on any other
    /// non-200 status or empty link list, or [`Error::Http`]/[`Error::Json`] on
    /// transport/parse failures.
    pub async fn resolve(&self, uri: &NxmUri) -> Result<DownloadTarget> {
        let url = self.download_link_url(uri);
        let headers = [
            ("apikey", self.api_key.clone()),
            ("accept", "application/json".to_owned()),
        ];
        let response = self.http.get(&url, &headers).await?;
        let rate = RateLimit::from_response(&response);
        if let Ok(mut slot) = self.last_rate_limit.lock() {
            *slot = rate.clone();
        }
        if response.status == 429 {
            return Err(Error::RateLimited(rate.retry_after.unwrap_or(60)));
        }
        if response.status != 200 {
            return Err(Error::Api(format!(
                "download_link returned HTTP {}",
                response.status
            )));
        }
        let bytes = http::read_to_bytes(response).await?;
        let links: Vec<CdnLink> =
            serde_json::from_slice(&bytes).map_err(|e| Error::Json(e.to_string()))?;
        let first = links
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("no download links".to_owned()))?;
        let file_name = file_name_from_url(&first.uri);
        Ok(DownloadTarget {
            url: first.uri,
            file_name,
        })
    }

    /// Download a resolved target to `dest`, resuming and verifying MD5 when one
    /// is supplied, reporting progress as bytes arrive.
    ///
    /// # Errors
    ///
    /// Returns any transport, I/O, or checksum error from the download engine.
    pub async fn download<P>(
        &self,
        target: &DownloadTarget,
        dest: &Path,
        expected_md5: Option<&str>,
        on_progress: P,
    ) -> Result<u64>
    where
        P: FnMut(Progress),
    {
        download::download_to(&self.http, &target.url, dest, expected_md5, on_progress).await
    }

    fn download_link_url(&self, uri: &NxmUri) -> String {
        use std::fmt::Write as _;
        let mut url = format!("{}{}", self.base, uri.download_link_path());
        // Free-user links must echo `key`/`expires`; premium links omit them.
        if let (Some(key), Some(expires)) = (uri.key.as_deref(), uri.expires) {
            let _ = write!(url, "?key={key}&expires={expires}");
        }
        url
    }
}

/// Derive a file name from a CDN URL (last path segment, query stripped).
fn file_name_from_url(url: &str) -> String {
    let without_query = url.split('?').next().unwrap_or(url);
    let last = without_query.rsplit('/').next().unwrap_or("download");
    if last.is_empty() {
        "download".to_owned()
    } else {
        percent_decode(last)
    }
}

/// Minimal percent-decoding for a file name (`%20` etc.). Unknown escapes are
/// left as-is.
fn percent_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut bytes = input.bytes().peekable();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = bytes.next();
            let lo = bytes.next();
            match (hi.and_then(hex_val), lo.and_then(hex_val)) {
                (Some(h), Some(l)) => out.push(char::from(h.wrapping_mul(16).wrapping_add(l))),
                _ => out.push('%'),
            }
        } else {
            out.push(char::from(b));
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    // Guards guarantee no underflow; wrapping ops satisfy the arithmetic lint.
    match b {
        b'0'..=b'9' => Some(b.wrapping_sub(b'0')),
        b'a'..=b'f' => Some(b.wrapping_sub(b'a').wrapping_add(10)),
        b'A'..=b'F' => Some(b.wrapping_sub(b'A').wrapping_add(10)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn derives_file_name_from_url() {
        assert_eq!(
            file_name_from_url("https://cdn/foo/bar%20baz.zip?x=1"),
            "bar baz.zip"
        );
        assert_eq!(file_name_from_url("https://cdn/"), "download");
    }

    #[tokio::test]
    async fn resolve_reads_cdn_url_and_rate_limits() {
        let server = MockServer::start().await;
        let body = r#"[{"name":"Nexus CDN","short_name":"Global","URI":"https://cdn.example/file.zip?token=abc"}]"#;
        Mock::given(method("GET"))
            .and(path(
                "/v1/games/skyrimspecialedition/mods/1/files/2/download_link.json",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-rl-hourly-remaining", "2399")
                    .insert_header("x-rl-daily-remaining", "9999")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;

        let base = format!("{}/", server.uri());
        let client = NexusClient::with_base("test-key", base).unwrap();
        let uri = NxmUri::parse("nxm://skyrimspecialedition/mods/1/files/2").unwrap();
        let target = client.resolve(&uri).await.unwrap();

        assert_eq!(target.url, "https://cdn.example/file.zip?token=abc");
        assert_eq!(target.file_name, "file.zip");
        assert_eq!(client.rate_limit().hourly_remaining, Some(2399));
    }

    #[tokio::test]
    async fn resolve_maps_429_to_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "120"))
            .mount(&server)
            .await;
        let client = NexusClient::with_base("k", format!("{}/", server.uri())).unwrap();
        let uri = NxmUri::parse("nxm://game/mods/1/files/2").unwrap();
        match client.resolve(&uri).await {
            Err(Error::RateLimited(secs)) => assert_eq!(secs, 120),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }
}
