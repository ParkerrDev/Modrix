// SPDX-License-Identifier: GPL-2.0-only
//! The visual identity: a restrained, modern dark palette.
//!
//! Where Vortex leans on a loud orange, ModManager uses graphite surfaces,
//! hairline borders, and a muted gold accent - the classy version. Every
//! widget style lives here so the views stay purely structural.

use iced::theme::Palette;
use iced::widget::{button, container, pick_list, progress_bar, text_input, toggler};
use iced::{Background, Border, Color, Shadow, Theme};

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

/// Window background.
pub const BG: Color = hex(0x11_1216);
/// Sidebar background.
pub const SURFACE: Color = hex(0x16_171C);
/// Card / table background.
pub const CARD: Color = hex(0x1C_1E24);
/// Slightly raised card (hover, inset fields).
pub const CARD_HI: Color = hex(0x23_252D);
/// Hairline borders.
pub const HAIRLINE: Color = hex(0x2A_2D36);
/// Primary text.
pub const TEXT: Color = hex(0xE8_E6E1);
/// Secondary text.
pub const MUTED: Color = hex(0x8E_93A2);
/// Tertiary text (labels, hints).
pub const FAINT: Color = hex(0x5D_6373);
/// The accent: muted gold.
pub const ACCENT: Color = hex(0xD9_A65A);
/// Success green.
pub const OK: Color = hex(0x8F_B573);
/// Danger red.
pub const DANGER: Color = hex(0xCC_5F56);
/// Informational blue-grey.
pub const INFO: Color = hex(0x7F_A6C9);

/// The application [`Theme`], built once at startup.
pub fn app_theme() -> Theme {
    Theme::custom(
        "ModManager".to_owned(),
        Palette {
            background: BG,
            text: TEXT,
            primary: ACCENT,
            success: OK,
            danger: DANGER,
        },
    )
}

fn rounded(radius: f32) -> Border {
    Border {
        color: Color::TRANSPARENT,
        width: 0.0,
        radius: radius.into(),
    }
}

fn hairline(radius: f32) -> Border {
    Border {
        color: HAIRLINE,
        width: 1.0,
        radius: radius.into(),
    }
}

/// The left navigation column.
pub fn sidebar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border {
            color: HAIRLINE,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// A raised content card.
pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(CARD)),
        border: hairline(12.0),
        shadow: Shadow {
            color: faded(Color::BLACK, 0.25),
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    }
}

/// An inset well inside a card (token boxes, empty states).
pub fn inset(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG)),
        border: hairline(8.0),
        ..container::Style::default()
    }
}

/// A table row; `even` rows get a faint stripe.
pub fn table_row(even: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(if even { CARD } else { CARD_HI })),
        border: rounded(8.0),
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

/// A sidebar navigation entry; the active one carries the accent.
pub fn nav(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: if active {
                Some(Background::Color(faded(ACCENT, 0.12)))
            } else if hovered {
                Some(Background::Color(faded(TEXT, 0.05)))
            } else {
                None
            },
            text_color: if active { ACCENT } else { MUTED },
            border: rounded(8.0),
            ..button::Style::default()
        }
    }
}

/// The filled accent button for the primary action.
pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => hex(0xE4_B36B),
        button::Status::Disabled => faded(ACCENT, 0.3),
        button::Status::Active => ACCENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: hex(0x1A_1408),
        border: rounded(8.0),
        ..button::Style::default()
    }
}

/// A quiet, bordered secondary button.
pub fn ghost(_: &Theme, status: button::Status) -> button::Style {
    outlined(TEXT, MUTED, status)
}

/// A quiet, bordered destructive button.
pub fn danger_ghost(_: &Theme, status: button::Status) -> button::Style {
    outlined(DANGER, faded(DANGER, 0.8), status)
}

fn outlined(hot: Color, idle: Color, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let disabled = matches!(status, button::Status::Disabled);
    button::Style {
        background: hovered.then_some(Background::Color(faded(hot, 0.08))),
        text_color: if disabled {
            FAINT
        } else if hovered {
            hot
        } else {
            idle
        },
        border: Border {
            color: if hovered { faded(hot, 0.5) } else { HAIRLINE },
            width: 1.0,
            radius: 8.0.into(),
        },
        ..button::Style::default()
    }
}

/// A tiny square icon button (reorder arrows, cancel).
pub fn icon(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(faded(TEXT, 0.08))),
        text_color: match status {
            button::Status::Disabled => faded(FAINT, 0.4),
            _ if hovered => TEXT,
            _ => MUTED,
        },
        border: rounded(6.0),
        ..button::Style::default()
    }
}

/// The slim download progress bar.
pub fn progress(_: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(BG),
        bar: Background::Color(ACCENT),
        border: rounded(99.0),
    }
}

/// Dark dropdown pickers (game/profile selectors).
pub fn picker(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    let open = matches!(
        status,
        pick_list::Status::Hovered | pick_list::Status::Opened
    );
    pick_list::Style {
        text_color: TEXT,
        placeholder_color: FAINT,
        handle_color: if open { ACCENT } else { MUTED },
        background: Background::Color(BG),
        border: Border {
            color: if open { faded(ACCENT, 0.5) } else { HAIRLINE },
            width: 1.0,
            radius: 8.0.into(),
        },
    }
}

/// The enable/disable switch on each mod row.
pub fn toggle(_: &Theme, status: toggler::Status) -> toggler::Style {
    let on = match status {
        toggler::Status::Active { is_toggled } | toggler::Status::Hovered { is_toggled } => {
            is_toggled
        }
        toggler::Status::Disabled => false,
    };
    toggler::Style {
        background: if on { ACCENT } else { HAIRLINE },
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: if on { BG } else { MUTED },
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
    }
}

/// Dark text inputs matching the inset wells.
pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused);
    text_input::Style {
        background: Background::Color(BG),
        border: Border {
            color: if focused { faded(ACCENT, 0.6) } else { HAIRLINE },
            width: 1.0,
            radius: 8.0.into(),
        },
        icon: MUTED,
        placeholder: FAINT,
        value: TEXT,
        selection: faded(ACCENT, 0.35),
    }
}
