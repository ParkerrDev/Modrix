// SPDX-License-Identifier: GPL-2.0-only
//! The Dashboard: the selected game's blurred hero art (Steam's own library
//! imagery) behind at-a-glance glass cards - or onboarding when nothing is
//! set up.

use iced::widget::{button, column, container, image, row, stack, text};
use iced::{ContentFit, Length};
use modrix_download::DownloadState;

use super::{BOLD, El, empty_state};
use crate::app::{App, Message, Screen};
use crate::theme;

/// Height of each dashboard card row - fixed, so cards in a row always align.
const ROW_HEIGHT: f32 = 170.0;

/// The dashboard body.
pub(super) fn body(app: &App) -> El<'_> {
    if app.games.is_empty() {
        return onboarding(app);
    }
    let cards = column![
        row![game_card(app), deploy_card(app)]
            .spacing(16)
            .height(ROW_HEIGHT),
        row![downloads_card(app), handoff_card(app)]
            .spacing(16)
            .height(ROW_HEIGHT),
    ]
    .spacing(16);
    // The selected game's pre-blurred hero fills the screen behind the cards,
    // under a scrim that keeps text readable in both themes.
    let hero = app
        .selected_game
        .and_then(|id| app.art.get(&id))
        .and_then(|art| art.hero_blur.clone());
    let Some(hero) = hero else {
        return cards.into();
    };
    stack![
        container(
            image(hero)
                .content_fit(ContentFit::Cover)
                .width(Length::Fill)
                .height(Length::Fill)
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .clip(true),
        container(cards)
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::hero_scrim),
    ]
    .into()
}

fn game_card(app: &App) -> El<'_> {
    let Some(game) = app.games.iter().find(|g| Some(g.id) == app.selected_game) else {
        return empty_state("Select a game.");
    };
    let stats = format!("{} mods · {} enabled", app.mods.len(), app.order.len());
    let inner = column![
        text(&game.name).size(17).font(BOLD),
        text(game.install_path.display().to_string())
            .size(12)
            .color(theme::muted()),
        text(stats).size(13),
        button(text("Manage mods").size(13))
            .padding([8, 14])
            .style(theme::ghost)
            .on_press(Message::Navigate(Screen::Mods)),
    ]
    .spacing(10);
    tall_card("ACTIVE GAME", inner.into())
}

fn deploy_card(app: &App) -> El<'_> {
    let profile = app
        .active_profile
        .as_ref()
        .map_or_else(|| "no profile".to_owned(), |p| p.name.clone());
    let hint = format!("{} mods will deploy · {profile}", app.order.len());
    let mut actions = row![].spacing(10);
    if app.busy {
        actions = actions.push(text("Working…").size(13).color(theme::accent()));
    } else {
        actions = actions
            .push(
                button(text("Deploy").size(13))
                    .padding([8, 16])
                    .style(theme::primary)
                    .on_press(Message::Deploy),
            )
            .push(
                button(text("Verify").size(13))
                    .padding([8, 14])
                    .style(theme::ghost)
                    .on_press(Message::Verify),
            );
    }
    let inner = column![text(hint).size(13), actions].spacing(14);
    tall_card("DEPLOYMENT", inner.into())
}

fn downloads_card(app: &App) -> El<'_> {
    let active = app
        .downloads
        .iter()
        .filter(|d| matches!(d.state, DownloadState::Active | DownloadState::Queued))
        .count();
    let summary = if app.downloads.is_empty() {
        "None yet".to_owned()
    } else {
        format!("{active} active · {} total", app.downloads.len())
    };
    let inner = column![
        text(summary).size(13),
        button(text("View downloads").size(13))
            .padding([8, 14])
            .style(theme::ghost)
            .on_press(Message::Navigate(Screen::Downloads)),
    ]
    .spacing(12);
    tall_card("DOWNLOADS", inner.into())
}

fn handoff_card(app: &App) -> El<'_> {
    let line = match (&app.link, app.already_running) {
        (Some(link), _) => format!("Listening on 127.0.0.1:{}", link.port),
        (None, true) => "Another instance is receiving downloads".to_owned(),
        (None, false) => "Not running".to_owned(),
    };
    let inner = column![
        text(line).size(13),
        button(text("Pairing").size(13))
            .padding([8, 14])
            .style(theme::ghost)
            .on_press(Message::Navigate(Screen::Settings)),
    ]
    .spacing(12);
    tall_card("BROWSER HAND-OFF", inner.into())
}

fn onboarding(app: &App) -> El<'_> {
    let port = app
        .link
        .as_ref()
        .map_or_else(|| "-".to_owned(), |l| l.port.to_string());
    let steps = column![
        step("1", "Register your game"),
        step(
            "2",
            "Load the browser extension and paste the token from Settings"
        ),
        step("3", "Click Download on nexusmods.com"),
    ]
    .spacing(12);
    let inner = column![
        text("Welcome").size(18).font(BOLD),
        text(format!("Hand-off service on port {port}. No API keys."))
            .size(13)
            .color(theme::muted()),
        steps,
        button(text("Register a game").size(13))
            .padding([9, 18])
            .style(theme::primary)
            .on_press(Message::Navigate(Screen::Games)),
    ]
    .spacing(18);
    container(inner)
        .padding(28)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

/// A dashboard card that fills its fixed-height row, so a row's cards
/// always align whatever their content.
fn tall_card<'a>(label: &'a str, content: El<'a>) -> El<'a> {
    let inner = column![text(label).size(10).color(theme::faint()), content].spacing(12);
    container(inner)
        .padding(18)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::card)
        .into()
}

fn step<'a>(n: &'a str, title: &'a str) -> El<'a> {
    row![
        container(text(n).size(13).font(BOLD).color(theme::accent()))
            .padding([4, 11])
            .style(theme::chip(theme::accent())),
        text(title).size(14),
    ]
    .spacing(14)
    .align_y(iced::Alignment::Center)
    .into()
}
