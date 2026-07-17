// SPDX-License-Identifier: GPL-2.0-only
//! Game artwork: Steam's own library images, turned into the UI's identity.
//!
//! Steam keeps every installed game's storefront art under
//! `<steam>/appcache/librarycache/<appid>/` - no network, no API keys. Each
//! asset has a distinct job, and we use the right one in the right place:
//!
//! * `library_hero.jpg` - the wide, **text-free** cinematic background. We
//!   bake it into the **backdrop** (sharp + bright at the top behind the
//!   header, blurring and fading to nothing toward the bottom) and pull the
//!   **accent palette** from it, so the UI's color harmonizes with what fills
//!   the screen behind it.
//! * `library_600x900.jpg` - the **portrait cover** (key art + title). Shown
//!   as the "game card" in the sidebar.
//! * `logo.png` - the **transparent themed logo** (the title as art). Shown
//!   on the dashboard's active-game card.
//!
//! A missing local file is fetched once from Steam's public CDN. All decoding,
//! blurring, and color work is bounded and cached to disk, and runs off the UI
//! thread (see [`resolve`]).

use std::path::{Path, PathBuf};

use iced::Color;
use image::imageops::{self, FilterType};
use image::{GenericImageView, RgbaImage};

/// Everything the UI needs to dress itself in a game's identity.
#[derive(Debug, Clone, Default)]
pub struct ArtSet {
    /// Portrait cover (key art + title) - the sidebar "game card".
    pub cover: Option<PathBuf>,
    /// Transparent themed logo - the dashboard active-game card.
    pub logo: Option<PathBuf>,
    /// The baked full-bleed backdrop (blur ramp + bottom fade), from the hero.
    pub backdrop: Option<PathBuf>,
    /// Vibrant swatches of the hero art, most representative first; the theme
    /// derives the accent palette from these.
    pub swatches: Vec<Color>,
}

/// Width the backdrop is baked at (upscaled to fill at draw time - it is a
/// soft background, so a modest resolution keeps the bake fast).
const BACKDROP_W: u32 = 900;
/// Largest artwork file we will download from the CDN.
const MAX_ART_BYTES: usize = 4 * 1024 * 1024;

/// Resolve a game's artwork: locate (or fetch once) each Steam asset, then
/// bake the backdrop and extract the accent swatches off the UI thread.
pub async fn resolve(appid: i64, cache_dir: PathBuf) -> ArtSet {
    let dir = cache_dir.join("artwork").join(appid.to_string());
    let _ = std::fs::create_dir_all(&dir);
    let hero = asset(appid, "library_hero.jpg", &dir).await;
    let cover = asset(appid, "library_600x900.jpg", &dir).await;
    let logo = asset(appid, "logo.png", &dir).await;
    // Decode/blur/color work is CPU-bound - never run it on the UI executor.
    // The `-vN` suffix invalidates any backdrop baked by an older recipe.
    let backdrop_dest = dir.join("backdrop-v3.png");
    let hero_cpu = hero.clone();
    let (backdrop, swatches) = tokio::task::spawn_blocking(move || {
        let backdrop = hero_cpu
            .as_deref()
            .and_then(|h| bake_backdrop(h, &backdrop_dest));
        let swatches = hero_cpu
            .as_deref()
            .map(compute_swatches)
            .unwrap_or_default();
        (backdrop, swatches)
    })
    .await
    .unwrap_or_default();
    ArtSet {
        cover,
        logo,
        backdrop,
        swatches,
    }
}

/// Locate one asset: Steam's local cache first, else the public CDN (fetched
/// once and mirrored under the cache dir).
async fn asset(appid: i64, file: &str, dir: &Path) -> Option<PathBuf> {
    for root in modrix_core::detect::steam_roots() {
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
        .resize_exact(BACKDROP_W, h, FilterType::Lanczos3)
        .to_rgba8();
    // A smooth Gaussian blur: blur at reduced resolution (fast) then upscale
    // with Lanczos3 - no blocky downscale-upscale artifacts.
    let bw = (BACKDROP_W / 3).max(1);
    let bh = (h / 3).max(1);
    let down = img.resize_exact(bw, bh, FilterType::Lanczos3).to_rgba8();
    let blurred_small = imageops::blur(&down, 5.0);
    let blurred = imageops::resize(&blurred_small, BACKDROP_W, h, FilterType::Lanczos3);

    let mut out = RgbaImage::new(BACKDROP_W, h);
    for y in 0..h {
        #[expect(clippy::cast_precision_loss, reason = "row index is small")]
        let t = y as f32 / h as f32;
        // Sharp only in the thin top strip behind the header; fully blurred
        // above where any content card begins, so a card never straddles the
        // sharp→blur transition (which would look like a gradient through it).
        let blur_mix = smoothstep(0.0, 0.11, t);
        let alpha = 1.0 - smoothstep(0.34, 0.96, t); // fade out toward the bottom
        // A vignette on the *background*: crisp top stays bright, the body dims
        // down. Content cards are frosted-opaque, so this fade shows only in
        // the background itself, not through the cards.
        let dim = 0.82 - 0.44 * smoothstep(0.10, 0.66, t);
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
