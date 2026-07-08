// SPDX-License-Identifier: GPL-2.0-only
//! The Games screen: registered installs + the registration form.

use iced::widget::{button, column, container, pick_list, row, text, text_input};
use iced::{Alignment, Length};
use modman_core::Game;

use super::{BOLD, El, empty_state, labeled_card};
use crate::app::{App, GameChoice, Message};
use crate::theme;

/// The games body.
pub(super) fn body(app: &App) -> El<'_> {
    let mut list = column![].spacing(10);
    if app.games.is_empty() {
        list = list.push(empty_state(
            "No games registered yet - add one below and Nexus downloads will route to it \
             automatically.",
        ));
    }
    for game in &app.games {
        list = list.push(game_card(app, game));
    }
    column![list, register_card(app)].spacing(16).into()
}

fn game_card<'a>(app: &App, game: &'a Game) -> El<'a> {
    let selected = Some(game.id) == app.selected_game;
    let mut meta = format!("{} · {}", game.plugin_id, game.store);
    if let Some(appid) = game.steam_appid {
        use std::fmt::Write as _;
        let _ = write!(meta, " · Steam {appid}");
    }
    let mut head = row![
        text(&game.name)
            .size(15)
            .font(BOLD)
            .width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if selected {
        head = head.push(
            container(text("ACTIVE").size(10))
                .padding([2, 8])
                .style(theme::chip(theme::ACCENT)),
        );
    }
    let inner = column![
        head,
        text(meta).size(12).color(theme::MUTED),
        text(game.install_path.display().to_string())
            .size(12)
            .color(theme::FAINT),
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
    .style(theme::picker);
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
        container(submit).align_x(Alignment::End).width(Length::Fill),
    ]
    .spacing(10);
    labeled_card("REGISTER A GAME", inner.into())
}
