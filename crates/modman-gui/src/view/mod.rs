// SPDX-License-Identifier: GPL-2.0-only
//! The view layer: a fixed sidebar shell around one of six screens.
//!
//! Purely structural - every color and widget style comes from
//! [`crate::theme`], every fact on screen comes from [`crate::app::App`].

mod dashboard;
mod downloads;
mod games;
mod load_order;
mod mods;
mod settings;

use iced::widget::{Space, button, column, container, pick_list, row, text};
use iced::{Alignment, Font, Length};

use crate::app::{App, GameChoice, Message, Screen, StatusLine, Tone};
use crate::theme;

/// The message-typed element every view helper returns.
pub type El<'a> = iced::Element<'a, Message>;

/// A heavier weight for headings.
pub const BOLD: Font = Font {
    weight: iced::font::Weight::Bold,
    ..Font::DEFAULT
};

const NAV: [(Screen, &str); 5] = [
    (Screen::Dashboard, "Dashboard"),
    (Screen::Games, "Games"),
    (Screen::Mods, "Mods"),
    (Screen::LoadOrder, "Load Order"),
    (Screen::Downloads, "Downloads"),
];

/// The whole window.
pub fn view(app: &App) -> El<'_> {
    if let Some(error) = &app.boot_error {
        return boot_error(error);
    }
    row![sidebar(app), content(app)].into()
}

fn boot_error(error: &str) -> El<'_> {
    let body = column![
        text("ModManager could not start").size(20).font(BOLD),
        text(error).size(14).color(theme::DANGER),
    ]
    .spacing(12)
    .align_x(Alignment::Center);
    container(container(body).padding(32).style(theme::card))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

// --- sidebar -----------------------------------------------------------------

fn sidebar(app: &App) -> El<'_> {
    let column = column![
        wordmark(),
        game_picker(app),
        section_label("LIBRARY"),
        nav_items(app),
        Space::with_height(Length::Fill),
        service_dot(app),
        nav_item(app, Screen::Settings, "Settings"),
    ]
    .spacing(10)
    .padding(16);
    container(column)
        .width(240)
        .height(Length::Fill)
        .style(theme::sidebar)
        .into()
}

fn wordmark() -> El<'static> {
    let mark = row![
        text("MOD").size(19).font(BOLD).color(theme::ACCENT),
        text("MANAGER").size(19).font(BOLD).color(theme::TEXT),
    ];
    container(mark).padding([8, 6]).into()
}

fn game_picker(app: &App) -> El<'_> {
    let choices: Vec<GameChoice> = app
        .games
        .iter()
        .map(|g| GameChoice {
            id: g.id,
            name: g.name.clone(),
        })
        .collect();
    let selected = app.selected_game.and_then(|id| {
        choices
            .iter()
            .find(|c| c.id == id)
            .cloned()
    });
    pick_list(choices, selected, Message::GamePicked)
        .placeholder("No game registered")
        .text_size(13)
        .padding([8, 10])
        .width(Length::Fill)
        .style(theme::picker)
        .into()
}

fn nav_items(app: &App) -> El<'_> {
    let mut items = column![].spacing(2);
    for (screen, label) in NAV {
        items = items.push(nav_item(app, screen, label));
    }
    items.into()
}

fn nav_item<'a>(app: &App, screen: Screen, label: &'a str) -> El<'a> {
    let active = app.screen == screen;
    let inner = row![
        dot(6.0, if active { theme::ACCENT } else { theme::HAIRLINE }),
        text(label).size(14).font(if active { BOLD } else { Font::DEFAULT }),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    button(inner)
        .width(Length::Fill)
        .padding([9, 12])
        .style(theme::nav(active))
        .on_press(Message::Navigate(screen))
        .into()
}

/// A small colored circle drawn as a widget (no font-coverage worries).
fn dot(size: f32, color: iced::Color) -> El<'static> {
    container(Space::new(size, size))
        .style(move |_| iced::widget::container::Style {
            background: Some(iced::Background::Color(color)),
            border: iced::Border {
                color: iced::Color::TRANSPARENT,
                width: 0.0,
                radius: 99.0.into(),
            },
            ..iced::widget::container::Style::default()
        })
        .into()
}

fn section_label(label: &str) -> El<'_> {
    container(text(label).size(10).color(theme::FAINT))
        .padding([6, 6])
        .into()
}

fn service_dot(app: &App) -> El<'_> {
    let (color, label) = match (&app.link, app.already_running, app.service.is_some()) {
        (Some(link), _, _) => (theme::OK, format!("Hand-off active · :{}", link.port)),
        (None, true, _) => (theme::INFO, "Served by another instance".to_owned()),
        (None, false, true) => (theme::DANGER, "Hand-off inactive".to_owned()),
        _ => (theme::FAINT, "Starting…".to_owned()),
    };
    let inner = row![
        dot(7.0, color),
        text(label).size(11).color(theme::MUTED),
    ]
    .spacing(7)
    .align_y(Alignment::Center);
    container(inner).padding([6, 8]).into()
}

// --- content shell -----------------------------------------------------------

fn content(app: &App) -> El<'_> {
    let body: El<'_> = match app.screen {
        Screen::Dashboard => dashboard::body(app),
        Screen::Games => games::body(app),
        Screen::Mods => mods::body(app),
        Screen::LoadOrder => load_order::body(app),
        Screen::Downloads => downloads::body(app),
        Screen::Settings => settings::body(app),
    };
    let mut shell = column![header(app)].spacing(16).padding(28);
    if let Some(status) = &app.status {
        shell = shell.push(banner(status));
    }
    container(shell.push(body))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn header(app: &App) -> El<'_> {
    let (title, subtitle) = title_of(app);
    let mut bar = row![
        column![
            text(title).size(22).font(BOLD),
            text(subtitle).size(13).color(theme::MUTED),
        ]
        .spacing(3),
        Space::with_width(Length::Fill),
    ]
    .align_y(Alignment::Center);
    if app.busy {
        bar = bar.push(text("Working…").size(13).color(theme::ACCENT));
    }
    bar.into()
}

fn title_of(app: &App) -> (&'static str, String) {
    let game = app
        .games
        .iter()
        .find(|g| Some(g.id) == app.selected_game)
        .map_or_else(|| "no game selected".to_owned(), |g| g.name.clone());
    match app.screen {
        Screen::Dashboard => ("Dashboard", game),
        Screen::Games => ("Games", format!("{} registered", app.games.len())),
        Screen::Mods => ("Mods", game),
        Screen::LoadOrder => ("Load Order", "later mods win file conflicts".to_owned()),
        Screen::Downloads => ("Downloads", "hand-offs from your browser".to_owned()),
        Screen::Settings => ("Settings", "service, locations, extension".to_owned()),
    }
}

// --- helpers shared by the screens --------------------------------------------

/// A raised card with a small uppercase label.
fn labeled_card<'a>(label: &'a str, content: El<'a>) -> El<'a> {
    let inner = column![text(label).size(10).color(theme::FAINT), content].spacing(12);
    container(inner)
        .padding(18)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

/// A quiet inset panel for empty lists.
fn empty_state(message: &str) -> El<'_> {
    container(text(message).size(13).color(theme::MUTED))
        .padding(24)
        .width(Length::Fill)
        .style(theme::inset)
        .into()
}

/// A small "Copy" button placing `value` on the clipboard.
fn copy_button(value: String) -> El<'static> {
    button(text("Copy").size(11))
        .padding([4, 10])
        .style(theme::ghost)
        .on_press(Message::CopyText(value))
        .into()
}

fn banner(status: &StatusLine) -> El<'_> {
    let color = match status.tone {
        Tone::Ok => theme::OK,
        Tone::Error => theme::DANGER,
    };
    let inner = row![
        text(&status.text).size(13).color(color).width(Length::Fill),
        button(text("×").size(14))
            .padding([2, 8])
            .style(theme::icon)
            .on_press(Message::DismissStatus),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    container(inner)
        .padding([10, 14])
        .width(Length::Fill)
        .style(theme::chip(color))
        .into()
}
