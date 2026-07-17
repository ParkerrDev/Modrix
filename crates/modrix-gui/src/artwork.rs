// SPDX-License-Identifier: GPL-2.0-only
//! Game artwork: Steam's own library images, turned into the UI's identity.
//!
//! Steam keeps every installed game's storefront art under
//! `<steam>/appcache/librarycache/<appid>/` (the wide `header.jpg`, the
//! cinematic `library_hero.jpg`) - no network, no API keys. From that art we
//! derive three things per game:
//!
//! * a **backdrop** - a baked PNG that is sharp and bright at the top (it sits
//!   behind the header of every tab), progressively blurs downward, and fades
//!   its alpha to nothing near the bottom, so the image dissipates into the
//!   window rather than fighting the content;
//! * the **accent palette** - the vibrant swatches of the art, which become
//!   the whole app's accent color for that game;
//! * the **header** image itself, shown in the sidebar and the dashboard card.
//!
//! A missing local file is fetched once from Steam's public CDN. All decoding,
//! blurring, and color work is bounded and cached to disk, and runs off the UI
//! thread (see [`resolve`]).

use std::path::{Path, PathBuf};

use iced::Color;
use image::{GenericImageView, RgbaImage, imageops::FilterType};

/// Everything the UI needs to dress itself in a game's identity.
#[derive(Debug, Clone, Default)]
pub struct ArtSet {
    /// The wide header image (sidebar banner + dashboard card).
    pub header: Option<PathBuf>,
    /// The baked full-bleed backdrop (blur ramp + bottom fade).
    pub backdrop: Option<PathBuf>,
    /// Vibrant swatches of the art, most representative first; the theme
    /// derives the accent palette from these.
    pub swatches: Vec<Color>,
}

/// Width the backdrop is baked at (upscaled to fill at draw time - it is a
/// soft background, so a modest resolution keeps the bake fast).
const BACKDROP_W: u32 = 900;
/// Largest artwork file we will download from the CDN.
const MAX_ART_BYTES: usize = 4 * 1024 * 1024;

/// Resolve a game's artwork: locate (or fetch once) its source images, then
/// bake the backdrop and extract the accent swatches off the UI thread.
pub async fn resolve(appid: i64, cache_dir: PathBuf) -> ArtSet {
    let dir = cache_dir.join("artwork").join(appid.to_string());
    let _ = std::fs::create_dir_all(&dir);
    let mut sources = steam_sources(appid);
    if sources.hero.is_none() {
        sources.hero = fetch_cdn(appid, "library_hero.jpg", &dir).await;
    }
    if sources.header.is_none() {
        sources.header = fetch_cdn(appid, "header.jpg", &dir).await;
    }
    // Decode/blur/color work is CPU-bound - never run it on the UI executor.
    let backdrop_dest = dir.join("backdrop.png");
    let (backdrop, swatches) =
        tokio::task::spawn_blocking(move || process(&sources, &backdrop_dest))
            .await
            .unwrap_or_default();
    ArtSet {
        header: header_for(appid, &dir),
        backdrop,
        swatches,
    }
}

/// The header path (Steam cache first, then the CDN mirror this maintains).
fn header_for(appid: i64, dir: &Path) -> Option<PathBuf> {
    steam_sources(appid)
        .header
        .or_else(|| existing(dir.join("header.jpg")))
}

/// The source images of a game, from Steam's local cache.
#[derive(Default)]
struct Sources {
    hero: Option<PathBuf>,
    header: Option<PathBuf>,
}

fn steam_sources(appid: i64) -> Sources {
    for root in modrix_core::detect::steam_roots() {
        let dir = root.join("appcache/librarycache").join(appid.to_string());
        if dir.is_dir() {
            return Sources {
                hero: existing(dir.join("library_hero.jpg")),
                header: existing(dir.join("header.jpg")),
            };
        }
    }
    Sources::default()
}

/// CPU stage: bake the backdrop from the hero and pull swatches from the
/// header (falling back to the hero). Returns `(backdrop, swatches)`.
fn process(sources: &Sources, backdrop_dest: &Path) -> (Option<PathBuf>, Vec<Color>) {
    let backdrop = sources
        .hero
        .as_deref()
        .and_then(|hero| bake_backdrop(hero, backdrop_dest));
    let swatch_src = sources.header.as_deref().or(sources.hero.as_deref());
    let swatches = swatch_src.map(compute_swatches).unwrap_or_default();
    (backdrop, swatches)
}

/// Bake the tab backdrop: sharp at the top, blurring downward, alpha fading to
/// nothing near the bottom, dimmed for text legibility. Cached - an existing
/// bake is reused.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "alpha is in [0,1] so 255*alpha is a non-negative byte"
)]
fn bake_backdrop(hero: &Path, dest: &Path) -> Option<PathBuf> {
    if dest.is_file() {
        return Some(dest.to_path_buf());
    }
    let img = image::open(hero).ok()?;
    let (w0, h0) = img.dimensions();
    if w0 == 0 || h0 == 0 {
        return None;
    }
    let h = BACKDROP_W
        .saturating_mul(h0)
        .checked_div(w0)
        .unwrap_or(BACKDROP_W)
        .max(1);
    let sharp = img
        .resize_exact(BACKDROP_W, h, FilterType::Triangle)
        .to_rgba8();
    // A cheap heavy blur: downscale hard, then upscale back.
    let small = img.resize_exact(BACKDROP_W / 14, h / 14, FilterType::Triangle);
    let blurred = small
        .resize_exact(BACKDROP_W, h, FilterType::Triangle)
        .to_rgba8();

    let mut out = RgbaImage::new(BACKDROP_W, h);
    for y in 0..h {
        #[expect(clippy::cast_precision_loss, reason = "row index is small")]
        let t = y as f32 / h as f32;
        let blur_mix = smoothstep(0.0, 0.55, t); // 0 = sharp (top) → 1 = blurred
        let alpha = 1.0 - smoothstep(0.30, 0.94, t); // fade out toward the bottom
        let dim = 0.66; // darken so light UI text stays readable over it
        for x in 0..BACKDROP_W {
            let s = sharp.get_pixel(x, y).0;
            let b = blurred.get_pixel(x, y).0;
            let px = [
                mix_u8(s[0], b[0], blur_mix, dim),
                mix_u8(s[1], b[1], blur_mix, dim),
                mix_u8(s[2], b[2], blur_mix, dim),
                (255.0 * alpha) as u8,
            ];
            out.put_pixel(x, y, image::Rgba(px));
        }
    }
    out.save(dest).ok().map(|()| dest.to_path_buf())
}

/// Lerp two channels by `mix`, then scale by `dim`, back to a byte.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to [0,255] before the cast"
)]
fn mix_u8(a: u8, b: u8, mix: f32, dim: f32) -> u8 {
    let v = (f32::from(a) * (1.0 - mix) + f32::from(b) * mix) * dim;
    v.clamp(0.0, 255.0) as u8
}

/// Cubic smoothstep in `[edge0, edge1]`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return f32::from(x >= edge1);
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Hue buckets for swatch extraction.
const BUCKETS: usize = 24;

/// The most representative *vibrant* colors of an image: bucket saturated,
/// mid-bright pixels by hue, score each bucket by weighted population, and
/// return the top buckets' colors, brightest/most-saturated first. Empty when
/// the art has no colorful region (the theme then keeps its own accent).
#[expect(
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "channel names are conventional; the hue-bucket cast is bounded and non-negative"
)]
fn compute_swatches(path: &Path) -> Vec<Color> {
    let Ok(img) = image::open(path) else {
        return Vec::new();
    };
    let small = img.resize_exact(64, 64, FilterType::Triangle).to_rgba8();
    let mut weight = [0.0_f32; BUCKETS];
    let mut sum = [[0.0_f32; 3]; BUCKETS];
    for px in small.pixels() {
        let [r, g, b] = [px.0[0], px.0[1], px.0[2]].map(|c| f32::from(c) / 255.0);
        let (h, s, l) = rgb_to_hsl(r, g, b);
        if !(0.12..=0.95).contains(&l) || s < 0.28 {
            continue; // skip near-black, near-white, and washed-out pixels
        }
        // Prefer saturated colors near mid brightness (the "poster" colors).
        let w = s * (1.0 - (l - 0.55).abs() * 1.3).max(0.15);
        let bucket = ((h * BUCKETS as f32) as usize).min(BUCKETS - 1);
        // `.get_mut` (never `[]`) keeps the Power-of-Ten no-index rule; the
        // bucket is provably in range, so this always hits.
        if let (Some(wref), Some(sref)) = (weight.get_mut(bucket), sum.get_mut(bucket)) {
            *wref += w;
            for (acc, ch) in sref.iter_mut().zip([r, g, b]) {
                *acc += ch * w;
            }
        }
    }
    let mut scored: Vec<(f32, Color)> = weight
        .iter()
        .zip(sum.iter())
        .filter(|(w, _)| **w > 0.0)
        .map(|(&w, &[sr, sg, sb])| (w, Color::from_rgb(sr / w, sg / w, sb / w)))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(3).map(|(_, c)| c).collect()
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

fn existing(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}

/// RGB (0..1) → HSL (h in 0..1, s in 0..1, l in 0..1).
#[expect(
    clippy::many_single_char_names,
    reason = "r/g/b/h/s/l are the conventional channel names"
)]
pub fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f32::midpoint(max, min);
    let d = max - min;
    if d.abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if (max - r).abs() < f32::EPSILON {
        (((g - b) / d) % 6.0 + 6.0) % 6.0
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s.clamp(0.0, 1.0), l)
}

/// HSL (0..1) → an iced [`Color`].
#[expect(
    clippy::many_single_char_names,
    reason = "h/s/l are the conventional channel names"
)]
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Color {
    if s.abs() < f32::EPSILON {
        return Color::from_rgb(l, l, l);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
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
    Color::from_rgb(hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trips() {
        for (r, g, b) in [(0.2, 0.6, 0.9), (0.8, 0.1, 0.3), (0.5, 0.5, 0.5)] {
            let (h, s, l) = rgb_to_hsl(r, g, b);
            let c = hsl_to_rgb(h, s, l);
            assert!((c.r - r).abs() < 0.02, "r {} vs {}", c.r, r);
            assert!((c.g - g).abs() < 0.02, "g {} vs {}", c.g, g);
            assert!((c.b - b).abs() < 0.02, "b {} vs {}", c.b, b);
        }
    }

    #[test]
    fn smoothstep_is_bounded_and_monotone() {
        assert!(smoothstep(0.0, 1.0, -1.0).abs() < 1e-6);
        assert!((smoothstep(0.0, 1.0, 2.0) - 1.0).abs() < 1e-6);
        assert!(smoothstep(0.0, 1.0, 0.25) < smoothstep(0.0, 1.0, 0.75));
    }

    #[test]
    fn swatches_pick_the_vibrant_region() {
        // A mostly-dark image with a saturated orange block returns orange.
        let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([10, 10, 12, 255]));
        for y in 0..20 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgba([230, 120, 20, 255]));
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.png");
        img.save(&path).unwrap();
        let swatches = compute_swatches(&path);
        assert!(!swatches.is_empty());
        let top = swatches[0];
        assert!(top.r > top.b, "expected a warm swatch, got {top:?}");
    }
}
