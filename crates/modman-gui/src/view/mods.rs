// SPDX-License-Identifier: GPL-2.0-only
//! The Mods screen: the enable/disable table plus deploy controls.

use std::collections::HashSet;

use iced::widget::{
    button, column, container, pick_list, row, scrollable, text, text_input, toggler,
};
use iced::{Alignment, Length};
use modman_core::{Mod, ModId};

use super::{BOLD, El, empty_state, labeled_card};
use crate::app::{App, Message};
use crate::theme;

/// The mods body.
pub(super) fn body(app: &App) -> El<'_> {
    let enabled: HashSet<ModId> = app.order.iter().map(|m| m.id).collect();
    column![
        toolbar(app),
        table(app, &enabled),
        stage_card(app),
    ]
    .spacing(16)
    .into()
}

fn toolbar(app: &App) -> El<'_> {
    let names: Vec<String> = app.profiles.iter().map(|p| p.name.clone()).collect();
    let active = app.active_profile.as_ref().map(|p| p.name.clone());
    let profile = pick_list(names, active, Message::ProfilePicked)
        .placeholder("profile")
        .text_size(13)
        .padding([7, 10])
        .style(theme::picker);
    let mut bar = row![
        text("Profile").size(12).color(theme::MUTED),
        profile,
        iced::widget::Space::with_width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if app.busy {
        bar = bar.push(text("Working…").size(13).color(theme::ACCENT));
    } else {
        bar = bar
            .push(action("Verify", theme::ghost, Message::Verify))
            .push(action("Purge", theme::danger_ghost, Message::Purge))
            .push(action("Deploy", theme::primary, Message::Deploy));
    }
    bar.into()
}

fn action(
    label: &str,
    style: fn(&iced::Theme, button::Status) -> button::Style,
    message: Message,
) -> El<'_> {
    button(text(label).size(13))
        .padding([8, 16])
        .style(style)
        .on_press(message)
        .into()
}

fn table<'a>(app: &'a App, enabled: &HashSet<ModId>) -> El<'a> {
    if app.mods.is_empty() {
        return empty_state(
            "No mods staged for this game yet. Click Download on nexusmods.com (with the \
             extension installed) or stage a local archive below.",
        );
    }
    let mut rows = column![].spacing(4);
    for (i, m) in app.mods.iter().enumerate() {
        rows = rows.push(mod_row(m, i, enabled.contains(&m.id)));
    }
    let listing = column![
        header_row(),
        scrollable(rows).height(Length::Fill),
        text(format!(
            "{} mod(s) · {} enabled - deploy to apply changes",
            app.mods.len(),
            app.order.len()
        ))
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
            cell("ENABLED").width(70),
            cell("MOD").width(Length::Fill),
            cell("VERSION").width(110),
            cell("SOURCE").width(90),
        ]
        .spacing(10),
    )
    .padding([4, 12])
    .into()
}

fn mod_row(m: &Mod, index: usize, on: bool) -> El<'_> {
    let switch = toggler(on)
        .on_toggle(move |now| Message::ToggleMod(m.id, now))
        .size(18)
        .style(theme::toggle);
    let version = m.version.as_deref().unwrap_or("-");
    let inner = row![
        container(switch).width(70),
        text(&m.name)
            .size(13)
            .color(if on { theme::TEXT } else { theme::MUTED })
            .width(Length::Fill),
        text(version).size(12).color(theme::MUTED).width(110),
        container(
            container(text(&m.source).size(10))
                .padding([2, 8])
                .style(theme::chip(theme::INFO))
        )
        .width(90),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    container(inner)
        .padding([7, 12])
        .style(theme::table_row(index.is_multiple_of(2)))
        .into()
}

fn stage_card(app: &App) -> El<'_> {
    let path = text_input("Path to a .zip or an extracted mod folder", &app.form.mod_path)
        .on_input(Message::ModPathChanged)
        .size(13)
        .padding(9)
        .width(Length::Fill)
        .style(theme::input);
    let name = text_input("Name (optional)", &app.form.mod_name)
        .on_input(Message::ModNameChanged)
        .on_submit(Message::AddLocalMod)
        .size(13)
        .padding(9)
        .width(Length::Fill)
        .style(theme::input);
    let add = button(text("Stage mod").size(13).font(BOLD))
        .padding([8, 16])
        .style(theme::ghost)
        .on_press(Message::AddLocalMod);
    let inner = column![
        path,
        row![name, add].spacing(10).align_y(Alignment::Center),
    ]
    .spacing(10);
    labeled_card("STAGE A LOCAL ARCHIVE", inner.into())
}
