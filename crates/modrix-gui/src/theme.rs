// SPDX-License-Identifier: GPL-2.0-only
//! The visual identity: switchable theme specs behind one style API.
//!
//! Two themes ship: **Aurora** (the default - dark glass surfaces with a
//! cyan→indigo→violet gradient accent, translucent window) and **Gold** (the
//! original graphite + muted-gold look, banked verbatim). Every widget style
//! lives here so the views stay purely structural; views read colors through
//! accessor functions (`accent()`, `muted()`, …) and never hold a palette,
//! so switching themes restyles the whole application at once.

use std::sync::RwLock;

use iced::gradient::Linear;
use iced::theme::Palette;
use iced::widget::{button, container, pick_list, progress_bar, text_input, toggler};
use iced::{Background, Border, Color, Gradient, Radians, Shadow, Theme};

/// Build a solid [`Color`] from a `0xRRGGBB` literal.
#[expect(
    clippy::cast_precision_loss,
    reason = "channel values are <= 255 and exactly representable in f32"
)]
const fn hex(rgb: u32) -> Color {
    Color {
        r: ((rgb >> 16) & 0xFF) as f32 / 255.0,
        g: ((rgb >> 8) & 0xFF) as f32 / 255.0,
        b: (rgb & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

/// `color` at `alpha` opacity.
const fn faded(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

/// One complete visual identity.
pub struct ThemeSpec {
    /// Stable id persisted in settings (`aurora`, `gold`).
    pub id: &'static str,
    /// Display name for the theme picker.
    pub name: &'static str,
    /// Window background.
    pub bg: Color,
    /// Sidebar background.
    pub surface: Color,
    /// Card / table background.
    pub card: Color,
    /// Slightly raised card (hover, inset fields).
    pub card_hi: Color,
    /// Hairline borders.
    pub hairline: Color,
    /// Primary text.
    pub text: Color,
    /// Secondary text.
    pub muted: Color,
    /// Tertiary text (labels, hints).
    pub faint: Color,
    /// The accent color (buttons, highlights, active nav).
    pub accent: Color,
    /// The accent's hover shift.
    pub accent_hot: Color,
    /// Text on an accent-filled control.
    pub on_accent: Color,
    /// The accent gradient stops (primary buttons, progress, active nav).
    pub gradient: [Color; 3],
    /// Success green.
    pub ok: Color,
    /// Danger red.
    pub danger: Color,
    /// Informational blue.
    pub info: Color,
    /// Window-background alpha: < 1.0 = glassmorphism (the compositor shows
    /// through); 1.0 = opaque.
    pub glass: f32,
}

/// Aurora: dark glass + a cyan→indigo→violet sweep. The default.
pub const AURORA: ThemeSpec = ThemeSpec {
    id: "aurora",
    name: "Aurora (glass)",
    bg: hex(0x0B_0E14),
    surface: hex(0x0F_1420),
    card: hex(0x13_1A28),
    card_hi: hex(0x1A_2233),
    hairline: hex(0x24_2C40),
    text: hex(0xF0_F3F9),
    muted: hex(0xAF_B7CB),
    faint: hex(0x78_8098),
    accent: hex(0x22_D3EE),
    accent_hot: hex(0x7D_D3FC),
    on_accent: hex(0x06_1B22),
    gradient: [hex(0x22_D3EE), hex(0x63_66F1), hex(0xA8_55F7)],
    ok: hex(0x34_D399),
    danger: hex(0xF0_6A5E),
    info: hex(0x7D_A7E0),
    glass: 0.86,
};

/// Gold: the original graphite + muted-gold identity, kept selectable.
pub const GOLD: ThemeSpec = ThemeSpec {
    id: "gold",
    name: "Gold (classic)",
    bg: hex(0x11_1216),
    surface: hex(0x16_171C),
    card: hex(0x1C_1E24),
    card_hi: hex(0x23_252D),
    hairline: hex(0x2A_2D36),
    text: hex(0xE8_E6E1),
    muted: hex(0x8E_93A2),
    faint: hex(0x5D_6373),
    accent: hex(0xD9_A65A),
    accent_hot: hex(0xE4_B36B),
    on_accent: hex(0x1A_1408),
    gradient: [hex(0xD9_A65A), hex(0xD9_A65A), hex(0xE4_B36B)],
    ok: hex(0x8F_B573),
    danger: hex(0xCC_5F56),
    info: hex(0x7F_A6C9),
    glass: 1.0,
};

/// Every selectable theme, in picker order.
pub const ALL: [&ThemeSpec; 2] = [&AURORA, &GOLD];

/// The live theme. A lock (not a const) so Settings can switch at runtime;
/// contention is nil (writes happen on a click).
static ACTIVE: RwLock<&'static ThemeSpec> = RwLock::new(&AURORA);

/// The active theme spec.
pub fn spec() -> &'static ThemeSpec {
    ACTIVE.read().map_or(&AURORA, |guard| *guard)
}

/// Switch the active theme by id (unknown ids keep the current theme).
pub fn set_theme(id: &str) {
    if let Some(found) = ALL.iter().find(|s| s.id == id)
        && let Ok(mut active) = ACTIVE.write()
    {
        *active = found;
    }
}

// --- per-game accent (derived from the selected game's artwork) -------------

/// An accent palette derived from a game's artwork. When one is set it
/// overrides the theme's own accent everywhere, so the whole UI takes on the
/// colors of the game being modded.
#[derive(Debug, Clone, Copy)]
pub struct GameAccent {
    /// The primary accent.
    accent: Color,
    /// Its hover shift.
    accent_hot: Color,
    /// Text drawn on an accent fill.
    on_accent: Color,
    /// Three stops for accent gradients (a gentle same-family sheen).
    gradient: [Color; 3],
}

impl GameAccent {
    /// Build a palette from an image's swatches (most representative first),
    /// tuned so it always reads as a vivid accent on a dark surface. `None`
    /// when the art had no usable color (the theme keeps its own accent).
    #[must_use]
    pub fn from_swatches(swatches: &[Color]) -> Option<Self> {
        let accent = fit_for_dark(*swatches.first()?);
        let accent_hot = lighten(accent, 0.14);
        let on_accent = if luminance(accent) > 0.55 {
            Color::from_rgb(0.05, 0.06, 0.08)
        } else {
            Color::from_rgb(0.97, 0.98, 1.0)
        };
        // A subtle sheen, not a rainbow: accent → a second art swatch (or a
        // small hue shift of the accent) → the hover tint.
        let second = swatches
            .get(1)
            .copied()
            .map_or_else(|| shift_hue(accent, 0.05), fit_for_dark);
        let gradient = [accent, mix(accent, second, 0.6), accent_hot];
        Some(Self {
            accent,
            accent_hot,
            on_accent,
            gradient,
        })
    }
}

/// The active per-game accent, if any.
static GAME_ACCENT: RwLock<Option<GameAccent>> = RwLock::new(None);

/// Set (or clear) the per-game accent. Cleared = the theme's own accent.
pub fn set_game_accent(accent: Option<GameAccent>) {
    if let Ok(mut guard) = GAME_ACCENT.write() {
        *guard = accent;
    }
}

fn game_accent() -> Option<GameAccent> {
    GAME_ACCENT.read().ok().and_then(|g| *g)
}

/// The effective accent: the game's, else the theme's.
fn eff_accent() -> Color {
    game_accent().map_or_else(|| spec().accent, |g| g.accent)
}
fn eff_accent_hot() -> Color {
    game_accent().map_or_else(|| spec().accent_hot, |g| g.accent_hot)
}
fn eff_on_accent() -> Color {
    game_accent().map_or_else(|| spec().on_accent, |g| g.on_accent)
}
fn eff_gradient() -> [Color; 3] {
    game_accent().map_or_else(|| spec().gradient, |g| g.gradient)
}

/// Nudge a color into the vivid-but-readable range for a dark surface.
fn fit_for_dark(c: Color) -> Color {
    let (h, s, l) = crate::artwork::rgb_to_hsl(c.r, c.g, c.b);
    crate::artwork::hsl_to_rgb(h, s.clamp(0.45, 0.92), l.clamp(0.52, 0.68))
}

fn shift_hue(c: Color, delta: f32) -> Color {
    let (h, s, l) = crate::artwork::rgb_to_hsl(c.r, c.g, c.b);
    crate::artwork::hsl_to_rgb((h + delta).rem_euclid(1.0), s, l)
}

fn lighten(c: Color, amount: f32) -> Color {
    let (h, s, l) = crate::artwork::rgb_to_hsl(c.r, c.g, c.b);
    crate::artwork::hsl_to_rgb(h, s, (l + amount).min(0.92))
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgb(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
    )
}

fn luminance(c: Color) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

// --- color accessors (views read these, never a palette) --------------------

/// Primary text.
pub fn text() -> Color {
    spec().text
}
/// Secondary text.
pub fn muted() -> Color {
    spec().muted
}
/// Tertiary text.
pub fn faint() -> Color {
    spec().faint
}
/// Hairline borders.
pub fn hairline_color() -> Color {
    spec().hairline
}
/// The accent.
pub fn accent() -> Color {
    eff_accent()
}
/// Success green.
pub fn ok() -> Color {
    spec().ok
}
/// Danger red.
pub fn danger() -> Color {
    spec().danger
}
/// Informational blue.
pub fn info() -> Color {
    spec().info
}

/// The accent gradient as a background (top-left → bottom-right sweep).
fn gradient_bg() -> Background {
    let [a, b, c] = eff_gradient();
    Background::Gradient(Gradient::Linear(
        Linear::new(Radians(std::f32::consts::FRAC_PI_4 * 3.0))
            .add_stop(0.0, a)
            .add_stop(0.5, b)
            .add_stop(1.0, c),
    ))
}

/// The accent gradient, faded to `alpha` (selections, active nav).
fn gradient_bg_faded(alpha: f32) -> Background {
    let [a, b, c] = eff_gradient();
    Background::Gradient(Gradient::Linear(
        Linear::new(Radians(std::f32::consts::FRAC_PI_4 * 3.0))
            .add_stop(0.0, faded(a, alpha))
            .add_stop(0.5, faded(b, alpha))
            .add_stop(1.0, faded(c, alpha)),
    ))
}

/// The application [`Theme`] for the active spec.
pub fn app_theme() -> Theme {
    let s = spec();
    Theme::custom(
        format!("Modrix {}", s.name),
        Palette {
            background: s.bg,
            text: s.text,
            primary: eff_accent(),
            success: s.ok,
            danger: s.danger,
        },
    )
}

/// The window clear color: translucent for glass themes (the compositor
/// shows through), opaque otherwise.
pub fn window_background() -> Color {
    let s = spec();
    faded(s.bg, s.glass)
}

fn rounded(radius: f32) -> Border {
    Border {
        color: Color::TRANSPARENT,
        width: 0.0,
        radius: radius.into(),
    }
}

/// Translucent surface tint for glass themes; opaque for solid themes, so the
/// banked Gold look is unaffected.
fn glass(color: Color, alpha: f32) -> Color {
    if spec().glass < 1.0 {
        faded(color, alpha)
    } else {
        color
    }
}

/// A frosted-glass rim: a light top-edge highlight for glass themes (the
/// specular line on a pane of glass), a dark hairline for solid themes.
fn glass_border(radius: f32) -> Border {
    let s = spec();
    let color = if s.glass < 1.0 {
        faded(s.text, 0.20)
    } else {
        s.hairline
    };
    Border {
        color,
        width: 1.0,
        radius: radius.into(),
    }
}

/// The left navigation column - a solid-ish glass so the nav stays legible
/// against the (transparent) window edge.
pub fn sidebar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(glass(spec().surface, 0.80))),
        border: Border {
            color: spec().hairline,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// A raised content card - a **frosted** panel: a single flat tint (only
/// lightly translucent, so the background's top-to-bottom fade does NOT bleed
/// through and appear as a gradient on the card), a bright glass rim, and a
/// soft shadow for depth.
pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(glass(spec().card, 0.90))),
        border: glass_border(14.0),
        shadow: Shadow {
            color: faded(Color::BLACK, 0.40),
            offset: iced::Vector::new(0.0, 6.0),
            blur_radius: 22.0,
        },
        ..container::Style::default()
    }
}

/// An inset well inside a card (image frames, token boxes, empty states).
pub fn inset(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(glass(spec().bg, 0.72))),
        border: glass_border(10.0),
        ..container::Style::default()
    }
}

/// A table row; `even` rows get a faint stripe. Rows are near-transparent so
/// the card's own glass shows through as one continuous surface.
pub fn table_row(even: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: even.then(|| Background::Color(glass(spec().card_hi, 0.35))),
        border: rounded(8.0),
        ..container::Style::default()
    }
}

/// A selected (highlighted) table row.
pub fn table_row_selected(_: &Theme) -> container::Style {
    container::Style {
        background: Some(gradient_bg_faded(0.24)),
        border: Border {
            color: faded(eff_accent(), 0.6),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// The row currently being dragged in the load order.
pub fn table_row_dragging(_: &Theme) -> container::Style {
    container::Style {
        background: Some(gradient_bg_faded(0.22)),
        border: Border {
            color: eff_accent(),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// The click/drop target for adding mod archives.
pub fn drop_zone(_: &Theme) -> container::Style {
    container::Style {
        background: Some(gradient_bg_faded(0.05)),
        border: Border {
            color: faded(eff_accent(), 0.35),
            width: 1.0,
            radius: 12.0.into(),
        },
        ..container::Style::default()
    }
}

/// The dimmed backdrop behind modal overlays.
pub fn backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(faded(Color::BLACK, 0.6))),
        ..container::Style::default()
    }
}

/// The notification popup - a frosted glass panel. It floats top-right over
/// the blurred part of the backdrop, so its translucency reads as real
/// frosted glass (the softness comes from the already-blurred art behind it),
/// finished with a bright glass rim and a deep shadow. (iced 0.13 cannot blur
/// arbitrary content behind a widget, so over solid UI it is simply
/// translucent, not a true backdrop-blur.)
pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(glass(spec().card_hi, 0.82))),
        border: glass_border(12.0),
        shadow: Shadow {
            color: faded(Color::BLACK, 0.5),
            offset: iced::Vector::new(0.0, 8.0),
            blur_radius: 28.0,
        },
        ..container::Style::default()
    }
}

/// A small tinted status chip.
pub fn chip(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        text_color: Some(color),
        background: Some(Background::Color(faded(color, 0.12))),
        border: Border {
            color: faded(color, 0.35),
            width: 1.0,
            radius: 99.0.into(),
        },
        ..container::Style::default()
    }
}

/// A sidebar navigation entry; the active one carries the accent gradient.
pub fn nav(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let s = spec();
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: if active {
                Some(gradient_bg_faded(0.16))
            } else if hovered {
                Some(Background::Color(faded(s.text, 0.05)))
            } else {
                None
            },
            text_color: if active { eff_accent() } else { s.muted },
            border: rounded(8.0),
            ..button::Style::default()
        }
    }
}

/// A selectable list card that stands on its own over the backdrop (the game
/// rows on the Games screen) - a frosted-opaque panel so the background never
/// bleeds through, marked when selected by an accent rim and tint.
pub fn list_card(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let s = spec();
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let base = if selected {
            mix(s.card, eff_accent(), 0.12)
        } else if hovered {
            s.card_hi
        } else {
            s.card
        };
        button::Style {
            background: Some(Background::Color(glass(base, 0.92))),
            text_color: s.text,
            border: if selected {
                Border {
                    color: faded(eff_accent(), 0.6),
                    width: 1.0,
                    radius: 12.0.into(),
                }
            } else {
                glass_border(12.0)
            },
            ..button::Style::default()
        }
    }
}

/// The filled accent button for the primary action - the gradient sweep.
pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => {
            Some(Background::Color(eff_accent_hot()))
        }
        button::Status::Disabled => Some(gradient_bg_faded(0.3)),
        button::Status::Active => Some(gradient_bg()),
    };
    button::Style {
        background,
        text_color: eff_on_accent(),
        border: rounded(8.0),
        ..button::Style::default()
    }
}

/// A quiet, bordered secondary button.
pub fn ghost(_: &Theme, status: button::Status) -> button::Style {
    outlined(spec().text, spec().muted, status)
}

/// A quiet, bordered destructive button.
pub fn danger_ghost(_: &Theme, status: button::Status) -> button::Style {
    outlined(spec().danger, faded(spec().danger, 0.8), status)
}

fn outlined(hot: Color, idle: Color, status: button::Status) -> button::Style {
    let s = spec();
    let glassy = s.glass < 1.0;
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let disabled = matches!(status, button::Status::Disabled);
    // A subtle frosted fill at rest (glass themes) so buttons read as glass
    // chips rather than bare outlines; brighter on hover.
    let background = if disabled {
        None
    } else if hovered {
        Some(Background::Color(faded(hot, 0.16)))
    } else if glassy {
        Some(Background::Color(faded(s.text, 0.07)))
    } else {
        None
    };
    button::Style {
        background,
        text_color: if disabled {
            s.faint
        } else if hovered {
            hot
        } else {
            idle
        },
        border: Border {
            color: if hovered {
                faded(hot, 0.6)
            } else if glassy {
                faded(s.text, 0.16)
            } else {
                s.hairline
            },
            width: 1.0,
            radius: 8.0.into(),
        },
        ..button::Style::default()
    }
}

/// A tiny square icon button (reorder arrows, cancel).
pub fn icon(_: &Theme, status: button::Status) -> button::Style {
    let s = spec();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(faded(s.text, 0.08))),
        text_color: match status {
            button::Status::Disabled => faded(s.faint, 0.4),
            _ if hovered => s.text,
            _ => s.muted,
        },
        border: rounded(6.0),
        ..button::Style::default()
    }
}

/// The slim progress bar - the gradient sweep as the fill.
pub fn progress(_: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(spec().bg),
        bar: gradient_bg(),
        border: rounded(99.0),
    }
}

/// Dark dropdown pickers (game/profile selectors). When open, the field's
/// bottom corners square off so it reads as connected to its menu.
pub fn picker(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    let s = spec();
    let opened = matches!(status, pick_list::Status::Opened);
    let hot = opened || matches!(status, pick_list::Status::Hovered);
    let radius = if opened {
        iced::border::Radius::default().top_left(8).top_right(8)
    } else {
        8.0.into()
    };
    pick_list::Style {
        text_color: s.text,
        placeholder_color: s.faint,
        handle_color: if hot { eff_accent() } else { s.muted },
        background: Background::Color(s.bg),
        border: Border {
            color: if hot {
                faded(eff_accent(), 0.5)
            } else {
                s.hairline
            },
            width: 1.0,
            radius,
        },
    }
}

/// The dropdown menu under an open picker: squared top corners so it visually
/// continues the field above it.
pub fn picker_menu(_: &Theme) -> iced::overlay::menu::Style {
    let s = spec();
    iced::overlay::menu::Style {
        background: Background::Color(s.card_hi),
        border: Border {
            color: faded(eff_accent(), 0.5),
            width: 1.0,
            radius: iced::border::Radius::default()
                .bottom_left(8)
                .bottom_right(8),
        },
        text_color: s.text,
        selected_text_color: eff_accent(),
        selected_background: Background::Color(faded(eff_accent(), 0.12)),
    }
}

/// The enable/disable switch on each mod row.
pub fn toggle(_: &Theme, status: toggler::Status) -> toggler::Style {
    let s = spec();
    let on = match status {
        toggler::Status::Active { is_toggled } | toggler::Status::Hovered { is_toggled } => {
            is_toggled
        }
        toggler::Status::Disabled => false,
    };
    toggler::Style {
        background: if on { eff_accent() } else { s.hairline },
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: if on { s.bg } else { s.muted },
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
    }
}

/// Dark text inputs matching the inset wells.
pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let s = spec();
    let focused = matches!(status, text_input::Status::Focused);
    text_input::Style {
        background: Background::Color(s.bg),
        border: Border {
            color: if focused {
                faded(eff_accent(), 0.6)
            } else {
                s.hairline
            },
            width: 1.0,
            radius: 8.0.into(),
        },
        icon: s.muted,
        placeholder: s.faint,
        value: s.text,
        selection: faded(eff_accent(), 0.35),
    }
}
