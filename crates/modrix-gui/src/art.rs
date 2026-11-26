// SPDX-License-Identifier: GPL-2.0-only
//! Game artwork: Steam's own library images, turned into the UI's identity.
//!
//! Steam keeps each installed game's store art under
//! `<steam>/appcache/librarycache/<appid>/`, so no network and no API key are
//! needed. A missing file is fetched once from Steam's public CDN.
//!
//! * `library_hero.jpg`: the wide, text-free background. It fills the top of
//!   the window, and a soft copy (blurred, dimmed) tints the whole backdrop.
//!   Its vibrant swatches become the accent colour.
//! * `library_600x900.jpg`: the portrait cover, on the game switcher.
//! * `logo.png`: the transparent title art, on the overview hero.
//!
//! Decoding and resizing run on a blocking worker, never on the UI thread.

use std::path::{Path, PathBuf};

use image::RgbaImage;
use image::imageops::{self, FilterType};

/// Decoded RGBA: width, height, bytes.
pub type Rgba = (u32, u32, Vec<u8>);

/// Art decoded off the UI thread, ready to upload.
#[derive(Debug, Default)]
pub struct ArtData {
    /// The wide hero, sharp.
    pub hero: Option<Rgba>,
    /// The hero, blurred and dimmed, for the full-window backdrop.
    pub soft: Option<Rgba>,
    /// The portrait cover.
    pub cover: Option<Rgba>,
    /// The transparent logo.
    pub logo: Option<Rgba>,
    /// Vibrant swatches (sRGB, 0..1), most representative first.
    pub swatches: Vec<[f32; 3]>,
}

/// Art the view draws.
#[derive(Debug, Clone, Default)]
pub struct ArtSet {
    /// The wide hero.
    pub hero: Option<rune::Pixels>,
    /// The soft backdrop.
    pub soft: Option<rune::Pixels>,
    /// The portrait cover.
    pub cover: Option<rune::Pixels>,
    /// The logo.
    pub logo: Option<rune::Pixels>,
    /// Vibrant swatches, linear colour.
    pub swatches: Vec<rune::Color>,
}

impl ArtSet {
    /// Upload decoded art into pixel buffers.
    #[must_use]
    pub fn from_data(data: ArtData) -> ArtSet {
        let upload = |img: Option<Rgba>| {
            img.map(|(w, h, bytes)| {
                let pixels = rune::Pixels::new();
                pixels.set(w, h, bytes);
                pixels
            })
        };
        ArtSet {
            hero: upload(data.hero),
            soft: upload(data.soft),
            cover: upload(data.cover),
            logo: upload(data.logo),
            swatches: data
                .swatches
                .into_iter()
                .map(|c| srgb(c[0], c[1], c[2]))
                .collect(),
        }
    }
}

/// An sRGB colour (0..1 channels) as Rune's linear colour.
#[must_use]
pub fn srgb(r: f32, g: f32, b: f32) -> rune::Color {
    use rune::color::srgb_decode;
    rune::Color {
        r: srgb_decode(r),
        g: srgb_decode(g),
        b: srgb_decode(b),
        a: 1.0,
    }
}

/// Largest artwork file fetched from the CDN.
const MAX_ART_BYTES: usize = 4 * 1024 * 1024;
/// Hero width kept for drawing.
const HERO_W: u32 = 1920;
/// Soft backdrop width (it is blurred, so small is fine).
const SOFT_W: u32 = 480;

/// Locate (or fetch once) each asset, then decode it on a blocking worker.
pub async fn resolve(appid: i64, cache_dir: PathBuf) -> ArtData {
    let dir = cache_dir.join("artwork").join(appid.to_string());
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::debug!(%error, "artwork cache unavailable");
    }
    let hero = asset(appid, "library_hero.jpg", &dir).await;
    let cover = asset(appid, "library_600x900.jpg", &dir).await;
    let logo = asset(appid, "logo.png", &dir).await;
    tokio::task::spawn_blocking(move || {
        decode_all(hero.as_deref(), cover.as_deref(), logo.as_deref())
    })
    .await
    .unwrap_or_default()
}

/// Art already in a local Steam library cache, decoded now (snapshot scenes).
#[must_use]
pub fn local(appid: i64) -> ArtData {
    let find = |file: &str| {
        modrix_core::detect::steam_roots()
            .into_iter()
            .take(16)
            .map(|root| {
                root.join("appcache/librarycache")
                    .join(appid.to_string())
                    .join(file)
            })
            .find(|p| p.is_file())
    };
    decode_all(
        find("library_hero.jpg").as_deref(),
        find("library_600x900.jpg").as_deref(),
        find("logo.png").as_deref(),
    )
}

fn decode_all(hero: Option<&Path>, cover: Option<&Path>, logo: Option<&Path>) -> ArtData {
    let hero_img = hero.and_then(|p| image::open(p).ok());
    let mut data = ArtData::default();
    if let Some(img) = &hero_img {
        data.swatches = swatches(&img.to_rgba8());
        let sharp = fit_width(img, HERO_W);
        data.soft = Some(soften(img));
        data.hero = Some(into_rgba(sharp));
    }
    data.cover = cover
        .and_then(|p| image::open(p).ok())
        .map(|img| into_rgba(img.resize_exact(360, 540, FilterType::Lanczos3).to_rgba8()));
    data.logo = logo
        .and_then(|p| image::open(p).ok())
        .map(|img| into_rgba(fit_width(&img, 800)));
    data
}

fn into_rgba(img: RgbaImage) -> Rgba {
    let (w, h) = img.dimensions();
    (w, h, img.into_raw())
}

/// Scale down to at most `max_w` wide, keeping the aspect.
fn fit_width(img: &image::DynamicImage, max_w: u32) -> RgbaImage {
    if img.width() <= max_w {
        return img.to_rgba8();
    }
    let h = max_w
        .saturating_mul(img.height())
        .checked_div(img.width())
        .unwrap_or(1)
        .max(1);
    img.resize_exact(max_w, h, FilterType::Lanczos3).to_rgba8()
}

/// A heavily blurred, dimmed copy for the backdrop wash.
fn soften(img: &image::DynamicImage) -> Rgba {
    let h = SOFT_W
        .saturating_mul(img.height())
        .checked_div(img.width().max(1))
        .unwrap_or(1)
        .max(1);
    let small = img
        .resize_exact((SOFT_W / 4).max(1), (h / 4).max(1), FilterType::Triangle)
        .to_rgba8();
    let blurred = imageops::blur(&small, 3.0);
    let mut out = imageops::resize(&blurred, SOFT_W, h, FilterType::Triangle);
    for px in out.pixels_mut() {
        for c in px.0.iter_mut().take(3) {
            *c = scale_byte(*c, 0.72);
        }
    }
    into_rgba(out)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to [0,255] before the cast"
)]
fn scale_byte(c: u8, by: f32) -> u8 {
    (f32::from(c) * by).clamp(0.0, 255.0) as u8
}

/// Steam's local cache first, else the public CDN (mirrored once).
async fn asset(appid: i64, file: &str, dir: &Path) -> Option<PathBuf> {
    for root in modrix_core::detect::steam_roots().into_iter().take(16) {
        let p = root
            .join("appcache/librarycache")
            .join(appid.to_string())
            .join(file);
        if p.is_file() {
            return Some(p);
        }
    }
    fetch_cdn(appid, file, dir).await
}

async fn fetch_cdn(appid: i64, file: &str, dir: &Path) -> Option<PathBuf> {
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

/// Hue buckets for swatch extraction.
const BUCKETS: usize = 24;

/// The most representative vibrant colours: bucket saturated, mid-bright
/// pixels by hue, weight each by population, return the top three.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "the hue bucket cast is bounded and non-negative"
)]
#[must_use]
pub fn swatches(img: &RgbaImage) -> Vec<[f32; 3]> {
    let small = imageops::resize(img, 64, 64, FilterType::Triangle);
    let mut weight = [0.0_f32; BUCKETS];
    let mut sum = [[0.0_f32; 3]; BUCKETS];
    for px in small.pixels() {
        let rgb = [px.0[0], px.0[1], px.0[2]].map(|c| f32::from(c) / 255.0);
        let (hue, sat, light) = rgb_to_hsl(rgb);
        if !(0.12..=0.95).contains(&light) || sat < 0.28 {
            continue;
        }
        let w = sat * (1.0 - (light - 0.55).abs() * 1.3).max(0.15);
        let bucket = ((hue * BUCKETS as f32) as usize).min(BUCKETS - 1);
        if let (Some(wref), Some(sref)) = (weight.get_mut(bucket), sum.get_mut(bucket)) {
            *wref += w;
            for (acc, ch) in sref.iter_mut().zip(rgb) {
                *acc += ch * w;
            }
        }
    }
    let mut scored: Vec<(f32, [f32; 3])> = weight
        .iter()
        .zip(sum.iter())
        .filter(|(w, _)| **w > 0.0)
        .map(|(&w, s)| (w, s.map(|c| c / w)))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(3).map(|(_, c)| c).collect()
}

/// RGB (0..1) to HSL (each 0..1).
#[must_use]
pub fn rgb_to_hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let light = f32::midpoint(max, min);
    let d = max - min;
    if d.abs() < f32::EPSILON {
        return (0.0, 0.0, light);
    }
    let sat = d / (1.0 - (2.0 * light - 1.0).abs());
    let hue = if (max - r).abs() < f32::EPSILON {
        (((g - b) / d) % 6.0 + 6.0) % 6.0
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (hue / 6.0, sat.clamp(0.0, 1.0), light)
}

/// HSL (each 0..1) to RGB (0..1): the inverse, for the round-trip test.
#[cfg(test)]
#[must_use]
pub fn hsl_to_rgb(hue: f32, sat: f32, light: f32) -> [f32; 3] {
    if sat.abs() < f32::EPSILON {
        return [light; 3];
    }
    let q = if light < 0.5 {
        light * (1.0 + sat)
    } else {
        light + sat - light * sat
    };
    let p = 2.0 * light - q;
    let channel = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [
        channel(hue + 1.0 / 3.0),
        channel(hue),
        channel(hue - 1.0 / 3.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trips() {
        for rgb in [[0.2, 0.6, 0.9], [0.8, 0.1, 0.3], [0.5, 0.5, 0.5]] {
            let (h, s, l) = rgb_to_hsl(rgb);
            let back = hsl_to_rgb(h, s, l);
            for (a, b) in back.iter().zip(rgb) {
                assert!((a - b).abs() < 0.02, "{back:?} vs {rgb:?}");
            }
        }
    }

    #[test]
    fn swatches_pick_the_vibrant_region() {
        let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([10, 10, 12, 255]));
        for y in 0..20 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgba([230, 120, 20, 255]));
            }
        }
        let found = swatches(&img);
        let top = found.first().copied().unwrap();
        assert!(top[0] > top[2], "expected a warm swatch, got {top:?}");
    }

    #[test]
    fn soften_keeps_aspect() {
        let img = image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            1920,
            620,
            image::Rgba([200, 100, 50, 255]),
        ));
        let (w, h, bytes) = soften(&img);
        assert_eq!(w, SOFT_W);
        assert_eq!(h, 155);
        assert_eq!(bytes.len(), 480 * 155 * 4);
    }
}
