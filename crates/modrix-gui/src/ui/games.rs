// SPDX-License-Identifier: GPL-2.0-only
//! Games: the registered ones as art cards, supported games found on this
//! computer, support from the community registry, and manual setup.

use rune::{El, FileDialogOptions, SurfaceFit, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Select, TextInput};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::state::{App, Popup, Screen};
use crate::ui::parts::{self, glyph};

/// Cards per row.
const PER_ROW: usize = 3;

/// The Games screen.
pub fn view(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let mut page = col().w_full().gap(space::LG);
    if !s.lib.games.is_empty() {
        page = page.child(registered(s, look));
    }
    let found = s.unregistered_detected();
    if !found.is_empty() {
        page = page.child(detected(s, look, &found));
    }
    let support = s.available_support();
    if !support.is_empty() || s.registry_loading {
        page = page.child(registry(s, look, &support));
    }
    page.child(manual(s, look, layer))
}

fn registered(s: &App, look: &Look) -> El {
    let cards: Vec<El> = s.lib.games.iter().map(|g| game_card(s, look, g)).collect();
    parts::section(look, "registered", "Your games", None, grid(cards))
}

fn grid(cards: Vec<El>) -> El {
    let mut rows = Vec::new();
    let mut current = Vec::new();
    for card in cards.into_iter().take(300) {
        current.push(card);
        if current.len() == PER_ROW {
            rows.push(
                row()
                    .w_full()
                    .gap(space::MD)
                    .children(std::mem::take(&mut current)),
            );
        }
    }
    if !current.is_empty() {
        let pad = PER_ROW.saturating_sub(current.len());
        let fillers = (0..pad).map(|_| el().grow(1.0).w(0.0));
        rows.push(
            row()
                .w_full()
                .gap(space::MD)
                .children(current)
                .children(fillers),
        );
    }
    col()
        .w_full()
        .px(18.0)
        .pb(18.0)
        .gap(space::MD)
        .children(rows)
}

fn game_card(s: &App, look: &Look, g: &modrix_core::Game) -> El {
    let id = g.id;
    let on = s.lib.game == Some(id);
    col()
        .key(format!("game-{id}"))
        .grow(1.0)
        .w(0.0)
        .radius(14.0)
        .clip_rounded(14.0)
        .fill(look.glass_strong)
        .border(
            if on { 2.0 } else { 1.0 },
            if on { look.accent } else { look.edge },
        )
        .hover_fill(look.glass_strong.mix(look.accent, 0.06))
        .role("button")
        .label(format!("Manage {}", g.name))
        .on_press(move |s: &mut App| {
            s.switch_game(id);
            s.screen = Screen::Dashboard;
        })
        .child(banner(look, s.art.get(&id), &g.name))
        .child(caption(look, g, on))
}

/// The card's art strip: the hero with the logo over it, or the name on a
/// tinted gradient when the game has no art.
fn banner(look: &Look, art: Option<&crate::art::ArtSet>, name: &str) -> El {
    let strip = el().w_full().h(120.0);
    let Some(hero) = art.and_then(|a| a.hero.as_ref()) else {
        return strip
            .linear_gradient(
                2.4,
                [
                    (0.0, look.accent.alpha(0.40)),
                    (1.0, look.accent.alpha(0.06)),
                ],
            )
            .padding(16.0)
            .child(glyph(&icons::GAMEPAD, 22.0, look.text().alpha(0.7)))
            .child(
                text(name.to_owned())
                    .font(ty::HEADING)
                    .color(look.text())
                    .absolute(16.0, 82.0),
            );
    };
    let logo = art.and_then(|a| a.logo.as_ref()).map(|px| {
        el().absolute(14.0, 66.0)
            .w(150.0)
            .h(44.0)
            .pixels_fit(px, SurfaceFit::Fit)
    });
    strip
        .pixels_fit(hero, SurfaceFit::Fill)
        .child(
            el().absolute(0.0, 60.0)
                .w_full()
                .h(60.0)
                .gradient(look.base.alpha(0.0), look.base.alpha(0.6)),
        )
        .children(logo)
}

fn caption(look: &Look, g: &modrix_core::Game, on: bool) -> El {
    col()
        .padding(14.0)
        .gap(3.0)
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(
                    text(g.name.clone())
                        .font(ty::BODY_STRONG)
                        .color(look.text())
                        .truncate()
                        .grow(1.0),
                )
                .children(on.then(|| parts::chip("Active", look.accent))),
        )
        .child(
            text(g.install_path.display().to_string())
                .font(ty::SMALL)
                .color(look.faint())
                .truncate_head(),
        )
}

fn detected(s: &App, look: &Look, found: &[(usize, &crate::state::Detected)]) -> El {
    let rows = found.iter().map(|(index, d)| {
        let index = *index;
        row()
            .key(format!("found-{}", d.def.id))
            .w_full()
            .px(18.0)
            .py(10.0)
            .gap(space::MD)
            .center_y()
            .child(parts::icon_tile(&icons::GAMEPAD, look.accent, 34.0))
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child(
                        text(d.def.name.clone())
                            .font(ty::BODY_STRONG)
                            .color(look.text()),
                    )
                    .child(
                        text(d.install.display().to_string())
                            .font(ty::SMALL)
                            .color(look.faint())
                            .truncate_head(),
                    ),
            )
            .child(
                Button::new("Add")
                    .icon(&icons::PLUS)
                    .variant(ButtonVariant::Submit)
                    .on_press(move |s: &mut App| s.add_detected(index))
                    .build(&look.k),
            )
    });
    let _ = s;
    parts::section(
        look,
        "detected",
        "Found on this computer",
        Some(parts::chip(found.len().to_string(), look.accent)),
        col().w_full().pb(8.0).children(rows),
    )
}

fn registry(s: &App, look: &Look, support: &[(usize, &crate::state::RegistryChoice)]) -> El {
    let body = if s.registry_loading && support.is_empty() {
        row().px(18.0).pb(16.0).child(
            text("Loading the community registry")
                .font(ty::BODY)
                .color(look.muted()),
        )
    } else {
        let rows = support.iter().take(200).map(|(index, r)| {
            let index = *index;
            row()
                .key(format!("reg-{}", r.id))
                .w_full()
                .px(18.0)
                .py(9.0)
                .gap(space::MD)
                .center_y()
                .child(glyph(&icons::PUZZLE, 16.0, look.faint()))
                .child(
                    text(r.name.clone())
                        .font(ty::BODY)
                        .color(look.text())
                        .grow(1.0)
                        .truncate(),
                )
                .child(parts::tag(look, r.version.clone()))
                .child(
                    Button::new("Install")
                        .variant(ButtonVariant::Ghost)
                        .on_press(move |s: &mut App| s.install_support(index))
                        .build(&look.k),
                )
        });
        col().w_full().pb(8.0).children(rows)
    };
    parts::section(
        look,
        "registry",
        "More games from the community",
        None,
        body,
    )
}

fn def_picker(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let names: Vec<&str> = s.defs.iter().map(|d| d.name.as_str()).collect();
    let current = s
        .form
        .def_id
        .as_deref()
        .and_then(|id| s.defs.iter().find(|d| d.id == id))
        .map(|d| d.name.as_str());
    Select::new("def-picker", &names)
        .current(current)
        .placeholder("Choose a game")
        .open(s.popup == Some(Popup::DefPicker))
        .on_toggle(|s: &mut App| {
            s.popup = if s.popup == Some(Popup::DefPicker) {
                None
            } else {
                Some(Popup::DefPicker)
            }
        })
        .on_close(|s: &mut App| s.popup = None)
        .on_choose(|s: &mut App, name: &str, _| {
            s.popup = None;
            s.form.def_id = s.defs.iter().find(|d| d.name == name).map(|d| d.id.clone());
        })
        .width(300.0)
        .build(&look.k, layer)
}

fn folder_field(s: &App, look: &Look) -> El {
    row()
        .gap(space::SM)
        .child(
            TextInput::new(&s.form.install_path)
                .placeholder("Game folder")
                .leading_icon(&icons::FOLDER)
                .width(Some(380.0))
                .on_edit(|s: &mut App| &mut s.form.install_path)
                .on_submit(|s: &mut App, _| s.add_game())
                .build(&look.k),
        )
        .child(
            Button::new("Browse")
                .variant(ButtonVariant::Neutral)
                .on_press_tx(|_: &mut App, tx| {
                    let options = FileDialogOptions {
                        title: "Game folder".to_owned(),
                        ..FileDialogOptions::default()
                    };
                    tx.open_folder(options, |s: &mut App, picked, _| {
                        if let Ok(Some(dir)) = picked {
                            s.form.install_path = rune::TextEdit::new(dir.display().to_string());
                        }
                    });
                })
                .build(&look.k),
        )
}

fn manual(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let picker = def_picker(s, look, layer);
    let folder = folder_field(s, look);
    let custom = TextInput::new(&s.form.def_path)
        .placeholder("Optional: path to a game.toml")
        .width(Some(380.0))
        .on_edit(|s: &mut App| &mut s.form.def_path)
        .build(&look.k);
    let body = col()
        .w_full()
        .pb(14.0)
        .child(parts::kv(look, "Game", picker))
        .child(parts::kv(look, "Install folder", folder))
        .child(parts::kv(look, "Definition", custom))
        .child(
            row().w_full().px(18.0).pt(6.0).justify_end().child(
                Button::new("Add game")
                    .icon(&icons::PLUS)
                    .variant(ButtonVariant::Submit)
                    .on_press(|s: &mut App| s.add_game())
                    .build(&look.k),
            ),
        );
    parts::section(look, "manual", "Set up a game by hand", None, body)
}
