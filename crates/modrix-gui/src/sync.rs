// SPDX-License-Identifier: GPL-2.0-only
//! Reading engine state (on the engine thread) and folding it, plus the
//! in-memory download list, into the UI state (on the UI thread).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use modrix_core::{Engine, GameId, ModId};
use modrix_download::DownloadState;
use modrix_service::InstallOutcome;

use crate::bus::{Event, Job, Work};
use crate::state::{App, Library, MAX_NOTES, NOTE_TTL, Note, Screen, SortKey, TOAST_TTL, Tone};

/// Seconds between idle refreshes.
const IDLE_PERIOD: f64 = 2.0;
/// Seconds between refreshes while something is running.
const LIVE_PERIOD: f64 = 0.15;

/// Read everything the UI shows for `game` (engine thread). Errors read as
/// empty sections: the next refresh tries again.
#[must_use]
pub fn gather(engine: &Engine, game: Option<GameId>, heavy: bool) -> Library {
    let games = engine.games().unwrap_or_default();
    let game = game
        .filter(|id| games.iter().any(|g| g.id == *id))
        .or_else(|| engine.active_game().ok().flatten().map(|g| g.id))
        .or_else(|| games.first().map(|g| g.id));
    let mut lib = Library {
        games,
        game,
        ..Library::default()
    };
    let Some(game) = game else {
        return lib;
    };
    lib.profiles = engine.profiles(game).unwrap_or_default();
    lib.profile = engine.active_profile(game).ok();
    lib.mods = engine.mods(game).unwrap_or_default();
    lib.capabilities = engine.capabilities(game).ok();
    if let Some(profile) = lib.profile.as_ref().map(|p| p.id) {
        lib.order = engine.enabled_mods(profile).unwrap_or_default();
        if heavy {
            lib.plugins = engine.plugins(profile).unwrap_or_default();
            lib.health = engine.health(profile).unwrap_or_default();
            lib.conflicts = engine.mod_conflicts(profile).unwrap_or_default();
        }
    }
    if heavy {
        lib.external = engine.external_mods(game).unwrap_or_default();
    }
    lib.enabled = lib.order.iter().map(|m| m.id).collect();
    lib
}

/// Plugins at the top level of each staged tree (bounded scan).
#[must_use]
pub fn plugin_counts(items: &[(ModId, PathBuf)]) -> HashMap<ModId, usize> {
    items
        .iter()
        .take(10_000)
        .map(|(id, path)| (*id, staged_plugin_count(path)))
        .collect()
}

fn staged_plugin_count(staged: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(staged) else {
        return 0;
    };
    entries
        .flatten()
        .take(512)
        .filter(|e| {
            modrix_core::esp::is_plugin_name(&e.file_name().to_string_lossy()) && e.path().is_file()
        })
        .count()
}

impl App {
    /// Queue engine work; returns false before boot.
    pub fn queue(&mut self, job: Job) -> bool {
        let Some(jobs) = &self.jobs else {
            return false;
        };
        self.job_seq = self.job_seq.saturating_add(1);
        jobs.send((self.job_seq, job)).is_ok()
    }

    /// Run an engine call on the engine thread, then refresh.
    pub fn run(&mut self, slow: bool, work: Work) {
        let game = self.want_game.or(self.lib.game);
        if self.queue(Job::Run { slow, work, game }) && slow {
            self.busy = self.busy.saturating_add(1);
        }
    }

    /// Ask for fresh engine state.
    pub fn request_refresh(&mut self, heavy: bool) {
        if self.refresh_queued && !heavy {
            return;
        }
        let game = self.want_game.or(self.lib.game);
        if self.queue(Job::Refresh { game, heavy }) {
            self.refresh_queued = true;
            self.last_refresh = self.now;
        }
    }

    /// Record a notification.
    pub fn note(&mut self, tone: Tone, text: impl Into<String>) {
        self.note_seq = self.note_seq.saturating_add(1);
        self.notes.insert(
            0,
            Note {
                id: self.note_seq,
                tone,
                text: text.into(),
                at: self.now,
            },
        );
        self.notes.truncate(MAX_NOTES);
        if self.popup != Some(crate::state::Popup::Notes) {
            self.unread = self.unread.saturating_add(1).min(self.notes.len());
        }
    }

    /// The per-frame poll: drain events, tick the live sections, schedule
    /// the next wake. Returns whether state changed.
    pub fn poll(&mut self, now: f64, tx: &mut rune::Tx) -> bool {
        self.now = now;
        let mut changed = false;
        for _ in 0..512 {
            let Ok(event) = self.inbox.try_recv() else {
                break;
            };
            self.handle(event, tx);
            changed = true;
        }
        if !self.booted {
            let op = self.progress.snapshot();
            changed |= op.is_some() != self.op.is_some();
            self.op = op;
            tx.wake_after(std::time::Duration::from_millis(150));
            return changed;
        }
        let live = self.busy > 0 || self.op.is_some() || self.active_downloads() > 0;
        let period = if live { LIVE_PERIOD } else { IDLE_PERIOD };
        if now - self.last_refresh >= period {
            changed |= self.tick(now);
        }
        tx.wake_after(std::time::Duration::from_secs_f64(period));
        changed
    }

    /// Re-read the live sections. Returns whether anything on screen moved,
    /// so an idle window draws nothing between snapshots.
    fn tick(&mut self, now: f64) -> bool {
        let before = self.last_refresh;
        self.last_refresh = now;
        let op = self.progress.snapshot();
        let mut changed = op != self.op;
        self.op = op;
        changed |= self.read_downloads();
        // A toast that timed out since the last tick has to leave the screen.
        changed |= self
            .notes
            .iter()
            .any(|n| before - n.at < TOAST_TTL && now - n.at >= TOAST_TTL);
        let notes = self.notes.len();
        self.notes.retain(|n| now - n.at < NOTE_TTL);
        changed |= notes != self.notes.len();
        self.unread = self.unread.min(self.notes.len());
        // The light read is SQLite-only; skip it while a slow action holds
        // the engine (its own refresh follows it).
        if self.busy == 0 && !self.refresh_queued {
            self.request_refresh(false);
        }
        changed
    }

    /// Pull the download list and install outcomes (in-memory, cheap).
    /// Returns whether the list or the notes changed.
    fn read_downloads(&mut self) -> bool {
        let Some(service) = &self.service else {
            return false;
        };
        let mut downloads = service.manager().list();
        downloads.sort_by_key(|d| std::cmp::Reverse(d.id));
        self.outcomes = downloads
            .iter()
            .filter(|d| d.state == DownloadState::Complete)
            .filter_map(|d| service.install_outcome(d.id).map(|o| (d.id, o)))
            .collect();
        let notes = self.notes.len();
        let changed = downloads != self.downloads;
        self.downloads = downloads;
        self.notify_new_outcomes();
        changed || notes != self.notes.len()
    }

    fn notify_new_outcomes(&mut self) {
        let mut fresh: Vec<(modrix_download::DownloadId, InstallOutcome)> = self
            .outcomes
            .iter()
            .filter(|(id, _)| !self.outcome_seen.contains(*id))
            .map(|(id, o)| (*id, o.clone()))
            .collect();
        fresh.sort_by_key(|(id, _)| *id);
        for (id, outcome) in fresh {
            self.outcome_seen.insert(id);
            match outcome {
                InstallOutcome::Installed { name, configurable } => {
                    let text = if configurable {
                        format!("{name} installed, options set to defaults")
                    } else {
                        format!("{name} installed")
                    };
                    self.note(Tone::Ok, text);
                    self.request_refresh(true);
                }
                InstallOutcome::Failed(error) => {
                    self.note(Tone::Error, format!("Install failed: {error}"));
                }
                InstallOutcome::NoGame => self.note(Tone::Info, "Download kept: no game matched"),
            }
        }
    }

    /// Fold a snapshot in. Stale ones (gathered before a job the UI already
    /// queued, or for a game the user has switched away from) are dropped so
    /// optimistic edits never flicker back.
    pub fn apply_snapshot(&mut self, seq: u64, lib: Library, heavy: bool) {
        self.refresh_queued = false;
        if seq < self.job_seq {
            return;
        }
        if self.want_game.is_some_and(|want| lib.game != Some(want))
            && lib.games.iter().any(|g| Some(g.id) == self.want_game)
        {
            return;
        }
        let game_changed = lib.game != self.lib.game;
        let old = std::mem::replace(&mut self.lib, lib);
        self.lib_gen = self.lib_gen.wrapping_add(1);
        if !heavy && !game_changed {
            self.lib.plugins = old.plugins;
            self.lib.health = old.health;
            self.lib.conflicts = old.conflicts;
            self.lib.external = old.external;
        }
        self.lib.plugin_counts = if game_changed {
            HashMap::new()
        } else {
            old.plugin_counts
        };
        self.want_game = None;
        self.sort_mods();
        self.mod_sel.retain_below(self.visible_mods().len());
        self.plugin_sel.retain_below(self.lib.plugins.len());
        if !self.screen_visible(self.screen) {
            self.screen = Screen::Dashboard;
        }
        self.request_art();
        self.queue_unreviewed_fomods();
        if game_changed && !heavy {
            self.request_refresh(true);
        }
    }

    /// Sort `lib.mods` by the chosen column (stable, name tiebreak).
    pub fn sort_mods(&mut self) {
        let (key, ascending) = self.mod_sort;
        self.lib_gen = self.lib_gen.wrapping_add(1);
        let enabled = &self.lib.enabled;
        let names = |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase());
        self.lib.mods.sort_by(|a, b| {
            let ord = match key {
                SortKey::Installed => (a.created_at.unwrap_or(i64::MIN), a.id)
                    .cmp(&(b.created_at.unwrap_or(i64::MIN), b.id)),
                SortKey::Name => names(&a.name, &b.name),
                SortKey::Enabled => enabled
                    .contains(&b.id)
                    .cmp(&enabled.contains(&a.id))
                    .then_with(|| names(&a.name, &b.name)),
                SortKey::Version => a
                    .version
                    .cmp(&b.version)
                    .then_with(|| names(&a.name, &b.name)),
                SortKey::Source => a
                    .source
                    .cmp(&b.source)
                    .then_with(|| names(&a.name, &b.name)),
            };
            if ascending { ord } else { ord.reverse() }
        });
    }

    /// Offer the options wizard once for each FOMOD mod installed with
    /// defaults applied automatically.
    fn queue_unreviewed_fomods(&mut self) {
        let pending: Vec<ModId> = self
            .lib
            .mods
            .iter()
            .filter(|m| m.install_state == "fomod-auto" && !self.prompted.contains(&m.id))
            .map(|m| m.id)
            .collect();
        for id in pending {
            self.prompted.insert(id);
            self.wizard_queue.push(id);
        }
        self.open_next_wizard();
    }

    /// Start art resolution for games that have none yet.
    fn request_art(&mut self) {
        let (Some(rt), Some(paths)) = (&self.rt, &self.paths) else {
            return;
        };
        let cache = paths.cache_dir().to_path_buf();
        for game in &self.lib.games {
            let Some(appid) = game.steam_appid else {
                continue;
            };
            if !self.art_requested.insert(game.id) {
                continue;
            }
            let (id, bus, cache) = (game.id, self.bus.clone(), cache.clone());
            rt.spawn(async move {
                let art = crate::art::resolve(appid, cache).await;
                bus.send(Event::Art(id, Box::new(art)));
            });
        }
    }
}
