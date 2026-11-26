// SPDX-License-Identifier: GPL-2.0-only
//! Modrix's graphical frontend: a native GPU UI built with Rune over
//! [`modrix_core::Engine`]. It embeds the same [`modrix_service::Service`] as
//! `modrix serve`, so browser hand-offs install mods while the window is open.
//!
//! The engine runs on its own thread ([`bus`]); the view only reads the
//! state in [`state::App`], so frames never wait on the disk.

mod actions;
mod art;
mod bake;
mod boot;
mod bus;
mod demo;
mod events;
mod fmt;
mod icons;
mod keys;
mod look;
mod prefs;
mod state;
mod store;
mod sync;
mod ui;
mod wizard;

use rune::{App, AppSpec};

use crate::state::App as State;

/// The window's opening size.
const SIZE: (f32, f32) = (1320.0, 860.0);

fn main() -> anyhow::Result<()> {
    init_tracing();
    // One window per user: a live primary already serving the loopback port
    // means a second launch exits. A crashed primary leaves a stale lock with
    // nothing listening, which reads as not running.
    if !rune::is_headless() && another_instance_running() {
        tracing::info!("Modrix is already running; not opening a second window.");
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("modrix-rt")
        .build()?;
    let handle = runtime.handle().clone();
    rune::set_app_id("modrix-gui");
    let spec = AppSpec::<State> {
        min_size: Some((860.0, 560.0)),
        scenes: demo::SCENES,
        setup: Some(setup),
        poll: Some(state::App::poll),
        on_key: Some(keys::on_key),
        ..AppSpec::new("Modrix", SIZE, || State::new(None), ui::view)
    };
    App::new(spec).init(move || State::new(Some(handle))).run();
    drop(runtime);
    Ok(())
}

/// Attach the waker, then boot the service unless this is a headless snap.
fn setup(s: &mut State, waker: rune::Waker) {
    s.bus.set_waker(waker);
    if rune::is_headless() {
        return;
    }
    if let Some(rt) = s.rt.clone() {
        boot::start(rt, std::sync::Arc::clone(&s.progress), s.bus.clone());
    }
}

/// Whether a live primary Modrix instance already holds the loopback port.
fn another_instance_running() -> bool {
    modrix_core::Paths::resolve()
        .is_ok_and(|paths| modrix_ipc::primary_is_live(&paths.instance_lock()))
}

/// `RUST_LOG`-filtered tracing to stderr.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
