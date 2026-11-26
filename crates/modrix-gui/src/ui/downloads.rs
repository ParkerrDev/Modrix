// SPDX-License-Identifier: GPL-2.0-only
//! Browser hand-off downloads: live progress, install results, and how to
//! pair the extension.

use modrix_download::{DownloadState, DownloadStatus};
use modrix_service::InstallOutcome;
use rune::{El, col, el, row, text};
use rune_kit::{IconButton, IconButtonVariant, Progress};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::state::App;
use crate::ui::parts::{self, glyph};

/// The Downloads screen.
pub fn view(s: &App, look: &Look) -> El {
    let list = if s.downloads.is_empty() {
        parts::panel(look, "downloads-empty").child(parts::empty(
            look,
            &icons::DOWNLOAD,
            "No downloads yet",
            "With the Modrix browser extension installed, click Mod Manager Download on a Nexus mod page and it lands here, then installs itself.",
            None,
        ))
    } else {
        let rows = s
            .downloads
            .iter()
            .take(200)
            .map(|d| download_row(s, look, d));
        parts::section(
            look,
            "downloads",
            "Transfers",
            None,
            col().w_full().pb(8.0).children(rows),
        )
    };
    col()
        .w_full()
        .gap(space::LG)
        .child(list)
        .child(pairing(s, look))
}

fn download_row(s: &App, look: &Look, d: &DownloadStatus) -> El {
    let name = d.file.file_name().map_or_else(
        || d.file.display().to_string(),
        |n| n.to_string_lossy().trim_end_matches(".part").to_owned(),
    );
    let (_, tint) = status(s, look, d);
    let live = matches!(d.state, DownloadState::Active | DownloadState::Queued);
    let id = d.id;
    row()
        .key(format!("dl-{id:?}"))
        .w_full()
        .px(18.0)
        .py(12.0)
        .gap(space::MD)
        .center_y()
        .child(parts::icon_tile(&icons::PACKAGE, tint, 36.0))
        .child(download_body(s, look, d, name))
        .children(live.then(|| {
            IconButton::new(&icons::CLOSE)
                .variant(IconButtonVariant::GhostMuted)
                .label("Cancel download")
                .on_press(move |s: &mut App| s.cancel_download(id))
                .build(&look.k)
        }))
}

fn download_body(s: &App, look: &Look, d: &DownloadStatus, name: String) -> El {
    let (label, tint) = status(s, look, d);
    let size = match d.total {
        Some(t) => format!("{} of {}", crate::fmt::bytes(d.done), crate::fmt::bytes(t)),
        None => crate::fmt::bytes(d.done),
    };
    let live = matches!(d.state, DownloadState::Active | DownloadState::Queued);
    let fraction = match d.state {
        DownloadState::Complete => Some(1.0),
        DownloadState::Queued => None,
        _ => Some(crate::fmt::fraction(d.done, d.total)),
    };
    col()
        .grow(1.0)
        .min_w(0.0)
        .gap(6.0)
        .child(
            row()
                .w_full()
                .gap(space::SM)
                .center_y()
                .child(
                    text(name)
                        .font(ty::BODY_STRONG)
                        .color(look.text())
                        .truncate()
                        .grow(1.0),
                )
                .child(text(size).font(ty::MONO).color(look.muted())),
        )
        .children(live.then(|| Progress::new(fraction).build(&look.k)))
        .child(
            row()
                .gap(space::SM)
                .center_y()
                .child(el().size(6.0, 6.0).radius(3.0).fill(tint))
                .child(text(label).font(ty::SMALL).color(look.muted()))
                .children((d.state == DownloadState::Active).then(|| {
                    text(format!(
                        "{} connections, {}",
                        d.connections,
                        crate::fmt::percent(d.done, d.total)
                    ))
                    .font(ty::SMALL)
                    .color(look.faint())
                })),
        )
}

fn status(s: &App, look: &Look, d: &DownloadStatus) -> (String, rune::Color) {
    match d.state {
        DownloadState::Queued => ("Queued".to_owned(), look.faint()),
        DownloadState::Active => ("Downloading".to_owned(), look.accent),
        DownloadState::Paused => ("Paused".to_owned(), look.warn),
        DownloadState::Failed => ("Failed".to_owned(), look.bad),
        DownloadState::Complete => match s.outcomes.get(&d.id) {
            Some(InstallOutcome::Installed { name, .. }) => {
                (format!("Installed as {name}"), look.good)
            }
            Some(InstallOutcome::Failed(error)) => (format!("Install failed: {error}"), look.bad),
            Some(InstallOutcome::NoGame) => ("Kept: no game matched".to_owned(), look.warn),
            None => ("Installing".to_owned(), look.accent),
        },
    }
}

fn pairing(s: &App, look: &Look) -> El {
    let body = if let Some(link) = &s.link {
        let token = link.token.clone();
        col()
            .w_full()
            .pb(10.0)
            .child(parts::kv(
                look,
                "Port",
                text(link.port.to_string())
                    .font(ty::MONO)
                    .color(look.text()),
            ))
            .child(parts::kv(
                look,
                "Token",
                row()
                    .gap(space::SM)
                    .center_y()
                    .child(text(mask(&link.token)).font(ty::MONO).color(look.text()))
                    .child(
                        IconButton::new(&icons::COPY)
                            .variant(IconButtonVariant::GhostMuted)
                            .label("Copy token")
                            .on_press_tx(move |_: &mut App, tx| tx.copy(token.clone()))
                            .build(&look.k),
                    ),
            ))
    } else {
        col().w_full().px(18.0).pb(16.0).child(
            text(if s.already_running {
                "Another Modrix window is receiving browser downloads. Close it to receive them here."
            } else {
                "The browser link starts with the engine."
            })
            .font(ty::BODY)
            .color(look.muted())
            .wrap(),
        )
    };
    parts::section(
        look,
        "pairing",
        "Browser extension",
        Some(glyph(&icons::LINK, 16.0, look.faint())),
        body,
    )
}

fn mask(token: &str) -> String {
    let tail: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("........{tail}")
}
