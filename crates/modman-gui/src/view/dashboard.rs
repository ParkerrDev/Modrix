// SPDX-License-Identifier: GPL-2.0-only
//! The Dashboard: at-a-glance cards, or onboarding when nothing is set up.

use iced::widget::{button, column, container, row, text};
use iced::Length;
use modman_download::DownloadState;

use super::{BOLD, El, empty_state, labeled_card};
use crate::app::{App, Message, Screen};
use crate::theme;

/// The dashboard body.
pub(super) fn body(app: &App) -> El<'_> {
    if app.games.is_empty() {
        return onboarding(app);
    }
    column![
        row![game_card(app), deploy_card(app)].spacing(16),
        row![downloads_card(app), handoff_card(app)].spacing(16),
    ]
    .spacing(16)
    .into()
}

fn game_card(app: &App) -> El<'_> {
    let Some(game) = app.games.iter().find(|g| Some(g.id) == app.selected_game) else {
        return empty_state("Select a game in the sidebar.");
    };
    let stats = format!("{} staged · {} enabled", app.mods.len(), app.order.len());
    let inner = column![
        text(&game.name).size(17).font(BOLD),
        text(game.install_path.display().to_string())
            .size(12)
            .color(theme::MUTED),
        text(stats).size(13).color(theme::TEXT),
        button(text("Manage mods").size(13))
            .padding([8, 14])
            .style(theme::ghost)
            .on_press(Message::Navigate(Screen::Mods)),
    ]
    .spacing(10);
    labeled_card("ACTIVE GAME", inner.into())
}

fn deploy_card(app: &App) -> El<'_> {
    let profile = app
        .active_profile
        .as_ref()
        .map_or_else(|| "no profile".to_owned(), |p| format!("profile “{}”", p.name));
    let hint = format!("{} mod(s) will deploy · {profile}", app.order.len());
    let mut actions = row![].spacing(10);
    if app.busy {
        actions = actions.push(text("Working…").size(13).color(theme::ACCENT));
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
    let inner = column![text(hint).size(13).color(theme::TEXT), actions].spacing(14);
    labeled_card("DEPLOYMENT", inner.into())
}

fn downloads_card(app: &App) -> El<'_> {
    let active = app
        .downloads
        .iter()
        .filter(|d| matches!(d.state, DownloadState::Active | DownloadState::Queued))
        .count();
    let summary = if app.downloads.is_empty() {
        "Nothing yet - downloads appear here the moment you click one in your browser.".to_owned()
    } else {
        format!("{active} active · {} total", app.downloads.len())
    };
    let inner = column![
        text(summary).size(13).color(theme::TEXT),
        button(text("View downloads").size(13))
            .padding([8, 14])
            .style(theme::ghost)
            .on_press(Message::Navigate(Screen::Downloads)),
    ]
    .spacing(12);
    labeled_card("DOWNLOADS", inner.into())
}

fn handoff_card(app: &App) -> El<'_> {
    let line = match (&app.link, app.already_running) {
        (Some(link), _) => format!(
            "Listening on 127.0.0.1:{} - clicks on nexusmods.com install automatically.",
            link.port
        ),
        (None, true) => {
            "Another ModManager instance is receiving browser downloads.".to_owned()
        }
        (None, false) => "The hand-off listener is not running.".to_owned(),
    };
    let inner = column![
        text(line).size(13).color(theme::TEXT),
        button(text("Pairing details").size(13))
            .padding([8, 14])
            .style(theme::ghost)
            .on_press(Message::Navigate(Screen::Settings)),
    ]
    .spacing(12);
    labeled_card("BROWSER HAND-OFF", inner.into())
}

fn onboarding(app: &App) -> El<'_> {
    let port = app
        .link
        .as_ref()
        .map_or_else(|| "-".to_owned(), |l| l.port.to_string());
    let steps = column![
        step(
            "1",
            "Register your game",
            "Pick a definition and point at the install directory.",
        ),
        step(
            "2",
            "Install the browser extension",
            "Load the `extension/` folder unpacked, then paste the port and token from Settings.",
        ),
        step(
            "3",
            "Click Download on nexusmods.com",
            "The file lands here, staged and ready to enable + deploy.",
        ),
    ]
    .spacing(14);
    let inner = column![
        text("Welcome to ModManager").size(18).font(BOLD),
        text(format!("Hand-off service on port {port} - no API keys, ever."))
            .size(13)
            .color(theme::MUTED),
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

fn step<'a>(n: &'a str, title: &'a str, detail: &'a str) -> El<'a> {
    row![
        container(text(n).size(13).font(BOLD).color(theme::ACCENT))
            .padding([4, 11])
            .style(theme::chip(theme::ACCENT)),
        column![
            text(title).size(14).font(BOLD),
            text(detail).size(12).color(theme::MUTED),
        ]
        .spacing(3),
    ]
    .spacing(14)
    .into()
}
