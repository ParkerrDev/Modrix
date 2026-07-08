// SPDX-License-Identifier: GPL-2.0-only
//! The FOMOD wizard: a modal that walks the installer's steps and groups.

use iced::widget::{button, checkbox, column, container, mouse_area, opaque, row, scrollable, text};
use iced::{Alignment, Length};
use modman_plugin::fomod;

use super::{BOLD, El};
use crate::app::{App, Message, Wizard};
use crate::theme;

/// The dimmed backdrop + centered wizard card.
///
/// The whole layer is `opaque` so no event leaks through to the UI below,
/// and there is deliberately **no** click-outside-to-dismiss: passive spots
/// inside the card do not capture presses, so a backdrop dismiss handler
/// would swallow ordinary clicks and close the wizard. Only `×` closes.
pub(super) fn overlay<'a>(_app: &'a App, wizard: &'a Wizard) -> El<'a> {
    let card = container(card_body(wizard))
        .padding(24)
        .width(680)
        .max_height(720)
        .style(theme::card);
    opaque(
        container(card)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(theme::backdrop),
    )
}

fn card_body(wizard: &Wizard) -> El<'_> {
    let visible = wizard.visible_steps();
    let page = wizard.step.min(visible.len().saturating_sub(1));
    let Some(step_index) = visible.get(page).copied() else {
        return finish_only(wizard);
    };
    let flags = fomod::flags_of(&wizard.installer, &wizard.selections);
    let step = wizard.installer.steps.get(step_index);
    let step_name = step.map_or("", |s| s.name.as_str());
    let mut groups = column![].spacing(16);
    if let Some(step) = step {
        for (g, group) in step.groups.iter().enumerate() {
            groups = groups.push(group_view(wizard, step_index, g, group, &flags));
        }
    }
    let panes = row![
        // Fixed viewport: a Fill scrollable inside a shrink-sized modal
        // collapses to zero height in iced - never use Fill here.
        scrollable(groups).height(440).width(Length::Fill),
        preview(wizard, step_index),
    ]
    .spacing(18);
    column![
        header(wizard, step_name, page, visible.len()),
        panes,
        footer(page, visible.len()),
    ]
    .spacing(16)
    .into()
}

/// The right pane: image + description of the focused option (Vortex-style).
fn preview(wizard: &Wizard, current_step: usize) -> El<'_> {
    let plugin = focused_plugin(wizard, current_step);
    let mut pane = column![].spacing(10).width(300);
    let image_rel = plugin
        .and_then(|p| p.image.as_deref())
        .or(wizard.installer.module_image.as_deref());
    if let Some(rel) = image_rel
        && let Some(path) = fomod::source_path(&wizard.staged_path, rel)
    {
        pane = pane.push(iced::widget::image(path).width(Length::Fill));
    }
    match plugin {
        Some(p) => {
            pane = pane.push(text(&p.name).size(13).font(BOLD));
            let description = p.description.trim();
            if !description.is_empty() {
                pane = pane.push(
                    scrollable(text(compact(description)).size(12).color(theme::MUTED))
                        .height(Length::Fill),
                );
            }
        }
        None => {
            pane = pane.push(
                text("Hover an option to preview it")
                    .size(12)
                    .color(theme::FAINT),
            );
        }
    }
    container(pane)
        .padding(12)
        .height(440)
        .style(theme::inset)
        .into()
}

/// The plugin the preview shows: the hovered/last-touched one, else the first
/// selected on the current page.
fn focused_plugin(wizard: &Wizard, current_step: usize) -> Option<&fomod::Plugin> {
    let lookup = |(s, g, p): (usize, usize, usize)| {
        wizard
            .installer
            .steps
            .get(s)?
            .groups
            .get(g)?
            .plugins
            .get(p)
    };
    if let Some(focus) = wizard.focus
        && let Some(plugin) = lookup(focus)
    {
        return Some(plugin);
    }
    let step = wizard.installer.steps.get(current_step)?;
    for (g, group) in step.groups.iter().enumerate() {
        if let Some(first) = wizard
            .selections
            .get(current_step)
            .and_then(|sg| sg.get(g))
            .and_then(|sel| sel.first())
            && let Some(plugin) = group.plugins.get(*first)
        {
            return Some(plugin);
        }
    }
    None
}

fn header<'a>(wizard: &'a Wizard, step_name: &'a str, page: usize, pages: usize) -> El<'a> {
    let progress = format!("{} / {pages}", page.saturating_add(1));
    column![
        row![
            text(&wizard.mod_name).size(17).font(BOLD).width(Length::Fill),
            button(text("×").size(15))
                .padding([2, 10])
                .style(theme::icon)
                .on_press(Message::WizardCancel),
        ]
        .align_y(Alignment::Center),
        row![
            text(step_name).size(13).color(theme::ACCENT),
            iced::widget::Space::with_width(Length::Fill),
            text(progress).size(12).color(theme::FAINT),
        ],
    ]
    .spacing(6)
    .into()
}

fn footer(page: usize, pages: usize) -> El<'static> {
    let last = page.saturating_add(1) >= pages;
    let mut bar = row![].spacing(10).align_y(Alignment::Center);
    if page > 0 {
        bar = bar.push(
            button(text("Back").size(13))
                .padding([8, 16])
                .style(theme::ghost)
                .on_press(Message::WizardBack),
        );
    }
    bar = bar.push(iced::widget::Space::with_width(Length::Fill));
    bar = bar.push(if last {
        button(text("Install").size(13).font(BOLD))
            .padding([8, 20])
            .style(theme::primary)
            .on_press(Message::WizardFinish)
    } else {
        button(text("Next").size(13))
            .padding([8, 20])
            .style(theme::primary)
            .on_press(Message::WizardNext)
    });
    bar.into()
}

fn finish_only(wizard: &Wizard) -> El<'_> {
    column![
        text(&wizard.mod_name).size(17).font(BOLD),
        text("No options to choose").size(13).color(theme::MUTED),
        button(text("Install").size(13))
            .padding([8, 20])
            .style(theme::primary)
            .on_press(Message::WizardFinish),
    ]
    .spacing(14)
    .into()
}

fn group_view<'a>(
    wizard: &'a Wizard,
    step: usize,
    g: usize,
    group: &'a fomod::Group,
    flags: &std::collections::HashMap<String, String>,
) -> El<'a> {
    let mut items = column![].spacing(6);
    let selected = wizard
        .selections
        .get(step)
        .and_then(|s| s.get(g));
    for (p, plugin) in group.plugins.iter().enumerate() {
        let slot = Slot {
            step,
            group: g,
            plugin: p,
            kind: fomod::plugin_kind(&plugin.kind, flags),
            on: selected.is_some_and(|s| s.contains(&p)),
            group_kind: group.kind,
        };
        items = items.push(plugin_row(plugin, slot));
    }
    column![
        row![
            text(&group.name).size(13).font(BOLD),
            text(rule_hint(group.kind)).size(11).color(theme::FAINT),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        items,
    ]
    .spacing(8)
    .into()
}

const fn rule_hint(kind: fomod::GroupKind) -> &'static str {
    match kind {
        fomod::GroupKind::ExactlyOne => "pick one",
        fomod::GroupKind::AtMostOne => "pick one or none",
        fomod::GroupKind::AtLeastOne => "pick at least one",
        fomod::GroupKind::Any => "pick any",
        fomod::GroupKind::All => "all included",
    }
}

/// Everything a plugin row needs to render and route its toggle.
#[derive(Clone, Copy)]
struct Slot {
    step: usize,
    group: usize,
    plugin: usize,
    kind: fomod::PluginKind,
    on: bool,
    group_kind: fomod::GroupKind,
}

fn plugin_row(plugin: &fomod::Plugin, slot: Slot) -> El<'_> {
    let locked = matches!(
        slot.kind,
        fomod::PluginKind::Required | fomod::PluginKind::NotUsable
    ) || slot.group_kind == fomod::GroupKind::All;
    let mut check = checkbox(&plugin.name, slot.on).size(16).text_size(13);
    if !locked {
        let (step, group, plugin) = (slot.step, slot.group, slot.plugin);
        check = check.on_toggle(move |_| Message::WizardPick { step, group, plugin });
    }
    let mut head = row![check].spacing(8).align_y(Alignment::Center);
    if let Some((label, color)) = kind_chip(slot.kind) {
        head = head.push(
            container(text(label).size(9))
                .padding([1, 6])
                .style(theme::chip(color)),
        );
    }
    mouse_area(head)
        .on_enter(Message::WizardHover {
            step: slot.step,
            group: slot.group,
            plugin: slot.plugin,
        })
        .into()
}

const fn kind_chip(kind: fomod::PluginKind) -> Option<(&'static str, iced::Color)> {
    match kind {
        fomod::PluginKind::Required => Some(("REQUIRED", theme::ACCENT)),
        fomod::PluginKind::Recommended => Some(("RECOMMENDED", theme::OK)),
        fomod::PluginKind::NotUsable => Some(("UNAVAILABLE", theme::DANGER)),
        fomod::PluginKind::CouldBeUsable => Some(("CHECK NOTES", theme::INFO)),
        fomod::PluginKind::Optional => None,
    }
}

/// Collapse installer descriptions (often whole paragraphs of whitespace)
/// into a short readable line.
fn compact(description: &str) -> String {
    let mut out = String::with_capacity(description.len().min(1200));
    let mut last_space = true;
    for ch in description.chars().take(1200) {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    if description.chars().count() > 1200 {
        out.push('…');
    }
    out
}
