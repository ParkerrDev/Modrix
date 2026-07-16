// SPDX-License-Identifier: GPL-2.0-only
//! The Games screen: registered installs + the registration form.

use iced::widget::{button, column, container, pick_list, row, text, text_input};
use iced::{Alignment, Length};
use modrix_core::Game;

use super::{BOLD, El, empty_state, labeled_card};
use crate::app::{App, Detected, GameChoice, Message, RegistryChoice};
use crate::theme;

/// The games body.
pub(super) fn body(app: &App) -> El<'_> {
    let mut list = column![].spacing(10);
    if app.games.is_empty() {
        list = list.push(empty_state(
            "No games registered yet - pick one found installed below, or add it manually. \
             Nexus downloads route to the active game automatically.",
        ));
    }
    for game in &app.games {
        list = list.push(game_card(app, game));
    }
    let mut root = column![list].spacing(16);
    if let Some(card) = detected_card(app) {
        root = root.push(card);
    }
    if let Some(card) = registry_card(app) {
        root = root.push(card);
    }
    root.push(register_card(app)).into()
}

/// Community game-support plugins available in the registry but not
/// installed locally. Installing one auto-registers the game when its
/// install is found on disk. `None` when everything is already local (or
/// the registry is unreachable).
fn registry_card(app: &App) -> Option<El<'_>> {
    let available = app.available_support();
    if available.is_empty() {
        return None;
    }
    let mut rows = column![].spacing(8);
    for (index, choice) in available {
        rows = rows.push(registry_row(index, choice, app.busy));
    }
    Some(labeled_card("MORE GAMES - community plugins", rows.into()))
}

fn registry_row(index: usize, choice: &RegistryChoice, busy: bool) -> El<'_> {
    let info = column![
        text(&choice.name).size(14).font(BOLD),
        text(format!("{} · v{}", choice.id, choice.version))
            .size(12)
            .color(theme::faint()),
    ]
    .spacing(4)
    .width(Length::Fill);
    let mut install = button(text("Install support").size(13))
        .padding([8, 16])
        .style(theme::primary);
    if !busy {
        install = install.on_press(Message::InstallSupport(index));
    }
    container(row![info, install].spacing(12).align_y(Alignment::Center))
        .padding(12)
        .width(Length::Fill)
        .style(theme::inset)
        .into()
}

/// A one-click "add and switch" list of supported games found installed on
/// disk but not yet registered. `None` when nothing new was detected.
fn detected_card(app: &App) -> Option<El<'_>> {
    let found = app.unregistered_detected();
    if found.is_empty() {
        return None;
    }
    let mut rows = column![].spacing(8);
    for (index, detected) in found {
        rows = rows.push(detected_row(index, detected));
    }
    Some(labeled_card("FOUND INSTALLED", rows.into()))
}

fn detected_row(index: usize, detected: &Detected) -> El<'_> {
    let mut meta = detected.install.display().to_string();
    if let Some(appid) = detected.appid {
        meta = format!("Steam {appid} · {meta}");
    }
    let info = column![
        text(&detected.def.name).size(14).font(BOLD),
        text(meta).size(12).color(theme::faint()),
    ]
    .spacing(4)
    .width(Length::Fill);
    let add = button(text("Add & switch").size(13))
        .padding([8, 16])
        .style(theme::primary)
        .on_press(Message::AddDetected(index));
    container(row![info, add].spacing(12).align_y(Alignment::Center))
        .padding(12)
        .width(Length::Fill)
        .style(theme::inset)
        .into()
}

fn game_card<'a>(app: &App, game: &'a Game) -> El<'a> {
    let selected = Some(game.id) == app.selected_game;
    let mut meta = format!("{} · {}", game.plugin_id, game.store);
    if let Some(appid) = game.steam_appid {
        use std::fmt::Write as _;
        let _ = write!(meta, " · Steam {appid}");
    }
    let mut head = row![text(&game.name).size(15).font(BOLD).width(Length::Fill),]
        .spacing(12)
        .align_y(Alignment::Center);
    if selected {
        head = head.push(
            container(text("ACTIVE").size(10))
                .padding([2, 8])
                .style(theme::chip(theme::accent())),
        );
    }
    let inner = column![
        head,
        text(meta).size(12).color(theme::muted()),
        text(game.install_path.display().to_string())
            .size(12)
            .color(theme::faint()),
    ]
    .spacing(6)
    .width(Length::Fill);
    button(inner)
        .padding(16)
        .width(Length::Fill)
        .style(theme::nav(selected))
        .on_press(Message::GamePicked(GameChoice {
            id: game.id,
            name: game.name.clone(),
        }))
        .into()
}

fn register_card(app: &App) -> El<'_> {
    let picker = pick_list(
        app.defs.clone(),
        app.form.def_choice.clone(),
        Message::DefPicked,
    )
    .placeholder("Choose a game definition")
    .text_size(13)
    .padding([8, 10])
    .width(Length::Fill)
    .style(theme::picker)
    .menu_style(theme::picker_menu);
    let def_path = text_input("…or a path to a custom game.toml", &app.form.def_path)
        .on_input(Message::DefPathChanged)
        .size(13)
        .padding(9)
        .width(Length::Fill)
        .style(theme::input);
    let install = text_input(
        "Game install directory (e.g. ~/.steam/…/Skyrim Special Edition)",
        &app.form.install_path,
    )
    .on_input(Message::InstallPathChanged)
    .on_submit(Message::AddGame)
    .size(13)
    .padding(9)
    .style(theme::input);
    let submit = button(text("Register game").size(13))
        .padding([9, 18])
        .style(theme::primary)
        .on_press(Message::AddGame);
    let inner = column![
        picker,
        def_path,
        install,
        container(submit)
            .align_x(Alignment::End)
            .width(Length::Fill),
    ]
    .spacing(10);
    labeled_card("REGISTER A GAME", inner.into())
}
