// SPDX-License-Identifier: GPL-2.0-only
//! The Mods screen: selectable table, bulk actions, and the install drop zone.

use std::collections::HashSet;

use iced::widget::{
    button, column, container, mouse_area, pick_list, row, scrollable, text, toggler,
};
use iced::{Alignment, Length};
use modman_core::{Mod, ModId};

use super::{BOLD, El, empty_state};
use crate::app::{App, Message};
use crate::theme;

/// The mods body.
pub(super) fn body(app: &App) -> El<'_> {
    let enabled: HashSet<ModId> = app.order.iter().map(|m| m.id).collect();
    column![toolbar(app, &enabled), table(app, &enabled), drop_zone()]
        .spacing(14)
        .into()
}

fn toolbar<'a>(app: &'a App, enabled: &HashSet<ModId>) -> El<'a> {
    if !app.selection.is_empty() {
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
            .push(action("Purge".to_owned(), theme::danger_ghost, Message::Purge))
            .push(action("Deploy".to_owned(), theme::primary, Message::Deploy));
    }
    bar.into()
}

/// The bar shown while rows are selected: mass enable/disable/delete/reinstall.
fn selection_bar(app: &App) -> El<'_> {
    let n = app.selection.len();
    row![
        text(format!("{n} selected")).size(13).font(BOLD).color(theme::ACCENT),
        iced::widget::Space::with_width(Length::Fill),
        action("Enable".to_owned(), theme::ghost, Message::SetSelectedEnabled(true)),
        action("Disable".to_owned(), theme::ghost, Message::SetSelectedEnabled(false)),
        action("Reinstall".to_owned(), theme::ghost, Message::ReinstallSelected),
        action("Delete".to_owned(), theme::danger_ghost, Message::DeleteSelected),
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
    if app.mods.is_empty() {
        return empty_state("No mods yet.");
    }
    let mut rows = column![].spacing(4);
    for (i, m) in app.mods.iter().enumerate() {
        rows = rows.push(mod_row(
            m,
            i,
            enabled.contains(&m.id),
            app.selection.contains(&m.id),
        ));
    }
    let listing = column![
        header_row(),
        scrollable(rows).height(Length::Fill),
        text(format!("{} mods · {} enabled", app.mods.len(), app.order.len()))
            .size(12)
            .color(theme::FAINT),
    ]
    .spacing(8);
    container(listing)
        .padding(14)
        .height(Length::Fill)
        .style(theme::card)
        .into()
}

fn header_row() -> El<'static> {
    let cell = |label: &'static str| text(label).size(10).color(theme::FAINT);
    container(
        row![
            cell("ON").width(56),
            cell("MOD").width(Length::Fill),
            cell("VERSION").width(100),
            cell("SOURCE").width(150),
        ]
        .spacing(10),
    )
    .padding([4, 12])
    .into()
}

fn mod_row(m: &Mod, index: usize, on: bool, selected: bool) -> El<'_> {
    let switch = toggler(on)
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
    if m.install_state == "fomod" {
        tail = tail.push(
            button(text("Options").size(11))
                .padding([3, 10])
                .style(theme::ghost)
                .on_press(Message::Configure(m.id)),
        );
    }
    let inner = row![
        container(switch).width(56),
        text(&m.name)
            .size(13)
            .color(if on { theme::TEXT } else { theme::MUTED })
            .width(Length::Fill),
        text(m.version.as_deref().unwrap_or("-"))
            .size(12)
            .color(theme::MUTED)
            .width(100),
        container(tail).width(150),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let styled = container(inner).padding([7, 12]).style(if selected {
        theme::table_row_selected as fn(&iced::Theme) -> iced::widget::container::Style
    } else if index.is_multiple_of(2) {
        |t: &iced::Theme| theme::table_row(true)(t)
    } else {
        |t: &iced::Theme| theme::table_row(false)(t)
    });
    mouse_area(styled)
        .on_press(Message::RowClicked(m.id))
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
