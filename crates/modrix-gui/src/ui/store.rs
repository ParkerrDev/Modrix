// SPDX-License-Identifier: GPL-2.0-only
//! The Mod store screen: the game's community catalog, searchable, with a
//! one-click install that queues the mod and its dependencies.

use std::collections::HashSet;

use modrix_service::store::{StoreItem, installed_keys};
use rune::{El, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Spinner, TextInput};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::state::App;
use crate::ui::parts;

/// Rows drawn at most; the search narrows the rest.
const MAX_ROWS: usize = 200;

/// The Mod store screen.
pub fn view(s: &App, look: &Look) -> El {
    let body = if s.store.loading {
        loading(look)
    } else if let Some(error) = &s.store.error {
        failed(look, error)
    } else {
        list(s, look)
    };
    col()
        .w_full()
        .gap(space::MD)
        .child(toolbar(s, look))
        .child(parts::panel(look, "store").child(body))
}

fn toolbar(s: &App, look: &Look) -> El {
    row()
        .w_full()
        .gap(space::SM)
        .center_y()
        .child(
            TextInput::new(&s.form.store_search)
                .placeholder("Search the store")
                .leading_icon(&icons::SEARCH)
                .clear_button()
                .width(Some(320.0))
                .on_edit(|s: &mut App| &mut s.form.store_search)
                .build(&look.k),
        )
        .child(el().grow(1.0))
        .child(
            Button::new("Refresh")
                .icon(&icons::REFRESH)
                .variant(ButtonVariant::Ghost)
                .disabled(s.store.loading)
                .on_press(|s: &mut App| s.fetch_store())
                .build(&look.k),
        )
}

fn loading(look: &Look) -> El {
    row()
        .w_full()
        .py(40.0)
        .gap(10.0)
        .center()
        .child(Spinner::new().size(16.0).color(look.accent).build(&look.k))
        .child(
            text("Loading the catalog")
                .font(ty::BODY)
                .color(look.muted()),
        )
}

fn failed(look: &Look, error: &str) -> El {
    parts::empty(
        look,
        &icons::WARNING,
        "The store did not answer",
        error,
        Some(
            Button::new("Try again")
                .icon(&icons::REFRESH)
                .variant(ButtonVariant::Neutral)
                .on_press(|s: &mut App| s.fetch_store())
                .build(&look.k),
        ),
    )
}

fn list(s: &App, look: &Look) -> El {
    let needle = s.form.store_search.text.trim().to_lowercase();
    let installed = installed_keys(s.lib.mods.iter().map(|m| m.name.as_str()));
    let rows: Vec<El> = s
        .store
        .items
        .iter()
        .filter(|i| needle.is_empty() || matches(i, &needle))
        .take(MAX_ROWS)
        .map(|i| item_row(s, look, i, &installed))
        .collect();
    if rows.is_empty() {
        return col().w_full().py(30.0).center().child(
            text(if s.store.items.is_empty() {
                "The catalog is empty"
            } else {
                "Nothing in the store matches the search"
            })
            .font(ty::BODY)
            .color(look.muted()),
        );
    }
    col().w_full().children(rows)
}

fn matches(item: &StoreItem, needle: &str) -> bool {
    item.name.to_lowercase().contains(needle)
        || item
            .short_description
            .as_deref()
            .is_some_and(|d| d.to_lowercase().contains(needle))
        || item
            .author()
            .is_some_and(|a| a.to_lowercase().contains(needle))
}

fn item_row(s: &App, look: &Look, item: &StoreItem, installed: &HashSet<String>) -> El {
    let have = installed.contains(&item.name.trim().to_lowercase())
        || installed.contains(&item.mod_id.trim().to_lowercase());
    let busy = s.store.installing.contains(&item.mod_id);
    let id = item.mod_id.clone();
    let action = Button::new(if have {
        "Installed"
    } else if busy {
        "Queuing"
    } else {
        "Install"
    })
    .icon(if have {
        &icons::CHECK
    } else {
        &icons::DOWNLOAD
    })
    .variant(if have {
        ButtonVariant::Ghost
    } else {
        ButtonVariant::Neutral
    })
    .disabled(have || busy)
    .on_press(move |s: &mut App| s.store_install(id.clone()))
    .build(&look.k);
    row()
        .key(format!("store-{}", item.mod_id))
        .w_full()
        .px(18.0)
        .py(12.0)
        .gap(14.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], look.edge)
        .child(describe(look, item).grow(1.0).min_w(0.0))
        .children(item.latest_version.clone().map(|v| parts::tag(look, v)))
        .child(
            text(format!("{} downloads", item.downloads))
                .font(ty::SMALL)
                .color(look.faint()),
        )
        .child(action)
}

fn describe(look: &Look, item: &StoreItem) -> El {
    let mut head = row().gap(8.0).center_y().child(
        text(item.name.clone())
            .font(ty::BODY_STRONG)
            .color(look.text())
            .truncate(),
    );
    if item.is_library() {
        head = head.child(parts::tag(look, "Library"));
    }
    if let Some(author) = item.author() {
        head = head.child(
            text(format!("by {author}"))
                .font(ty::SMALL)
                .color(look.faint()),
        );
    }
    col().gap(3.0).child(head).children(
        item.short_description
            .clone()
            .map(|d| text(d).font(ty::SMALL).color(look.muted()).truncate()),
    )
}
