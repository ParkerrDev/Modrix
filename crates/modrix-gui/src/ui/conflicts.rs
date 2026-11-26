// SPDX-License-Identifier: GPL-2.0-only
//! Conflicts: what blocks a deploy, then each pair of mods that ship the
//! same files, with a rule picker and per-file pins.

use modrix_core::{ConflictFile, ModConflict, ModId};
use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, SegmentedControl, Select};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::state::{App, Popup};
use crate::ui::parts::{self, glyph};

/// The pin choice that defers to the rule.
const FOLLOW: &str = "Follow the rule";
/// Files listed under an expanded pair.
const MAX_FILES: usize = 200;

/// The Conflicts screen.
pub fn view(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let blockers: Vec<&modrix_core::Issue> = s.blockers().collect();
    let mut page = col().w_full().gap(space::LG);
    if !blockers.is_empty() {
        let rows = blockers.iter().enumerate().map(|(i, issue)| {
            row()
                .key(format!("blocker-{i}"))
                .w_full()
                .px(18.0)
                .py(9.0)
                .gap(12.0)
                .child(glyph(&icons::WARNING, 16.0, look.bad))
                .child(
                    text(issue.message.clone())
                        .font(ty::BODY)
                        .color(look.text())
                        .wrap()
                        .grow(1.0),
                )
        });
        page = page.child(parts::section(
            look,
            "blockers",
            "Blocking deploy",
            Some(parts::chip(format!("{}", blockers.len()), look.bad)),
            col().w_full().pb(10.0).children(rows),
        ));
    }
    if s.lib.conflicts.is_empty() {
        return page.child(parts::panel(look, "no-conflicts").child(parts::empty(
            look,
            &icons::SHIELD,
            "No file conflicts",
            "No two enabled mods ship the same file.",
            None,
        )));
    }
    let pairs = s.lib.conflicts.iter().map(|c| pair(s, look, c, layer));
    page.child(summary(s, look))
        .child(col().w_full().gap(space::SM).children(pairs))
}

fn summary(s: &App, look: &Look) -> El {
    let open = s.unresolved_conflicts();
    let total = s.lib.conflicts.len();
    let files: usize = s.lib.conflicts.iter().map(|c| c.files.len()).sum();
    row()
        .w_full()
        .gap(space::MD)
        .flex_wrap()
        .child(parts::stat(
            look,
            "c-pairs",
            (&icons::CONFLICT, look.accent),
            total.to_string(),
            "Conflicting pairs",
        ))
        .child(parts::stat(
            look,
            "c-open",
            (
                &icons::WARNING,
                if open > 0 { look.warn } else { look.good },
            ),
            open.to_string(),
            "Without a rule",
        ))
        .child(parts::stat(
            look,
            "c-files",
            (&icons::PACKAGE, look.faint()),
            files.to_string(),
            "Contested files",
        ))
}

fn pair(s: &App, look: &Look, c: &ModConflict, layer: &mut Vec<El>) -> El {
    let (a, b) = (c.first, c.second);
    let open = s.expanded_conflict == Some((a, b));
    let resolved = c.resolved();
    let status = if let Some(rule) = &c.rule {
        parts::chip(format!("{} wins", s.mod_name(rule.winner)), look.good)
    } else if resolved {
        parts::chip("Pinned per file", look.good)
    } else {
        parts::chip("Load order decides", look.warn)
    };
    let head = pair_head(s, look, c, open).child(status);
    let mut card = parts::panel(look, &format!("pair-{a}-{b}")).child(head);
    if open {
        card = card
            .child(parts::hair(look))
            .child(rules(s, look, c))
            .child(files(s, look, c, layer));
    }
    card
}

fn pair_head(s: &App, look: &Look, c: &ModConflict, open: bool) -> El {
    let (a, b) = (c.first, c.second);
    row()
        .key(format!("pair-head-{a}-{b}"))
        .w_full()
        .h(56.0)
        .px(16.0)
        .gap(space::MD)
        .center_y()
        .hover_fill(look.hover())
        .role("button")
        .label("Show files")
        .on_press(move |s: &mut App| {
            s.expanded_conflict = if s.expanded_conflict == Some((a, b)) {
                None
            } else {
                Some((a, b))
            };
        })
        .child(glyph(
            if open {
                &icons::CHEVRON_DOWN
            } else {
                &icons::CHEVRON_RIGHT
            },
            16.0,
            look.faint(),
        ))
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .gap(8.0)
                .center_y()
                .child(
                    text(s.mod_name(a))
                        .font(ty::BODY_STRONG)
                        .color(look.text())
                        .truncate(),
                )
                .child(text("and").font(ty::SMALL).color(look.faint()))
                .child(
                    text(s.mod_name(b))
                        .font(ty::BODY_STRONG)
                        .color(look.text())
                        .truncate(),
                ),
        )
        .child(
            text(format!("{} files", c.files.len()))
                .font(ty::SMALL)
                .color(look.muted()),
        )
}

fn rules(s: &App, look: &Look, conflict: &ModConflict) -> El {
    let (first, second) = (conflict.first, conflict.second);
    let labels = [
        "Load order".to_owned(),
        format!("{} wins", short(&s.mod_name(first))),
        format!("{} wins", short(&s.mod_name(second))),
    ];
    let current = match &conflict.rule {
        None => 0,
        Some(r) if r.winner == first => 1,
        Some(_) => 2,
    };
    row()
        .w_full()
        .px(18.0)
        .py(12.0)
        .gap(space::MD)
        .center_y()
        .child(text("Rule").font(ty::SMALL_STRONG).color(look.muted()))
        .child(
            SegmentedControl::new(&labels, Some(current))
                .on_pick(move |s: &mut App, i, _| match i {
                    1 => s.set_rule(second, first),
                    2 => s.set_rule(first, second),
                    _ => s.clear_rule(first, second),
                })
                .width(None)
                .build(&look.k)
                .grow(1.0)
                .w(0.0)
                .max_w(560.0),
        )
}

fn short(name: &str) -> String {
    let mut out: String = name.chars().take(22).collect();
    if name.chars().count() > 22 {
        out.push_str("...");
    }
    out
}

fn files(s: &App, look: &Look, conflict: &ModConflict, layer: &mut Vec<El>) -> El {
    let pair = (conflict.first, conflict.second);
    let rows = conflict
        .files
        .iter()
        .take(MAX_FILES)
        .enumerate()
        .map(|entry| file_row(s, look, pair, entry, layer))
        .collect::<Vec<_>>();
    let more = conflict.files.len().saturating_sub(MAX_FILES);
    col()
        .w_full()
        .pb(10.0)
        .children(rows)
        .children((more > 0).then(|| {
            row().px(18.0).py(8.0).child(
                text(format!("and {more} more files"))
                    .font(ty::SMALL)
                    .color(look.faint()),
            )
        }))
        .child(
            row().w_full().px(18.0).pt(6.0).justify_end().child(
                Button::new("Close")
                    .variant(ButtonVariant::GhostMuted)
                    .on_press(|s: &mut App| s.expanded_conflict = None)
                    .build(&look.k),
            ),
        )
}

fn pin_picker(
    s: &App,
    look: &Look,
    pair: (ModId, ModId),
    entry: (usize, &ConflictFile),
    layer: &mut Vec<El>,
) -> El {
    let (first, second) = pair;
    let (index, file) = entry;
    let names = [FOLLOW.to_owned(), s.mod_name(first), s.mod_name(second)];
    let current = if file.overridden {
        s.mod_name(file.winner)
    } else {
        FOLLOW.to_owned()
    };
    let target = file.target.clone();
    let popup = Popup::PinPicker(index);
    Select::new(format!("pin-{first}-{second}-{index}"), &names)
        .current(Some(current.as_str()))
        .open(s.popup == Some(popup))
        .on_toggle(move |s: &mut App| {
            s.popup = if s.popup == Some(popup) {
                None
            } else {
                Some(popup)
            };
        })
        .on_close(|s: &mut App| s.popup = None)
        .on_choose(move |s: &mut App, choice: &str, _| {
            let provider = pin_choice(s, choice, first, second);
            s.pin_file(target.clone(), provider);
        })
        .width(220.0)
        .build(&look.k, layer)
}

fn file_row(
    s: &App,
    look: &Look,
    pair: (ModId, ModId),
    entry: (usize, &ConflictFile),
    layer: &mut Vec<El>,
) -> El {
    let (index, file) = entry;
    let (icon, tint) = if file.overridden {
        (&icons::PIN, look.accent)
    } else {
        (&icons::PACKAGE, look.faint())
    };
    row()
        .key(format!("file-{index}"))
        .w_full()
        .min_h(40.0)
        .px(18.0)
        .gap(space::MD)
        .center_y()
        .child(glyph(icon, 14.0, tint))
        .child(
            text(file.target.clone())
                .font(ty::MONO)
                .color(look.text())
                .truncate_head()
                .grow(1.0)
                .w(0.0),
        )
        .child(pin_picker(s, look, pair, entry, layer))
}

fn pin_choice(s: &App, choice: &str, a: ModId, b: ModId) -> Option<ModId> {
    if choice == s.mod_name(a) {
        Some(a)
    } else if choice == s.mod_name(b) {
        Some(b)
    } else {
        None
    }
}
