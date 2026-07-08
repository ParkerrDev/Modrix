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
}

/// How a completed download's *install* phase ended (the download state
/// itself is [`DownloadStatus::state`]).
#[derive(Debug, Clone)]
pub enum InstallOutcome {
    /// Staged into the library.
    Installed {
        /// The (auto-detected) mod name.
        name: String,
        /// Whether a FOMOD installer configured it (defaults applied; the
        /// user can re-run the wizard).
        configurable: bool,
    },
    /// Downloaded fine, but no registered game matched the source page -
    /// the file stays in the download directory.
    NoGame,
    /// Staging failed (unsupported archive, extraction error, …).
    Failed(String),
}

/// Post-stage FOMOD pass shared by every install path: if the staged tree
/// carries an installer, apply its **default** selections (parking the
/// original layout for later re-configuration) and adopt its metadata.
/// Returns whether the mod is FOMOD-configurable.
///
/// # Errors
///
/// Returns an error if the installer parses but cannot be applied.
pub fn fomod_pass(engine: &Engine, staged: &modman_core::Mod) -> Result<bool> {
    use modman_plugin::fomod;
    let Some(installer) = fomod::parse(&staged.staged_path)? else {
        return Ok(false);
    };
    let selections = fomod::defaults(&installer);
    let ops = fomod::resolve(&installer, &selections);
    fomod::apply(&staged.staged_path, &ops).context("applying FOMOD defaults")?;
    // Adopt the installer's (usually cleaner) name - unless another mod of
    // this game already uses it (some archives ship stale metadata).
    let name = installer
        .info_name
        .filter(|n| !n.trim().is_empty())
        .filter(|n| {
            engine
                .mods(staged.game_id)
                .is_ok_and(|mods| !mods.iter().any(|m| m.id != staged.id && m.name == *n))
        });
    let version = installer.info_version.filter(|v| !v.trim().is_empty());
    engine.set_mod_meta(staged.id, name.as_deref(), version.as_deref())?;
    // `fomod-auto`: defaults applied, not yet reviewed - frontends use this
    // to offer the options wizard once. The wizard sets it to `fomod`.
    engine.set_install_state(staged.id, "fomod-auto")?;
    Ok(true)
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
    outcomes: Arc<Mutex<HashMap<DownloadId, InstallOutcome>>>,
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
            seq: Arc::new(AtomicU64::new(next_free_subdir(&download_dir))),
            download_dir,
            routes: Arc::new(Mutex::new(HashMap::new())),
            outcomes: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// How the install phase of a completed download ended, if it ran yet.
    #[must_use]
    pub fn install_outcome(&self, id: DownloadId) -> Option<InstallOutcome> {
        lock(&self.outcomes).ok()?.get(&id).cloned()
    }

    fn record(&self, id: DownloadId, outcome: InstallOutcome) {
        if let Ok(mut outcomes) = lock(&self.outcomes) {
            outcomes.insert(id, outcome);
        }
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
            "/downloads" => Reply::ok(self.list_json()),
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
        let id = self
            .manager
            .submit(request)
            .context("queuing the download")?;
        lock(&self.routes)?.insert(id, Route { game });
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
                Some(status) => Reply::ok(self.status_json(&status)),
                None => Reply::bad_request("no such download"),
            },
            None => Reply::bad_request("bad download id"),
        }
    }

    /// One download as JSON, including how its install phase ended.
    fn status_json(&self, status: &DownloadStatus) -> String {
        let install = match self.install_outcome(status.id) {
            None => "null".to_owned(),
            Some(InstallOutcome::Installed { name, configurable }) => format!(
                "{{\"state\":\"installed\",\"mod\":\"{}\",\"configurable\":{configurable}}}",
                json_escape(&name)
            ),
            Some(InstallOutcome::NoGame) => "{\"state\":\"no_game\"}".to_owned(),
            Some(InstallOutcome::Failed(error)) => {
                format!("{{\"state\":\"failed\",\"error\":\"{}\"}}", json_escape(&error))
            }
        };
        format!(
            "{{\"id\":{},\"state\":\"{}\",\"done\":{},\"total\":{},\"connections\":{},\"file\":\"{}\",\"install\":{install}}}",
            status.id.get(),
            state_str(status),
            status.done,
            status.total.unwrap_or(0),
            status.connections,
            json_escape(&status.file.file_name().unwrap_or_default().to_string_lossy()),
        )
    }

    fn list_json(&self) -> String {
        let items = self
            .manager
            .list()
            .iter()
            .map(|s| self.status_json(s))
            .collect::<Vec<_>>()
            .join(",");
        format!("[{items}]")
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
            self.record(id, InstallOutcome::NoGame);
            return;
        };
        let outcome = {
            let Ok(engine) = self.engine.lock() else {
                tracing::warn!("engine lock poisoned; cannot stage");
                self.record(id, InstallOutcome::Failed("engine lock poisoned".to_owned()));
                return;
            };
            install_file(&engine, game, file)
        };
        match outcome {
            Ok(outcome) => {
                tracing::info!(?outcome, "hand-off install finished");
                self.record(id, outcome);
            }
            Err(error) => {
                tracing::warn!(%error, "failed to stage downloaded mod");
                self.record(id, InstallOutcome::Failed(format!("{error:#}")));
            }
        }
    }
}

/// First download-subdirectory number not used by any previous session, so a
/// fresh session can never clobber an earlier session's downloaded file.
fn next_free_subdir(download_dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(download_dir) else {
        return 1;
    };
    let highest = entries
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    highest.saturating_add(1)
}

/// Extract a Nexus game domain from a page URL like
/// `https://www.nexusmods.com/<domain>/mods/123`.
fn nexus_domain_from_url(url: &str) -> Option<String> {
    let after = url.split_once("nexusmods.com/")?.1;
    let segment = after.split(['/', '?', '#']).next()?;
    (!segment.is_empty()).then(|| segment.to_owned())
}

/// Stage one archive into a game with automatic naming and the FOMOD pass -
/// the single install path used for hand-offs, local files, and reinstalls.
///
/// # Errors
///
/// Returns any staging or FOMOD-application error.
pub fn install_file(engine: &Engine, game: GameId, file: &Path) -> Result<InstallOutcome> {
    let staged = engine.stage_auto(game, file).context("staging")?;
    let configurable = fomod_pass(engine, &staged)?;
    // New installs start enabled (deploy applies them; disable to opt out).
    if let Ok(profile) = engine.active_profile(game) {
        let _ = engine.set_enabled(profile.id, staged.id, true);
    }
    let name = engine.get_mod(staged.id).map_or(staged.name, |m| m.name);
    Ok(InstallOutcome::Installed { name, configurable })
}

/// Minimal JSON string escaping (backslash, quote, control characters).
fn json_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
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
    fn install_file_reports_staging_failures() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = modman_core::Paths::rooted_at(dir.path());
        let engine = Engine::open(&paths).expect("open engine");
        let def = modman_core::GameDef::from_toml_str(
            "api_version = 1\nid = \"g\"\nname = \"G\"\nmod_root = \"Data\"\n",
            std::path::Path::new("<test>"),
        )
        .expect("def");
        let game = engine.add_game(&def, dir.path(), "manual").expect("game");
        let missing = dir.path().join("nope.zip");
        assert!(install_file(&engine, game.id, &missing).is_err());
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
    fn download_subdirs_continue_past_previous_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(next_free_subdir(dir.path()), 1);
        std::fs::create_dir(dir.path().join("1")).expect("mkdir");
        std::fs::create_dir(dir.path().join("7")).expect("mkdir");
        std::fs::create_dir(dir.path().join("not-a-number")).expect("mkdir");
        assert_eq!(next_free_subdir(dir.path()), 8);
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
