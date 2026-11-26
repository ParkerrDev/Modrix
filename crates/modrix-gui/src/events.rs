// SPDX-License-Identifier: GPL-2.0-only
//! Folding background events into the UI state.

use modrix_core::GameId;

use crate::bus::{Event, Reply};
use crate::state::{App, Tone, UpdateState};

impl App {
    /// Route one background event.
    pub fn handle(&mut self, event: Event, tx: &mut rune::Tx) {
        match event {
            Event::Booted(result) => self.on_booted(result),
            Event::Snapshot { seq, lib, heavy } => self.apply_snapshot(seq, *lib, heavy),
            Event::Done { slow, reply } => {
                if slow {
                    self.busy = self.busy.saturating_sub(1);
                }
                self.on_reply(reply);
            }
            Event::PluginCounts(counts) => self.lib.plugin_counts = counts,
            Event::Hashed(items) => self.on_hashed(items),
            Event::Registry(result) => {
                self.registry_loading = false;
                match result {
                    Ok(choices) => self.registry = choices,
                    Err(error) => tracing::debug!(%error, "registry index unavailable"),
                }
            }
            Event::SupportInstalled(result) => self.on_support_installed(result),
            Event::UpdateChecked(result) => self.on_update_checked(result),
            Event::UpdateApplied(result) => self.on_update_applied(result, tx),
            Event::StoreCatalog(game, result) => self.on_store_catalog(game, result),
            Event::StoreQueued(id, result) => self.on_store_queued(&id, result),
            Event::Art(game, data) => {
                self.art.insert(game, crate::art::ArtSet::from_data(*data));
            }
        }
    }

    fn on_booted(&mut self, result: Result<Box<crate::boot::Booted>, String>) {
        let booted = match result {
            Ok(booted) => *booted,
            Err(error) => {
                self.boot_error = Some(error);
                return;
            }
        };
        self.already_running = booted.link.is_none();
        self.link = booted.link;
        self.defs = booted.defs;
        self.detected = booted.detected;
        self.paths = Some(booted.paths);
        self.jobs = crate::bus::spawn_engine_thread(
            std::sync::Arc::clone(booted.service.engine()),
            self.bus.clone(),
        );
        self.service = Some(booted.service);
        self.booted = true;
        self.refresh_installed_plugins();
        self.request_refresh(true);
        self.fetch_registry();
        self.check_updates();
    }

    fn on_reply(&mut self, reply: Result<Reply, String>) {
        match reply {
            Ok(Reply::Quiet) => {}
            Ok(Reply::Note(tone, text)) => self.note(tone, text),
            Ok(Reply::Duplicate(prompt)) => self.dup_queue.push(prompt),
            Ok(Reply::Wizard(wizard)) => {
                if self.wizard.is_none() {
                    self.wizard = Some(*wizard);
                }
            }
            Ok(Reply::SelectGame(game, text)) => {
                self.note(Tone::Ok, text);
                self.switch_game(game);
            }
            Err(error) => self.note(Tone::Error, error),
        }
    }

    /// Manage another game: drop per-game UI state and ask for its data.
    pub fn switch_game(&mut self, game: GameId) {
        self.want_game = Some(game);
        self.mod_sel.clear();
        self.plugin_sel.clear();
        self.expanded_conflict = None;
        self.popup = None;
        self.run(
            false,
            Box::new(move |e| {
                e.set_active_game(game)
                    .map(|()| Reply::Quiet)
                    .map_err(|e| e.to_string())
            }),
        );
    }

    fn on_support_installed(&mut self, result: Result<String, String>) {
        self.busy = self.busy.saturating_sub(1);
        let id = match result {
            Ok(id) => id,
            Err(error) => return self.note(Tone::Error, format!("Plugin install failed: {error}")),
        };
        self.note(Tone::Ok, format!("{id} support installed"));
        self.rediscover();
        let found = self
            .detected
            .iter()
            .position(|d| d.def.id == id)
            .filter(|_| !self.lib.games.iter().any(|g| g.plugin_id == id));
        if let Some(index) = found {
            self.add_detected(index);
        }
    }

    fn on_update_checked(&mut self, result: Result<Option<modrix_update::UpdateInfo>, String>) {
        match result {
            Ok(Some(info)) => {
                let version = info.version.clone();
                self.update = UpdateState::Available(info);
                self.note(Tone::Info, format!("Modrix {version} is available"));
            }
            Ok(None) => self.update = UpdateState::Idle,
            Err(error) => {
                tracing::debug!(%error, "update check failed");
                self.update = UpdateState::Idle;
            }
        }
    }

    fn on_update_applied(&mut self, result: Result<(), String>, tx: &mut rune::Tx) {
        match result {
            Ok(()) => tx.quit(),
            Err(error) => {
                if let UpdateState::Installing(info) = &self.update {
                    self.update = UpdateState::Available(info.clone());
                }
                self.note(Tone::Error, format!("Update failed: {error}"));
            }
        }
    }
}
