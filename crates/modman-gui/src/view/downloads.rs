// SPDX-License-Identifier: GPL-2.0-only
//! The Downloads screen: live hand-off transfers.

use iced::widget::{button, column, container, progress_bar, row, scrollable, text};
use iced::{Alignment, Color, Length};
use modman_download::{DownloadState, DownloadStatus};
use modman_service::InstallOutcome;

use super::{El, copy_button, empty_state};
use crate::app::{App, Message};
use crate::{fmt, theme};

/// The downloads body.
pub(super) fn body(app: &App) -> El<'_> {
    let mut page = column![].spacing(16);
    page = page.push(pairing_strip(app));
    if app.downloads.is_empty() {
        page = page.push(empty_state(
            "No downloads yet. With the extension installed, click Download on nexusmods.com - \
             the transfer appears here and installs itself when done.",
        ));
    } else {
        let mut rows = column![].spacing(8);
        for status in &app.downloads {
            rows = rows.push(download_row(status, app.outcomes.get(&status.id)));
        }
        page = page.push(
            container(scrollable(rows).height(Length::Fill))
                .padding(14)
                .height(Length::Fill)
                .style(theme::card),
        );
    }
    page.into()
}

fn pairing_strip(app: &App) -> El<'_> {
    let inner: El<'_> = match (&app.link, app.already_running) {
        (Some(link), _) => row![
            text("•").size(16).color(theme::OK),
            text(format!("Hand-off listener on 127.0.0.1:{}", link.port)).size(13),
            iced::widget::Space::with_width(Length::Fill),
            text("extension token").size(12).color(theme::FAINT),
            copy_button(link.token.clone()),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into(),
        (None, true) => text(
            "Another ModManager instance (e.g. `modman serve`) is receiving browser \
             downloads - close it and restart the GUI to manage them here.",
        )
        .size(13)
        .color(theme::INFO)
        .into(),
        (None, false) => text("The hand-off listener is not running.")
            .size(13)
            .color(theme::DANGER)
            .into(),
    };
    container(inner)
        .padding([10, 14])
        .width(Length::Fill)
        .style(theme::inset)
        .into()
}

fn download_row<'a>(status: &'a DownloadStatus, outcome: Option<&'a InstallOutcome>) -> El<'a> {
    let name = status
        .file
        .file_name()
        .map_or_else(|| "download".to_owned(), |n| n.to_string_lossy().into_owned());
    let (color, label) = badge(status.state, outcome);
    let head = row![
        text(name).size(13).width(Length::Fill),
        container(text(label).size(10)).padding([2, 8]).style(theme::chip(color)),
        cancel_slot(status),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let meta = row![
        text(size_line(status)).size(12).color(theme::MUTED),
        iced::widget::Space::with_width(Length::Fill),
        text(fmt::percent(status.done, status.total)).size(12).color(theme::MUTED),
    ];
    let mut inner = column![
        head,
        progress_bar(0.0..=1.0, fmt::fraction(status.done, status.total))
            .height(5)
            .style(theme::progress),
        meta,
    ]
    .spacing(7);
    if let Some(detail) = outcome_detail(outcome) {
        inner = inner.push(detail);
    }
    container(inner)
        .padding([10, 14])
        .style(theme::table_row(true))
        .into()
}

/// The extra line explaining how the install phase ended.
fn outcome_detail(outcome: Option<&InstallOutcome>) -> Option<El<'_>> {
    match outcome {
        Some(InstallOutcome::Installed(name)) => Some(
            text(format!("installed as “{name}” - enable it under Mods, then deploy"))
                .size(12)
                .color(theme::OK)
                .into(),
        ),
        Some(InstallOutcome::Failed(error)) => Some(
            text(format!("install failed: {error}"))
                .size(12)
                .color(theme::DANGER)
                .into(),
        ),
        Some(InstallOutcome::NoGame) => Some(
            text("no registered game matched this download - register the game, then \
                  stage the file from the Mods screen")
                .size(12)
                .color(theme::INFO)
                .into(),
        ),
        None => None,
    }
}

fn cancel_slot(status: &DownloadStatus) -> El<'_> {
    let live = matches!(
        status.state,
        DownloadState::Active | DownloadState::Queued | DownloadState::Paused
    );
    button(text("×").size(14))
        .padding([3, 8])
        .style(theme::icon)
        .on_press_maybe(live.then_some(Message::CancelDownload(status.id)))
        .into()
}

fn size_line(status: &DownloadStatus) -> String {
    let done = fmt::bytes(status.done);
    match status.total {
        Some(total) if total > 0 => {
            format!("{done} of {} · {} connection(s)", fmt::bytes(total), status.connections)
        }
        _ => format!("{done} · {} connection(s)", status.connections),
    }
}

fn badge(state: DownloadState, outcome: Option<&InstallOutcome>) -> (Color, &'static str) {
    match state {
        DownloadState::Queued => (theme::FAINT, "QUEUED"),
        DownloadState::Active => (theme::ACCENT, "ACTIVE"),
        DownloadState::Paused => (theme::INFO, "PAUSED"),
        DownloadState::Failed => (theme::DANGER, "FAILED"),
        DownloadState::Complete => match outcome {
            Some(InstallOutcome::Installed(_)) => (theme::OK, "INSTALLED"),
            Some(InstallOutcome::Failed(_)) => (theme::DANGER, "INSTALL FAILED"),
            Some(InstallOutcome::NoGame) => (theme::INFO, "DOWNLOADED"),
            None => (theme::ACCENT, "FINISHING…"),
        },
    }
}
