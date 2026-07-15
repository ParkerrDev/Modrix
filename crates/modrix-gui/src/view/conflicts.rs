// SPDX-License-Identifier: GPL-2.0-only
//! The Conflicts screen: every pair of mods contesting files, with the rule
//! that resolves them (Vortex model).
//!
//! Red dot = no rule (install order decides, unresolved - deploy is blocked).
//! Green dot = a rule or full per-file pins cover the pair. Expanding a pair
//! lists the contested files; each can be pinned to either side.

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Alignment, Length};
use modrix_core::{ModConflict, ModId};

use super::{BOLD, El, empty_state};
use crate::app::{App, Message};
use crate::{icons, theme};

/// Most contested files listed inside an expanded pair.
const MAX_FILES_SHOWN: usize = 300;

/// The conflicts body.
pub(super) fn body(app: &App) -> El<'_> {
    let mut page = column![].spacing(14).height(Length::Fill);
    if let Some(blockers) = blockers_card(app) {
        page = page.push(blockers);
    }
    if app.conflicts.is_empty() {
        return page
            .push(empty_state("No file conflicts between enabled mods."))
            .into();
    }
    page = page.push(summary(app));
    let mut rows = column![].spacing(6).width(Length::Fill);
    for conflict in &app.conflicts {
        rows = rows.push(pair_card(app, conflict));
    }
    let list = scrollable(rows).height(Length::Fill);
    page.push(
        container(list)
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::card),
    )
    .into()
}

/// Everything currently blocking a deploy, so the user sees why the button
/// is off and what to fix - conflicts, missing masters, rule cycles.
fn blockers_card(app: &App) -> Option<El<'_>> {
    let blocking: Vec<&modrix_core::Issue> = app.health.iter().filter(|i| i.blocking).collect();
    if blocking.is_empty() {
        return None;
    }
    let mut list = column![
        text("DEPLOY BLOCKED UNTIL RESOLVED")
            .size(10)
            .color(theme::DANGER)
    ]
    .spacing(6);
    for issue in blocking.iter().take(8) {
        list = list.push(
            row![
                icons::dot(6.0, theme::DANGER),
                text(&issue.message).size(12).color(theme::TEXT),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    Some(
        container(list)
            .padding([10, 14])
            .width(Length::Fill)
            .style(theme::inset)
            .into(),
    )
}

fn summary(app: &App) -> El<'_> {
    let total = app.conflicts.len();
    let unresolved = app.conflicts.iter().filter(|c| !c.resolved()).count();
    let label = if unresolved == 0 {
        format!("{total} conflicting pairs, all resolved")
    } else {
        format!("{total} conflicting pairs · {unresolved} need a rule")
    };
    let color = if unresolved == 0 {
        theme::OK
    } else {
        theme::DANGER
    };
    row![
        icons::dot(8.0, color),
        text(label).size(13).color(theme::MUTED),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// One conflicting pair: status, names, rule buttons, and the file list
/// when expanded. Clicking the row (outside its buttons) expands it.
fn pair_card<'a>(app: &'a App, conflict: &'a ModConflict) -> El<'a> {
    let (a, b) = (conflict.first, conflict.second);
    let (name_a, name_b) = (app.mod_name(a), app.mod_name(b));
    let expanded = app.expanded_conflict == Some((a, b));
    let mut card = column![head_row(conflict, &name_a, &name_b)].spacing(8);
    if expanded {
        card = card.push(file_list(conflict, &name_a, &name_b));
    }
    iced::widget::mouse_area(
        container(card)
            .padding([8, 12])
            .width(Length::Fill)
            .style(theme::inset),
    )
    .on_press(Message::ExpandConflict(a, b))
    .into()
}

fn head_row(conflict: &ModConflict, name_a: &str, name_b: &str) -> El<'static> {
    let a = conflict.first;
    let dot = icons::dot(
        8.0,
        if conflict.resolved() {
            theme::OK
        } else {
            theme::DANGER
        },
    );
    let files = conflict.files.len();
    let (state, color) = match conflict.rule {
        Some(rule) => {
            let winner = if rule.winner == a { &name_a } else { &name_b };
            (format!("{files} files · rule: {winner} wins"), theme::OK)
        }
        None if conflict.resolved() => (format!("{files} files · all pinned"), theme::OK),
        None => (
            format!("{files} files · no rule · install order decides"),
            theme::DANGER,
        ),
    };
    // Stacked lines, never side-by-side with the wide rule buttons: sharing
    // a row squeezes the title into a sliver on narrow windows.
    column![
        row![
            dot,
            text(format!("{name_a}  ×  {name_b}"))
                .size(13)
                .font(BOLD)
                .width(Length::Fill),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        text(state).size(11).color(color),
        rule_controls(conflict, name_a, name_b),
    ]
    .spacing(6)
    .into()
}

/// The "A wins" / "B wins" rule buttons (the active side highlighted), plus
/// "clear" when a rule exists.
fn rule_controls(conflict: &ModConflict, name_a: &str, name_b: &str) -> El<'static> {
    let (a, b) = (conflict.first, conflict.second);
    let choice = |label: String, loser: ModId, winner: ModId, active: bool| {
        button(text(label).size(11))
            .padding([4, 10])
            .style(if active { theme::primary } else { theme::ghost })
            .on_press(Message::SetRule { loser, winner })
    };
    let mut controls = row![
        choice(
            format!("{} wins", short(name_a, 20)),
            b,
            a,
            conflict.rule.is_some_and(|r| r.winner == a),
        ),
        choice(
            format!("{} wins", short(name_b, 20)),
            a,
            b,
            conflict.rule.is_some_and(|r| r.winner == b),
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if conflict.rule.is_some() {
        controls = controls.push(
            button(text("clear").size(11))
                .padding([4, 10])
                .style(theme::danger_ghost)
                .on_press(Message::ClearRule(a, b)),
        );
    }
    controls.into()
}

/// The first `max` characters of a name (char-safe, for button labels).
fn short(name: &str, max: usize) -> String {
    name.chars().take(max).collect()
}

/// The contested files of an expanded pair, each pinnable to either side.
fn file_list<'a>(conflict: &'a ModConflict, name_a: &str, name_b: &str) -> El<'a> {
    let (a, b) = (conflict.first, conflict.second);
    let (short_a, short_b) = (short(name_a, 24), short(name_b, 24));
    let mut list = column![].spacing(3);
    for file in conflict.files.iter().take(MAX_FILES_SHOWN) {
        let winner_name = if file.winner == a { &short_a } else { &short_b };
        let pin = |label: String, provider: ModId, active: bool| {
            button(text(label).size(10))
                .padding([2, 8])
                .style(if active { theme::primary } else { theme::ghost })
                .on_press(Message::PinFile {
                    target: file.target.clone(),
                    provider: Some(provider),
                })
        };
        let mut controls = row![
            pin(short_a.clone(), a, file.overridden && file.winner == a),
            pin(short_b.clone(), b, file.overridden && file.winner == b),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        if file.overridden {
            controls = controls.push(
                button(text("auto").size(10))
                    .padding([2, 8])
                    .style(theme::ghost)
                    .on_press(Message::PinFile {
                        target: file.target.clone(),
                        provider: None,
                    }),
            );
        }
        list = list.push(
            row![
                text(&file.target)
                    .size(11)
                    .color(theme::MUTED)
                    .width(Length::Fill),
                text(format!("→ {winner_name}"))
                    .size(11)
                    .color(if file.overridden {
                        theme::ACCENT
                    } else {
                        theme::FAINT
                    }),
                controls,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
    }
    let extra = conflict.files.len().saturating_sub(MAX_FILES_SHOWN);
    if extra > 0 {
        list = list.push(text(format!("+{extra} more")).size(10).color(theme::FAINT));
    }
    container(scrollable(list).height(Length::Shrink))
        .padding([8, 10])
        .width(Length::Fill)
        .style(theme::inset)
        .into()
}
