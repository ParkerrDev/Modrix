// SPDX-License-Identifier: GPL-2.0-only
//! Window-level keyboard control for the lists. Arrows move the cursor,
//! Shift extends, Ctrl moves the selected plugins, Ctrl+A selects all,
//! Escape closes the open surface or clears the selection.

use rune::{Key, KeyEvent, Tx};

use crate::state::{App, Pane, Screen};

/// The app-level key hook. Returns whether the key was used.
pub fn on_key(s: &mut App, event: &KeyEvent, _tx: &mut Tx) -> bool {
    if matches!(event.key, Key::Escape) {
        return escape(s);
    }
    if s.wizard.is_some() || s.popup.is_some() || !s.dup_queue.is_empty() {
        return false;
    }
    let pane = match s.screen {
        Screen::Mods => Pane::Mods,
        Screen::LoadOrder => Pane::Plugins,
        _ => return false,
    };
    s.focus_pane = pane;
    let len = match pane {
        Pane::Mods => s.visible_mods().len(),
        Pane::Plugins => s.lib.plugins.len(),
    };
    let ctrl = event.mods.ctrl || event.mods.cmd;
    let delta: i8 = match &event.key {
        Key::Command('a' | 'A') => {
            selection(s, pane).all(len);
            return true;
        }
        Key::Up => -1,
        Key::Down => 1,
        _ => return false,
    };
    if ctrl && pane == Pane::Plugins {
        s.move_plugins(delta);
        return true;
    }
    let sel = selection(s, pane);
    let next = sel
        .cursor
        .map_or(0, |c| crate::actions::step(c, delta, len));
    if len == 0 {
        return true;
    }
    if event.mods.shift {
        sel.range_to(next);
    } else {
        sel.only(next);
    }
    true
}

fn selection(s: &mut App, pane: Pane) -> &mut crate::state::Selection {
    match pane {
        Pane::Mods => &mut s.mod_sel,
        Pane::Plugins => &mut s.plugin_sel,
    }
}

fn escape(s: &mut App) -> bool {
    if s.popup.take().is_some() {
        return true;
    }
    if s.confirm_delete {
        s.confirm_delete = false;
        return true;
    }
    if s.wizard.is_some() {
        s.wizard_cancel();
        return true;
    }
    if !s.dup_queue.is_empty() {
        s.dup_cancel();
        return true;
    }
    match s.screen {
        Screen::Mods if !s.mod_sel.items.is_empty() => s.mod_sel.clear(),
        Screen::LoadOrder if !s.plugin_sel.items.is_empty() => s.plugin_sel.clear(),
        _ => return false,
    }
    true
}
