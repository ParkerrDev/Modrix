// SPDX-License-Identifier: GPL-2.0-only
//! The view layer: a fixed sidebar shell around one of six screens, with a
//! notification center and the FOMOD wizard overlay.

mod conflicts;
mod dashboard;
mod downloads;
mod games;
mod load_order;
mod mods;
mod settings;
mod wizard;

use iced::widget::{
    Space, button, column, container, mouse_area, opaque, pick_list, row, scrollable, text,
};
use iced::{Alignment, Font, Length};

use crate::app::{App, DupPrompt, GameChoice, Message, Note, Screen, Tone};
use crate::{icons, theme};

/// The message-typed element every view helper returns.
pub type El<'a> = iced::Element<'a, Message>;

/// A heavier weight for headings.
pub const BOLD: Font = Font {
    weight: iced::font::Weight::Bold,
    ..Font::DEFAULT
};

const NAV: [(Screen, &str); 6] = [
    (Screen::Dashboard, "Dashboard"),
    (Screen::Games, "Games"),
    (Screen::Mods, "Mods"),
    (Screen::LoadOrder, "Load Order"),
    (Screen::Conflicts, "Conflicts"),
    (Screen::Downloads, "Downloads"),
];

/// The whole window.
pub fn view(app: &App) -> El<'_> {
    if let Some(error) = &app.boot_error {
        return boot_error(error);
    }
    // Until the embedded service finishes starting (which includes recovering
    // an interrupted deploy - potentially thousands of file restores), show a
    // deliberate loading screen so a slow boot never looks like a hang.
    if app.service.is_none() {
        return starting(app);
    }
    let base: El<'_> = row![sidebar(app), content(app)].into();
    // Overlays stack over the base UI: the notification popup (non-blocking,
    // top-right), then modal layers (duplicate prompt, FOMOD wizard).
    let mut layers: Vec<El<'_>> = vec![base];
    if app.notes_open {
        layers.push(notes_overlay(app));
    }
    if let Some(prompt) = app.dup_queue.first() {
        layers.push(dup_overlay(prompt));
    }
    if let Some(wizard) = &app.wizard {
        layers.push(wizard::overlay(app, wizard));
    }
    if layers.len() == 1 {
        return layers.remove(0);
    }
    iced::widget::Stack::with_children(layers).into()
}

/// The boot screen: wordmark plus a live progress bar and status line while
/// the engine starts (crash recovery can take a while on big libraries).
fn starting(app: &App) -> El<'_> {
    let mut body = column![row![
        text("MOD").size(24).font(BOLD).color(theme::ACCENT),
        text("RIX").size(24).font(BOLD).color(theme::TEXT),
    ],]
    .spacing(14)
    .align_x(Alignment::Center);
    body = body.push(progress_line(app, 360.0, "Starting…"));
    container(body)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

/// A progress bar + percentage + status line for the live operation, or a
/// quiet fallback message when nothing is running.
///
/// Every element has a fixed width: the status message changes length each
/// tick, and if it drove the layout the centered bar would shift ("bounce")
/// with every update.
fn progress_line<'a>(app: &'a App, width: f32, idle: &'a str) -> El<'a> {
    let Some(op) = &app.op else {
        return text(idle).size(13).color(theme::MUTED).into();
    };
    let (value, pct): (f32, String) = match op.fraction() {
        Some(f) => (f, format!("{:.0}%", f64::from(f) * 100.0)),
        None => (0.0, "…".to_owned()),
    };
    // Bar + spacing + a fixed percentage cell = the column's constant width.
    let pct_width = 42.0_f32;
    let total = width + 10.0 + pct_width;
    column![
        row![
            iced::widget::progress_bar(0.0..=1.0, value)
                .height(6)
                .width(width)
                .style(theme::progress),
            text(pct).size(12).color(theme::ACCENT).width(pct_width),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        // Clipped to one line so a long path cannot grow the layout either.
        container(
            text(op.message.clone())
                .size(12)
                .color(theme::MUTED)
                .width(total)
                .align_x(Alignment::Center)
        )
        .height(18)
        .clip(true),
    ]
    .spacing(6)
    .width(total)
    .into()
}

fn boot_error(error: &str) -> El<'_> {
    let body = column![
        text("Modrix could not start").size(20).font(BOLD),
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
        text("RIX").size(19).font(BOLD).color(theme::TEXT),
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
    let selected = app
        .selected_game
        .and_then(|id| choices.iter().find(|c| c.id == id).cloned());
    pick_list(choices, selected, Message::GamePicked)
        .placeholder("No game")
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
        icons::dot(
            6.0,
            if active {
                theme::ACCENT
            } else {
                theme::HAIRLINE
            }
        ),
        text(label)
            .size(14)
            .font(if active { BOLD } else { Font::DEFAULT }),
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

fn service_dot(app: &App) -> El<'_> {
    let (color, label) = match (&app.link, app.already_running, app.service.is_some()) {
        (Some(link), _, _) => (theme::OK, format!("Hand-off · :{}", link.port)),
        (None, true, _) => (theme::INFO, "Another instance active".to_owned()),
        (None, false, true) => (theme::DANGER, "Hand-off inactive".to_owned()),
        _ => (theme::FAINT, "Starting…".to_owned()),
    };
    let inner = row![
        icons::dot(7.0, color),
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
        Screen::Conflicts => conflicts::body(app),
        Screen::Downloads => downloads::body(app),
        Screen::Settings => settings::body(app),
    };
    // Fill, not the default Shrink: a Fill-height list inside a Shrink
    // column collapses in iced, hiding everything below the fold.
    let shell = column![header(app)]
        .spacing(16)
        .padding(28)
        .height(Length::Fill);
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
    .spacing(14)
    .align_y(Alignment::Center);
    if app.busy || app.op.is_some() {
        bar = bar.push(progress_line(app, 220.0, "Working…"));
    }
    bar.push(bell(app)).into()
}

/// The notification bell: a drawn bell with a status dot coloured by the most
/// severe pending notification (red errors/conflicts, yellow warnings, blue
/// info, green all-clear).
fn bell(app: &App) -> El<'_> {
    let severity = notification_severity(app);
    let dot_color = match severity {
        Severity::Error => theme::DANGER,
        Severity::Warning => theme::ACCENT,
        Severity::Info => theme::INFO,
        Severity::Clear => theme::OK,
    };
    let mut inner = row![icons::bell(theme::MUTED, dot_color)]
        .spacing(6)
        .align_y(Alignment::Center);
    // Unread events plus live issues: the number tracks what the panel shows.
    let pending = app.unread.saturating_add(app.health.len());
    if pending > 0 {
        inner = inner.push(text(pending).size(12).color(dot_color));
    }
    button(inner)
        .padding([6, 12])
        .style(theme::ghost)
        .on_press(Message::ToggleNotes)
        .into()
}

/// Severity of the bell dot, from live health + unread notifications.
enum Severity {
    Error,
    Warning,
    Info,
    Clear,
}

fn notification_severity(app: &App) -> Severity {
    use modrix_core::Severity as S;
    if app.health.iter().any(|i| i.severity == S::Error) {
        return Severity::Error;
    }
    if app.health.iter().any(|i| i.severity == S::Warning) {
        return Severity::Warning;
    }
    if app.unread > 0 {
        let worst = app
            .notes
            .iter()
            .take(app.unread)
            .map(|n| n.tone)
            .fold(Tone::Info, |acc, t| match (acc, t) {
                (Tone::Error, _) | (_, Tone::Error) => Tone::Error,
                _ => acc,
            });
        return match worst {
            Tone::Error => Severity::Error,
            _ => Severity::Info,
        };
    }
    Severity::Clear
}

/// The notification popup, anchored under the bell in the top-right corner.
/// A click anywhere outside the panel dismisses it; the panel itself swallows
/// clicks so interacting with it does not close it. Not `opaque`: the rest of
/// the UI stays visible and interactive-looking beneath.
fn notes_overlay(app: &App) -> El<'_> {
    let panel =
        mouse_area(container(notes_panel(app)).width(380).max_height(480)).on_press(Message::NoOp);
    let positioned = container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Top)
        .padding([64, 24]);
    mouse_area(positioned).on_press(Message::ToggleNotes).into()
}

/// Panel contents: live issues first (rendered straight from `app.health`,
/// so a resolved issue vanishes on the next health pass), then recent events.
fn notes_panel(app: &App) -> El<'_> {
    let mut list = column![].spacing(6);
    if app.health.is_empty() && app.notes.is_empty() {
        list = list.push(text("All clear").size(12).color(theme::FAINT));
    }
    for issue in &app.health {
        list = list.push(issue_row(issue));
    }
    if !app.health.is_empty() && !app.notes.is_empty() {
        list = list.push(text("RECENT").size(9).color(theme::FAINT));
    }
    for note in app.notes.iter().take(50) {
        list = list.push(note_row(note));
    }
    let mut head = row![
        text("Notifications")
            .size(13)
            .font(BOLD)
            .width(Length::Fill)
    ]
    .align_y(Alignment::Center);
    if !app.notes.is_empty() {
        head = head.push(
            button(text("Clear all").size(11))
                .padding([3, 10])
                .style(theme::ghost)
                .on_press(Message::ClearNotes),
        );
    }
    container(column![head, scrollable(list).height(Length::Shrink)].spacing(10))
        .padding(14)
        .width(Length::Fill)
        .style(theme::panel)
        .into()
}

/// A live health issue: colored by severity, no timestamp (it is current
/// state, not history).
fn issue_row(issue: &modrix_core::Issue) -> El<'_> {
    let color = match issue.severity {
        modrix_core::Severity::Error => theme::DANGER,
        modrix_core::Severity::Warning => theme::ACCENT,
        modrix_core::Severity::Info => theme::INFO,
    };
    row![
        icons::dot(6.0, color),
        text(&issue.message).size(12).color(theme::TEXT),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// The duplicate-install modal: the dropped archive's content is already
/// installed. Same modal mechanics as the FOMOD wizard (opaque + backdrop).
fn dup_overlay(prompt: &DupPrompt) -> El<'_> {
    let file = prompt.path.file_name().map_or_else(
        || prompt.path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let body = column![
        text("Already installed").size(16).font(BOLD),
        text(format!(
            "{file} is the same archive as \"{}\", which is already installed.",
            prompt.existing_name
        ))
        .size(13)
        .color(theme::MUTED),
        row![
            Space::with_width(Length::Fill),
            button(text("Cancel").size(13))
                .padding([8, 16])
                .style(theme::ghost)
                .on_press(Message::DupCancel),
            button(text("Reinstall").size(13))
                .padding([8, 16])
                .style(theme::primary)
                .on_press(Message::DupConfirm),
        ]
        .spacing(10),
    ]
    .spacing(16);
    let card = container(body).padding(24).width(440).style(theme::card);
    opaque(
        container(card)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(theme::backdrop),
    )
}

fn note_row(note: &Note) -> El<'_> {
    let color = match note.tone {
        Tone::Ok => theme::OK,
        Tone::Error => theme::DANGER,
        Tone::Info => theme::INFO,
    };
    row![
        icons::dot(6.0, color),
        text(&note.text).size(12).color(theme::TEXT),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn title_of(app: &App) -> (&'static str, String) {
    let game = app
        .games
        .iter()
        .find(|g| Some(g.id) == app.selected_game)
        .map_or_else(String::new, |g| g.name.clone());
    match app.screen {
        Screen::Dashboard => ("Dashboard", game),
        Screen::Games => ("Games", format!("{} registered", app.games.len())),
        Screen::Mods => ("Mods", game),
        Screen::LoadOrder => (
            "Load Order",
            "plugins load top to bottom · masters first".to_owned(),
        ),
        Screen::Conflicts => (
            "Conflicts",
            "rules decide which mod provides each contested file".to_owned(),
        ),
        Screen::Downloads => ("Downloads", String::new()),
        Screen::Settings => ("Settings", String::new()),
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
