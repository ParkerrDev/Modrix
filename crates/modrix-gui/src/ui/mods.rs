// SPDX-License-Identifier: GPL-2.0-only
//! The mod library: search, add, a sortable table with switches, and an
//! action bar for the selection.

use rune::{El, FileDialogOptions, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Switch, TextInput};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::state::{App, Pane, SortKey};
use crate::ui::parts::{self, glyph};

/// Row height in the table.
const ROW_H: f32 = 52.0;

/// Ask for archives to install.
pub fn pick_archives(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Install mods".to_owned(),
        ..FileDialogOptions::default()
    }
    .filter("Mod archives", &["zip", "7z", "rar", "tar", "gz", "xz"]);
    tx.open_files(options, |s: &mut App, picked, _| match picked {
        Ok(Some(paths)) => s.install_files(paths),
        Ok(None) => {}
        Err(error) => s.note(crate::state::Tone::Error, error.to_string()),
    });
}

/// The Mods screen.
pub fn view(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    if s.lib.mods.is_empty() {
        return parts::panel(look, "mods-empty").child(parts::empty(
            look,
            &icons::LAYERS,
            "No mods yet",
            "Drop archives anywhere on this window, use Add mods, or click a mod manager download on Nexus with the browser extension.",
            Some(add_button(look)),
        ));
    }
    let selected = s.mod_sel.items.len();
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(space::MD)
        .child(toolbar(s, look))
        .children((selected > 0).then(|| action_bar(s, look, selected)))
        .child(table(s, look, layer))
}

fn add_button(look: &Look) -> El {
    Button::new("Add mods")
        .icon(&icons::PLUS)
        .variant(ButtonVariant::Submit)
        .on_press_tx(|_: &mut App, tx| pick_archives(tx))
        .build(&look.k)
}

fn toolbar(s: &App, look: &Look) -> El {
    let disabled = s.lib.mods.iter().any(|m| !s.lib.enabled.contains(&m.id));
    row()
        .w_full()
        .gap(space::SM)
        .center_y()
        .child(
            TextInput::new(&s.form.search)
                .placeholder("Search mods")
                .leading_icon(&icons::SEARCH)
                .clear_button()
                .width(Some(320.0))
                .on_edit(|s: &mut App| &mut s.form.search)
                .on_change(|s: &mut App, _| s.mod_sel.clear())
                .build(&look.k),
        )
        .child(el().grow(1.0))
        .children(disabled.then(|| {
            Button::new("Enable all")
                .icon(&icons::CHECK)
                .variant(ButtonVariant::Ghost)
                .on_press(|s: &mut App| s.enable_all())
                .build(&look.k)
        }))
        .child(add_button(look))
}

fn action_bar(s: &App, look: &Look, selected: usize) -> El {
    let one = s
        .selected_mod_ids()
        .first()
        .copied()
        .filter(|_| selected == 1);
    let configurable = one
        .and_then(|id| s.lib.mods.iter().find(|m| m.id == id))
        .is_some_and(|m| m.install_state.starts_with("fomod"));
    row()
        .key("action-bar")
        .w_full()
        .h(50.0)
        .px(16.0)
        .gap(space::SM)
        .center_y()
        .radius(14.0)
        .fill(look.accent_soft)
        .border(1.0, look.accent.alpha(0.3))
        .transition(rune::Transition::opacity())
        .child(
            text(format!("{selected} selected"))
                .font(ty::BODY_STRONG)
                .color(look.text()),
        )
        .child(el().grow(1.0))
        .child(ghost(look, "Enable", &icons::CHECK, |s| {
            let ids = s.selected_mod_ids();
            s.set_many_enabled(ids, true);
        }))
        .child(ghost(look, "Disable", &icons::CLOSE, |s| {
            let ids = s.selected_mod_ids();
            s.set_many_enabled(ids, false);
        }))
        .children(configurable.then(|| {
            ghost(look, "Options", &icons::SLIDERS, move |s| {
                if let Some(id) = one {
                    s.configure(id);
                }
            })
        }))
        .child(ghost(
            look,
            "Reinstall",
            &icons::REFRESH,
            App::reinstall_selected,
        ))
        .child(
            Button::new("Delete")
                .icon(&icons::TRASH)
                .variant(ButtonVariant::Danger)
                .on_press(|s: &mut App| s.confirm_delete = true)
                .build(&look.k),
        )
}

fn ghost(look: &Look, label: &str, i: &'static rune::Icon, f: impl Fn(&mut App) + 'static) -> El {
    Button::new(label)
        .icon(i)
        .variant(ButtonVariant::Ghost)
        .on_press(f)
        .build(&look.k)
}

fn table(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let visible = s.visible_mods().len();
    let body_look = look.clone();
    let rows = col()
        .key("mod-rows")
        .w_full()
        .virtual_rows(
            visible,
            ROW_H,
            |s: &App, i| {
                let id = s
                    .visible_mods()
                    .get(i)
                    .and_then(|m| s.lib.mods.get(*m))
                    .map_or(0, |m| m.id.get());
                format!("mod-{id}").into()
            },
            move |s: &App, i| mod_row(s, &body_look, i),
        )
        .growth(rune::GrowthSpec::default())
        .virtual_rows_version((
            s.mod_sort.0 as u8,
            s.mod_sort.1,
            s.form.search.text.as_str(),
            s.mods_narrow,
        ));
    let list = col()
        .key("mod-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec::default())
        .child(rows)
        .child(el().h(10.0));
    let footer = (visible == 0).then(|| {
        col().w_full().py(30.0).center().child(
            text("No mods match the search")
                .font(ty::BODY)
                .color(look.muted()),
        )
    });
    let panel = parts::panel(look, "mods-table")
        .grow(1.0)
        .min_h(0.0)
        .on_layout(|s: &mut App, [_, _, w, _]| s.mods_narrow = narrow_level(w))
        .child(header(s, look))
        .child(parts::hair(look))
        .children(footer)
        .child(list)
        .child(parts::hair(look))
        .child(summary(s, look));
    super::menus::mods(s, look, panel, layer)
}

/// The line under the table: what the library holds.
fn summary(s: &App, look: &Look) -> El {
    let mods = s.lib.mods.len();
    let enabled = s
        .lib
        .mods
        .iter()
        .filter(|m| s.lib.enabled.contains(&m.id))
        .count();
    let plugins: usize = s
        .lib
        .plugin_counts
        .values()
        .copied()
        .fold(0, usize::saturating_add);
    let assets = s
        .lib
        .mods
        .iter()
        .filter(|m| s.lib.plugin_counts.get(&m.id).copied().unwrap_or(0) == 0)
        .count();
    let mut parts = vec![
        count(mods, "mod", "mods"),
        format!("{enabled} enabled"),
        count(plugins, "plugin", "plugins"),
        format!("{assets} asset-only"),
    ];
    if !s.lib.external.is_empty() {
        parts.push(format!("{} external (not managed)", s.lib.external.len()));
    }
    row()
        .key("mods-summary")
        .w_full()
        .px(18.0)
        .py(10.0)
        .child(text(parts.join(", ")).font(ty::SMALL).color(look.muted()))
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Table widths below which a column is dropped, Source first.
const SOURCE_MIN_W: f32 = 820.0;
const INSTALLED_MIN_W: f32 = 680.0;

/// How many columns a table `w` points wide drops.
fn narrow_level(w: f32) -> u8 {
    u8::from(w < INSTALLED_MIN_W).saturating_add(u8::from(w < SOURCE_MIN_W))
}

fn header(s: &App, look: &Look) -> El {
    let narrow = s.mods_narrow;
    row()
        .key("mods-head")
        .w_full()
        .h(40.0)
        .px(18.0)
        .gap(space::MD)
        .center_y()
        .child(sort_cell(s, look, "Name", SortKey::Name).grow(1.0))
        .child(sort_cell(s, look, "Version", SortKey::Version).w(110.0))
        .children((narrow < 1).then(|| sort_cell(s, look, "Source", SortKey::Source).w(90.0)))
        .children(
            (narrow < 2).then(|| sort_cell(s, look, "Installed", SortKey::Installed).w(110.0)),
        )
        .child(sort_cell(s, look, "On", SortKey::Enabled).w(54.0))
}

fn sort_cell(s: &App, look: &Look, label: &str, key: SortKey) -> El {
    let (current, ascending) = s.mod_sort;
    let on = current == key;
    let arrow = on.then(|| {
        glyph(
            if ascending {
                &icons::ARROW_UP
            } else {
                &icons::ARROW_DOWN
            },
            12.0,
            look.accent,
        )
    });
    row()
        .key(format!("sort-{label}"))
        .gap(5.0)
        .center_y()
        .role("button")
        .label(format!("Sort by {label}"))
        .on_press(move |s: &mut App| s.sort_by(key))
        .child(text(label.to_uppercase()).font(ty::LABEL).color(if on {
            look.text()
        } else {
            look.faint()
        }))
        .children(arrow)
}

fn mod_row(s: &App, look: &Look, i: usize) -> El {
    let Some(m) = s.visible_mods().get(i).and_then(|m| s.lib.mods.get(*m)) else {
        return el();
    };
    let id = m.id;
    let on = s.lib.enabled.contains(&id);
    let selected = s.mod_sel.items.contains(&i);
    let plugins = s.lib.plugin_counts.get(&id).copied().unwrap_or(0);
    let mut name = row().gap(8.0).center_y().min_w(0.0).child(
        text(m.name.clone())
            .font(ty::BODY_STRONG)
            .color(if on { look.text() } else { look.muted() })
            .truncate(),
    );
    if m.install_state.starts_with("fomod") {
        name = name.child(parts::chip("Options", look.accent));
    }
    if plugins > 0 {
        name = name.child(parts::tag(
            look,
            if plugins == 1 {
                "1 plugin".to_owned()
            } else {
                format!("{plugins} plugins")
            },
        ));
    }
    row()
        .key(format!("mod-row-{}", id.get()))
        .w_full()
        .h(ROW_H)
        .px(18.0)
        .gap(space::MD)
        .center_y()
        .fill(if selected {
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
        .label(m.name.clone())
        .on_press_tx(move |s: &mut App, tx| s.row_click(Pane::Mods, i, tx.mods()))
        .on_double_press(move |s: &mut App| s.configure(id))
        .on_secondary_press(move |s: &mut App, p, _| s.open_mod_menu(i, p))
        .children(mod_cells(look, m, name, on, s.mods_narrow))
}

fn mod_cells(look: &Look, m: &modrix_core::Mod, name: El, on: bool, narrow: u8) -> Vec<El> {
    let id = m.id;
    let installed = m.created_at.map_or_else(|| "-".to_owned(), date);
    let source = (narrow < 1).then(|| el().w(90.0).child(parts::tag(look, m.source.clone())));
    let installed =
        (narrow < 2).then(|| text(installed).font(ty::SMALL).color(look.faint()).w(110.0));
    let mut cells = vec![
        name.grow(1.0).w(0.0),
        text(m.version.clone().unwrap_or_else(|| "-".to_owned()))
            .font(ty::MONO)
            .color(look.muted())
            .truncate()
            .w(110.0),
    ];
    cells.extend(source);
    cells.extend(installed);
    cells.push(
        el().w(54.0).child(
            Switch::new(on)
                .hide_label()
                .label(format!("Enable {}", m.name))
                .on_toggle(move |s: &mut App| s.set_mod_enabled(id, !on))
                .build(&look.k),
        ),
    );
    cells
}

/// A unix time as `2026-11-25`.
#[must_use]
pub fn date(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
#[expect(
    clippy::arithmetic_side_effects,
    reason = "the day count is clamped to +-100 million years, far inside i64"
)]
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days.clamp(-36_500_000_000, 36_500_000_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_764_028_800), "2025-11-25");
        assert_eq!(date(951_782_400), "2000-02-29");
    }
}
