// SPDX-License-Identifier: GPL-2.0-only
//! Modrix graphical frontend - a classy, modern take on the Vortex layout.
//!
//! A thin Iced 0.13 face over [`modrix_core::Engine`]. It embeds the same
//! [`modrix_service::Service`] as `modrix serve`, so browser download
//! hand-offs install mods while the window is open. All state lives in
//! [`app::App`]; all styling in [`theme`]; all layout in [`view`].

mod app;
mod artwork;
mod fmt;
mod icons;
mod theme;
mod view;

fn main() -> iced::Result {
    init_tracing();
    // Single-instance guard: if a primary is already serving the loopback port,
    // do not open a second window. This is checked here, before the window is
    // created, so a duplicate launch exits cleanly instead of running in a
    // degraded "another instance active" mode. A crashed primary leaves a stale
    // lockfile with nothing listening, which reads as not-running, so recovery
    // launches through normally.
    if another_instance_running() {
        tracing::info!("Modrix is already running; not opening a second window.");
        return Ok(());
    }
    iced::application("Modrix", app::update, view::view)
        // Explicit app id: the Wayland window class the desktop entry's
        // StartupWMClass matches against.
        .settings(iced::Settings {
            id: Some("modrix-gui".to_owned()),
            ..iced::Settings::default()
        })
        .subscription(app::subscription)
        .theme(|state| state.theme.clone())
        // The window clear color comes from the active theme: glass themes
        // use alpha < 1 so the compositor shows through (transparent(true)
        // below makes that possible; opaque themes simply ignore it).
        .style(|_state, _theme| iced::application::Appearance {
            background_color: theme::window_background(),
            text_color: theme::text(),
        })
        .transparent(true)
        .antialiasing(true)
        .window_size(iced::Size::new(1280.0, 840.0))
        .centered()
        .run_with(app::boot)
}

/// Whether a live primary Modrix instance already holds the loopback port.
/// A failure to resolve the data directory reads as "not running" so the real
/// error surfaces during boot rather than silently blocking launch.
fn another_instance_running() -> bool {
    modrix_core::Paths::resolve()
        .is_ok_and(|paths| modrix_ipc::primary_is_live(&paths.instance_lock()))
}

/// Route `RUST_LOG`-filtered tracing to stderr for terminal launches.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
