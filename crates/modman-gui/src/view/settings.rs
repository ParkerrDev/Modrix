// SPDX-License-Identifier: GPL-2.0-only
//! The Settings screen: service pairing, locations, profiles, about.

use iced::widget::{button, column, row, scrollable, text, text_input};
use iced::{Alignment, Length};

use super::{BOLD, El, copy_button, labeled_card};
use crate::app::{App, Message};
use crate::theme;

/// The settings body.
pub(super) fn body(app: &App) -> El<'_> {
    scrollable(
        column![
            service_card(app),
            extension_card(),
            locations_card(app),
            profiles_card(app),
            about_card(),
        ]
        .spacing(16),
    )
    .height(Length::Fill)
    .into()
}

fn service_card(app: &App) -> El<'_> {
    let inner: El<'_> = match (&app.link, app.already_running) {
        (Some(link), _) => column![
            kv_copy("Address", format!("127.0.0.1:{}", link.port)),
            kv_copy("Extension token", link.token.clone()),
            text("Rotates each restart.").size(12).color(theme::FAINT),
        ]
        .spacing(10)
        .into(),
        (None, true) => text("Another instance holds the port. Close it and restart the GUI.")
        .size(13)
        .color(theme::INFO)
        .into(),
        (None, false) => text("The hand-off listener failed to start.")
            .size(13)
            .color(theme::DANGER)
            .into(),
    };
    labeled_card("HAND-OFF SERVICE", inner)
}

fn extension_card() -> El<'static> {
    let steps = column![
        bullet("Load the `extension/` folder unpacked (developer mode)."),
        bullet("Paste the address and token above into its options."),
        bullet("Click Download on nexusmods.com."),
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
                .color(theme::FAINT),
        ]
        .spacing(10)
        .into(),
        None => text("Still starting…").size(13).color(theme::MUTED).into(),
    };
    labeled_card("LOCATIONS", inner)
}

fn profiles_card(app: &App) -> El<'_> {
    let mut listing = column![].spacing(6);
    for profile in &app.profiles {
        listing = listing.push(
            row![
                text("•").size(15).color(if profile.is_active {
                    theme::ACCENT
                } else {
                    theme::FAINT
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
    labeled_card(
        "PROFILES",
        column![listing, form].spacing(12).into(),
    )
}

fn about_card() -> El<'static> {
    let inner = column![
        row![
            text("ModManager").size(14).font(BOLD),
            text(concat!("v", env!("CARGO_PKG_VERSION")))
                .size(12)
                .color(theme::MUTED),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        text("GPL-2.0-only · no API keys · no telemetry")
            .size(12)
            .color(theme::FAINT),
    ]
    .spacing(6);
    labeled_card("ABOUT", inner.into())
}

fn kv_copy(label: &str, value: String) -> El<'_> {
    row![
        text(label).size(12).color(theme::MUTED).width(130),
        text(value.clone()).size(12).width(Length::Fill),
        copy_button(value),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn bullet(line: &str) -> El<'_> {
    row![
        text("·").size(13).color(theme::ACCENT),
        text(line).size(12).color(theme::TEXT),
    ]
    .spacing(8)
    .into()
}
