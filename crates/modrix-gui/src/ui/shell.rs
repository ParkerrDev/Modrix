// SPDX-License-Identifier: GPL-2.0-only
//! The frame around every screen: the sidebar (wordmark, game switcher,
//! navigation, service status), the top bar (title, progress, profile,
//! Deploy, notifications), the toasts and the drop target.

use rune::{El, Spring, SurfaceFit, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, IconButton, IconButtonVariant, Menu, MenuItem, Placement, Popover,
    Select, Spinner,
};

use crate::icons;
use crate::look::{Look, space, ty};
use crate::state::{App, Popup, Screen, TOAST_TTL, UpdateState};
use crate::ui::parts::{self, glyph};

/// Height of one navigation row, gaps included.
const NAV_STEP: f32 = 40.0;

/// The left column.
pub fn sidebar(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    col()
        .key("sidebar")
        .w(space::SIDEBAR)
        .h_full()
        .px(14.0)
        .pt(18.0)
        .pb(14.0)
        .gap(18.0)
        .fill(look.glass)
        .backdrop_blur(30.0)
        .border(1.0, look.edge)
        .child(wordmark(look))
        .child(game_switcher(s, look, layer))
        .child(nav(s, look))
        .child(el().grow(1.0))
        .child(footer(s, look))
}

fn wordmark(look: &Look) -> El {
    row()
        .key("wordmark")
        .px(6.0)
        .h(28.0)
        .center_y()
        .window_drag()
        .child(text("MOD").font(ty::MARK).color(look.accent))
        .child(text("RIX").font(ty::MARK).color(look.text()))
        .child(el().grow(1.0))
        .child(parts::tag(look, concat!("v", env!("CARGO_PKG_VERSION"))))
}

fn switcher_trigger(s: &App, look: &Look, open: bool) -> El {
    let game = s.game();
    let cover = s.game_art().and_then(|a| a.cover.as_ref());
    let thumb = match cover {
        Some(px) => el()
            .size(38.0, 54.0)
            .radius(7.0)
            .clip_rounded(7.0)
            .pixels_fit(px, SurfaceFit::Fill),
        None => parts::icon_tile(&icons::GAMEPAD, look.accent, 38.0),
    };
    let (name, detail) = match game {
        Some(g) => (
            g.name.clone(),
            s.lib
                .profile
                .as_ref()
                .map_or_else(|| g.store.clone(), |p| format!("Profile: {}", p.name)),
        ),
        None => ("No game yet".to_owned(), "Add one to start".to_owned()),
    };
    row()
        .key("game-switcher")
        .w_full()
        .padding(8.0)
        .gap(10.0)
        .center_y()
        .radius(12.0)
        .fill(if open { look.pressed() } else { look.hover() })
        .border(1.0, look.edge)
        .hover_fill(look.pressed())
        .role("button")
        .label("Switch game")
        .on_press(|s: &mut App| toggle(s, Popup::GameMenu))
        .child(thumb)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(name)
                        .font(ty::BODY_STRONG)
                        .color(look.text())
                        .truncate(),
                )
                .child(text(detail).font(ty::SMALL).color(look.muted()).truncate()),
        )
        .child(glyph(&icons::CHEVRON_UP_DOWN, 16.0, look.faint()))
}

fn game_switcher(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let open = s.popup == Some(Popup::GameMenu);
    let trigger = switcher_trigger(s, look, open);
    let mut menu = Menu::new("game-menu").trigger(trigger).min_width(220.0);
    for g in &s.lib.games {
        let id = g.id;
        menu = menu.item(
            MenuItem::new(g.name.clone())
                .icon(&icons::GAMEPAD)
                .checked(Some(id) == s.lib.game)
                .on_select(move |s: &mut App| s.switch_game(id)),
        );
    }
    menu.separator()
        .item(
            MenuItem::new("Add a game")
                .icon(&icons::PLUS)
                .on_select(|s: &mut App| {
                    s.popup = None;
                    s.screen = Screen::Games;
                }),
        )
        .open(open)
        .on_close(|s: &mut App| s.popup = None)
        .build(&look.k, layer)
}

fn toggle(s: &mut App, popup: Popup) {
    s.popup = if s.popup == Some(popup) {
        None
    } else {
        Some(popup)
    };
    if s.popup == Some(Popup::Notes) {
        s.unread = 0;
    }
}

fn nav_badge(s: &App, screen: Screen) -> Option<(usize, bool)> {
    match screen {
        Screen::Mods => Some((s.lib.mods.len(), false)),
        Screen::LoadOrder => Some((s.lib.plugins.len(), false)),
        Screen::Conflicts => Some((
            s.unresolved_conflicts()
                .saturating_add(s.blockers().count()),
            true,
        )),
        Screen::Downloads => Some((s.active_downloads(), true)),
        _ => None,
    }
}

fn nav_icon(screen: Screen) -> &'static rune::Icon {
    match screen {
        Screen::Dashboard => &icons::HOME,
        Screen::Mods => &icons::LAYERS,
        Screen::LoadOrder => &icons::ORDER,
        Screen::Conflicts => &icons::CONFLICT,
        Screen::Downloads => &icons::DOWNLOAD,
        Screen::Games => &icons::GAMEPAD,
        Screen::Settings => &icons::SETTINGS,
    }
}

fn nav(s: &App, look: &Look) -> El {
    let screens: Vec<Screen> = Screen::ALL
        .into_iter()
        .filter(|x| s.screen_visible(*x))
        .collect();
    let at = screens.iter().position(|x| *x == s.screen).unwrap_or(0);
    #[expect(clippy::cast_precision_loss, reason = "a handful of nav rows")]
    let top = at as f32 * NAV_STEP;
    let pill = el()
        .key("nav-pill")
        .absolute(0.0, top)
        .w_full()
        .h(NAV_STEP - 4.0)
        .radius(10.0)
        .fill(look.accent_soft)
        .border(1.0, look.accent.alpha(0.22))
        .motion(Spring::new(0.38, 0.18))
        .child(
            el().absolute(0.0, 9.0)
                .size(3.0, 18.0)
                .radius(2.0)
                .fill(look.accent),
        );
    let rows = screens.into_iter().map(|screen| nav_row(s, look, screen));
    el().key("nav")
        .w_full()
        .role("navigation")
        .child(pill)
        .child(col().w_full().gap(4.0).children(rows))
}

fn nav_row(s: &App, look: &Look, screen: Screen) -> El {
    let on = s.screen == screen;
    let ink = if on { look.text() } else { look.muted() };
    let badge = nav_badge(s, screen)
        .filter(|(n, _)| *n > 0)
        .map(|(n, loud)| {
            if loud {
                parts::count(look, n, look.accent)
            } else {
                text(n.to_string()).font(ty::SMALL).color(look.faint())
            }
        });
    row()
        .key(screen.label())
        .w_full()
        .h(NAV_STEP - 4.0)
        .px(12.0)
        .gap(12.0)
        .center_y()
        .radius(10.0)
        .hover_fill(if on {
            rune::Color::TRANSPARENT
        } else {
            look.hover()
        })
        .role("tab")
        .checked(on)
        .label(screen.label())
        .on_press(move |s: &mut App| {
            s.screen = screen;
            s.popup = None;
        })
        .child(glyph(
            nav_icon(screen),
            18.0,
            if on { look.accent } else { look.faint() },
        ))
        .child(
            text(screen.label())
                .font(if on { ty::BODY_STRONG } else { ty::BODY })
                .color(ink)
                .grow(1.0),
        )
        .children(badge)
}

fn footer(s: &App, look: &Look) -> El {
    let (dot, line) = if let Some(error) = &s.boot_error {
        (look.bad, format!("Engine failed: {error}"))
    } else if !s.booted {
        (look.warn, "Starting the engine".to_owned())
    } else if s.already_running {
        (
            look.warn,
            "Another Modrix owns browser downloads".to_owned(),
        )
    } else if let Some(link) = &s.link {
        (look.good, format!("Browser link on port {}", link.port))
    } else {
        (look.faint(), "Browser link off".to_owned())
    };
    let update = match &s.update {
        UpdateState::Available(info) => Some(
            Button::new(format!("Update to {}", info.version))
                .icon(&icons::UPDATE)
                .variant(ButtonVariant::Submit)
                .expanded(true)
                .on_press(|s: &mut App| s.start_update())
                .build(&look.k),
        ),
        UpdateState::Installing(_) => Some(
            Button::new("Installing update")
                .variant(ButtonVariant::Loading)
                .expanded(true)
                .build(&look.k),
        ),
        _ => None,
    };
    col()
        .key("sidebar-footer")
        .w_full()
        .gap(10.0)
        .children(update)
        .child(
            row()
                .px(8.0)
                .gap(8.0)
                .center_y()
                .child(
                    el().size(7.0, 7.0)
                        .radius(4.0)
                        .fill(dot)
                        .shadow(0.0, 6.0, dot.alpha(0.6)),
                )
                .child(
                    text(line)
                        .font(ty::SMALL)
                        .color(look.muted())
                        .truncate()
                        .grow(1.0),
                ),
        )
}

/// The bar over the screen.
pub fn topbar(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let (title, subtitle) = heading(s);
    row()
        .key("topbar")
        .w_full()
        .h(76.0)
        .px(space::XL)
        .gap(space::MD)
        .center_y()
        .window_drag()
        .child(
            parts::page_title(look, title, &subtitle)
                .grow(1.0)
                .min_w(0.0),
        )
        .children(activity(s, look))
        .children(profile_picker(s, look, layer))
        .children(deploy_button(s, look, layer))
        .child(bell(s, look, layer))
}

fn heading(s: &App) -> (&'static str, String) {
    let game = s
        .game()
        .map_or_else(|| "No game selected".to_owned(), |g| g.name.clone());
    let enabled = s.lib.enabled.len();
    match s.screen {
        Screen::Dashboard => ("Overview", game),
        Screen::Mods => (
            "Mods",
            format!("{} installed, {enabled} enabled", s.lib.mods.len()),
        ),
        Screen::LoadOrder => (
            "Load order",
            format!("{} plugins, later wins", s.lib.plugins.len()),
        ),
        Screen::Conflicts => (
            "Conflicts",
            format!(
                "{} pairs, {} unresolved",
                s.lib.conflicts.len(),
                s.unresolved_conflicts()
            ),
        ),
        Screen::Downloads => ("Downloads", format!("{} active", s.active_downloads())),
        Screen::Games => ("Games", format!("{} registered", s.lib.games.len())),
        Screen::Settings => (
            "Settings",
            "Appearance, paths, pairing and updates".to_owned(),
        ),
    }
}

fn activity(s: &App, look: &Look) -> Option<El> {
    let message =
        s.op.as_ref()
            .map(|op| op.message.clone())
            .or_else(|| (s.busy > 0).then(|| "Working".to_owned()))?;
    let progress =
        s.op.as_ref()
            .filter(|op| op.total > 0)
            .map(|op| crate::fmt::fraction(op.done, Some(op.total)));
    Some(
        row()
            .key("activity")
            .h(34.0)
            .px(12.0)
            .gap(10.0)
            .center_y()
            .radius(17.0)
            .fill(look.glass_strong)
            .border(1.0, look.edge)
            .max_w(320.0)
            .child(Spinner::new().size(14.0).color(look.accent).build(&look.k))
            .child(
                text(message)
                    .font(ty::SMALL_STRONG)
                    .color(look.text())
                    .truncate(),
            )
            .children(progress.map(|f| {
                text(format!("{:.0}%", f * 100.0))
                    .font(ty::MONO)
                    .color(look.muted())
            })),
    )
}

fn profile_picker(s: &App, look: &Look, layer: &mut Vec<El>) -> Option<El> {
    if s.lib.profiles.is_empty() {
        return None;
    }
    let names: Vec<&str> = s.lib.profiles.iter().map(|p| p.name.as_str()).collect();
    let current = s.lib.profile.as_ref().map(|p| p.name.as_str());
    Some(
        Select::new("profile", &names)
            .current(current)
            .open(s.popup == Some(Popup::ProfileMenu))
            .on_toggle(|s: &mut App| toggle(s, Popup::ProfileMenu))
            .on_close(|s: &mut App| s.popup = None)
            .on_choose(|s: &mut App, name: &str, _| {
                s.popup = None;
                s.pick_profile(name);
            })
            .width(170.0)
            .build(&look.k, layer),
    )
}

fn deploy_main(look: &Look, blocked: bool, busy: bool) -> El {
    let ink = if blocked {
        look.muted()
    } else {
        look.on_accent
    };
    row()
        .key("deploy")
        .h(34.0)
        .pl(14.0)
        .pr(12.0)
        .gap(8.0)
        .center_y()
        .fill(if blocked { look.hover() } else { look.accent })
        .hover_fill(if blocked {
            look.pressed()
        } else {
            look.accent.mix(rune::Color::WHITE, 0.12)
        })
        .role("button")
        .label("Deploy")
        .help(if blocked {
            "Fix the blocking issues first"
        } else {
            "Link enabled mods into the game"
        })
        .on_press(move |s: &mut App| {
            if !blocked && !busy {
                s.deploy();
            }
        })
        .child(glyph(&icons::DEPLOY, 16.0, ink))
        .child(text("Deploy").font(ty::BODY_STRONG).color(ink))
}

fn deploy_split(look: &Look, blocked: bool, busy: bool) -> El {
    let ink = if blocked {
        look.muted()
    } else {
        look.on_accent
    };
    let more = row()
        .key("deploy-more")
        .h(34.0)
        .w(30.0)
        .center()
        .fill(if blocked {
            look.hover()
        } else {
            look.accent.mix(rune::Color::BLACK, 0.12)
        })
        .hover_fill(look.accent.mix(rune::Color::BLACK, 0.2))
        .role("button")
        .label("More deploy actions")
        .on_press(|s: &mut App| toggle(s, Popup::DeployMenu))
        .child(glyph(&icons::CHEVRON_DOWN, 14.0, ink));
    row()
        .key("deploy-split")
        .radius(10.0)
        .clip_rounded(10.0)
        .shadow(
            4.0,
            14.0,
            look.accent.alpha(if blocked { 0.0 } else { 0.35 }),
        )
        .child(deploy_main(look, blocked, busy))
        .child(el().w(1.0).h(34.0).fill(look.on_accent.alpha(0.2)))
        .child(more)
}

fn deploy_button(s: &App, look: &Look, layer: &mut Vec<El>) -> Option<El> {
    s.lib.profile.as_ref()?;
    let split = deploy_split(look, s.blockers().next().is_some(), s.busy > 0);
    Some(
        Menu::new("deploy-menu")
            .trigger(split)
            .placement(Placement::BottomEnd)
            .min_width(200.0)
            .item(
                MenuItem::new("Verify files")
                    .icon(&icons::SHIELD)
                    .on_select(|s: &mut App| s.verify()),
            )
            .item(
                MenuItem::new("Auto-sort plugins")
                    .icon(&icons::SPARKLES)
                    .on_select(|s: &mut App| {
                        s.popup = None;
                        s.auto_sort();
                    }),
            )
            .separator()
            .item(
                MenuItem::new("Purge deployment")
                    .icon(&icons::ERASE)
                    .danger(true)
                    .on_select(|s: &mut App| s.purge()),
            )
            .open(s.popup == Some(Popup::DeployMenu))
            .on_close(|s: &mut App| s.popup = None)
            .build(&look.k, layer),
    )
}

fn bell(s: &App, look: &Look, layer: &mut Vec<El>) -> El {
    let button = IconButton::new(&icons::BELL)
        .variant(IconButtonVariant::Ghost)
        .label("Notifications")
        .on_press(|s: &mut App| toggle(s, Popup::Notes))
        .build(&look.k);
    let trigger = el()
        .key("bell")
        .child(button)
        .children((s.unread > 0).then(|| {
            el().absolute(18.0, -2.0)
                .child(parts::count(look, s.unread.min(99), look.bad))
        }));
    Popover::new("notes")
        .trigger(trigger)
        .title("Notifications")
        .child(notes_list(s, look))
        .placement(Placement::BottomEnd)
        .open(s.popup == Some(Popup::Notes))
        .on_close(|s: &mut App| s.popup = None)
        .build(&look.k, layer)
}

fn notes_list(s: &App, look: &Look) -> El {
    if s.notes.is_empty() {
        return col()
            .w(340.0)
            .py(18.0)
            .center()
            .child(text("Nothing new").font(ty::BODY).color(look.muted()));
    }
    let rows = s.notes.iter().take(40).map(|n| {
        row()
            .key(format!("note-{}", n.id))
            .w_full()
            .py(8.0)
            .gap(10.0)
            .child(
                el().mt(5.0)
                    .size(7.0, 7.0)
                    .radius(4.0)
                    .fill(parts::tone_color(look, n.tone)),
            )
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child(
                        text(n.text.clone())
                            .font(ty::BODY)
                            .color(look.text())
                            .wrap(),
                    )
                    .child(text(ago(s.now - n.at)).font(ty::SMALL).color(look.faint())),
            )
    });
    col()
        .w(360.0)
        .max_h(420.0)
        .scroll(rune::ScrollSpec::default())
        .children(rows)
        .child(
            row().w_full().pt(6.0).justify_end().child(
                Button::new("Clear all")
                    .variant(ButtonVariant::GhostMuted)
                    .on_press(|s: &mut App| {
                        s.notes.clear();
                        s.unread = 0;
                    })
                    .build(&look.k),
            ),
        )
}

/// "just now", "3 min ago".
#[must_use]
pub fn ago(seconds: f64) -> String {
    if seconds < 45.0 {
        "Just now".to_owned()
    } else {
        format!("{:.0} min ago", (seconds / 60.0).max(1.0))
    }
}

/// Recent notes as toasts in the bottom right.
pub fn toasts(s: &App, look: &Look) -> El {
    let fresh = s
        .notes
        .iter()
        .filter(|n| s.now - n.at < TOAST_TTL)
        .take(3)
        .map(|n| {
            let tint = parts::tone_color(look, n.tone);
            let id = n.id;
            row()
                .key(format!("toast-{id}"))
                .w(360.0)
                .padding(14.0)
                .gap(12.0)
                .center_y()
                .radius(14.0)
                .fill(look.glass_strong)
                .border(1.0, look.edge)
                .backdrop_blur(24.0)
                .shadow(10.0, 30.0, look.shadow())
                .transition(rune::Transition::opacity().combined(rune::Transition::scale(0.96)))
                .child(el().size(4.0, 28.0).radius(2.0).fill(tint))
                .child(
                    text(n.text.clone())
                        .font(ty::BODY)
                        .color(look.text())
                        .wrap()
                        .grow(1.0),
                )
                .on_press(move |s: &mut App| s.notes.retain(|x| x.id != id))
        });
    col()
        .key("toasts")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .padding(22.0)
        .gap(10.0)
        .justify_end()
        .items_end()
        .children(fresh)
}

/// The full-window drop target shown while files hover.
pub fn drop_overlay(s: &App, look: &Look) -> Option<El> {
    if !s.drop_hover {
        return None;
    }
    Some(
        el().key("drop")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(18.0)
            .fill(look.base.alpha(0.55))
            .backdrop_blur(10.0)
            .child(
                col()
                    .w_full()
                    .h_full()
                    .radius(22.0)
                    .border(2.0, look.accent)
                    .border_dash(10.0, 8.0)
                    .fill(look.accent.alpha(0.06))
                    .center()
                    .gap(10.0)
                    .child(parts::icon_tile(&icons::DOWNLOAD, look.accent, 64.0))
                    .child(text("Drop to install").font(ty::TITLE).color(look.text()))
                    .child(
                        text("Archives and folders install into the current game")
                            .font(ty::BODY)
                            .color(look.muted()),
                    ),
            ),
    )
}
