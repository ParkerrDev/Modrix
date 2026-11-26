// SPDX-License-Identifier: GPL-2.0-only
//! The visual language: a kit palette seeded from the game's own art, plus
//! the glass, type and spacing tokens the screens share.

use rune::{Color, TextStyle, oklch, oklcha};
use rune_kit::{Kit, PaletteSeed};

/// Hue of the default accent (a clear blue, as in the brand mark).
const DEFAULT_HUE: f32 = 256.0;

/// Everything a screen needs to paint itself.
#[derive(Clone)]
pub struct Look {
    /// The component kit.
    pub k: Kit,
    /// Dark appearance.
    pub dark: bool,
    /// The accent.
    pub accent: Color,
    /// A wash of the accent for selected rows and chips.
    pub accent_soft: Color,
    /// Text on the accent.
    pub on_accent: Color,
    /// Translucent panel fill over the backdrop.
    pub glass: Color,
    /// A stronger panel fill (popovers, sticky headers).
    pub glass_strong: Color,
    /// Panel edge.
    pub edge: Color,
    /// The window colour under everything.
    pub base: Color,
    /// Positive state.
    pub good: Color,
    /// Warning state.
    pub warn: Color,
    /// Error state.
    pub bad: Color,
}

/// Type scale.
pub mod ty {
    use super::TextStyle;
    /// The big page title.
    pub const TITLE: TextStyle = TextStyle::new(26.0, 680.0).tracking(-0.4);
    /// Hero name over the art.
    pub const HERO: TextStyle = TextStyle::new(34.0, 720.0).tracking(-0.6);
    /// A section heading.
    pub const HEADING: TextStyle = TextStyle::new(15.0, 640.0).tracking(-0.1);
    /// A large number on a stat tile.
    pub const STAT: TextStyle = TextStyle::new(28.0, 680.0).tracking(-0.5);
    /// Body text.
    pub const BODY: TextStyle = TextStyle::new(13.5, 450.0);
    /// Body, emphasised.
    pub const BODY_STRONG: TextStyle = TextStyle::new(13.5, 580.0);
    /// Secondary lines.
    pub const SMALL: TextStyle = TextStyle::new(12.0, 450.0);
    /// Secondary, emphasised.
    pub const SMALL_STRONG: TextStyle = TextStyle::new(12.0, 600.0);
    /// Uppercase labels.
    pub const LABEL: TextStyle = TextStyle::new(10.5, 650.0).tracking(0.9);
    /// The wordmark.
    pub const MARK: TextStyle = TextStyle::new(19.0, 800.0).tracking(1.6);
    /// Monospace values.
    pub const MONO: TextStyle = TextStyle::new(12.0, 450.0).mono();
}

/// Spacing scale.
pub mod space {
    /// Tight gaps.
    pub const SM: f32 = 8.0;
    /// Default gaps.
    pub const MD: f32 = 12.0;
    /// Section gaps.
    pub const LG: f32 = 20.0;
    /// Page padding.
    pub const XL: f32 = 28.0;
    /// Panel radius.
    pub const PANEL: f32 = 16.0;
    /// The sidebar width.
    pub const SIDEBAR: f32 = 248.0;
}

/// The dark value or the light one.
const fn pick<T: Copy>(dark: bool, on_dark: T, on_light: T) -> T {
    if dark { on_dark } else { on_light }
}

impl Look {
    /// The look for an appearance, tinted by the game's art swatches.
    #[must_use]
    pub fn new(dark: bool, swatches: &[Color]) -> Look {
        let hue = swatches.first().map_or(DEFAULT_HUE, |c| hue_of(*c));
        let chroma = swatches
            .first()
            .map_or(0.16, |c| chroma_of(*c).clamp(0.09, 0.17));
        let accent = pick(dark, oklch(0.72, chroma, hue), oklch(0.56, chroma, hue));
        let base = pick(dark, oklch(0.155, 0.014, hue), oklch(0.972, 0.006, hue));
        let fg = pick(dark, oklch(0.96, 0.004, hue), oklch(0.22, 0.012, hue));
        let mut k = Kit::from_palette(PaletteSeed {
            bg: base,
            fg,
            accent,
        });
        k.c.bg_accent = accent;
        k.c.border_focus = accent;
        k.c.text_accent = accent;
        k.c.icon_accent = accent;
        Look {
            k,
            dark,
            accent,
            accent_soft: oklcha(pick(dark, 0.72, 0.56), chroma, hue, pick(dark, 0.16, 0.12)),
            on_accent: pick(dark, oklch(0.17, 0.02, hue), Color::WHITE),
            glass: pick(
                dark,
                oklcha(0.21, 0.016, hue, 0.80),
                oklcha(0.995, 0.004, hue, 0.84),
            ),
            glass_strong: pick(
                dark,
                oklcha(0.20, 0.016, hue, 0.90),
                oklcha(0.99, 0.004, hue, 0.94),
            ),
            edge: pick(dark, Color::WHITE.alpha(0.075), Color::BLACK.alpha(0.07)),
            base,
            good: pick(dark, oklch(0.76, 0.15, 150.0), oklch(0.55, 0.15, 150.0)),
            warn: pick(dark, oklch(0.82, 0.14, 80.0), oklch(0.62, 0.15, 70.0)),
            bad: pick(dark, oklch(0.70, 0.17, 25.0), oklch(0.56, 0.19, 25.0)),
        }
    }

    /// Primary text.
    #[must_use]
    pub fn text(&self) -> Color {
        self.k.c.text_base
    }

    /// Secondary text.
    #[must_use]
    pub fn muted(&self) -> Color {
        self.k.c.text_muted
    }

    /// Tertiary text.
    #[must_use]
    pub fn faint(&self) -> Color {
        self.k.c.text_faint
    }

    /// A hover wash.
    #[must_use]
    pub fn hover(&self) -> Color {
        if self.dark {
            Color::WHITE.alpha(0.055)
        } else {
            Color::BLACK.alpha(0.045)
        }
    }

    /// A pressed wash.
    #[must_use]
    pub fn pressed(&self) -> Color {
        if self.dark {
            Color::WHITE.alpha(0.09)
        } else {
            Color::BLACK.alpha(0.08)
        }
    }

    /// The drop shadow under floating panels.
    #[must_use]
    pub fn shadow(&self) -> Color {
        Color::BLACK.alpha(if self.dark { 0.42 } else { 0.12 })
    }
}

fn lch(c: Color) -> (f32, f32, f32) {
    let [l, a, b, _] = c.to_oklab();
    (l, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
}

fn hue_of(c: Color) -> f32 {
    lch(c).2
}

fn chroma_of(c: Color) -> f32 {
    lch(c).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_follows_the_art() {
        let warm = crate::art::srgb(0.9, 0.45, 0.1);
        let look = Look::new(true, &[warm]);
        let (_, _, hue) = lch(look.accent);
        assert!((30.0..90.0).contains(&hue), "hue {hue}");
        let plain = Look::new(false, &[]);
        let (_, _, blue) = lch(plain.accent);
        assert!((blue - DEFAULT_HUE).abs() < 3.0, "hue {blue}");
    }
}
