// SPDX-License-Identifier: GPL-2.0-only
//! The Load Order screen: drag (or arrow) to reorder; conflicts up top.

use iced::widget::{button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Length};
use modman_core::Mod;

use super::{BOLD, El, empty_state};
use crate::app::{App, Message};
use crate::{icons, theme};

/// The load-order body.
pub(super) fn body(app: &App) -> El<'_> {
    if app.order.is_empty() {
        return empty_state("Enable mods first.");
    }
    let mut page = column![].spacing(14);
    page = page.push(conflicts(app));
    let last = app.order.len().saturating_sub(1);
    let mut rows = column![].spacing(4);
    for (i, m) in app.order.iter().enumerate() {
        rows = rows.push(order_row(m, i, last, app.drag == Some(i)));
    }
    let table = container(scrollable(rows).height(Length::Fill))
        .padding(14)
        .height(Length::Fill)
        .style(theme::card);
    // Releasing anywhere over the list commits the drag.
    page.push(mouse_area(table).on_release(Message::DragEnd)).into()
}

fn conflicts(app: &App) -> El<'_> {
    if app.conflicts.is_empty() {
        return container(text("No conflicts").size(12).color(theme::OK))
            .padding([8, 14])
            .width(Length::Fill)
            .style(theme::inset)
            .into();
    }
    let mut list = column![].spacing(4);
    for c in app.conflicts.iter().take(12) {
        list = list.push(
            row![
                text(&c.winner).size(12).font(BOLD),
                text("overrides").size(12).color(theme::FAINT),
                text(format!("{} files", c.files)).size(12).color(theme::ACCENT),
                text("from").size(12).color(theme::FAINT),
                text(&c.loser).size(12).color(theme::MUTED),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }
    let extra = app.conflicts.len().saturating_sub(12);
    if extra > 0 {
        list = list.push(text(format!("+{extra} more")).size(11).color(theme::FAINT));
    }
    container(column![text("CONFLICTS").size(10).color(theme::FAINT), list].spacing(8))
        .padding([10, 14])
        .width(Length::Fill)
        .style(theme::inset)
        .into()
}

fn order_row(m: &Mod, index: usize, last: usize, dragging: bool) -> El<'_> {
    let handle = mouse_area(icons::grip::<Message>())
        .interaction(iced::mouse::Interaction::Grab)
        .on_press(Message::DragStart(index));
    let position = index.saturating_add(1);
    let arrows = row![
        arrow_button(true, (index > 0).then_some(Message::MoveMod(index, -1))),
        arrow_button(false, (index < last).then_some(Message::MoveMod(index, 1))),
    ]
    .spacing(4);
    let inner = row![
        handle,
        container(text(format!("{position:>2}")).size(12).font(BOLD).color(theme::ACCENT))
            .padding([3, 9])
            .style(theme::chip(theme::ACCENT)),
        text(&m.name).size(13).width(Length::Fill),
        text(m.version.as_deref().unwrap_or(""))
            .size(12)
            .color(theme::MUTED),
        arrows,
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let styled = container(inner).padding([7, 12]).style(if dragging {
        theme::table_row_dragging as fn(&iced::Theme) -> iced::widget::container::Style
    } else if index.is_multiple_of(2) {
        |t: &iced::Theme| theme::table_row(true)(t)
    } else {
        |t: &iced::Theme| theme::table_row(false)(t)
    });
    mouse_area(styled)
        .on_enter(Message::DragOver(index))
        .on_release(Message::DragEnd)
        .into()
}

fn arrow_button(up: bool, message: Option<Message>) -> El<'static> {
    button(icons::arrow::<Message>(up))
        .padding([6, 8])
        .style(theme::icon)
        .on_press_maybe(message)
        .into()
}
