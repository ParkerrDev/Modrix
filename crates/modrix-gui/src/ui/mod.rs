// SPDX-License-Identifier: GPL-2.0-only
//! The view: the game's art as a baked backdrop, a sidebar, a top bar, and
//! one screen at a time in glass panels over it.

mod conflicts;
mod dashboard;
mod dialogs;
mod downloads;
mod games;
mod load_order;
mod menus;
mod mods;
pub mod parts;
mod settings;
mod shell;
mod store;
mod wizard;

use rune::{Cx, El, ScrollSpec, SurfaceFit, col, el, row};

use crate::look::{Look, space};
use crate::state::{App, Screen};

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
        .children(backdrop(s, &look))
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

/// The art behind the top of the window: one image baked from the hero,
/// its soft wash, the accent glow and the fade (see `bake`). It keeps the
/// bake's aspect, so it scales with the window and is never squished, and it
/// ends in the base colour the window is filled with.
fn backdrop(s: &App, look: &Look) -> Option<El> {
    // Work screens keep the art quieter so tables and text stay readable.
    let calm = s.screen != Screen::Dashboard;
    let band = s.game_art()?.band(look.dark, calm)?;
    Some(
        el().key("backdrop")
            .absolute(0.0, 0.0)
            .w_full()
            .aspect_ratio(crate::bake::RATIO)
            .a11y_hidden()
            .pixels_fit(band, SurfaceFit::Fill),
    )
}

fn main_column(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let content = match s.screen {
        Screen::Dashboard => dashboard::view(s, look, layer),
        Screen::Mods => mods::view(s, look, layer),
        Screen::LoadOrder => load_order::view(s, look),
        Screen::Conflicts => conflicts::view(s, look, layer),
        Screen::Downloads => downloads::view(s, look),
        Screen::Store => store::view(s, look),
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
        Screen::Store => "screen-store",
        Screen::Games => "screen-games",
        Screen::Settings => "screen-settings",
    }
}
