// SPDX-License-Identifier: GPL-2.0-only
//! Game artwork: Steam's own library images, straight from its local cache.
//!
//! Steam keeps every installed game's storefront art under
//! `<steam>/appcache/librarycache/<appid>/` - a pre-blurred hero
//! (`library_hero_blur.jpg`, our dashboard backdrop for free), the portrait
//! (`library_600x900.jpg`), the wide header, and the logo. No network, no
//! API keys. When the local cache misses (non-Steam install), a best-effort
//! fetch from Steam's public CDN fills `<cache>/artwork/<appid>/` once.

use std::path::PathBuf;

/// The artwork files of one game, resolved to local paths.
#[derive(Debug, Clone, Default)]
pub struct ArtSet {
    /// Pre-blurred hero (dashboard backdrop).
    pub hero_blur: Option<PathBuf>,
    /// Wide header (game cards).
    pub header: Option<PathBuf>,
}

/// Look the game's art up in Steam's local cache.
#[must_use]
pub fn local(appid: i64) -> ArtSet {
    for root in modrix_core::detect::steam_roots() {
        let dir = root.join("appcache/librarycache").join(appid.to_string());
        if dir.is_dir() {
            return ArtSet {
                hero_blur: existing(dir.join("library_hero_blur.jpg"))
                    .or_else(|| existing(dir.join("library_hero.jpg"))),
                header: existing(dir.join("header.jpg")),
            };
        }
    }
    ArtSet::default()
}

/// Local cache first, then the on-disk CDN mirror this function maintains.
/// Missing files are fetched from Steam's public CDN once (best-effort,
/// size-capped); a game with no Steam art simply renders without imagery.
pub async fn resolve(appid: i64, cache_dir: PathBuf) -> ArtSet {
    let from_steam = local(appid);
    if from_steam.hero_blur.is_some() && from_steam.header.is_some() {
        return from_steam;
    }
    let dir = cache_dir.join("artwork").join(appid.to_string());
    let _ = std::fs::create_dir_all(&dir);
    ArtSet {
        hero_blur: match from_steam.hero_blur {
            Some(found) => Some(found),
            None => fetch_cdn(appid, "library_hero.jpg", &dir).await,
        },
        header: match from_steam.header {
            Some(found) => Some(found),
            None => fetch_cdn(appid, "header.jpg", &dir).await,
        },
    }
}

/// Largest artwork file we will download.
const MAX_ART_BYTES: usize = 4 * 1024 * 1024;

async fn fetch_cdn(appid: i64, file: &str, dir: &std::path::Path) -> Option<PathBuf> {
    let dest = dir.join(file);
    if dest.is_file() {
        return Some(dest);
    }
    let url = format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/{file}");
    let client = modrix_download::http::HttpClient::new().ok()?;
    let response = client.get(&url, &[]).await.ok()?;
    if response.status != 200 {
        return None;
    }
    let bytes = response.bytes(MAX_ART_BYTES).await.ok()?;
    std::fs::write(&dest, &bytes).ok()?;
    Some(dest)
}

fn existing(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}
