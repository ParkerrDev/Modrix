// SPDX-License-Identifier: GPL-2.0-only
//! The Downloads screen: live hand-off transfers.

use iced::widget::{button, column, container, progress_bar, row, scrollable, text};
use iced::{Alignment, Color, Length};
use modman_download::{DownloadState, DownloadStatus};
use modman_service::InstallOutcome;

use super::{El, copy_button, empty_state};
use crate::app::{App, Message};
use crate::{fmt, icons, theme};

/// The downloads body.
pub(super) fn body(app: &App) -> El<'_> {
    let mut page = column![].spacing(14);
    page = page.push(pairing_strip(app));
    if app.downloads.is_empty() {
        page = page.push(empty_state("No downloads yet."));
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
            icons::dot(7.0, theme::OK),
            text(format!("127.0.0.1:{}", link.port)).size(13),
            iced::widget::Space::with_width(Length::Fill),
            copy_button(link.token.clone()),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into(),
        (None, true) => text("Another ModManager instance is receiving downloads.")
            .size(13)
            .color(theme::INFO)
            .into(),
        (None, false) => text("Hand-off listener not running.")
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
    if let Some(InstallOutcome::Failed(error)) = outcome {
        inner = inner.push(text(error).size(12).color(theme::DANGER));
    }
    container(inner)
        .padding([10, 14])
        .style(theme::table_row(true))
        .into()
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
        Some(total) if total > 0 => format!("{done} of {}", fmt::bytes(total)),
        _ => done,
    }
}

fn badge(state: DownloadState, outcome: Option<&InstallOutcome>) -> (Color, &'static str) {
    match state {
        DownloadState::Queued => (theme::FAINT, "QUEUED"),
        DownloadState::Active => (theme::ACCENT, "ACTIVE"),
        DownloadState::Paused => (theme::INFO, "PAUSED"),
        DownloadState::Failed => (theme::DANGER, "FAILED"),
        DownloadState::Complete => match outcome {
            Some(InstallOutcome::Installed { .. }) => (theme::OK, "INSTALLED"),
            Some(InstallOutcome::Failed(_)) => (theme::DANGER, "INSTALL FAILED"),
            Some(InstallOutcome::NoGame) => (theme::INFO, "DOWNLOADED"),
            None => (theme::ACCENT, "FINISHING"),
        },
    }
}
