// SPDX-License-Identifier: GPL-2.0-only
//! Plugin load order: drag rows by the grip (or Ctrl+arrows), toggle them,
//! auto-sort. Later plugins win.

use rune::{Drag, El, Phase, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Checkbox, IconButton, IconButtonVariant};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::state::{App, Pane, PluginDrag};
use crate::ui::parts::{self, glyph};
use modrix_core::plugins::GamePlugin;

/// Row height, fixed so a drag maps to a slot.
const ROW_H: f32 = 50.0;

/// The Load order screen.
pub fn view(s: &App, look: &Look) -> El {
    if s.lib.plugins.is_empty() {
        return parts::panel(look, "order-empty").child(parts::empty(
            look,
            &icons::ORDER,
            "No plugins",
            "Enable mods that ship plugins and they appear here in load order.",
            None,
        ));
    }
    let rows = s
        .lib
        .plugins
        .iter()
        .enumerate()
        .map(|(i, _)| plugin_row(s, look, i));
    let list = col()
        .key("order-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec::default())
        .child(
            col()
                .key("order-rows")
                .w_full()
                .children(rows)
                .children(drop_line(s, look)),
        );
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(space::MD)
        .child(toolbar(s, look))
        .child(
            parts::panel(look, "order-table")
                .grow(1.0)
                .min_h(0.0)
                .child(list),
        )
}

fn toolbar(s: &App, look: &Look) -> El {
    let any = !s.plugin_sel.items.is_empty();
    let missing = s
        .lib
        .plugins
        .iter()
        .filter(|p| !p.missing_masters.is_empty())
        .count();
    row()
        .w_full()
        .gap(space::SM)
        .center_y()
        .child(
            text("Drag the grip or use Ctrl and the arrow keys to move plugins.")
                .font(ty::SMALL)
                .color(look.muted()),
        )
        .children((missing > 0).then(|| {
            parts::chip(
                if missing == 1 {
                    "1 plugin missing masters".to_owned()
                } else {
                    format!("{missing} plugins missing masters")
                },
                look.bad,
            )
        }))
        .child(el().grow(1.0))
        .child(
            IconButton::new(&icons::ARROW_UP)
                .variant(IconButtonVariant::Ghost)
                .label("Move up")
                .disabled(!any)
                .on_press(|s: &mut App| s.move_plugins(-1))
                .build(&look.k),
        )
        .child(
            IconButton::new(&icons::ARROW_DOWN)
                .variant(IconButtonVariant::Ghost)
                .label("Move down")
                .disabled(!any)
                .on_press(|s: &mut App| s.move_plugins(1))
                .build(&look.k),
        )
        .child(
            Button::new("Auto-sort")
                .icon(&icons::SPARKLES)
                .variant(ButtonVariant::Submit)
                .on_press(|s: &mut App| s.auto_sort())
                .build(&look.k),
        )
}

/// Slot a drag lands in.
#[must_use]
pub fn target(from: usize, dy: f32, len: usize) -> usize {
    #[expect(clippy::cast_possible_truncation, reason = "a row count fits in i64")]
    let steps = (dy / ROW_H).round() as i64;
    let from = i64::try_from(from).unwrap_or(0);
    let last = i64::try_from(len.saturating_sub(1)).unwrap_or(0);
    usize::try_from(from.saturating_add(steps).clamp(0, last)).unwrap_or(0)
}

fn drop_line(s: &App, look: &Look) -> Option<El> {
    let drag = s.drag?;
    let to = target(drag.from, drag.dy, s.lib.plugins.len());
    if to == drag.from {
        return None;
    }
    let below = usize::from(to > drag.from);
    #[expect(clippy::cast_precision_loss, reason = "row indices are small")]
    let y = to.saturating_add(below) as f32 * ROW_H - 1.0;
    Some(
        el().key("drop-line")
            .absolute(14.0, y)
            .w_full()
            .mr(28.0)
            .h(2.0)
            .radius(1.0)
            .fill(look.accent)
            .shadow(0.0, 8.0, look.accent),
    )
}

fn on_grip(s: &mut App, i: usize, d: &mut Drag, _tx: &mut Tx) {
    match d.phase {
        Phase::Down => s.drag = Some(PluginDrag { from: i, dy: 0.0 }),
        Phase::Move => {
            if let Some(drag) = &mut s.drag {
                drag.dy = d.dy;
            }
        }
        Phase::Up => {
            if let Some(drag) = s.drag.take() {
                let to = target(drag.from, drag.dy, s.lib.plugins.len());
                s.move_plugin_to(drag.from, to);
            }
        }
    }
}

fn plugin_row(s: &App, look: &Look, i: usize) -> El {
    let Some(p) = s.lib.plugins.get(i) else {
        return el();
    };
    let selected = s.plugin_sel.items.contains(&i);
    let dragging = s.drag.filter(|d| d.from == i);
    let mut line = row()
        .key(format!("plugin-{}", p.name))
        .w_full()
        .h(ROW_H)
        .px(14.0)
        .gap(space::MD)
        .center_y()
        .fill(if selected || dragging.is_some() {
            look.accent_soft
        } else {
            rune::Color::TRANSPARENT
        })
        .hover_fill(if selected {
            look.accent_soft
        } else {
            look.hover()
        })
        .border_sides([0.0, 0.0, 1.0, 0.0], look.edge)
        .role("row")
        .checked(selected)
        .label(p.name.clone())
        .on_press_tx(move |s: &mut App, tx| s.row_click(Pane::Plugins, i, tx.mods()))
        .children(plugin_cells(look, i, p));
    if let Some(d) = dragging {
        line = line
            .offset(0.0, d.dy)
            .shadow(12.0, 28.0, look.shadow())
            .fill(look.glass_strong)
            .radius(10.0);
    }
    line
}

fn badges(look: &Look, p: &GamePlugin) -> El {
    let mut badges = row().gap(6.0);
    if p.is_master {
        badges = badges.child(parts::chip("Master", look.warn));
    }
    if p.is_light {
        badges = badges.child(parts::chip("Light", look.accent));
    }
    if !p.missing_masters.is_empty() {
        badges = badges.child(parts::chip(
            format!("Missing {}", p.missing_masters.join(", ")),
            look.bad,
        ));
    }
    badges
}

fn plugin_cells(look: &Look, i: usize, p: &GamePlugin) -> [El; 5] {
    let grip = el()
        .key(format!("grip-{i}"))
        .size(26.0, 34.0)
        .center()
        .radius(6.0)
        .hover_fill(look.hover())
        .cursor(rune::Cursor::Grab)
        .label("Drag to reorder")
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| on_grip(s, i, d, tx))
        .child(glyph(&icons::GRIP, 16.0, look.faint()));
    [
        grip,
        text(format!("{i:02}"))
            .font(ty::MONO)
            .color(look.faint())
            .w(28.0),
        Checkbox::new(p.enabled)
            .hide_label()
            .label(format!("Activate {}", p.name))
            .on_toggle(move |s: &mut App| s.toggle_plugin(i))
            .build(&look.k),
        col()
            .grow(1.0)
            .min_w(0.0)
            .gap(2.0)
            .child(
                text(p.name.clone())
                    .font(ty::BODY_STRONG)
                    .color(if p.enabled { look.text() } else { look.muted() })
                    .truncate(),
            )
            .child(
                text(p.mod_name.clone())
                    .font(ty::SMALL)
                    .color(look.faint())
                    .truncate(),
            ),
        badges(look, p),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_targets_clamp() {
        assert_eq!(target(2, 0.0, 5), 2);
        assert_eq!(target(2, ROW_H * 1.4, 5), 3);
        assert_eq!(target(2, -ROW_H * 9.0, 5), 0);
        assert_eq!(target(2, ROW_H * 9.0, 5), 4);
    }
}
