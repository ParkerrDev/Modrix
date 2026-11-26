// SPDX-License-Identifier: GPL-2.0-only
//! What the user can do. Each action updates the UI optimistically where it
//! is safe to, then queues the real engine call on the engine thread.

use std::path::{Path, PathBuf};

use modrix_core::{Engine, GameDef, GameId, ModId, ProfileId};
use modrix_service::InstallOutcome;

use crate::bus::{Event, Reply};
use crate::state::{App, DupPrompt, Pane, SortKey, Tone};

/// Engine results as display strings.
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl App {
    /// The active profile id, if any.
    #[must_use]
    pub fn profile_id(&self) -> Option<ProfileId> {
        self.lib.profile.as_ref().map(|p| p.id)
    }

    /// Switch the active profile.
    pub fn pick_profile(&mut self, name: &str) {
        let Some(profile) = self.lib.profiles.iter().find(|p| p.name == name).cloned() else {
            return;
        };
        self.lib.profile = Some(profile.clone());
        self.run(
            false,
            Box::new(move |e| {
                e.set_active_profile(profile.id)
                    .map(|()| Reply::Quiet)
                    .map_err(err)
            }),
        );
    }

    /// Create a profile named by the form field.
    pub fn create_profile(&mut self) {
        let name = self.form.profile_name.text.trim().to_owned();
        let Some(game) = self.lib.game else {
            return;
        };
        if name.is_empty() {
            return;
        }
        self.form.profile_name = rune::TextEdit::default();
        self.run(
            false,
            Box::new(move |e| {
                let profile = e.create_profile(game, &name).map_err(err)?;
                Ok(Reply::Note(
                    Tone::Ok,
                    format!("Profile {} created", profile.name),
                ))
            }),
        );
    }

    /// Enable or disable one mod.
    pub fn set_mod_enabled(&mut self, id: ModId, on: bool) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        if on {
            self.lib.enabled.insert(id);
        } else {
            self.lib.enabled.remove(&id);
        }
        self.run(
            false,
            Box::new(move |e| {
                e.set_enabled(profile, id, on)
                    .map(|()| Reply::Quiet)
                    .map_err(err)
            }),
        );
    }

    /// Enable or disable a set of mods with one summary note.
    pub fn set_many_enabled(&mut self, ids: Vec<ModId>, on: bool) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        for id in &ids {
            if on {
                self.lib.enabled.insert(*id);
            } else {
                self.lib.enabled.remove(id);
            }
        }
        let count = ids.len();
        self.run(
            false,
            Box::new(move |e| {
                for id in ids {
                    e.set_enabled(profile, id, on).map_err(err)?;
                }
                let verb = if on { "enabled" } else { "disabled" };
                Ok(Reply::Note(Tone::Ok, format!("{count} mods {verb}")))
            }),
        );
    }

    /// Enable every disabled mod.
    pub fn enable_all(&mut self) {
        let ids: Vec<ModId> = self
            .lib
            .mods
            .iter()
            .filter(|m| !self.lib.enabled.contains(&m.id))
            .map(|m| m.id)
            .collect();
        self.set_many_enabled(ids, true);
    }

    /// Delete the selected mods.
    pub fn delete_selected(&mut self) {
        let ids = self.selected_mod_ids();
        let count = ids.len();
        self.confirm_delete = false;
        self.mod_sel.clear();
        self.lib.mods.retain(|m| !ids.contains(&m.id));
        self.run(
            false,
            Box::new(move |e| {
                for id in ids {
                    e.delete_mod(id).map_err(err)?;
                }
                Ok(Reply::Note(Tone::Ok, format!("{count} mods deleted")))
            }),
        );
    }

    /// Reinstall the selected mods from their archives.
    pub fn reinstall_selected(&mut self) {
        let ids = self.selected_mod_ids();
        if ids.is_empty() {
            return;
        }
        self.mod_sel.clear();
        self.run(true, Box::new(move |e| run_reinstall_many(e, &ids)));
    }

    /// Deploy the active profile.
    pub fn deploy(&mut self) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        self.popup = None;
        self.run(
            true,
            Box::new(move |e| {
                let r = e.deploy(profile).map_err(err)?;
                Ok(Reply::Note(
                    Tone::Ok,
                    format!(
                        "Deployed: {} added, {} removed, {} unchanged",
                        r.added(),
                        r.removed(),
                        r.unchanged()
                    ),
                ))
            }),
        );
    }

    /// Undeploy (purge) the active profile.
    pub fn purge(&mut self) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        self.popup = None;
        self.run(
            true,
            Box::new(move |e| {
                let r = e.undeploy(profile).map_err(err)?;
                Ok(Reply::Note(
                    Tone::Ok,
                    format!("Purged: {} files restored", r.removed()),
                ))
            }),
        );
    }

    /// Verify the deployment.
    pub fn verify(&mut self) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        self.popup = None;
        self.run(
            true,
            Box::new(move |e| {
                let r = e.verify(profile).map_err(err)?;
                Ok(if r.is_clean() {
                    Reply::Note(Tone::Ok, format!("Verified: {} files healthy", r.checked()))
                } else {
                    Reply::Note(
                        Tone::Error,
                        format!(
                            "Verify: {} of {} files changed",
                            r.issues().len(),
                            r.checked()
                        ),
                    )
                })
            }),
        );
    }

    /// LOOT-style plugin sort.
    pub fn auto_sort(&mut self) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        self.run(
            true,
            Box::new(move |e| {
                let plugins = e.auto_sort_plugins(profile).map_err(err)?;
                Ok(Reply::Note(
                    Tone::Ok,
                    format!("Load order sorted: {} plugins", plugins.len()),
                ))
            }),
        );
    }

    /// Flip a plugin's activation.
    pub fn toggle_plugin(&mut self, index: usize) {
        if let Some(plugin) = self.lib.plugins.get_mut(index) {
            plugin.enabled = !plugin.enabled;
            self.commit_plugin_order();
        }
    }

    /// Persist the current plugin order.
    pub fn commit_plugin_order(&mut self) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        let order: Vec<(String, bool)> = self
            .lib
            .plugins
            .iter()
            .map(|p| (p.name.clone(), p.enabled))
            .collect();
        self.run(
            false,
            Box::new(move |e| {
                e.set_plugin_order(profile, &order)
                    .map(|()| Reply::Quiet)
                    .map_err(err)
            }),
        );
    }

    /// Move the selected plugins one step (`delta` is -1 or +1).
    pub fn move_plugins(&mut self, delta: i8) {
        let len = self.lib.plugins.len();
        let mut indices: Vec<usize> = self.plugin_sel.items.iter().copied().collect();
        if indices.is_empty() || len == 0 {
            return;
        }
        if delta > 0 {
            indices.sort_unstable_by(|a, b| b.cmp(a));
        } else {
            indices.sort_unstable();
        }
        let last = len.saturating_sub(1);
        if indices
            .iter()
            .any(|i| (delta < 0 && *i == 0) || (delta > 0 && *i >= last))
        {
            return;
        }
        let mut moved = std::collections::BTreeSet::new();
        for i in indices {
            let target = if delta < 0 {
                i.saturating_sub(1)
            } else {
                i.saturating_add(1)
            };
            self.lib.plugins.swap(i, target);
            moved.insert(target);
        }
        self.plugin_sel.cursor = self.plugin_sel.cursor.map(|c| step(c, delta, len));
        self.plugin_sel.items = moved;
        self.commit_plugin_order();
    }

    /// Move one plugin from `from` to `to` (drag and drop).
    pub fn move_plugin_to(&mut self, from: usize, to: usize) {
        let len = self.lib.plugins.len();
        if from == to || from >= len || to >= len {
            return;
        }
        let moved = self.lib.plugins.remove(from);
        self.lib.plugins.insert(to, moved);
        self.plugin_sel.only(to);
        self.commit_plugin_order();
    }

    /// `winner`'s files override `loser`'s.
    pub fn set_rule(&mut self, loser: ModId, winner: ModId) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        self.run(
            false,
            Box::new(move |e| {
                e.set_mod_rule(profile, loser, winner)
                    .map(|()| Reply::Quiet)
                    .map_err(err)
            }),
        );
    }

    /// Remove the rule between a pair.
    pub fn clear_rule(&mut self, a: ModId, b: ModId) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        self.run(
            false,
            Box::new(move |e| {
                e.clear_mod_rule(profile, a, b)
                    .map(|()| Reply::Quiet)
                    .map_err(err)
            }),
        );
    }

    /// Pin a target path to a provider (`None` returns it to the rules).
    pub fn pin_file(&mut self, target: String, provider: Option<ModId>) {
        let Some(profile) = self.profile_id() else {
            return;
        };
        self.popup = None;
        self.run(
            false,
            Box::new(move |e| {
                e.set_file_override(profile, &target, provider)
                    .map(|()| Reply::Quiet)
                    .map_err(err)
            }),
        );
    }

    /// Cancel a download.
    pub fn cancel_download(&mut self, id: modrix_download::DownloadId) {
        if let Some(service) = &self.service
            && let Err(error) = service.manager().cancel(id)
        {
            let text = error.to_string();
            self.note(Tone::Error, text);
        }
    }

    /// Sort the Mods table by a column (again flips direction).
    pub fn sort_by(&mut self, key: SortKey) {
        let (current, ascending) = self.mod_sort;
        self.mod_sort = if current == key {
            (key, !ascending)
        } else {
            (key, true)
        };
        self.mod_sel.clear();
        self.sort_mods();
    }

    /// A list row was clicked: plain selects, Ctrl toggles, Shift extends.
    pub fn row_click(&mut self, pane: Pane, index: usize, mods: rune::Mods) {
        self.focus_pane = pane;
        let sel = match pane {
            Pane::Mods => &mut self.mod_sel,
            Pane::Plugins => &mut self.plugin_sel,
        };
        if mods.shift {
            sel.range_to(index);
        } else if mods.cmd || mods.ctrl {
            sel.toggle(index);
        } else if sel.items.len() == 1 && sel.items.contains(&index) {
            sel.clear();
        } else {
            sel.only(index);
        }
    }

    /// Hash dropped or picked archives off-thread, then install or prompt.
    pub fn install_files(&mut self, paths: Vec<PathBuf>) {
        if self.lib.game.is_none() {
            self.note(Tone::Error, "Register a game first");
            return;
        }
        let bus = self.bus.clone();
        let spawned = std::thread::Builder::new()
            .name("modrix-hash".to_owned())
            .spawn(move || {
                let items = paths
                    .into_iter()
                    .take(256)
                    .filter(|p| p.exists())
                    .map(|p| {
                        let hash = if p.is_dir() {
                            None
                        } else {
                            modrix_core::sha256_file(&p).ok()
                        };
                        (p, hash)
                    })
                    .collect();
                bus.send(Event::Hashed(items));
            });
        if let Err(error) = spawned {
            self.note(Tone::Error, error.to_string());
        }
    }

    /// Hashes are in: queue an install (or a duplicate prompt) per archive.
    pub fn on_hashed(&mut self, items: Vec<(PathBuf, Option<String>)>) {
        let Some(game) = self.lib.game else {
            return;
        };
        for (path, hash) in items {
            self.run(
                true,
                Box::new(move |e| install_or_prompt(e, game, &path, hash.as_deref())),
            );
        }
    }

    /// Duplicate prompt accepted: reinstall the existing mod.
    pub fn dup_confirm(&mut self) {
        if self.dup_queue.is_empty() {
            return;
        }
        let prompt = self.dup_queue.remove(0);
        self.run(
            true,
            Box::new(move |e| run_reinstall_many(e, &[prompt.existing])),
        );
    }

    /// Duplicate prompt dismissed.
    pub fn dup_cancel(&mut self) {
        if !self.dup_queue.is_empty() {
            self.dup_queue.remove(0);
        }
    }

    /// Register a game from the manual form.
    pub fn add_game(&mut self) {
        let install = PathBuf::from(self.form.install_path.text.trim());
        if !install.is_dir() {
            return self.note(
                Tone::Error,
                format!("{} is not a folder", install.display()),
            );
        }
        let def = match self.chosen_def() {
            Ok(def) => def,
            Err(error) => return self.note(Tone::Error, error),
        };
        self.form.install_path = rune::TextEdit::default();
        self.form.def_path = rune::TextEdit::default();
        self.run(
            false,
            Box::new(move |e| register(e, &def, &install, "manual")),
        );
    }

    /// Register a detected install and switch to it.
    pub fn add_detected(&mut self, index: usize) {
        let Some(found) = self.detected.get(index).cloned() else {
            return;
        };
        let def = match GameDef::from_toml_str(&found.def.toml, Path::new("<detected>")) {
            Ok(def) => def,
            Err(error) => return self.note(Tone::Error, error.to_string()),
        };
        self.run(
            false,
            Box::new(move |e| register(e, &def, &found.install, "steam")),
        );
    }

    fn chosen_def(&self) -> Result<GameDef, String> {
        let explicit = self.form.def_path.text.trim();
        if !explicit.is_empty() {
            return GameDef::from_file(Path::new(explicit)).map_err(err);
        }
        let id = self
            .form
            .def_id
            .as_deref()
            .ok_or("Pick a game definition")?;
        let choice = self
            .defs
            .iter()
            .find(|d| d.id == id)
            .ok_or("Pick a game definition")?;
        GameDef::from_toml_str(&choice.toml, Path::new("<built-in>")).map_err(err)
    }

    /// Detected installs not yet registered.
    #[must_use]
    pub fn unregistered_detected(&self) -> Vec<(usize, &crate::state::Detected)> {
        self.detected
            .iter()
            .enumerate()
            .filter(|(_, d)| !self.lib.games.iter().any(|g| g.plugin_id == d.def.id))
            .collect()
    }
}

/// Step an index by `delta`, clamped to `0..len`.
#[must_use]
pub fn step(index: usize, delta: i8, len: usize) -> usize {
    if delta < 0 {
        index.saturating_sub(1)
    } else {
        index.saturating_add(1).min(len.saturating_sub(1))
    }
}

fn register(e: &Engine, def: &GameDef, install: &Path, store: &str) -> Result<Reply, String> {
    let game = e.add_game(def, install, store).map_err(err)?;
    e.set_active_game(game.id).map_err(err)?;
    Ok(Reply::SelectGame(game.id, format!("{} added", game.name)))
}

fn install_or_prompt(
    e: &Engine,
    game: GameId,
    path: &Path,
    hash: Option<&str>,
) -> Result<Reply, String> {
    if let Some(hash) = hash {
        let existing = e.find_by_archive_hash(game, hash).map_err(err)?;
        if let Some(m) = existing.into_iter().next() {
            return Ok(Reply::Duplicate(DupPrompt {
                path: path.to_path_buf(),
                existing: m.id,
                existing_name: m.name,
            }));
        }
    }
    match modrix_service::install_file(e, game, path) {
        Ok(InstallOutcome::Installed { name, configurable }) => Ok(Reply::Note(
            Tone::Ok,
            if configurable {
                format!("{name} installed, options set to defaults")
            } else {
                format!("{name} installed")
            },
        )),
        Ok(other) => Err(format!("Install did not finish: {other:?}")),
        Err(error) => Err(format!("{error:#}")),
    }
}

/// Reinstall each mod from its archive, re-run FOMOD defaults, re-enable.
fn run_reinstall_many(e: &Engine, ids: &[ModId]) -> Result<Reply, String> {
    let mut done: usize = 0;
    for id in ids {
        let fresh = e.reinstall_mod(*id).map_err(err)?;
        modrix_service::fomod_pass(e, &fresh).map_err(|e| format!("{e:#}"))?;
        if let Ok(profile) = e.active_profile(fresh.game_id) {
            e.set_enabled(profile.id, fresh.id, true).map_err(err)?;
        }
        done = done.saturating_add(1);
    }
    Ok(Reply::Note(Tone::Ok, format!("{done} mods reinstalled")))
}
