// SPDX-License-Identifier: GPL-2.0-only
//! The Load Order screen: the game's **plugins** (.esp/.esm/.esl), the unit
//! the engine actually sorts and writes to `Plugins.txt`.
//!
//! Reorder by drag (grip handle, commit on release), by the arrow buttons, or
//! by keyboard: arrows move the cursor, Shift extends the selection, Ctrl
//! (optionally with Shift for a block) moves the selected plugin(s).

use iced::widget::{button, column, container, mouse_area, row, scrollable, text, toggler};
use iced::{Alignment, Length};
use modrix_core::plugins::GamePlugin;

use super::{BOLD, El, empty_state};
use crate::app::{App, Message, Pane};
use crate::{icons, theme};

/// The load-order body.
pub(super) fn body(app: &App) -> El<'_> {
    if app.plugins.is_empty() {
        return empty_state("No plugins - enable mods that provide .esp/.esm/.esl files.");
    }
    // Fill, not Shrink: with the default Shrink the inner Fill scrollable
    // collapses and plugins past the fold are unreachable.
    let mut page = column![].spacing(14).height(Length::Fill);
    page = page.push(toolbar(app));
    let mut rows = column![].spacing(3).width(Length::Fill);
    for (i, plugin) in app.plugins.iter().enumerate() {
        let selected = app.plugin_sel.items.contains(&i) || app.plugin_sel.cursor == Some(i);
        let drag_target = app.drag.map(|(_, to)| to) == Some(i);
        rows = rows.push(plugin_row(plugin, i, app.plugins.len(), selected, drag_target));
    }
    let list = scrollable(rows)
        .id(iced::widget::scrollable::Id::new("plugins-list"))
        .on_scroll(|v| Message::Scrolled(Pane::Plugins, v.absolute_offset().y, v.bounds().height))
        .height(Length::Fill);
    let table = container(list)
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::card);
    // Release anywhere over the list commits a drag; clicking empty space
    // clears the selection.
    page.push(
        mouse_area(table)
            .on_release(Message::DragEnd)
            .on_press(Message::ClearSelection),
    )
    .into()
}

fn toolbar(app: &App) -> El<'_> {
    let count = app.plugins.len();
    let active = app.plugins.iter().filter(|p| p.enabled).count();
    // Mods without plugins (textures, meshes, SKSE DLLs) never appear here
    // or in the game's mods menu - say so, or the count mismatch looks
    // like mods failing to load.
    let asset_only = app
        .order
        .iter()
        .filter(|m| app.plugin_counts.get(&m.id).copied().unwrap_or(0) == 0)
        .count();
    let mut bar = row![
        text(format!(
            "{active} of {count} plugins active · {asset_only} enabled mods are asset-only (no plugin)"
        ))
        .size(12)
        .color(theme::MUTED),
        iced::widget::Space::with_width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if !app.busy {
        bar = bar.push(
            button(text("Auto-sort").size(13).font(BOLD))
                .padding([8, 16])
                .style(theme::primary)
                .on_press(Message::AutoSort),
        );
    }
    bar.into()
}

fn plugin_row(plugin: &GamePlugin, index: usize, len: usize, selected: bool, drop: bool) -> El<'_> {
    let handle = mouse_area(icons::grip::<Message>())
        .interaction(iced::mouse::Interaction::Grab)
        .on_press(Message::DragStart(Pane::Plugins, index));
    let position = index.saturating_add(1);
    let arrows = row![
        arrow_button(true, (index > 0).then_some(Message::MoveSelection {
            pane: Pane::Plugins,
            delta: -1,
        })),
        arrow_button(false, (index < len.saturating_sub(1)).then_some(Message::MoveSelection {
            pane: Pane::Plugins,
            delta: 1,
        })),
    ]
    .spacing(4);
    let activate = toggler(plugin.enabled)
        .size(15)
        .on_toggle(move |_| Message::TogglePlugin(index))
        .style(theme::toggle);
    let inner = row![
        handle,
        activate,
        container(text(format!("{position:>3}")).size(11).font(BOLD).color(theme::ACCENT))
            .padding([3, 8])
            .style(theme::chip(theme::ACCENT)),
        name_column(plugin),
        container(tier_chip(plugin)).width(44),
        arrows,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let style: fn(&iced::Theme) -> iced::widget::container::Style = if drop {
        theme::table_row_dragging
    } else if selected {
        theme::table_row_selected
    } else if index.is_multiple_of(2) {
        |t| theme::table_row(true)(t)
    } else {
        |t| theme::table_row(false)(t)
    };
    let styled = container(inner).width(Length::Fill).padding([6, 12]).style(style);
    mouse_area(styled)
        .on_press(Message::RowClick {
            pane: Pane::Plugins,
            index,
        })
        .on_enter(Message::DragOver(index))
        .into()
}

/// The plugin's name, mod origin, and any missing-master warning.
fn name_column(plugin: &GamePlugin) -> El<'_> {
    let name_color = if plugin.missing_masters.is_empty() {
        if plugin.enabled { theme::TEXT } else { theme::MUTED }
    } else {
        theme::DANGER
    };
    let mut col = column![
        text(&plugin.name).size(13).color(name_color),
        text(&plugin.mod_name).size(10).color(theme::FAINT),
    ]
    .spacing(1)
    .width(Length::Fill);
    if !plugin.missing_masters.is_empty() {
        col = col.push(
            text(format!("missing: {}", plugin.missing_masters.join(", ")))
                .size(10)
                .color(theme::DANGER),
        );
    }
    col.into()
}

fn tier_chip(plugin: &GamePlugin) -> El<'static> {
    let (label, color) = if plugin.is_light {
        ("ESL", theme::INFO)
    } else if plugin.is_master {
        ("ESM", theme::ACCENT)
    } else {
        return iced::widget::Space::with_width(0).into();
    };
    container(text(label).size(9))
        .padding([1, 6])
        .style(theme::chip(color))
        .into()
}

fn arrow_button(up: bool, message: Option<Message>) -> El<'static> {
    button(icons::arrow::<Message>(up))
        .padding([6, 8])
        .style(theme::icon)
        .on_press_maybe(message)
        .into()
}
