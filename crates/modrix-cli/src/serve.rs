// SPDX-License-Identifier: GPL-2.0-only
//! The `serve` command: the headless background service that turns a browser
//! click into an installed mod.
//!
//! All the machinery lives in [`modrix_service::Service`] - the exact same
//! service the GUI embeds - so a browser hand-off behaves identically whether
//! the GUI is open or only this headless process runs. This file just hosts it
//! on a runtime and prints how to reach it.

use std::io::Write;

use anyhow::{Context, Result, bail};
use modrix_core::Engine;
use modrix_service::{Binding, Service};

/// Simultaneously-active downloads.
const MAX_CONCURRENT: u8 = 4;

/// Run `serve` to completion (blocks, serving until killed).
pub fn run(engine: Engine, port: u16) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the async runtime")?;
    runtime.block_on(serve(engine, port))
}

async fn serve(engine: Engine, port: u16) -> Result<()> {
    let lockfile = engine.paths().instance_lock();
    let service = Service::new(engine, MAX_CONCURRENT)?;
    match service.bind(&lockfile, port)? {
        Binding::AlreadyRunning => bail!("another Modrix instance is already running"),
        Binding::Primary {
            port,
            token,
            serving,
        } => {
            announce(port, &token)?;
            serving
                .await
                .context("the loopback listener task failed")?
                .context("serving the loopback listener")
        }
    }
}

/// Print the port and token so the browser extension can be configured.
fn announce(port: u16, token: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "Modrix service listening on 127.0.0.1:{port}")?;
    writeln!(out, "session token: {token}")?;
    writeln!(
        out,
        "(configure the browser extension with this port and token)"
    )?;
    Ok(())
}
