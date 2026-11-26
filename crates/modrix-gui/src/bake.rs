// SPDX-License-Identifier: GPL-2.0-only
//! The backdrop, baked once per game into one image.
//!
//! The view used to stack five full-window layers: a soft wash of the hero,
//! an accent glow, the sharp hero, a fade into the base colour and a tint.
//! The renderer repaints every layer on every frame, which cost this laptop's
//! integrated GPU about 18 ms a frame at 2560x1600 (a 240 Hz frame is 4.2 ms).
//! Here the same layers are flattened on the art worker, in linear light as
//! the GPU blends them, into one band the view draws as a single image.
//!
//! Below the hero band the old stack resolved to the plain base colour, so
//! the band ends in exactly that colour and the window's own fill carries on.

use rune::Color;
use rune::color::{srgb_decode, srgb_encode};

use crate::art::Rgba;
use crate::look::Look;

/// The band's width in points at the reference window width.
const REF_W: f32 = 1600.0;
/// The band's height in points (the old hero band).
const BAND_H: f32 = 440.0;
/// The reference window height the wash and glow were laid out over.
const REF_WINDOW_H: f32 = 1000.0;
/// Device pixels per point in the bake.
const SCALE: f32 = 1.2;
/// Width over height, for the view's aspect lock.
pub const RATIO: f32 = REF_W / BAND_H;
/// The glow's radius in points and its centre as fractions of the window.
const GLOW_R: f32 = 900.0;
const GLOW_AT: (f32, f32) = (0.85, 0.05);

/// What a band is made from.
pub struct Sources<'a> {
    /// The sharp hero.
    pub hero: Option<&'a Rgba>,
    /// The blurred, dimmed hero.
    pub soft: Option<&'a Rgba>,
    /// The art's swatches, linear colour.
    pub swatches: &'a [Color],
}

/// The layer strengths for one appearance and screen kind.
#[derive(Clone, Copy)]
struct Mix {
    base: [f32; 3],
    accent: [f32; 3],
    wash: f32,
    glow: f32,
    hero: f32,
    fade_top: f32,
}

impl Mix {
    fn new(swatches: &[Color], dark: bool, calm: bool) -> Mix {
        let look = Look::new(dark, swatches);
        // Work screens keep the art quieter so tables and text stay readable.
        Mix {
            base: rgb(look.base),
            accent: rgb(look.accent),
            wash: if dark { 0.55 } else { 0.30 },
            glow: if dark { 0.14 } else { 0.08 },
            hero: match (dark, calm) {
                (true, false) => 0.92,
                (true, true) => 0.55,
                (false, false) => 0.75,
                (false, true) => 0.40,
            },
            fade_top: if calm { 0.55 } else { 0.10 },
        }
    }
}

/// The four bands, keyed `(dark, calm)`.
#[must_use]
pub fn all(src: &Sources) -> Vec<((bool, bool), Rgba)> {
    let decode = decode_table();
    [(true, false), (true, true), (false, false), (false, true)]
        .into_iter()
        .map(|(dark, calm)| {
            let mix = Mix::new(src.swatches, dark, calm);
            ((dark, calm), band(src, &decode, mix))
        })
        .collect()
}

fn band(src: &Sources, decode: &[f32; 256], mix: Mix) -> Rgba {
    let width = to_px(REF_W * SCALE);
    let height = to_px(BAND_H * SCALE);
    let mut out = Vec::with_capacity(len_of(width, height));
    let wash = src.soft.map(|img| Cover::new(img, REF_W, REF_WINDOW_H));
    let hero = src.hero.map(|img| Cover::new(img, REF_W, BAND_H));
    for py in 0..height {
        let at_y = (f32::from(u16::try_from(py).unwrap_or(u16::MAX)) + 0.5) / SCALE;
        // By row, so the last row is exactly the base colour (no seam).
        let rows = dim(height.saturating_sub(1));
        let fade = fade_alpha(
            mix.fade_top,
            f32::from(u16::try_from(py).unwrap_or(u16::MAX)) / rows,
        );
        for px in 0..width {
            let at_x = (f32::from(u16::try_from(px).unwrap_or(u16::MAX)) + 0.5) / SCALE;
            let mut color = mix.base;
            if let Some(wash) = &wash {
                color = lerp(color, wash.sample(decode, at_x, at_y), mix.wash);
            }
            color = lerp(color, mix.accent, mix.glow * glow_falloff(at_x, at_y));
            if let Some(hero) = &hero {
                color = lerp(color, hero.sample(decode, at_x, at_y), mix.hero);
            }
            color = lerp(color, mix.base, fade);
            out.extend(color.map(encode));
            out.push(255);
        }
    }
    (width, height, out)
}

/// The fade's alpha at `t` (0 top, 1 bottom of the band).
fn fade_alpha(top: f32, t: f32) -> f32 {
    let mid = top.max(0.45);
    if t <= 0.45 {
        top + (mid - top) * (t / 0.45)
    } else {
        mid + (1.0 - mid) * ((t - 0.45) / 0.55).min(1.0)
    }
}

/// The glow's strength at a point: 1 at its centre, 0 at its radius.
fn glow_falloff(x: f32, y: f32) -> f32 {
    let dx = x - GLOW_AT.0 * REF_W;
    let dy = y - GLOW_AT.1 * REF_WINDOW_H;
    (1.0 - (dx * dx + dy * dy).sqrt() / GLOW_R).max(0.0)
}

/// An image scaled to cover a rect (cropping, never squishing), centred.
struct Cover<'a> {
    img: &'a Rgba,
    scale: f32,
    ox: f32,
    oy: f32,
}

impl<'a> Cover<'a> {
    fn new(img: &'a Rgba, w: f32, h: f32) -> Cover<'a> {
        let (iw, ih) = (dim(img.0), dim(img.1));
        let scale = (w / iw).max(h / ih);
        Cover {
            img,
            scale,
            ox: (w - iw * scale) / 2.0,
            oy: (h - ih * scale) / 2.0,
        }
    }

    /// Bilinear sample at point `(x, y)` of the rect, linear colour.
    fn sample(&self, decode: &[f32; 256], x: f32, y: f32) -> [f32; 3] {
        let fx = ((x - self.ox) / self.scale - 0.5).max(0.0);
        let fy = ((y - self.oy) / self.scale - 0.5).max(0.0);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let at = |dx: f32, dy: f32| texel(self.img, decode, x0 + dx, y0 + dy);
        let top = lerp(at(0.0, 0.0), at(1.0, 0.0), tx);
        let bottom = lerp(at(0.0, 1.0), at(1.0, 1.0), tx);
        lerp(top, bottom, ty)
    }
}

/// One texel, clamped to the image, as linear colour.
fn texel(img: &Rgba, decode: &[f32; 256], x: f32, y: f32) -> [f32; 3] {
    let (w, h, bytes) = img;
    let col = to_px(x).min(w.saturating_sub(1));
    let row = to_px(y).min(h.saturating_sub(1));
    let at = len_of(*w, row).saturating_add(len_of(col, 1));
    let channel = |i: usize| {
        bytes
            .get(at.saturating_add(i))
            .and_then(|b| decode.get(usize::from(*b)))
            .copied()
            .unwrap_or(0.0)
    };
    [channel(0), channel(1), channel(2)]
}

fn decode_table() -> [f32; 256] {
    std::array::from_fn(|i| srgb_decode(f32::from(u8::try_from(i).unwrap_or(u8::MAX)) / 255.0))
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn rgb(c: Color) -> [f32; 3] {
    [c.r, c.g, c.b]
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to [0,255] before the cast"
)]
fn encode(v: f32) -> u8 {
    (srgb_encode(v.clamp(0.0, 1.0)) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to [0, 65535] before the cast"
)]
fn to_px(v: f32) -> u32 {
    v.clamp(0.0, 65_535.0) as u32
}

#[expect(clippy::cast_precision_loss, reason = "image sizes are far below 2^24")]
fn dim(v: u32) -> f32 {
    v.max(1) as f32
}

/// `w * h * 4` bytes, saturating.
fn len_of(w: u32, h: u32) -> usize {
    usize::try_from(u64::from(w).saturating_mul(u64::from(h)).saturating_mul(4)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, v: u8) -> Rgba {
        (w, h, vec![v; len_of(w, h)])
    }

    #[test]
    fn bands_keep_the_aspect_and_end_in_the_base_colour() {
        let hero = flat(64, 20, 200);
        let soft = flat(16, 10, 90);
        let src = Sources {
            hero: Some(&hero),
            soft: Some(&soft),
            swatches: &[],
        };
        let bands = all(&src);
        assert_eq!(bands.len(), 4);
        for ((dark, _), (w, h, bytes)) in &bands {
            assert_eq!(bytes.len(), len_of(*w, *h));
            let base = rgb(Look::new(*dark, &[]).base).map(encode);
            let last = len_of(*w, h.saturating_sub(1));
            let row = bytes.get(last..last.saturating_add(3));
            assert_eq!(row, Some(&base[..]), "the band must end in the base colour");
        }
    }

    #[test]
    fn the_fade_runs_from_its_top_to_opaque() {
        assert!((fade_alpha(0.1, 0.0) - 0.1).abs() < 1e-6);
        assert!((fade_alpha(0.1, 1.0) - 1.0).abs() < 1e-6);
        assert!(fade_alpha(0.55, 0.3) >= 0.55);
    }
}
