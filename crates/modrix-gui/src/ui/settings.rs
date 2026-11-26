// SPDX-License-Identifier: GPL-2.0-only
//! Settings: appearance, profiles, where things live, installed game
//! support, updates, about.

use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, IconButton, IconButtonVariant, SegmentedControl, TextInput};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::prefs::Appearance;
use crate::state::{App, Tone, UpdateState};
use crate::ui::parts;

/// The Settings screen.
pub fn view(s: &App, look: &Look) -> El {
    col()
        .w_full()
        .gap(space::LG)
        .child(appearance(s, look))
        .children(s.lib.game.is_some().then(|| profiles(s, look)))
        .child(paths(s, look))
        .child(plugins(s, look))
        .child(updates(s, look))
}

fn appearance(s: &App, look: &Look) -> El {
    let labels: Vec<&str> = Appearance::ALL.iter().map(|a| a.label()).collect();
    let current = Appearance::ALL.iter().position(|a| *a == s.appearance);
    let picker = SegmentedControl::new(&labels, current)
        .on_pick(|s: &mut App, i, _| {
            let Some(choice) = Appearance::ALL.get(i).copied() else {
                return;
            };
            s.appearance = choice;
            if let Some(paths) = &s.paths
                && let Err(error) = crate::prefs::save(paths, choice)
            {
                s.note(
                    Tone::Error,
                    format!("Could not save the appearance: {error}"),
                );
            }
        })
        .build(&look.k);
    parts::section(
        look,
        "appearance",
        "Appearance",
        None,
        col()
            .w_full()
            .pb(10.0)
            .child(parts::kv(look, "Theme", row().child(picker)))
            .child(parts::kv(
                look,
                "Accent",
                text("Taken from the selected game's artwork")
                    .font(ty::BODY)
                    .color(look.muted()),
            )),
    )
}

fn profiles(s: &App, look: &Look) -> El {
    let list = s.lib.profiles.iter().map(|p| {
        let active = s.lib.profile.as_ref().is_some_and(|a| a.id == p.id);
        let name = p.name.clone();
        row()
            .key(format!("profile-{}", p.id))
            .w_full()
            .min_h(44.0)
            .px(18.0)
            .gap(space::MD)
            .center_y()
            .child(
                text(p.name.clone())
                    .font(ty::BODY)
                    .color(look.text())
                    .grow(1.0),
            )
            .child(if active {
                parts::chip("Active", look.accent)
            } else {
                Button::new("Use")
                    .variant(ButtonVariant::Ghost)
                    .on_press(move |s: &mut App| s.pick_profile(&name))
                    .build(&look.k)
            })
    });
    let create = row()
        .w_full()
        .px(18.0)
        .py(10.0)
        .gap(space::SM)
        .child(
            TextInput::new(&s.form.profile_name)
                .placeholder("New profile name")
                .width(Some(300.0))
                .on_edit(|s: &mut App| &mut s.form.profile_name)
                .on_submit(|s: &mut App, _| s.create_profile())
                .build(&look.k),
        )
        .child(
            Button::new("Create")
                .icon(&icons::PLUS)
                .variant(ButtonVariant::Neutral)
                .on_press(|s: &mut App| s.create_profile())
                .build(&look.k),
        );
    parts::section(
        look,
        "profiles",
        "Profiles",
        None,
        col().w_full().pb(6.0).children(list).child(create),
    )
}

fn path_row(look: &Look, label: &str, path: Option<std::path::PathBuf>) -> El {
    let shown = path
        .as_ref()
        .map_or_else(|| "Not set".to_owned(), |p| p.display().to_string());
    let reveal = path.map(|p| {
        IconButton::new(&icons::FOLDER)
            .variant(IconButtonVariant::GhostMuted)
            .label("Show in file manager")
            .on_press_tx(move |s: &mut App, tx| {
                if let Err(error) = tx.reveal_path(&p) {
                    s.note(Tone::Error, error.to_string());
                }
            })
            .build(&look.k)
    });
    parts::kv(
        look,
        label,
        row()
            .gap(space::SM)
            .center_y()
            .child(
                text(shown)
                    .font(ty::MONO)
                    .color(look.text())
                    .truncate_head()
                    .grow(1.0)
                    .w(0.0),
            )
            .children(reveal),
    )
}

fn paths(s: &App, look: &Look) -> El {
    let p = s.paths.as_ref();
    let body = col()
        .w_full()
        .pb(10.0)
        .child(path_row(
            look,
            "Data",
            p.map(|p| p.data_dir().to_path_buf()),
        ))
        .child(path_row(
            look,
            "Config",
            p.map(|p| p.config_dir().to_path_buf()),
        ))
        .child(path_row(
            look,
            "Cache",
            p.map(|p| p.cache_dir().to_path_buf()),
        ))
        .child(path_row(
            look,
            "Staging",
            p.map(modrix_core::Paths::staging_root),
        ));
    parts::section(look, "paths", "Where things live", None, body)
}

fn plugins(s: &App, look: &Look) -> El {
    let rows = s.installed_plugins.iter().map(|p| {
        let id = p.id.clone();
        row()
            .key(format!("plugin-{}", p.id))
            .w_full()
            .min_h(44.0)
            .px(18.0)
            .gap(space::MD)
            .center_y()
            .child(
                text(p.name.clone())
                    .font(ty::BODY)
                    .color(look.text())
                    .grow(1.0),
            )
            .child(parts::tag(look, p.version.clone()))
            .child(
                IconButton::new(&icons::TRASH)
                    .variant(IconButtonVariant::GhostMuted)
                    .label(format!("Remove {}", p.name))
                    .on_press(move |s: &mut App| s.uninstall_plugin(&id))
                    .build(&look.k),
            )
    });
    let body = if s.installed_plugins.is_empty() {
        col().w_full().px(18.0).pb(16.0).child(
            text("Built-in games need nothing extra. Support installed from the community registry shows up here.")
                .font(ty::BODY)
                .color(look.muted())
                .wrap(),
        )
    } else {
        col().w_full().pb(6.0).children(rows)
    };
    parts::section(
        look,
        "installed-plugins",
        "Game support",
        Some(
            Button::new("Remove unused")
                .variant(ButtonVariant::GhostMuted)
                .on_press(|s: &mut App| s.gc_plugins())
                .build(&look.k),
        ),
        body,
    )
}

fn updates(s: &App, look: &Look) -> El {
    let (line, action) = match &s.update {
        UpdateState::Idle => (
            "Modrix is up to date".to_owned(),
            Button::new("Check now")
                .icon(&icons::REFRESH)
                .variant(ButtonVariant::Neutral)
                .on_press(|s: &mut App| s.check_updates())
                .build(&look.k),
        ),
        UpdateState::Checking => (
            "Checking for updates".to_owned(),
            Button::new("Checking")
                .variant(ButtonVariant::Loading)
                .build(&look.k),
        ),
        UpdateState::Available(info) => (
            format!("Modrix {} is available", info.version),
            Button::new("Install")
                .icon(&icons::UPDATE)
                .variant(ButtonVariant::Submit)
                .on_press(|s: &mut App| s.start_update())
                .build(&look.k),
        ),
        UpdateState::Installing(info) => (
            format!("Installing {}", info.version),
            Button::new("Installing")
                .variant(ButtonVariant::Loading)
                .build(&look.k),
        ),
    };
    let body = col()
        .w_full()
        .pb(10.0)
        .child(parts::kv(
            look,
            "Version",
            text(env!("CARGO_PKG_VERSION"))
                .font(ty::MONO)
                .color(look.text()),
        ))
        .child(parts::kv(
            look,
            "Updates",
            row()
                .gap(space::MD)
                .center_y()
                .child(text(line).font(ty::BODY).color(look.text()).grow(1.0))
                .child(action),
        ))
        .child(parts::kv(
            look,
            "License",
            text("GPL-2.0-only. Free software: share and change it.")
                .font(ty::BODY)
                .color(look.muted()),
        ));
    parts::section(look, "about", "About Modrix", None, body)
}
