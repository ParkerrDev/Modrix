// SPDX-License-Identifier: GPL-2.0-only
//! Modal surfaces: the FOMOD wizard, the duplicate-archive prompt and the
//! delete confirmation. At most one shows; the wizard waits behind the others.

use rune::{El, row, text};
use rune_kit::{Button, ButtonVariant, Dialog};

use crate::look::{Look, space, ty};
use crate::state::App;

/// Push whichever dialog is open onto the overlay layer.
pub fn build(s: &App, look: &Look, layer: &mut Vec<El>) {
    if let Some(dup) = s.dup_queue.first() {
        duplicate(look, dup, layer);
    } else if s.confirm_delete {
        delete(s, look, layer);
    } else if let Some(wizard) = &s.wizard {
        super::wizard::build(s, look, wizard, layer);
    }
}

fn actions(look: &Look, cancel: &str, confirm: El, on_cancel: fn(&mut App)) -> El {
    row()
        .w_full()
        .px(20.0)
        .pt(8.0)
        .pb(18.0)
        .gap(space::SM)
        .justify_end()
        .child(
            Button::new(cancel)
                .variant(ButtonVariant::GhostMuted)
                .on_press(on_cancel)
                .build(&look.k),
        )
        .child(confirm)
}

fn duplicate(look: &Look, dup: &crate::state::DupPrompt, layer: &mut Vec<El>) {
    let file = dup.path.file_name().map_or_else(
        || dup.path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    Dialog::new("duplicate", "Already installed")
        .open(true)
        .fit()
        .width(460.0)
        .description(format!(
            "{file} is the same archive as {}.",
            dup.existing_name
        ))
        .child(
            row().w_full().px(20.0).pb(6.0).child(
                text("Install it again as a separate mod, or skip it.")
                    .font(ty::BODY)
                    .color(look.muted())
                    .wrap(),
            ),
        )
        .child(actions(
            look,
            "Skip",
            Button::new("Install anyway")
                .variant(ButtonVariant::Submit)
                .on_press(|s: &mut App| s.dup_confirm())
                .build(&look.k),
            App::dup_cancel,
        ))
        .on_close(|s: &mut App| s.dup_cancel())
        .build(&look.k, layer);
}

fn delete(s: &App, look: &Look, layer: &mut Vec<El>) {
    let ids = s.selected_mod_ids();
    let title = match ids.as_slice() {
        [one] => format!("Delete {}?", s.mod_name(*one)),
        many => format!("Delete {} mods?", many.len()),
    };
    Dialog::new("confirm-delete", title)
        .open(true)
        .fit()
        .width(440.0)
        .description(
            "Their staged files are removed. Deploy afterwards to take them out of the game.",
        )
        .child(actions(
            look,
            "Keep",
            Button::new("Delete")
                .variant(ButtonVariant::Danger)
                .on_press(|s: &mut App| {
                    s.confirm_delete = false;
                    s.delete_selected();
                })
                .build(&look.k),
            |s| s.confirm_delete = false,
        ))
        .on_close(|s: &mut App| s.confirm_delete = false)
        .build(&look.k, layer);
}
