// SPDX-License-Identifier: GPL-2.0-only
//! Small building blocks the screens share: glass panels, headings, stat
//! tiles, chips, empty states.

use rune::{Color, El, Icon, col, el, icon, row, text};

use crate::look::{Look, space, ty};
use crate::state::App;

/// A translucent panel over the backdrop, with a soft blur behind it.
#[must_use]
pub fn panel(look: &Look, key: &str) -> El {
    col()
        .key(key)
        .w_full()
        .radius(space::PANEL)
        .fill(look.glass)
        .border(1.0, look.edge)
        .backdrop_blur(24.0)
        .clip_rounded(space::PANEL)
}

/// A panel with a heading row (title, optional trailing control).
#[must_use]
pub fn section(look: &Look, key: &str, title: &str, trailing: Option<El>, body: El) -> El {
    let head = row()
        .w_full()
        .px(18.0)
        .pt(16.0)
        .pb(10.0)
        .gap(space::SM)
        .center_y()
        .child(text(title).font(ty::HEADING).color(look.text()))
        .child(el().grow(1.0))
        .children(trailing);
    panel(look, key).child(head).child(body)
}

/// The page title block.
#[must_use]
pub fn page_title(look: &Look, title: &str, subtitle: &str) -> El {
    col()
        .gap(3.0)
        .child(text(title).font(ty::TITLE).color(look.text()))
        .child(text(subtitle).font(ty::BODY).color(look.muted()).truncate())
}

/// An icon tinted with a colour.
#[must_use]
pub fn glyph(i: &'static Icon, size: f32, color: Color) -> El {
    icon(i, size).color(color)
}

/// A rounded square holding an icon (stat tiles, list leads).
#[must_use]
pub fn icon_tile(i: &'static Icon, tint: Color, size: f32) -> El {
    el().size(size, size)
        .radius(size * 0.3)
        .fill(tint.alpha(0.16))
        .center()
        .child(glyph(i, size * 0.5, tint))
}

/// A number with a caption, for the overview.
#[must_use]
pub fn stat(
    look: &Look,
    key: &str,
    (i, tint): (&'static Icon, Color),
    value: String,
    caption: &str,
) -> El {
    panel(look, key).grow(1.0).w(0.0).child(
        row()
            .padding(16.0)
            .gap(14.0)
            .center_y()
            .child(icon_tile(i, tint, 40.0))
            .child(
                col()
                    .gap(1.0)
                    .child(text(value).font(ty::STAT).color(look.text()))
                    .child(text(caption).font(ty::SMALL).color(look.muted())),
            ),
    )
}

/// A small coloured pill.
#[must_use]
pub fn chip(label: impl Into<String>, color: Color) -> El {
    row()
        .h(20.0)
        .px(8.0)
        .radius(10.0)
        .fill(color.alpha(0.15))
        .center()
        .child(text(label).font(ty::SMALL_STRONG).color(color))
}

/// A neutral pill.
#[must_use]
pub fn tag(look: &Look, label: impl Into<String>) -> El {
    row()
        .h(20.0)
        .px(8.0)
        .radius(6.0)
        .fill(look.hover())
        .border(1.0, look.edge)
        .center()
        .child(text(label).font(ty::SMALL).color(look.muted()))
}

/// A count badge on the accent.
#[must_use]
pub fn count(look: &Look, n: usize, color: Color) -> El {
    row()
        .h(18.0)
        .min_w(18.0)
        .px(5.0)
        .radius(9.0)
        .fill(color)
        .center()
        .child(
            text(n.to_string())
                .font(ty::SMALL_STRONG)
                .color(look.on_accent),
        )
}

/// A centred placeholder for an empty list.
#[must_use]
pub fn empty(look: &Look, i: &'static Icon, title: &str, body: &str, action: Option<El>) -> El {
    col()
        .w_full()
        .py(48.0)
        .px(24.0)
        .gap(10.0)
        .center()
        .child(icon_tile(i, look.accent, 52.0))
        .child(text(title).font(ty::HEADING).color(look.text()))
        .child(
            text(body)
                .font(ty::BODY)
                .color(look.muted())
                .wrap()
                .max_w(420.0)
                .align(rune::Align::Center),
        )
        .children(action)
}

/// A one-line key/value row (settings, details).
#[must_use]
pub fn kv(look: &Look, key: &str, value: El) -> El {
    row()
        .w_full()
        .min_h(44.0)
        .px(18.0)
        .gap(space::MD)
        .center_y()
        .child(text(key).font(ty::BODY).color(look.muted()).w(180.0))
        .child(value.grow(1.0))
}

/// A divider line inside a panel.
#[must_use]
pub fn hair(look: &Look) -> El {
    el().w_full().h(1.0).fill(look.edge)
}

/// The tone colour of a note.
#[must_use]
pub fn tone_color(look: &Look, tone: crate::state::Tone) -> Color {
    match tone {
        crate::state::Tone::Ok => look.good,
        crate::state::Tone::Error => look.bad,
        crate::state::Tone::Info => look.accent,
    }
}

/// Whether the window should render dark.
#[must_use]
pub fn is_dark(s: &App, system_dark: bool) -> bool {
    match s.appearance {
        crate::prefs::Appearance::System => system_dark,
        crate::prefs::Appearance::Dark => true,
        crate::prefs::Appearance::Light => false,
    }
}
