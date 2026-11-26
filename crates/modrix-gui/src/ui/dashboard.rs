// SPDX-License-Identifier: GPL-2.0-only
//! The overview: the game's logo over its art, the numbers that matter,
//! setup health and what happened lately.

use modrix_core::health::Severity;
use rune::{El, SurfaceFit, col, el, row, text};
use rune_kit::{Button, ButtonVariant};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::state::{App, Screen};
use crate::ui::parts::{self, glyph};

/// The overview screen.
pub fn view(s: &App, look: &Look, _layer: &mut Vec<El>) -> El {
    let Some(game) = s.game() else {
        return welcome(s, look);
    };
    col()
        .w_full()
        .gap(space::LG)
        .child(hero(
            s,
            look,
            &game.name,
            &game.install_path.display().to_string(),
        ))
        .child(stats(s, look))
        .child(
            row()
                .w_full()
                .gap(space::LG)
                .child(health(s, look).grow(1.0).w(0.0))
                .child(recent(s, look).grow(1.0).w(0.0)),
        )
        .children((!s.lib.external.is_empty()).then(|| external(s, look)))
}

fn hero(s: &App, look: &Look, name: &str, path: &str) -> El {
    let logo = s.game_art().and_then(|a| a.logo.as_ref());
    let title = match logo {
        Some(px) => el()
            .h(110.0)
            .w(360.0)
            .pixels_fit(px, SurfaceFit::Fit)
            .shadow(0.0, 30.0, look.shadow()),
        None => text(name.to_owned()).font(ty::HERO).color(look.text()),
    };
    let blocked = s.blockers().next().is_some();
    let actions = row()
        .gap(space::SM)
        .child(
            Button::new("Deploy")
                .icon(&icons::DEPLOY)
                .variant(ButtonVariant::Submit)
                .disabled(blocked || s.busy > 0)
                .on_press(|s: &mut App| s.deploy())
                .build(&look.k),
        )
        .child(
            Button::new("Add mods")
                .icon(&icons::PLUS)
                .variant(ButtonVariant::Neutral)
                .on_press_tx(|_: &mut App, tx| crate::ui::mods::pick_archives(tx))
                .build(&look.k),
        )
        .child(
            Button::new("Verify")
                .icon(&icons::SHIELD)
                .variant(ButtonVariant::Ghost)
                .on_press(|s: &mut App| s.verify())
                .build(&look.k),
        );
    col()
        .key("hero-block")
        .w_full()
        .min_h(210.0)
        .pt(26.0)
        .gap(14.0)
        .justify_end()
        .child(title)
        .child(
            row()
                .gap(space::SM)
                .center_y()
                .child(glyph(&icons::FOLDER, 14.0, look.muted()))
                .child(
                    text(path.to_owned())
                        .font(ty::SMALL)
                        .color(look.muted())
                        .truncate(),
                ),
        )
        .child(actions)
}

fn stats(s: &App, look: &Look) -> El {
    let conflicts = s.unresolved_conflicts();
    row()
        .w_full()
        .gap(space::MD)
        .child(parts::stat(
            look,
            "stat-mods",
            (&icons::LAYERS, look.accent),
            s.lib.mods.len().to_string(),
            "Mods installed",
        ))
        .child(parts::stat(
            look,
            "stat-enabled",
            (&icons::CHECK, look.good),
            s.lib.enabled.len().to_string(),
            "Enabled",
        ))
        .children(s.screen_visible(Screen::LoadOrder).then(|| {
            parts::stat(
                look,
                "stat-plugins",
                (&icons::PUZZLE, look.warn),
                s.lib.plugins.len().to_string(),
                "Plugins",
            )
        }))
        .child(parts::stat(
            look,
            "stat-conflicts",
            (
                &icons::CONFLICT,
                if conflicts > 0 { look.bad } else { look.good },
            ),
            conflicts.to_string(),
            "Unresolved conflicts",
        ))
}

fn health(s: &App, look: &Look) -> El {
    let body = if s.lib.health.is_empty() {
        row()
            .w_full()
            .px(18.0)
            .pb(18.0)
            .gap(12.0)
            .center_y()
            .child(parts::icon_tile(&icons::SHIELD, look.good, 36.0))
            .child(
                col()
                    .gap(2.0)
                    .child(text("All clear").font(ty::BODY_STRONG).color(look.text()))
                    .child(
                        text("Setup checks found nothing to fix")
                            .font(ty::SMALL)
                            .color(look.muted()),
                    ),
            )
    } else {
        let rows = s.lib.health.iter().take(8).enumerate().map(|(i, issue)| {
            let (tint, i_icon) = match issue.severity {
                Severity::Error => (look.bad, &icons::WARNING),
                Severity::Warning => (look.warn, &icons::WARNING),
                Severity::Info => (look.accent, &icons::INFO),
            };
            row()
                .key(format!("issue-{i}"))
                .w_full()
                .px(18.0)
                .py(9.0)
                .gap(12.0)
                .child(glyph(i_icon, 16.0, tint))
                .child(
                    text(issue.message.clone())
                        .font(ty::BODY)
                        .color(look.text())
                        .wrap()
                        .grow(1.0),
                )
                .children(
                    issue
                        .blocking
                        .then(|| parts::chip("Blocks deploy", look.bad)),
                )
        });
        col().w_full().pb(10.0).children(rows)
    };
    parts::section(look, "health", "Setup health", None, body)
}

fn recent(s: &App, look: &Look) -> El {
    let body = if s.notes.is_empty() {
        col().w_full().px(18.0).pb(18.0).child(
            text("Installs, deploys and checks show up here")
                .font(ty::BODY)
                .color(look.muted()),
        )
    } else {
        let rows = s.notes.iter().take(6).map(|n| {
            row()
                .key(format!("recent-{}", n.id))
                .w_full()
                .px(18.0)
                .py(8.0)
                .gap(12.0)
                .center_y()
                .child(
                    el().size(7.0, 7.0)
                        .radius(4.0)
                        .fill(parts::tone_color(look, n.tone)),
                )
                .child(
                    text(n.text.clone())
                        .font(ty::BODY)
                        .color(look.text())
                        .truncate()
                        .grow(1.0),
                )
                .child(
                    text(crate::ui::shell::ago(s.now - n.at))
                        .font(ty::SMALL)
                        .color(look.faint()),
                )
        });
        col().w_full().pb(10.0).children(rows)
    };
    parts::section(look, "recent", "Recent activity", None, body)
}

fn external(s: &App, look: &Look) -> El {
    let rows = s.lib.external.iter().take(12).enumerate().map(|(i, m)| {
        row()
            .key(format!("ext-{i}"))
            .w_full()
            .px(18.0)
            .py(8.0)
            .gap(12.0)
            .center_y()
            .child(glyph(&icons::PACKAGE, 16.0, look.faint()))
            .child(
                text(m.name.clone())
                    .font(ty::BODY)
                    .color(look.text())
                    .grow(1.0)
                    .truncate(),
            )
            .child(parts::tag(look, m.label.clone()))
            .child(
                text(format!("{} files", m.files))
                    .font(ty::SMALL)
                    .color(look.faint()),
            )
    });
    parts::section(
        look,
        "external",
        "Found in the game folder",
        Some(
            text("Not managed by Modrix")
                .font(ty::SMALL)
                .color(look.faint()),
        ),
        col().w_full().pb(10.0).children(rows),
    )
}

fn welcome(s: &App, look: &Look) -> El {
    let found = s.unregistered_detected().len();
    let body = if found > 0 {
        format!(
            "Modrix found {found} supported games on this computer. Add one to start managing its mods."
        )
    } else {
        "Point Modrix at a game folder to start managing its mods.".to_owned()
    };
    col().w_full().pt(60.0).child(
        parts::panel(look, "welcome").child(parts::empty(
            look,
            &icons::GAMEPAD,
            "Welcome to Modrix",
            &body,
            Some(
                Button::new("Set up a game")
                    .icon(&icons::PLUS)
                    .variant(ButtonVariant::Submit)
                    .on_press(|s: &mut App| s.screen = Screen::Games)
                    .build(&look.k),
            ),
        )),
    )
}
