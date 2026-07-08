// SPDX-License-Identifier: GPL-2.0-only
//! ModManager graphical frontend - a classy, modern take on the Vortex layout.
//!
//! A thin Iced 0.13 face over [`modman_core::Engine`]. It embeds the same
//! [`modman_service::Service`] as `modman serve`, so browser download
//! hand-offs install mods while the window is open. All state lives in
//! [`app::App`]; all styling in [`theme`]; all layout in [`view`].

mod app;
mod fmt;
mod theme;
mod view;

fn main() -> iced::Result {
    init_tracing();
    iced::application("ModManager", app::update, view::view)
        .subscription(app::subscription)
        .theme(|state| state.theme.clone())
        .antialiasing(true)
        .window_size(iced::Size::new(1280.0, 840.0))
        .centered()
        .run_with(app::boot)
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
