// SPDX-License-Identifier: GPL-2.0-only
//! The `serve` command: run the headless primary instance that turns a browser
//! click into an installed mod.
//!
//! It binds the loopback IPC port (becoming the single instance), then for each
//! forwarded `nxm://` link it resolves the CDN URL via Nexus, downloads the file
//! (resumable, checksummed), and stages it into the game the link's domain maps
//! to - all with no window open. `modman-protocol` and the browser userscript
//! are its two clients.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use modman_core::Engine;
use modman_ipc::{Message, Reply, Role, acquire};
use modman_nexus::{NexusClient, NxmUri};

/// Run `serve` to completion (blocks, serving until killed).
pub fn run(engine: Engine, api_key: String, port: u16, api_base: Option<String>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the async runtime")?;
    runtime.block_on(serve(engine, api_key, port, api_base))
}

async fn serve(engine: Engine, api_key: String, port: u16, api_base: Option<String>) -> Result<()> {
    let lockfile = engine.paths().instance_lock();
    let downloads = engine.paths().cache_dir().join("downloads");
    tokio::fs::create_dir_all(&downloads)
        .await
        .with_context(|| format!("creating {}", downloads.display()))?;
    let nexus = match api_base {
        Some(base) => NexusClient::with_base(api_key, base),
        None => NexusClient::new(api_key),
    }
    .context("building the Nexus client")?;

    match acquire(&lockfile, port).context("acquiring the instance lock")? {
        Role::Secondary(_) => {
            bail!("another ModManager instance is already running");
        }
        Role::Primary(primary) => {
            announce(primary.port(), primary.token())?;
            let handler = Handler {
                engine: Arc::new(Mutex::new(engine)),
                nexus,
                downloads,
            };
            primary
                .serve(move |msg| {
                    let handler = handler.clone();
                    async move { handler.handle(msg).await }
                })
                .await
                .context("serving the loopback listener")
        }
    }
}

/// Print the port and token so the browser userscript can be configured.
fn announce(port: u16, token: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "ModManager service listening on 127.0.0.1:{port}")?;
    writeln!(out, "session token: {token}")?;
    writeln!(out, "(configure the userscript with this port and token)")?;
    Ok(())
}

/// Shared context for handling forwarded links.
#[derive(Clone)]
struct Handler {
    engine: Arc<Mutex<Engine>>,
    nexus: NexusClient,
    downloads: PathBuf,
}

impl Handler {
    async fn handle(&self, message: Message) -> Reply {
        if message.path != "/nxm" {
            return Reply::bad_request(format!("unknown endpoint {}", message.path));
        }
        match self.install(message.body.trim()).await {
            Ok(summary) => Reply::ok(summary),
            Err(error) => {
                tracing::warn!(%error, "failed to handle nxm link");
                Reply::error(error.to_string())
            }
        }
    }

    /// Resolve → download → stage. Returns a human summary on success.
    async fn install(&self, uri_str: &str) -> Result<String> {
        let uri = NxmUri::parse(uri_str).context("parsing the nxm link")?;
        // Route to a game by domain (a quick, lock-held query - no awaits).
        let (game_id, game_name) = {
            let engine = self
                .engine
                .lock()
                .map_err(|_| anyhow!("engine lock poisoned"))?;
            let game = engine
                .game_by_nexus_domain(&uri.domain)
                .with_context(|| format!("no game registered for `{}`", uri.domain))?;
            (game.id, game.name)
        };

        let target = self
            .nexus
            .resolve(&uri)
            .await
            .context("resolving the download")?;
        let dest = self.downloads.join(&target.file_name);
        self.nexus
            .download(&target, &dest, None, |_progress| {})
            .await
            .context("downloading the file")?;

        let mod_name = mod_name_from(&target.file_name);
        let staged = {
            let engine = self
                .engine
                .lock()
                .map_err(|_| anyhow!("engine lock poisoned"))?;
            engine
                .stage(game_id, &mod_name, &dest)
                .context("staging the mod")?
        };
        tracing::info!(mod = %staged.name, game = %game_name, "installed via nxm");
        Ok(format!("installed `{}` into {game_name}", staged.name))
    }
}

/// Derive a mod name from a downloaded file name (strip a trailing archive
/// extension).
fn mod_name_from(file_name: &str) -> String {
    for ext in [".zip", ".7z", ".rar", ".tar.gz"] {
        if let Some(stem) = file_name.strip_suffix(ext) {
            return stem.to_owned();
        }
    }
    file_name.to_owned()
}
