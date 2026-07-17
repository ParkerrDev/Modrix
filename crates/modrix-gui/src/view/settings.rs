// SPDX-License-Identifier: GPL-2.0-only
//! The Settings screen: service pairing, locations, profiles, about.

use iced::widget::{button, column, row, scrollable, text, text_input};
use iced::{Alignment, Length};

use super::{BOLD, El, copy_button, labeled_card};
use crate::app::{App, Message, UpdateState};
use crate::theme;

/// The settings body.
pub(super) fn body(app: &App) -> El<'_> {
    scrollable(
        column![
            theme_card(),
            updates_card(app),
            service_card(app),
            extension_card(),
            plugins_card(app),
            locations_card(app),
            profiles_card(app),
            about_card(),
        ]
        .spacing(16),
    )
    .height(Length::Fill)
    .into()
}

/// Update status: the installed version, and - when GitHub reports a newer
/// release - its notes plus a one-click install (Windows) or a manual link.
fn updates_card(app: &App) -> El<'_> {
    let current = row![
        text("Installed").size(12).color(theme::muted()).width(130),
        text(concat!("v", env!("CARGO_PKG_VERSION")))
            .size(12)
            .width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let inner: El<'_> = match &app.update {
        UpdateState::Available(info) => update_available(false, info),
        UpdateState::Installing(info) => update_available(true, info),
        UpdateState::Idle => row![
            text("You're on the latest version.")
                .size(12)
                .color(theme::muted())
                .width(Length::Fill),
            button(text("Check for updates").size(12))
                .padding([6, 12])
                .style(theme::ghost)
                .on_press(Message::CheckUpdates),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into(),
    };
    labeled_card("UPDATES", column![current, inner].spacing(12).into())
}

/// The "an update is available" body: version, a notes excerpt, and the action
/// (a self-install button on Windows, else a link to the release page).
fn update_available(updating: bool, info: &modrix_update::UpdateInfo) -> El<'static> {
    let mut col = column![
        text(format!("Update available: v{}", info.version))
            .size(13)
            .font(BOLD)
            .color(theme::accent()),
    ]
    .spacing(8);
    if !info.notes.is_empty() {
        col = col.push(
            text(notes_excerpt(&info.notes))
                .size(12)
                .color(theme::faint()),
        );
    }
    if info.can_self_install() {
        let label = if updating {
            "Installing…"
        } else {
            "Download & install"
        };
        let mut install = button(text(label).size(13))
            .padding([8, 16])
            .style(theme::primary);
        if !updating {
            install = install.on_press(Message::StartUpdate);
        }
        col.push(install).into()
    } else {
        col.push(kv_copy("Release", info.release_url.clone()))
            .into()
    }
}

/// The first line of the release notes, trimmed to one short line.
fn notes_excerpt(notes: &str) -> String {
    const MAX: usize = 160;
    let first = notes.lines().next().unwrap_or("").trim();
    if first.chars().count() > MAX {
        let head: String = first.chars().take(MAX).collect();
        format!("{head}…")
    } else {
        first.to_owned()
    }
}

/// The theme picker: every shipped theme, the active one marked.
fn theme_card() -> El<'static> {
    let active = theme::spec().id;
    let mut choices = row![].spacing(10);
    for spec in theme::ALL {
        let is_active = spec.id == active;
        let label = if is_active {
            format!("{} (active)", spec.name)
        } else {
            spec.name.to_owned()
        };
        let mut pick = button(text(label).size(13))
            .padding([8, 16])
            .style(if is_active {
                theme::primary
            } else {
                theme::ghost
            });
        if !is_active {
            pick = pick.on_press(Message::ThemePicked(spec.id.to_owned()));
        }
        choices = choices.push(pick);
    }
    labeled_card("THEME", choices.into())
}

/// Installed community plugins, with removal and cleanup.
fn plugins_card(app: &App) -> El<'_> {
    let mut listing = column![].spacing(8);
    if app.installed_plugins.is_empty() {
        listing = listing.push(
            text("None installed - game support fetched from the registry appears here.")
                .size(12)
                .color(theme::faint()),
        );
    }
    for plugin in &app.installed_plugins {
        let in_use = app.games.iter().any(|g| g.plugin_id == plugin.id);
        let mut remove = button(text("Remove").size(11))
            .padding([4, 10])
            .style(theme::danger_ghost);
        if !in_use {
            remove = remove.on_press(Message::UninstallPlugin(plugin.id.clone()));
        }
        let label = if in_use { "in use" } else { "" };
        listing = listing.push(
            row![
                text(&plugin.name).size(13).width(Length::Fill),
                text(format!("v{}", plugin.version))
                    .size(12)
                    .color(theme::muted()),
                text(label).size(11).color(theme::faint()),
                remove,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
    }
    let gc = button(text("Remove unused").size(12))
        .padding([6, 12])
        .style(theme::ghost)
        .on_press(Message::GcPlugins);
    labeled_card("COMMUNITY PLUGINS", column![listing, gc].spacing(12).into())
}

fn service_card(app: &App) -> El<'_> {
    let inner: El<'_> = match (&app.link, app.already_running) {
        (Some(link), _) => column![
            kv_copy("Address", format!("127.0.0.1:{}", link.port)),
            kv_copy("Extension token", link.token.clone()),
            text("Rotates each restart.").size(12).color(theme::faint()),
        ]
        .spacing(10)
        .into(),
        (None, true) => text("Another instance holds the port. Close it and restart the GUI.")
            .size(13)
            .color(theme::info())
            .into(),
        (None, false) => text("The hand-off listener failed to start.")
            .size(13)
            .color(theme::danger())
            .into(),
    };
    labeled_card("HAND-OFF SERVICE", inner)
}

fn extension_card() -> El<'static> {
    let steps = column![
        bullet("Load the `extension/` folder unpacked (developer mode)."),
        bullet("That's it - no token or pairing needed while Modrix runs."),
        bullet("Click Download on nexusmods.com."),
        bullet("The token above is an advanced fallback (extension options)."),
    ]
    .spacing(6);
    labeled_card("BROWSER EXTENSION", steps.into())
}

fn locations_card(app: &App) -> El<'_> {
    let inner: El<'_> = match &app.paths {
        Some(paths) => column![
            kv_copy("Data", paths.data_dir().display().to_string()),
            kv_copy("Config", paths.config_dir().display().to_string()),
            kv_copy("Cache", paths.cache_dir().display().to_string()),
            text("Extra game definitions: <config>/games/<id>/game.toml")
                .size(12)
                .color(theme::faint()),
        ]
        .spacing(10)
        .into(),
        None => text("Still starting…")
            .size(13)
            .color(theme::muted())
            .into(),
    };
    labeled_card("LOCATIONS", inner)
}

fn profiles_card(app: &App) -> El<'_> {
    let mut listing = column![].spacing(6);
    for profile in &app.profiles {
        listing = listing.push(
            row![
                text("•").size(15).color(if profile.is_active {
                    theme::accent()
                } else {
                    theme::faint()
                }),
                text(&profile.name).size(13),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    let form = row![
        text_input("New profile name", &app.form.profile_name)
            .on_input(Message::ProfileNameChanged)
            .on_submit(Message::CreateProfile)
            .size(13)
            .padding(9)
            .style(theme::input),
        button(text("Create").size(13))
            .padding([8, 16])
            .style(theme::ghost)
            .on_press(Message::CreateProfile),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    labeled_card("PROFILES", column![listing, form].spacing(12).into())
}

fn about_card() -> El<'static> {
    let inner = column![
        row![
            text("Modrix").size(14).font(BOLD),
            text(concat!("v", env!("CARGO_PKG_VERSION")))
                .size(12)
                .color(theme::muted()),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        text("GPL-2.0-only · no API keys · no telemetry")
            .size(12)
            .color(theme::faint()),
    ]
    .spacing(6);
    labeled_card("ABOUT", inner.into())
}

fn kv_copy(label: &str, value: String) -> El<'_> {
    row![
        text(label).size(12).color(theme::muted()).width(130),
        text(value.clone()).size(12).width(Length::Fill),
        copy_button(value),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn bullet(line: &str) -> El<'_> {
    row![
        text("·").size(13).color(theme::accent()),
        text(line).size(12).color(theme::text()),
    ]
    .spacing(8)
    .into()
}
