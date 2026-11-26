// SPDX-License-Identifier: GPL-2.0-only
//! The app's own right-click menus: on mod rows and on game cards.
//!
//! A secondary press stores what was pressed and where in `App::ctx`; the
//! menu is anchored at that point over the list it belongs to. A mod menu
//! acts on the selection when the pressed row is part of it, otherwise on
//! that row alone (the press selects it).

use rune::{El, PointerPress};
use rune_kit::{Menu, MenuItem, context_menu};

use crate::icons;
use crate::look::Look;
use crate::state::{App, Ctx, Pane, Screen};

impl App {
    /// Open the mod menu for visible row `index`.
    pub fn open_mod_menu(&mut self, index: usize, at: PointerPress) {
        self.focus_pane = Pane::Mods;
        if !self.mod_sel.items.contains(&index) {
            self.mod_sel.only(index);
        }
        self.ctx = Some((Ctx::Mod(index), at));
    }

    /// Open the game menu for a game card.
    pub fn open_game_menu(&mut self, game: modrix_core::GameId, at: PointerPress) {
        self.ctx = Some((Ctx::Game(game), at));
    }
}

/// The Mods table with its right-click menu. The menu's anchor wraps the
/// table in a row, so the wrapper takes over the table's growth.
pub fn mods(s: &App, look: &Look, table: El, layer: &mut Vec<El>) -> El {
    mod_menu(s, look, table.h_full(), layer)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
}

fn mod_menu(s: &App, look: &Look, table: El, layer: &mut Vec<El>) -> El {
    let point = match s.ctx {
        Some((Ctx::Mod(_), at)) => Some(at),
        _ => None,
    };
    let menu = context_menu("mod-menu", table, point)
        .min_width(220.0)
        .on_close(|s: &mut App| s.ctx = None);
    let Some((Ctx::Mod(index), _)) = s.ctx else {
        return menu.build(&look.k, layer);
    };
    let Some(m) = s.visible_mods().get(index).and_then(|i| s.lib.mods.get(*i)) else {
        return menu.build(&look.k, layer);
    };
    let ids = s.selected_mod_ids();
    let many = ids.len() > 1;
    let id = m.id;
    let staged = m.staged_path.display().to_string();
    let head = if many {
        format!("{} mods", ids.len())
    } else {
        m.name.clone()
    };
    let menu = edit_items(s, menu.group_label(head), m, ids);
    let menu = if !many && m.install_state.starts_with("fomod") {
        menu.item(
            MenuItem::new("Installer options")
                .icon(&icons::SLIDERS)
                .on_select(move |s: &mut App| {
                    s.ctx = None;
                    s.configure(id);
                }),
        )
    } else {
        menu
    };
    menu.separator()
        .item(copy_item("Copy staged path", staged).disabled(many))
        .separator()
        .item(
            MenuItem::new(if many { "Delete selected" } else { "Delete" })
                .icon(&icons::TRASH)
                .danger(true)
                .on_select(|s: &mut App| {
                    s.ctx = None;
                    s.confirm_delete = true;
                }),
        )
        .build(&look.k, layer)
}

/// Enable or Disable (from the pressed mod's state) and Reinstall.
fn edit_items(s: &App, menu: Menu, m: &modrix_core::Mod, ids: Vec<modrix_core::ModId>) -> Menu {
    let on = s.lib.enabled.contains(&m.id);
    menu.item(
        MenuItem::new(if on { "Disable" } else { "Enable" })
            .icon(if on { &icons::CLOSE } else { &icons::CHECK })
            .disabled(s.lib.profile.is_none())
            .on_select(move |s: &mut App| {
                s.ctx = None;
                s.set_many_enabled(ids.clone(), !on);
            }),
    )
    .item(
        MenuItem::new("Reinstall")
            .icon(&icons::REFRESH)
            .on_select(|s: &mut App| {
                s.ctx = None;
                s.reinstall_selected();
            }),
    )
}

/// The game cards with their right-click menu.
pub fn games(s: &App, look: &Look, cards: El, layer: &mut Vec<El>) -> El {
    game_menu(s, look, cards, layer).w_full()
}

fn game_menu(s: &App, look: &Look, cards: El, layer: &mut Vec<El>) -> El {
    let (point, game) = match s.ctx {
        Some((Ctx::Game(id), at)) => (Some(at), s.lib.games.iter().find(|g| g.id == id)),
        _ => (None, None),
    };
    let menu = context_menu("game-menu-ctx", cards, point)
        .min_width(220.0)
        .on_close(|s: &mut App| s.ctx = None);
    let Some(g) = game else {
        return menu.build(&look.k, layer);
    };
    let id = g.id;
    let path = g.install_path.display().to_string();
    menu.group_label(g.name.clone())
        .item(
            MenuItem::new("Set active")
                .icon(&icons::CHECK)
                .disabled(s.lib.game == Some(id))
                .on_select(move |s: &mut App| {
                    s.ctx = None;
                    s.switch_game(id);
                }),
        )
        .item(
            MenuItem::new("Manage mods")
                .icon(&icons::LAYERS)
                .on_select(move |s: &mut App| {
                    s.ctx = None;
                    s.switch_game(id);
                    s.screen = Screen::Mods;
                }),
        )
        .separator()
        .item(copy_item("Copy install path", path))
        .build(&look.k, layer)
}

/// An item that copies `text` to the clipboard.
fn copy_item(label: &str, text: String) -> MenuItem {
    MenuItem::new(label)
        .icon(&icons::COPY)
        .on_select_tx(move |s: &mut App, tx| {
            s.ctx = None;
            tx.copy(text.clone());
        })
}
