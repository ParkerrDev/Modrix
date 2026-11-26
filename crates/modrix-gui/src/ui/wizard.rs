// SPDX-License-Identifier: GPL-2.0-only
//! The FOMOD wizard dialog: one page per visible step, each group's options
//! picked by its rule, and a preview of the option last touched.

use modrix_plugin::fomod::{self, GroupKind, PluginKind};
use rune::{El, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Checkbox, Dialog};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::state::{App, Wizard};
use crate::ui::parts::{self, glyph};

/// Options listed per group.
const MAX_OPTIONS: usize = 200;

/// Push the wizard onto the overlay layer.
pub fn build(s: &App, look: &Look, w: &Wizard, layer: &mut Vec<El>) {
    let pages = w.visible_steps();
    let total = pages.len().max(1);
    let page = w.step.min(total.saturating_sub(1));
    let current = pages.get(page).copied();
    let title = w
        .installer
        .info_name
        .clone()
        .unwrap_or_else(|| w.mod_name.clone());
    let step_name = current
        .and_then(|i| w.installer.steps.get(i))
        .map_or_else(String::new, |st| st.name.clone());
    let body = row()
        .w_full()
        .h(440.0)
        .gap(0.0)
        .child(groups(look, w, current))
        .child(preview(look, w));
    let _ = s;
    Dialog::new("wizard", title)
        .open(true)
        .fit()
        .width(860.0)
        .description(format!(
            "Step {} of {total}: {step_name}",
            page.saturating_add(1)
        ))
        .child(parts::hair(look))
        .child(body)
        .child(parts::hair(look))
        .child(footer(look, page, total))
        .on_close(|s: &mut App| s.wizard_cancel())
        .build(&look.k, layer);
}

fn groups(look: &Look, w: &Wizard, step: Option<usize>) -> El {
    let Some(st) = step.and_then(|i| w.installer.steps.get(i).map(|st| (i, st))) else {
        return col().grow(1.0).padding(20.0).child(
            text("Nothing to choose here.")
                .font(ty::BODY)
                .color(look.muted()),
        );
    };
    let (index, st) = st;
    let flags = fomod::flags_of(&w.installer, &w.selections, &w.present);
    let list = st.groups.iter().enumerate().map(|(g, group)| {
        let picked = w.selections.get(index).and_then(|s| s.get(g));
        let options = group
            .plugins
            .iter()
            .take(MAX_OPTIONS)
            .enumerate()
            .map(|(p, plugin)| {
                let kind = fomod::plugin_kind(&plugin.kind, &flags, &w.present);
                let on = picked.is_some_and(|set| set.contains(&p));
                let focused = w.focus == Some((index, g, p));
                option(
                    look,
                    Pick {
                        step: index,
                        group: g,
                        plugin: p,
                    },
                    plugin,
                    Shown {
                        kind,
                        on,
                        focused,
                        group: group.kind,
                    },
                )
            });
        col()
            .key(format!("group-{index}-{g}"))
            .w_full()
            .gap(4.0)
            .child(group_head(look, index, g, group))
            .children(options)
    });
    col()
        .key("wizard-groups")
        .grow(1.0)
        .w(0.0)
        .h_full()
        .scroll(rune::ScrollSpec::default())
        .child(col().w_full().padding(18.0).gap(space::LG).children(list))
}

fn group_head(look: &Look, step: usize, g: usize, group: &fomod::Group) -> El {
    let rule = match group.kind {
        GroupKind::ExactlyOne => "Pick one",
        GroupKind::AtMostOne => "Pick one or none",
        GroupKind::AtLeastOne => "Pick at least one",
        GroupKind::Any => "Pick any",
        GroupKind::All => "All included",
    };
    let bulk = matches!(group.kind, GroupKind::Any | GroupKind::AtLeastOne).then(|| {
        row()
            .gap(4.0)
            .child(
                Button::new("All")
                    .variant(ButtonVariant::GhostMuted)
                    .on_press(move |s: &mut App| s.wizard_group_set(step, g, true))
                    .build(&look.k),
            )
            .child(
                Button::new("None")
                    .variant(ButtonVariant::GhostMuted)
                    .on_press(move |s: &mut App| s.wizard_group_set(step, g, false))
                    .build(&look.k),
            )
    });
    row()
        .w_full()
        .gap(space::SM)
        .center_y()
        .pb(4.0)
        .child(
            text(group.name.clone())
                .font(ty::HEADING)
                .color(look.text()),
        )
        .child(parts::tag(look, rule))
        .child(el().grow(1.0))
        .children(bulk)
}

/// Which option a press targets.
#[derive(Clone, Copy)]
struct Pick {
    step: usize,
    group: usize,
    plugin: usize,
}

/// How an option shows: its kind under the current flags, whether it is
/// picked or focused, and its group's rule.
#[derive(Clone, Copy)]
struct Shown {
    kind: PluginKind,
    on: bool,
    focused: bool,
    group: GroupKind,
}

/// The radio dot or checkbox in front of an option.
fn mark(look: &Look, plugin: &fomod::Plugin, on: bool, radio: bool) -> El {
    if radio {
        let ring = if on { look.accent } else { look.faint() };
        el().size(18.0, 18.0)
            .radius(9.0)
            .border(1.5, ring)
            .center()
            .children(on.then(|| el().size(8.0, 8.0).radius(4.0).fill(look.accent)))
    } else {
        Checkbox::new(on)
            .hide_label()
            .label(plugin.name.clone())
            .build(&look.k)
    }
}

fn option(look: &Look, at: Pick, plugin: &fomod::Plugin, shown: Shown) -> El {
    let Shown {
        kind,
        on,
        focused,
        group,
    } = shown;
    let locked =
        matches!(kind, PluginKind::NotUsable | PluginKind::Required) || group == GroupKind::All;
    let radio = matches!(group, GroupKind::ExactlyOne | GroupKind::AtMostOne);
    let badge = match kind {
        PluginKind::Recommended => Some(parts::chip("Recommended", look.good)),
        PluginKind::Required => Some(parts::chip("Required", look.accent)),
        PluginKind::NotUsable => Some(parts::chip("Not usable", look.bad)),
        _ => None,
    };
    let Pick {
        step,
        group: g,
        plugin: p,
    } = at;
    let mut line = row()
        .key(format!("opt-{step}-{g}-{p}"))
        .w_full()
        .min_h(40.0)
        .px(12.0)
        .gap(space::MD)
        .center_y()
        .radius(10.0)
        .fill(if focused {
            look.accent_soft
        } else {
            rune::Color::TRANSPARENT
        })
        .role(if radio { "radio" } else { "checkbox" })
        .checked(on)
        .label(plugin.name.clone())
        .child(mark(look, plugin, on, radio))
        .child(
            text(plugin.name.clone())
                .font(ty::BODY)
                .color(if kind == PluginKind::NotUsable {
                    look.faint()
                } else {
                    look.text()
                })
                .grow(1.0)
                .truncate(),
        )
        .children(badge);
    if !locked {
        line = line
            .hover_fill(look.hover())
            .on_press(move |s: &mut App| s.wizard_pick(step, g, p));
    }
    line
}

fn focused(w: &Wizard) -> Option<&fomod::Plugin> {
    let (step, group, plugin) = w.focus?;
    w.installer
        .steps
        .get(step)?
        .groups
        .get(group)?
        .plugins
        .get(plugin)
}

fn preview(look: &Look, w: &Wizard) -> El {
    let pane = col()
        .key("wizard-preview")
        .w(300.0)
        .h_full()
        .fill(look.glass)
        .scroll(rune::ScrollSpec::default());
    let Some(plugin) = focused(w) else {
        return pane.child(
            col()
                .w_full()
                .padding(22.0)
                .gap(space::SM)
                .items_center()
                .child(glyph(&icons::SLIDERS, 26.0, look.faint()))
                .child(
                    text("Pick an option to read about it.")
                        .font(ty::BODY)
                        .color(look.muted())
                        .wrap()
                        .align(rune::Align::Center),
                ),
        );
    };
    let description = if plugin.description.trim().is_empty() {
        "This option has no description.".to_owned()
    } else {
        plugin.description.trim().to_owned()
    };
    pane.child(
        col()
            .w_full()
            .padding(20.0)
            .gap(space::SM)
            .child(
                text(plugin.name.clone())
                    .font(ty::HEADING)
                    .color(look.text())
                    .wrap(),
            )
            .child(text(description).font(ty::BODY).color(look.muted()).wrap())
            .child(
                text(format!("{} files", plugin.files.len()))
                    .font(ty::SMALL)
                    .color(look.faint()),
            ),
    )
}

fn footer(look: &Look, page: usize, total: usize) -> El {
    let last = page.saturating_add(1) >= total;
    let next = if last {
        Button::new("Install")
            .icon(&icons::CHECK)
            .variant(ButtonVariant::Submit)
            .on_press(|s: &mut App| s.wizard_finish())
            .build(&look.k)
    } else {
        Button::new("Next")
            .icon(&icons::CHEVRON_RIGHT)
            .variant(ButtonVariant::Submit)
            .on_press(|s: &mut App| s.wizard_step(1))
            .build(&look.k)
    };
    let dots = (0..total.min(24)).map(|i| {
        el().size(if i == page { 18.0 } else { 6.0 }, 6.0)
            .radius(3.0)
            .fill(if i == page { look.accent } else { look.edge })
            .motion(rune::Spring::new(0.25, 0.1))
    });
    row()
        .w_full()
        .px(20.0)
        .py(14.0)
        .gap(space::SM)
        .center_y()
        .child(row().gap(5.0).center_y().children(dots))
        .child(el().grow(1.0))
        .child(
            Button::new("Skip options")
                .variant(ButtonVariant::GhostMuted)
                .on_press(|s: &mut App| s.wizard_cancel())
                .build(&look.k),
        )
        .child(
            Button::new("Back")
                .variant(ButtonVariant::Neutral)
                .disabled(page == 0)
                .on_press(|s: &mut App| s.wizard_step(-1))
                .build(&look.k),
        )
        .child(next)
}
