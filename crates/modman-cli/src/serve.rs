// SPDX-License-Identifier: GPL-2.0-only
//! The `serve` command: the headless background service that turns a browser
//! click into an installed mod.
//!
//! It binds the loopback IPC port (becoming the single instance) and accepts
//! **download hand-offs** from the browser extension - there is no Nexus API and
//! no API key. For each hand-off it validates the job, routes it to a game by
//! domain, downloads the file (segmented, resumable) via [`DownloadManager`], and
//! on completion stages it into that game. `modman-protocol` and the extension
//! are its clients.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use modman_core::{Engine, GameId};
use modman_download::{
    DownloadEvent, DownloadId, DownloadManager, DownloadState, DownloadStatus, HandoffJob,
};
use modman_ipc::{Message, Reply, Role, acquire};

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
    let download_dir = engine.paths().cache_dir().join("downloads");
    tokio::fs::create_dir_all(&download_dir)
        .await
        .with_context(|| format!("creating {}", download_dir.display()))?;

    let handler = Handler {
        engine: Arc::new(Mutex::new(engine)),
        manager: DownloadManager::new(MAX_CONCURRENT).context("building the download manager")?,
        download_dir,
        routes: Arc::new(Mutex::new(HashMap::new())),
        seq: Arc::new(AtomicU64::new(1)),
    };
    handler.spawn_stager();

    match acquire(&lockfile, port).context("acquiring the instance lock")? {
        Role::Secondary(_) => bail!("another ModManager instance is already running"),
        Role::Primary(primary) => {
            announce(primary.port(), primary.token())?;
            primary
                .serve(move |msg| {
                    let handler = handler.clone();
                    async move { handler.handle(&msg) }
                })
                .await
                .context("serving the loopback listener")
        }
    }
}

/// Where a completed download should be installed.
struct Route {
    game: Option<GameId>,
    mod_name: String,
}

/// Shared context for the loopback handler and the background stager.
#[derive(Clone)]
struct Handler {
    engine: Arc<Mutex<Engine>>,
    manager: DownloadManager,
    download_dir: PathBuf,
    routes: Arc<Mutex<HashMap<DownloadId, Route>>>,
    seq: Arc<AtomicU64>,
}

impl Handler {
    fn handle(&self, message: &Message) -> Reply {
        match message.path.as_str() {
            "/download" => self.enqueue(&message.body),
            "/downloads" => Reply::ok(list_json(&self.manager.list())),
            path if path.starts_with("/download/") => self.by_id(path),
            "/nxm" => Reply::bad_request(
                "nxm:// is no longer a download mechanism - use the browser extension",
            ),
            other => Reply::bad_request(format!("unknown endpoint {other}")),
        }
    }

    /// Validate and enqueue a hand-off job, recording where to install it.
    fn enqueue(&self, body: &str) -> Reply {
        match self.try_enqueue(body) {
            Ok(id) => Reply::ok(format!("{{\"accepted\":true,\"id\":{}}}", id.get())),
            Err(error) => {
                tracing::warn!(%error, "rejected download hand-off");
                Reply::bad_request(error.to_string())
            }
        }
    }

    fn try_enqueue(&self, body: &str) -> Result<DownloadId> {
        let job = HandoffJob::from_json(body).context("parsing the hand-off")?;
        let game = self.route(&job);
        // Each hand-off downloads into its own subdirectory, so two downloads
        // that share a filename never collide on .part / .mmdl / destination.
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let dir = self.download_dir.join(n.to_string());
        let request = job.into_request(&dir).context("validating the job")?;
        let mod_name = mod_name_from(&request.out);
        let id = self
            .manager
            .submit(request)
            .context("queuing the download")?;
        lock(&self.routes)?.insert(id, Route { game, mod_name });
        Ok(id)
    }

    /// Stage any completed routed downloads whose events we may have missed
    /// (e.g. after a broadcast lag). Idempotent - staging removes the route.
    fn reconcile(&self) {
        let ids: Vec<DownloadId> = match lock(&self.routes) {
            Ok(routes) => routes.keys().copied().collect(),
            Err(_) => return,
        };
        for id in ids {
            match self.manager.status(id) {
                Some(s) if s.state == DownloadState::Complete => self.stage(id, &s.file),
                Some(s) if s.state == DownloadState::Failed => {
                    let _ = lock(&self.routes).map(|mut r| r.remove(&id));
                }
                _ => {}
            }
        }
    }

    /// Decide which registered game this job installs into (domain → game); no
    /// API involved.
    fn route(&self, job: &HandoffJob) -> Option<GameId> {
        let domain = job
            .game_hint
            .as_ref()
            .and_then(|h| h.domain.clone())
            .or_else(|| job.page_url.as_deref().and_then(nexus_domain_from_url))?;
        let engine = self.engine.lock().ok()?;
        engine.game_by_nexus_domain(&domain).ok().map(|g| g.id)
    }

    fn by_id(&self, path: &str) -> Reply {
        let rest = path.strip_prefix("/download/").unwrap_or_default();
        if let Some(id_str) = rest.strip_suffix("/cancel") {
            return match id_str.parse::<u64>().ok().map(DownloadId::from_u64) {
                Some(id) => match self.manager.cancel(id) {
                    Ok(()) => Reply::ok("cancelled"),
                    Err(e) => Reply::bad_request(e.to_string()),
                },
                None => Reply::bad_request("bad download id"),
            };
        }
        match rest.parse::<u64>().ok().map(DownloadId::from_u64) {
            Some(id) => match self.manager.status(id) {
                Some(status) => Reply::ok(status_json(&status)),
                None => Reply::bad_request("no such download"),
            },
            None => Reply::bad_request("bad download id"),
        }
    }

    /// Subscribe to download events and stage each completed download into its
    /// routed game.
    fn spawn_stager(&self) {
        let handler = self.clone();
        tokio::spawn(async move {
            let mut events = handler.manager.subscribe();
            loop {
                use tokio::sync::broadcast::error::RecvError;
                match events.recv().await {
                    Ok(DownloadEvent::Complete { id, file, .. }) => handler.stage(id, &file),
                    Ok(DownloadEvent::Failed { id, error }) => {
                        let _ = lock(&handler.routes).map(|mut r| r.remove(&id));
                        tracing::warn!(%error, "download failed");
                    }
                    Err(RecvError::Closed) => return,
                    // Under broadcast lag some events were dropped - recover by
                    // reconciling routed downloads against their live status.
                    Err(RecvError::Lagged(_)) => handler.reconcile(),
                    Ok(_) => {}
                }
            }
        });
    }

    fn stage(&self, id: DownloadId, file: &std::path::Path) {
        let Some(route) = lock(&self.routes).ok().and_then(|mut r| r.remove(&id)) else {
            return;
        };
        let Some(game) = route.game else {
            tracing::warn!(
                file = %file.display(),
                "downloaded, but no game matched - left in the download dir"
            );
            return;
        };
        let staged = {
            let Ok(engine) = self.engine.lock() else {
                tracing::warn!("engine lock poisoned; cannot stage");
                return;
            };
            engine.stage(game, &route.mod_name, file)
        };
        match staged {
            Ok(m) => tracing::info!(mod = %m.name, "installed via extension hand-off"),
            Err(error) => tracing::warn!(%error, "failed to stage downloaded mod"),
        }
    }
}

/// Print the port and token so the browser extension can be configured.
fn announce(port: u16, token: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "ModManager service listening on 127.0.0.1:{port}")?;
    writeln!(out, "session token: {token}")?;
    writeln!(
        out,
        "(configure the browser extension with this port and token)"
    )?;
    Ok(())
}

/// Extract a Nexus game domain from a page URL like
/// `https://www.nexusmods.com/<domain>/mods/123`.
fn nexus_domain_from_url(url: &str) -> Option<String> {
    let after = url.split_once("nexusmods.com/")?.1;
    let segment = after.split(['/', '?', '#']).next()?;
    (!segment.is_empty()).then(|| segment.to_owned())
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

fn status_json(status: &DownloadStatus) -> String {
    format!(
        "{{\"id\":{},\"state\":\"{}\",\"done\":{},\"total\":{},\"connections\":{}}}",
        status.id.get(),
        state_str(status),
        status.done,
        status.total.unwrap_or(0),
        status.connections,
    )
}

fn list_json(statuses: &[DownloadStatus]) -> String {
    let items = statuses
        .iter()
        .map(status_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn state_str(status: &DownloadStatus) -> &'static str {
    use modman_download::DownloadState::{Active, Complete, Failed, Paused, Queued};
    match status.state {
        Queued => "queued",
        Active => "active",
        Paused => "paused",
        Complete => "complete",
        Failed => "failed",
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| anyhow!("serve state lock poisoned"))
}
