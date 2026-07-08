// SPDX-License-Identifier: GPL-2.0-only
//! The Load Order screen: reorder enabled mods; later entries win conflicts.

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Alignment, Length};
use modman_core::Mod;

use super::{BOLD, El, empty_state};
use crate::app::{App, Message};
use crate::theme;

/// The load-order body.
pub(super) fn body(app: &App) -> El<'_> {
    if app.order.is_empty() {
        return empty_state(
            "Nothing to order - enable at least one mod on the Mods screen first.",
        );
    }
    let last = app.order.len().saturating_sub(1);
    let mut rows = column![].spacing(4);
    for (i, m) in app.order.iter().enumerate() {
        rows = rows.push(order_row(m, i, last));
    }
    let inner = column![
        scrollable(rows).height(Length::Fill),
        text("Files from mods lower in the list overwrite the ones above - deploy to apply.")
            .size(12)
            .color(theme::FAINT),
    ]
    .spacing(10);
    container(inner)
        .padding(14)
        .height(Length::Fill)
        .style(theme::card)
        .into()
}

fn order_row(m: &Mod, index: usize, last: usize) -> El<'_> {
    let position = index.saturating_add(1);
    let arrows = row![
        arrow("Up", (index > 0).then_some(Message::MoveMod(index, -1))),
        arrow("Down", (index < last).then_some(Message::MoveMod(index, 1))),
    ]
    .spacing(4);
    let inner = row![
        container(text(format!("{position:>2}")).size(12).font(BOLD).color(theme::ACCENT))
            .padding([3, 9])
            .style(theme::chip(theme::ACCENT)),
        text(&m.name).size(13).width(Length::Fill),
        text(m.version.as_deref().unwrap_or("-"))
            .size(12)
            .color(theme::MUTED),
        arrows,
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    container(inner)
        .padding([7, 12])
        .style(theme::table_row(index.is_multiple_of(2)))
        .into()
}

fn arrow(label: &str, message: Option<Message>) -> El<'_> {
    button(text(label).size(11))
        .padding([4, 9])
        .style(theme::icon)
        .on_press_maybe(message)
        .into()
}
