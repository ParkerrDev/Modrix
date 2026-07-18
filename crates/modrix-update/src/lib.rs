// SPDX-License-Identifier: GPL-2.0-only
//! In-app updates via GitHub Releases.
//!
//! Reuses the workspace's GPLv2-clean HTTP stack ([`modrix_download::http`]),
//! so it never pulls reqwest/ring. Checking is best-effort: any network, auth
//! (the repo is private until release), or parse failure resolves to "no
//! update" rather than a hard error that could block startup - mirroring the
//! plugin registry's offline-tolerant behaviour.
//!
//! The flow is: [`check`] the latest release → if newer, [`download`] the
//! platform asset → [`apply`] it (on Windows, launch the installer and let the
//! caller exit so the running `.exe` is unlocked). Other platforms surface the
//! release for a manual download.

use std::path::{Path, PathBuf};

use modrix_download::http::HttpClient;
use serde::Deserialize;

/// The GitHub repository releases are published to.
pub const REPO: &str = "ParkerrDev/Modrix";

/// The "latest published release" endpoint (excludes drafts/prereleases).
const API_LATEST: &str = "https://api.github.com/repos/ParkerrDev/Modrix/releases/latest";

/// Largest release JSON we will buffer (the payload is a few KiB).
const MAX_JSON_BYTES: usize = 1024 * 1024;
/// Largest update asset we will download (installers are far smaller).
const MAX_ASSET_BYTES: usize = 300 * 1024 * 1024;

/// A release newer than the running build.
///
/// `asset_url` is empty when no asset matches this platform (e.g. on Linux,
/// where updates go through the OS package manager); the caller then offers
/// [`release_url`](Self::release_url) as a manual download instead.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    /// The new version, normalized (no leading `v`).
    pub version: String,
    /// The release notes (GitHub release body); may be empty.
    pub notes: String,
    /// The release's web page, for a manual download.
    pub release_url: String,
    /// The platform installer asset's file name, or empty if none matched.
    pub asset_name: String,
    /// The platform installer asset's download URL, or empty if none matched.
    pub asset_url: String,
}

impl UpdateInfo {
    /// Whether this build can install the update itself (a matching asset on a
    /// supported platform), versus only linking to a manual download.
    #[must_use]
    pub fn can_self_install(&self) -> bool {
        cfg!(windows) && !self.asset_url.is_empty()
    }
}

/// The updater's error type. Note that a *missing* update is not an error -
/// [`check`] returns `Ok(None)` for that.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A transport failure, or a non-success HTTP status on the asset fetch.
    #[error("http: {0}")]
    Http(String),
    /// The release JSON did not parse.
    #[error("parsing the release feed failed: {0}")]
    Json(String),
    /// A filesystem error while staging the downloaded asset.
    #[error("io: {0}")]
    Io(String),
    /// No downloadable asset was resolved for this platform.
    #[error("no downloadable update asset for this platform")]
    NoAsset,
    /// Launching the downloaded installer failed.
    #[error("launching the installer failed: {0}")]
    Spawn(String),
    /// In-app apply is not implemented for this platform.
    #[error("in-app updates are only supported on Windows; download the release manually")]
    Unsupported,
}

/// The updater result alias.
pub type Result<T> = std::result::Result<T, Error>;

impl From<modrix_download::Error> for Error {
    fn from(e: modrix_download::Error) -> Self {
        Self::Http(e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e.to_string())
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

// --- GitHub API JSON (only the fields we read) ---

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// The `User-Agent` GitHub's API requires (it 403s requests without one).
fn user_agent() -> String {
    format!(
        "Modrix/{} (+https://github.com/{REPO})",
        env!("CARGO_PKG_VERSION")
    )
}

/// Check GitHub for a release newer than `current` (e.g. `env!("CARGO_PKG_VERSION")`).
///
/// # Errors
///
/// Returns [`Error::Http`]/[`Error::Json`] only on an unexpected transport or
/// parse failure. The common "no update" cases - up to date, unreachable, the
/// repo private (401/404), a non-semver tag, a draft/prerelease - all resolve
/// to `Ok(None)` so a check can never block the app.
pub async fn check(current: &str) -> Result<Option<UpdateInfo>> {
    let headers: [(&str, String); 3] = [
        ("user-agent", user_agent()),
        ("accept", "application/vnd.github+json".to_owned()),
        ("x-github-api-version", "2022-11-28".to_owned()),
    ];
    let client = HttpClient::new()?;
    let response = client.get(API_LATEST, &headers).await?;
    if response.status != 200 {
        tracing::debug!(status = response.status, "no release feed available");
        return Ok(None);
    }
    let raw = response.bytes(MAX_JSON_BYTES).await?;
    let release: GhRelease = serde_json::from_slice(&raw)?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    Ok(select(current, release))
}

/// Decide whether `release` supersedes `current`, and pick its platform asset.
fn select(current: &str, release: GhRelease) -> Option<UpdateInfo> {
    let latest = parse_version(&release.tag_name)?;
    let running = parse_version(current)?;
    if latest <= running {
        return None;
    }
    let (asset_name, asset_url) = release
        .assets
        .into_iter()
        .find(|a| is_platform_asset(&a.name))
        .map(|a| (a.name, a.browser_download_url))
        .unwrap_or_default();
    Some(UpdateInfo {
        version: latest.to_string(),
        notes: release.body,
        release_url: release.html_url,
        asset_name,
        asset_url,
    })
}

/// Parse a release tag as a version, tolerating a leading `v`/`V`.
fn parse_version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Whether a release asset is the installer for the current platform.
#[cfg(windows)]
fn is_platform_asset(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("msi"))
}

/// In-app apply targets Windows; other platforms link to a manual download.
#[cfg(not(windows))]
fn is_platform_asset(_name: &str) -> bool {
    false
}

/// Download the update's installer asset into `dir`, returning the staged path.
///
/// # Errors
///
/// [`Error::NoAsset`] when there is no platform asset, [`Error::Http`] on a
/// failed fetch, or [`Error::Io`] if the file cannot be written.
pub async fn download(info: &UpdateInfo, dir: &Path) -> Result<PathBuf> {
    if info.asset_url.is_empty() {
        return Err(Error::NoAsset);
    }
    let headers: [(&str, String); 1] = [("user-agent", user_agent())];
    let client = HttpClient::new()?;
    let response = client.get(&info.asset_url, &headers).await?;
    if response.status != 200 {
        return Err(Error::Http(format!(
            "asset download failed: HTTP {}",
            response.status
        )));
    }
    let bytes = response.bytes(MAX_ASSET_BYTES).await?;
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(asset_file_name(&info.asset_name));
    std::fs::write(&dest, &bytes)?;
    Ok(dest)
}

/// The bare, path-separator-free file name for a downloaded asset (defends the
/// staging path against a hostile asset name).
fn asset_file_name(name: &str) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if leaf.is_empty() {
        "modrix-update".to_owned()
    } else {
        leaf.to_owned()
    }
}

/// Launch a downloaded Windows installer, then return.
///
/// The caller MUST exit promptly afterwards so the running `.exe` is unlocked
/// and the installer can replace it (the bundled NSIS script closes any lingering
/// instance and relaunches the app when it finishes).
///
/// # Errors
///
/// [`Error::Spawn`] if the installer process cannot be started.
#[cfg(windows)]
pub fn apply(installer: &Path) -> Result<()> {
    // `/S` runs the NSIS installer silently; it force-closes the running
    // instance, replaces the files, and relaunches the app when it finishes.
    std::process::Command::new(installer)
        .arg("/S")
        .spawn()
        .map(|_child| ())
        .map_err(|e| Error::Spawn(e.to_string()))
}

/// In-app apply is Windows-only for now.
///
/// # Errors
///
/// Always [`Error::Unsupported`] off Windows.
#[cfg(not(windows))]
pub fn apply(_installer: &Path) -> Result<()> {
    Err(Error::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_tolerates_v_prefix() {
        assert_eq!(parse_version("v1.2.3"), Some(semver::Version::new(1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Some(semver::Version::new(1, 2, 3)));
        assert_eq!(
            parse_version(" V0.1.0 "),
            Some(semver::Version::new(0, 1, 0))
        );
        assert_eq!(parse_version("not-a-version"), None);
    }

    #[test]
    fn select_requires_a_strictly_newer_tag() {
        let make = |tag: &str| GhRelease {
            tag_name: tag.to_owned(),
            body: String::new(),
            html_url: "https://example/rel".to_owned(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        };
        assert!(select("0.1.0", make("v0.2.0")).is_some());
        assert!(select("0.2.0", make("v0.2.0")).is_none());
        assert!(select("0.3.0", make("v0.2.0")).is_none());
        // A non-semver tag never counts as an update.
        assert!(select("0.1.0", make("nightly")).is_none());
    }

    #[test]
    fn asset_file_name_strips_any_path() {
        assert_eq!(
            asset_file_name("Modrix-Setup-0.1.0.exe"),
            "Modrix-Setup-0.1.0.exe"
        );
        assert_eq!(asset_file_name("../../evil.exe"), "evil.exe");
        assert_eq!(asset_file_name("a/b/c.msi"), "c.msi");
    }
}
