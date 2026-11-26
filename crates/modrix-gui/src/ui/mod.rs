// SPDX-License-Identifier: GPL-2.0-only
//! The view: the game's art as a living backdrop, a sidebar, a top bar, and
//! one screen at a time in glass panels over it.

mod conflicts;
mod dashboard;
mod dialogs;
mod downloads;
mod games;
mod load_order;
mod mods;
pub mod parts;
mod settings;
mod shell;
mod wizard;

use rune::{Color, Cx, El, ScrollSpec, SurfaceFit, col, el, row};

use crate::look::{Look, space};
use crate::state::{App, Screen};

/// Height of the sharp hero band at the top of the window.
const HERO_H: f32 = 440.0;

/// The whole window.
pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let dark = parts::is_dark(s, cx.system_dark());
    let swatches = s
        .game_art()
        .map(|a| a.swatches.as_slice())
        .unwrap_or_default();
    let look = Look::new(dark, swatches);
    let mut layer: Vec<El> = Vec::new();
    let body = row()
        .key("body")
        .w_full()
        .h_full()
        .child(shell::sidebar(s, &look, &mut layer))
        .child(main_column(s, &look, &mut layer));
    let window = el()
        .key("window")
        .w_full()
        .h_full()
        .fill(look.base)
        .child(backdrop(s, &look))
        .child(body)
        .children(shell::drop_overlay(s, &look))
        .on_drop_hover(|s: &mut App, on, _| s.drop_hover = on)
        .on_drop_files(|s: &mut App, paths, _| {
            s.drop_hover = false;
            s.install_files(paths.to_vec());
        });
    dialogs::build(s, &look, &mut layer);
    layer.push(shell::toasts(s, &look));
    rune_kit::root(window, layer)
}

/// The art behind everything: a soft wash over the whole window and the
/// sharp hero across the top, fading into the base colour.
fn backdrop(s: &App, look: &Look) -> El {
    let art = s.game_art();
    let base = look.base;
    // Work screens keep the art quieter so tables and text stay readable.
    let calm = s.screen != Screen::Dashboard;
    let top = if calm { 0.55 } else { 0.10 };
    let wash = art.and_then(|a| a.soft.as_ref()).map(|soft| {
        el().key("wash")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .pixels_fit(soft, SurfaceFit::Fill)
            .opacity(if look.dark { 0.55 } else { 0.30 })
    });
    let hero = art.and_then(|a| a.hero.as_ref()).map(|hero| {
        el().key("hero")
            .absolute(0.0, 0.0)
            .w_full()
            .h(HERO_H)
            .pixels_fit(hero, SurfaceFit::Fill)
            .opacity(match (look.dark, calm) {
                (true, false) => 0.92,
                (true, true) => 0.55,
                (false, false) => 0.75,
                (false, true) => 0.40,
            })
    });
    el().key("backdrop")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .a11y_hidden()
        .children(wash)
        .child(glow(look))
        .children(hero)
        .child(fade(base, top))
        .child(
            el().key("tint")
                .absolute(0.0, HERO_H)
                .w_full()
                .h_full()
                .gradient(base, base.alpha(if art.is_some() { 0.72 } else { 1.0 })),
        )
}

/// Dissolves the hero into the base colour, from `top` opacity down.
fn fade(base: Color, top: f32) -> El {
    el().key("hero-fade")
        .absolute(0.0, 0.0)
        .w_full()
        .h(HERO_H.max(1.0))
        .linear_gradient(
            std::f32::consts::PI,
            [
                (0.0, base.alpha(top)),
                (0.45, base.alpha(top.max(0.45))),
                (1.0, base),
            ],
        )
}

/// A soft bloom of the accent in the top right.
fn glow(look: &Look) -> El {
    el().key("glow")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .radial_gradient(
            [0.85, 0.05],
            900.0,
            [
                (0.0, look.accent.alpha(if look.dark { 0.14 } else { 0.08 })),
                (1.0, Color::TRANSPARENT),
            ],
        )
}

fn main_column(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let content = match s.screen {
        Screen::Dashboard => dashboard::view(s, look, layer),
        Screen::Mods => mods::view(s, look, layer),
        Screen::LoadOrder => load_order::view(s, look),
        Screen::Conflicts => conflicts::view(s, look, layer),
        Screen::Downloads => downloads::view(s, look),
        Screen::Games => games::view(s, look, layer),
        Screen::Settings => settings::view(s, look),
    };
    // The Mods and Load order screens scroll their own lists.
    let scrolls_itself = matches!(s.screen, Screen::Mods | Screen::LoadOrder);
    let page = col()
        .key(screen_key(s.screen))
        .w_full()
        .px(space::XL)
        .pt(6.0)
        .pb(space::XL)
        .gap(space::LG)
        .child(content);
    let page = if scrolls_itself {
        page.grow(1.0).min_h(0.0)
    } else {
        col()
            .key("page-scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec::default())
            .clip_fade(0.0, 12.0, 0.0, 24.0)
            .child(page)
    };
    col()
        .key("main")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .child(shell::topbar(s, look, layer))
        .child(page)
}

fn screen_key(screen: Screen) -> &'static str {
    match screen {
        Screen::Dashboard => "screen-overview",
        Screen::Mods => "screen-mods",
        Screen::LoadOrder => "screen-order",
        Screen::Conflicts => "screen-conflicts",
        Screen::Downloads => "screen-downloads",
        Screen::Games => "screen-games",
        Screen::Settings => "screen-settings",
    }
}
