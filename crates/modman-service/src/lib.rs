// SPDX-License-Identifier: GPL-2.0-only
//! The embedded hand-off service every frontend hosts.
//!
//! This is the machinery that turns a browser click into an installed mod,
//! shared by `modman serve` (headless) and the GUI (embedded): it accepts
//! **download hand-offs** from the browser extension over the loopback
//! listener (there is no Nexus API and no API key), routes each job to a game
//! by domain, downloads it (segmented, resumable) via [`DownloadManager`], and
//! stages the finished archive into that game.
//!
//! A [`Service`] is clone-cheap shared state (the engine, the manager, the
//! pending install routes). [`Service::bind`] makes this process the
//! single-instance primary and serves the listener in the background;
//! frontends keep the same `Service` to answer their own UI queries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use modman_core::{Engine, GameId};
use modman_download::{
    DownloadEvent, DownloadId, DownloadManager, DownloadState, DownloadStatus, HandoffJob,
};
use modman_ipc::{Message, Reply, Role, acquire};

/// Where a completed download should be installed.
struct Route {
    game: Option<GameId>,
    mod_name: String,
}

/// The outcome of [`Service::bind`].
pub enum Binding {
    /// We are the single instance: the loopback listener is being served in a
    /// background task, and the browser extension must present `token`.
    Primary {
        /// The bound loopback port.
        port: u16,
        /// The per-session token the extension authenticates with.
        token: String,
        /// The listener task; resolves only if the listener dies.
        serving: tokio::task::JoinHandle<modman_ipc::Result<()>>,
    },
    /// Another ModManager instance already holds the port; it is the one
    /// receiving browser hand-offs.
    AlreadyRunning,
}

/// Shared hand-off service state: engine + downloads + pending install routes.
#[derive(Clone)]
pub struct Service {
    engine: Arc<Mutex<Engine>>,
    manager: DownloadManager,
    download_dir: PathBuf,
    routes: Arc<Mutex<HashMap<DownloadId, Route>>>,
    seq: Arc<AtomicU64>,
}

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service")
            .field("download_dir", &self.download_dir)
            .finish_non_exhaustive()
    }
}

impl Service {
    /// Build a service around an open engine, downloading into the engine's
    /// cache. Allows `max_concurrent` simultaneously-active downloads.
    ///
    /// # Errors
    ///
    /// Returns an error if the download directory cannot be created or the
    /// download manager's HTTP client cannot be built.
    pub fn new(engine: Engine, max_concurrent: u8) -> Result<Self> {
        let download_dir = engine.paths().cache_dir().join("downloads");
        std::fs::create_dir_all(&download_dir)
            .with_context(|| format!("creating {}", download_dir.display()))?;
        Ok(Self {
            engine: Arc::new(Mutex::new(engine)),
            manager: DownloadManager::new(max_concurrent)
                .context("building the download manager")?,
            download_dir,
            routes: Arc::new(Mutex::new(HashMap::new())),
            seq: Arc::new(AtomicU64::new(1)),
        })
    }

    /// The shared engine handle.
    #[must_use]
    pub fn engine(&self) -> &Arc<Mutex<Engine>> {
        &self.engine
    }

    /// The download manager (for status queries, cancel, and event subscribing).
    #[must_use]
    pub fn manager(&self) -> &DownloadManager {
        &self.manager
    }

    /// Become the single instance and serve browser hand-offs in the
    /// background. Also starts the stager that installs completed downloads.
    ///
    /// Must be called within a tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the instance lock cannot be acquired (an I/O
    /// failure - a *taken* port is reported as [`Binding::AlreadyRunning`]).
    pub fn bind(&self, lockfile: &Path, port: u16) -> Result<Binding> {
        match acquire(lockfile, port).context("acquiring the instance lock")? {
            Role::Secondary(_) => Ok(Binding::AlreadyRunning),
            Role::Primary(primary) => {
                self.spawn_stager();
                let port = primary.port();
                let token = primary.token().to_owned();
                let service = self.clone();
                let serving = tokio::spawn(async move {
                    primary
                        .serve(move |msg| {
                            let service = service.clone();
                            async move { service.handle(&msg) }
                        })
                        .await
                });
                Ok(Binding::Primary {
                    port,
                    token,
                    serving,
                })
            }
        }
    }

    /// Dispatch one authenticated loopback request.
    #[must_use]
    pub fn handle(&self, message: &Message) -> Reply {
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
    /// routed game. Must be called within a tokio runtime.
    fn spawn_stager(&self) {
        let service = self.clone();
        tokio::spawn(async move {
            let mut events = service.manager.subscribe();
            loop {
                use tokio::sync::broadcast::error::RecvError;
                match events.recv().await {
                    Ok(DownloadEvent::Complete { id, file, .. }) => service.stage(id, &file),
                    Ok(DownloadEvent::Failed { id, error }) => {
                        let _ = lock(&service.routes).map(|mut r| r.remove(&id));
                        tracing::warn!(%error, "download failed");
                    }
                    Err(RecvError::Closed) => return,
                    // Under broadcast lag some events were dropped - recover by
                    // reconciling routed downloads against their live status.
                    Err(RecvError::Lagged(_)) => service.reconcile(),
                    Ok(_) => {}
                }
            }
        });
    }

    fn stage(&self, id: DownloadId, file: &Path) {
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
        .map_err(|_| anyhow!("service state lock poisoned"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_nexus_domain_from_a_mod_page_url() {
        assert_eq!(
            nexus_domain_from_url("https://www.nexusmods.com/skyrimspecialedition/mods/12604"),
            Some("skyrimspecialedition".to_owned())
        );
        assert_eq!(nexus_domain_from_url("https://example.com/whatever"), None);
        assert_eq!(nexus_domain_from_url("https://www.nexusmods.com/"), None);
    }

    #[test]
    fn derives_mod_names_by_stripping_archive_suffixes() {
        assert_eq!(mod_name_from("SkyUI_5_2_SE.zip"), "SkyUI_5_2_SE");
        assert_eq!(mod_name_from("mod.tar.gz"), "mod");
        assert_eq!(mod_name_from("plain-directory"), "plain-directory");
    }

    #[test]
    fn unknown_endpoints_and_nxm_are_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = modman_core::Paths::rooted_at(dir.path());
        let engine = Engine::open(&paths).expect("open engine");
        let service = Service::new(engine, 2).expect("service");
        let reply = service.handle(&Message {
            path: "/nope".to_owned(),
            body: String::new(),
        });
        assert_eq!(reply.status, 400);
        let reply = service.handle(&Message {
            path: "/nxm".to_owned(),
            body: "nxm://x/mods/1/files/2".to_owned(),
        });
        assert_eq!(reply.status, 400);
    }

    #[test]
    fn lists_downloads_as_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = modman_core::Paths::rooted_at(dir.path());
        let engine = Engine::open(&paths).expect("open engine");
        let service = Service::new(engine, 2).expect("service");
        let reply = service.handle(&Message {
            path: "/downloads".to_owned(),
            body: String::new(),
        });
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, "[]");
    }
}
