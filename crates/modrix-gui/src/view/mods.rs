// SPDX-License-Identifier: GPL-2.0-only
//! The Mods screen: the mod table, bulk actions, and the install drop zone.
//!
//! Selection: click a row (Shift+click or Shift+arrows for a range,
//! Ctrl+click to toggle); clicking empty space or pressing Escape clears.

use std::collections::HashSet;

use iced::widget::{
    button, column, container, mouse_area, pick_list, row, scrollable, text, toggler,
};
use iced::{Alignment, Length};
use modrix_core::{ExternalMod, Mod, ModId};

use super::{BOLD, El, empty_state};
use crate::app::{App, Message, Pane, SortKey};
use crate::{icons, theme};

/// The mods body.
pub(super) fn body(app: &App) -> El<'_> {
    let enabled: HashSet<ModId> = app.order.iter().map(|m| m.id).collect();
    // A click on the surrounding container (not a row) clears the selection.
    let cleared = mouse_area(
        column![toolbar(app, &enabled), table(app, &enabled), drop_zone()]
            .spacing(14)
            .height(Length::Fill),
    )
    .on_press(Message::ClearSelection);
    cleared.into()
}

fn toolbar<'a>(app: &'a App, enabled: &HashSet<ModId>) -> El<'a> {
    if !app.mod_sel.items.is_empty() {
        return selection_bar(app);
    }
    let names: Vec<String> = app.profiles.iter().map(|p| p.name.clone()).collect();
    let active = app.active_profile.as_ref().map(|p| p.name.clone());
    let profile = pick_list(names, active, Message::ProfilePicked)
        .placeholder("profile")
        .text_size(13)
        .padding([7, 10])
        .style(theme::picker);
    let mut bar = row![profile, iced::widget::Space::with_width(Length::Fill)]
        .spacing(10)
        .align_y(Alignment::Center);
    let disabled = app.mods.len().saturating_sub(enabled.len());
    if disabled > 0 {
        bar = bar.push(action(
            format!("Enable all ({disabled})"),
            theme::ghost,
            Message::EnableAll,
        ));
    }
    if !app.busy {
        bar = bar
            .push(action("Verify".to_owned(), theme::ghost, Message::Verify))
            .push(action(
                "Purge".to_owned(),
                theme::danger_ghost,
                Message::Purge,
            ));
        // Deploy is gated: unresolved conflicts or dependency errors send
        // the user to Conflicts instead of deploying a broken setup.
        let blockers = app.health.iter().filter(|i| i.blocking).count();
        bar = bar.push(if blockers > 0 {
            action(
                format!("Deploy blocked ({blockers})"),
                theme::danger_ghost,
                Message::Navigate(crate::app::Screen::Conflicts),
            )
        } else {
            action("Deploy".to_owned(), theme::primary, Message::Deploy)
        });
    }
    bar.into()
}

/// The bar shown while rows are selected: mass enable/disable/delete/reinstall.
fn selection_bar(app: &App) -> El<'_> {
    let n = app.mod_sel.items.len();
    row![
        text(format!("{n} selected"))
            .size(13)
            .font(BOLD)
            .color(theme::ACCENT),
        iced::widget::Space::with_width(Length::Fill),
        action(
            "Enable".to_owned(),
            theme::ghost,
            Message::SetSelectedEnabled(true)
        ),
        action(
            "Disable".to_owned(),
            theme::ghost,
            Message::SetSelectedEnabled(false)
        ),
        action(
            "Reinstall".to_owned(),
            theme::ghost,
            Message::ReinstallSelected
        ),
        action(
            "Delete".to_owned(),
            theme::danger_ghost,
            Message::DeleteSelected
        ),
        button(text("×").size(14))
            .padding([4, 10])
            .style(theme::icon)
            .on_press(Message::ClearSelection),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn action(
    label: String,
    style: fn(&iced::Theme, button::Status) -> button::Style,
    message: Message,
) -> El<'static> {
    button(text(label).size(13))
        .padding([8, 16])
        .style(style)
        .on_press(message)
        .into()
}

fn table<'a>(app: &'a App, enabled: &HashSet<ModId>) -> El<'a> {
    if app.mods.is_empty() && app.external.is_empty() {
        return empty_state("No mods yet.");
    }
    let mut rows = column![].spacing(4).width(Length::Fill);
    for (i, m) in app.mods.iter().enumerate() {
        rows = rows.push(mod_row(
            m,
            i,
            enabled.contains(&m.id),
            app.mod_sel.items.contains(&i),
        ));
    }
    // External mods live in the same scroll region, past a divider - visible
    // but read-only (no toggle, not selectable), so the user sees what is in
    // the game folder that Modrix did not put there.
    if !app.external.is_empty() {
        rows = rows.push(external_header(app.external.len()));
        for m in &app.external {
            rows = rows.push(external_row(m));
        }
    }
    let listing = column![
        header_row(app),
        scrollable(rows)
            .id(iced::widget::scrollable::Id::new("mods-list"))
            .on_scroll(|v| {
                Message::Scrolled(Pane::Mods, v.absolute_offset().y, v.bounds().height)
            })
            .height(Length::Fill),
        text(summary_line(app)).size(12).color(theme::FAINT),
    ]
    .spacing(8);
    container(listing)
        .padding(14)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::card)
        .into()
}

fn summary_line(app: &App) -> String {
    let mut line = format!(
        "{} mods · {} enabled · {} plugins · {} mods are asset-only",
        app.mods.len(),
        app.order.len(),
        app.plugin_counts.values().sum::<usize>(),
        app.plugin_counts.values().filter(|c| **c == 0).count(),
    );
    if !app.external.is_empty() {
        use std::fmt::Write as _;
        let _ = write!(line, " · {} external (not managed)", app.external.len());
    }
    line
}

/// The divider that separates managed rows from the read-only external list.
fn external_header(count: usize) -> El<'static> {
    let plural = if count == 1 { "" } else { "s" };
    container(
        column![
            text(format!(
                "EXTERNAL - {count} mod{plural} already installed, not managed by Modrix"
            ))
            .size(10)
            .color(theme::ACCENT),
            text(
                "Shown so you know they are here. Modrix does not enable, order, or remove \
                 them - manage them where you installed them."
            )
            .size(10)
            .color(theme::FAINT),
        ]
        .spacing(2),
    )
    .padding([12, 12])
    .width(Length::Fill)
    .into()
}

/// A read-only external-mod row: no toggle, not selectable, muted.
fn external_row(m: &ExternalMod) -> El<'_> {
    let files = format!(
        "{} · {} file{}",
        m.kind.label(),
        m.files,
        if m.files == 1 { "" } else { "s" }
    );
    let inner = row![
        container(text("-").size(13).color(theme::FAINT)).width(48),
        column![
            text(&m.name).size(13).color(theme::MUTED),
            text(files).size(10).color(theme::FAINT),
        ]
        .spacing(2)
        .width(Length::Fill),
        container(text("EXTERNAL").size(10))
            .padding([2, 8])
            .style(theme::chip(theme::FAINT)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    container(inner)
        .width(Length::Fill)
        .padding([7, 12])
        .style(theme::inset)
        .into()
}

/// Clickable column headers: click sorts, click again flips direction.
fn header_row(app: &App) -> El<'_> {
    container(
        row![
            sort_cell(app, "ON", SortKey::Enabled, 48),
            sort_cell(app, "MOD", SortKey::Name, 0),
            sort_cell(app, "ADDED", SortKey::Installed, 90),
            sort_cell(app, "VERSION", SortKey::Version, 100),
            sort_cell(app, "SOURCE", SortKey::Source, 150),
        ]
        .spacing(10),
    )
    .padding([4, 12])
    .into()
}

/// A compact "how long ago" for the ADDED column. Rows that predate the
/// provenance migration have no timestamp and show a dash.
fn added_label(created_at: Option<i64>) -> String {
    let Some(at) = created_at else {
        return "-".to_owned();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let elapsed = now.saturating_sub(at).max(0);
    match elapsed {
        s if s < 60 => "just now".to_owned(),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s if s < 2_592_000 => format!("{}d ago", s / 86_400),
        s => format!("{}mo ago", s / 2_592_000),
    }
}

fn sort_cell<'a>(app: &App, label: &'a str, key: SortKey, width: u16) -> El<'a> {
    let (active_key, ascending) = app.mod_sort;
    let mut cell = row![text(label).size(10).color(if active_key == key {
        theme::ACCENT
    } else {
        theme::FAINT
    })]
    .spacing(4)
    .align_y(Alignment::Center);
    if active_key == key {
        cell = cell.push(icons::arrow::<Message>(ascending));
    }
    let clickable = mouse_area(cell)
        .interaction(iced::mouse::Interaction::Pointer)
        .on_press(Message::SortBy(key));
    if width == 0 {
        container(clickable).width(Length::Fill).into()
    } else {
        container(clickable).width(width).into()
    }
}

fn mod_row(m: &Mod, index: usize, enabled: bool, selected: bool) -> El<'_> {
    let switch = toggler(enabled)
        .on_toggle(move |now| Message::ToggleMod(m.id, now))
        .size(18)
        .style(theme::toggle);
    let mut tail = row![
        container(text(&m.source).size(10))
            .padding([2, 8])
            .style(theme::chip(theme::INFO)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if m.install_state.starts_with("fomod") {
        tail = tail.push(
            button(text("Options").size(11))
                .padding([3, 10])
                .style(theme::ghost)
                .on_press(Message::Configure(m.id)),
        );
    }
    let inner = row![
        container(switch).width(48),
        text(&m.name)
            .size(13)
            .color(if enabled { theme::TEXT } else { theme::MUTED })
            .width(Length::Fill),
        text(added_label(m.created_at))
            .size(12)
            .color(theme::FAINT)
            .width(90),
        text(m.version.as_deref().unwrap_or("-"))
            .size(12)
            .color(theme::MUTED)
            .width(100),
        container(tail).width(150),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let styled = container(inner)
        .width(Length::Fill)
        .padding([7, 12])
        .style(if selected {
            theme::table_row_selected as fn(&iced::Theme) -> iced::widget::container::Style
        } else if index.is_multiple_of(2) {
            |t: &iced::Theme| theme::table_row(true)(t)
        } else {
            |t: &iced::Theme| theme::table_row(false)(t)
        });
    mouse_area(styled)
        .on_press(Message::RowClick {
            pane: Pane::Mods,
            index,
        })
        .into()
}

/// Click to browse, or drop archives anywhere on the window.
fn drop_zone() -> El<'static> {
    let inner = column![
        text("Add mods").size(13).font(BOLD).color(theme::ACCENT),
        text("Drop archives here, or click to browse")
            .size(12)
            .color(theme::MUTED),
    ]
    .spacing(4)
    .align_x(Alignment::Center);
    mouse_area(
        container(inner)
            .padding([18, 24])
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .style(theme::drop_zone),
    )
    .interaction(iced::mouse::Interaction::Pointer)
    .on_press(Message::PickFiles)
    .into()
}
