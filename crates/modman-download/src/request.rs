// SPDX-License-Identifier: GPL-2.0-only
//! The browser hand-off job and its trust-boundary validation.
//!
//! A `HandoffJob` arrives over the token-authed loopback endpoint (only a client
//! holding the session token can POST one). The token is the security boundary,
//! so we do not restrict *which* site a download may come from - this is a
//! general download manager. We do validate the schema and scheme, bound the URL
//! length, reject header CRLF injection, and - the one thing that names a file on
//! disk - sanitize the filename down to a safe basename (no traversal).

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{Error, Result};
use crate::types::{DownloadRequest, SegmentLimits};

/// The only hand-off schema this build understands.
const SUPPORTED_SCHEMA: u32 = 1;
/// Cap on a hand-off URL so a hostile job cannot make us allocate unboundedly.
const MAX_URL_LEN: usize = 8192;
/// Cap on a sanitized filename.
const MAX_NAME_LEN: usize = 255;

/// A download hand-off from the browser extension.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HandoffJob {
    /// Hand-off schema version.
    pub schema_version: u32,
    /// The (already-signed) URL the browser was about to fetch.
    pub url: String,
    /// The browser's tentative filename (sanitized engine-side).
    pub filename: String,
    /// Advisory MIME type.
    #[serde(default)]
    pub mime: Option<String>,
    /// The page the download came from (replayed as `Referer`).
    #[serde(default)]
    pub referrer: Option<String>,
    /// The browser's User-Agent (replayed verbatim).
    #[serde(default)]
    pub user_agent: Option<String>,
    /// One combined `Cookie:` header for the download's host.
    #[serde(default)]
    pub cookie: Option<String>,
    /// Any extra request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Advisory total size.
    #[serde(default)]
    pub total_bytes: Option<u64>,
    /// The originating tab URL (used to route to a game).
    #[serde(default)]
    pub page_url: Option<String>,
    /// An explicit game routing hint.
    #[serde(default)]
    pub game_hint: Option<GameHint>,
}

/// A hint about which game a download belongs to.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GameHint {
    /// The Nexus game domain (e.g. `skyrimspecialedition`).
    #[serde(default)]
    pub domain: Option<String>,
    /// The Nexus numeric game id from a CDN path.
    #[serde(default)]
    pub numeric_id: Option<u64>,
}

impl HandoffJob {
    /// Parse a job from its JSON body.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BadJob`] if the JSON is malformed or has unknown fields.
    pub fn from_json(body: &str) -> Result<Self> {
        serde_json::from_str(body).map_err(|e| Error::BadJob(e.to_string()))
    }

    /// Validate this job and normalize it into a [`DownloadRequest`] landing in
    /// `dir`, with the default segmentation tuning.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BadJob`] on any validation failure.
    pub fn into_request(self, dir: &Path) -> Result<DownloadRequest> {
        if self.schema_version != SUPPORTED_SCHEMA {
            return Err(Error::BadJob(format!(
                "unsupported schemaVersion {}",
                self.schema_version
            )));
        }
        if self.url.len() > MAX_URL_LEN {
            return Err(Error::BadJob("url too long".to_owned()));
        }
        require_web_scheme(&self.url)?;
        let out = sanitize_filename(&self.filename)
            .or_else(|| filename_from_url(&self.url))
            .ok_or_else(|| Error::BadJob("no usable filename".to_owned()))?;
        let headers = assemble_headers(&self)?;
        Ok(DownloadRequest {
            url: self.url,
            headers,
            dir: dir.to_path_buf(),
            out,
            expected_size: self.total_bytes,
            checksum: None,
            limits: SegmentLimits::default(),
        })
    }
}

/// The URL must be `http`/`https` - never `file:`/`data:`/etc.
fn require_web_scheme(url: &str) -> Result<()> {
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        Some("http" | "https") => Ok(()),
        _ => Err(Error::BadJob("url scheme must be http(s)".to_owned())),
    }
}

/// Reduce an arbitrary browser-supplied name to a safe basename, or `None`.
fn sanitize_filename(name: &str) -> Option<String> {
    // `file_name` strips every path component, so `../../etc/passwd` → `passwd`
    // and `..`/`/` → None.
    let base = Path::new(name).file_name()?.to_string_lossy();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .take(MAX_NAME_LEN)
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_owned();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Derive a filename from the URL's last path segment, query stripped.
fn filename_from_url(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = after_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let last = path.rsplit('/').next().unwrap_or("");
    sanitize_filename(last)
}

/// Build the replay headers, rejecting CRLF header injection.
fn assemble_headers(job: &HandoffJob) -> Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    if let Some(ua) = &job.user_agent {
        push_header(&mut headers, "User-Agent", ua)?;
    }
    if let Some(referer) = &job.referrer {
        push_header(&mut headers, "Referer", referer)?;
    }
    if let Some(cookie) = &job.cookie
        && !cookie.is_empty()
    {
        push_header(&mut headers, "Cookie", cookie)?;
    }
    for (name, value) in &job.headers {
        if !is_token(name) {
            return Err(Error::BadJob(format!("bad header name `{name}`")));
        }
        push_header(&mut headers, name, value)?;
    }
    Ok(headers)
}

fn push_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) -> Result<()> {
    if value.contains(['\r', '\n']) {
        return Err(Error::BadJob(format!("header `{name}` contains a newline")));
    }
    headers.push((name.to_owned(), value.to_owned()));
    Ok(())
}

/// An HTTP token (header name) - letters, digits, and a few punctuation marks.
fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(json: &str) -> Result<HandoffJob> {
        HandoffJob::from_json(json)
    }

    #[test]
    fn parses_and_normalizes_a_real_job() {
        let j = job(
            r#"{"schemaVersion":1,"url":"https://cdn.example/a/File-1.2.zip?t=x",
                "filename":"File-1.2.zip","userAgent":"Moz","referrer":"https://site/p",
                "cookie":"sid=abc","totalBytes":123,
                "gameHint":{"domain":"skyrimspecialedition"}}"#,
        )
        .unwrap();
        let req = j.into_request(Path::new("/dl")).unwrap();
        assert_eq!(req.out, "File-1.2.zip");
        assert_eq!(req.expected_size, Some(123));
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| k == "User-Agent" && v == "Moz")
        );
        assert!(
            req.headers
                .iter()
                .any(|(k, v)| k == "Cookie" && v == "sid=abc")
        );
    }

    #[test]
    fn path_traversal_filename_is_neutralized() {
        let j = job(r#"{"schemaVersion":1,"url":"https://cdn/x","filename":"../../etc/passwd"}"#)
            .unwrap();
        let req = j.into_request(Path::new("/dl")).unwrap();
        assert_eq!(req.out, "passwd");
        assert_eq!(req.dest(), Path::new("/dl/passwd"));
    }

    #[test]
    fn empty_filename_falls_back_to_url() {
        let j = job(r#"{"schemaVersion":1,"url":"https://cdn/path/thing.7z?x=1","filename":".."}"#)
            .unwrap();
        let req = j.into_request(Path::new("/dl")).unwrap();
        assert_eq!(req.out, "thing.7z");
    }

    #[test]
    fn rejects_unsupported_schema() {
        let j = job(r#"{"schemaVersion":99,"url":"https://cdn/x","filename":"f"}"#).unwrap();
        assert!(matches!(
            j.into_request(Path::new("/dl")),
            Err(Error::BadJob(_))
        ));
    }

    #[test]
    fn rejects_non_web_scheme() {
        let j = job(r#"{"schemaVersion":1,"url":"file:///etc/passwd","filename":"f"}"#).unwrap();
        assert!(matches!(
            j.into_request(Path::new("/dl")),
            Err(Error::BadJob(_))
        ));
    }

    #[test]
    fn rejects_header_injection() {
        let j = job(
            r#"{"schemaVersion":1,"url":"https://cdn/x","filename":"f","referrer":"a\r\nEvil: 1"}"#,
        )
        .unwrap();
        assert!(matches!(
            j.into_request(Path::new("/dl")),
            Err(Error::BadJob(_))
        ));
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(job(r#"{"schemaVersion":1,"url":"https://x","filename":"f","bogus":1}"#).is_err());
    }
}
