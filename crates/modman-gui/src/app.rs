// SPDX-License-Identifier: GPL-2.0-only
//! Application state and update logic.
//!
//! The GUI is a thin face over [`modman_core::Engine`] plus the embedded
//! [`modman_service::Service`] - the same service `modman serve` hosts - so a
//! browser hand-off installs mods while the window is open. All business
//! logic stays in the engine; this module only shuttles state.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::{Subscription, Task, Theme};
use modman_core::{
    Conflict, Engine, Game, GameDef, GameId, Mod, ModId, Paths, Profile, ProfileId,
};
use modman_download::{DownloadId, DownloadState, DownloadStatus};
use modman_plugin::fomod;
use modman_service::{Binding, InstallOutcome, Service};

use crate::theme;

/// Simultaneously-active downloads (matches `modman serve`).
const MAX_CONCURRENT: u8 = 4;

/// Game definitions compiled into the binary, available out of the box.
const BUILTIN_DEFS: [&str; 1] = [include_str!("../../../games/skyrimse/game.toml")];

/// Most files a definition scan will consider (bounded loop).
const MAX_DEF_SCAN: usize = 256;

/// Notifications kept before the oldest are dropped.
const MAX_NOTES: usize = 200;

/// Which main view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Overview cards.
    Dashboard,
    /// Registered games + registration form.
    Games,
    /// The mod table for the selected game.
    Mods,
    /// Reorderable enabled-mod list + conflicts.
    LoadOrder,
    /// Live download list + hand-off state.
    Downloads,
    /// Paths, service, extension pairing.
    Settings,
}

/// A selectable game definition (built-in or discovered on disk).
#[derive(Debug, Clone)]
pub struct DefChoice {
    /// Display name (the definition's `name`).
    pub name: String,
    /// The raw `game.toml` text.
    pub toml: String,
}

impl PartialEq for DefChoice {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
impl Eq for DefChoice {}
impl std::fmt::Display for DefChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// A game entry for the sidebar picker.
#[derive(Debug, Clone)]
pub struct GameChoice {
    /// The game's id.
    pub id: GameId,
    /// Its display name.
    pub name: String,
}

impl PartialEq for GameChoice {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for GameChoice {}
impl std::fmt::Display for GameChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// Notification severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Completed.
    Ok,
    /// Failed.
    Error,
    /// Informational.
    Info,
}

/// One entry in the notification center.
#[derive(Debug, Clone)]
pub struct Note {
    /// Severity.
    pub tone: Tone,
    /// The message.
    pub text: String,
}

/// The loopback pairing the browser extension needs.
#[derive(Debug, Clone)]
pub struct Link {
    /// Bound port.
    pub port: u16,
    /// Per-session token.
    pub token: String,
}

/// A summarized file conflict between two mods.
#[derive(Debug, Clone)]
pub struct ConflictRow {
    /// The mod whose files win (later in load order).
    pub winner: String,
    /// The mod being overridden.
    pub loser: String,
    /// How many files overlap.
    pub files: usize,
}

/// The FOMOD wizard modal.
#[derive(Debug, Clone)]
pub struct Wizard {
    /// The mod being configured.
    pub mod_id: ModId,
    /// Its display name.
    pub mod_name: String,
    /// The parsed installer.
    pub installer: fomod::Installer,
    /// Current selections.
    pub selections: fomod::Selections,
    /// Position within the *visible* steps.
    pub step: usize,
}

impl Wizard {
    /// Indices of steps visible under the current selections.
    #[must_use]
    pub fn visible_steps(&self) -> Vec<usize> {
        let flags = fomod::flags_of(&self.installer, &self.selections);
        self.installer
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.visible.as_ref().is_none_or(|d| d.eval(&flags)))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Form inputs, kept separate from engine-derived state so a refresh never
/// clobbers what the user is typing.
#[derive(Debug, Clone, Default)]
pub struct Forms {
    /// Chosen built-in/discovered definition.
    pub def_choice: Option<DefChoice>,
    /// Alternative: an explicit path to a `game.toml`.
    pub def_path: String,
    /// The game's install directory.
    pub install_path: String,
    /// Name for a new profile.
    pub profile_name: String,
}

/// The whole GUI state.
pub struct App {
    /// The embedded hand-off service (None until boot finishes).
    pub service: Option<Service>,
    /// Extension pairing when this process is the primary instance.
    pub link: Option<Link>,
    /// True when another instance holds the loopback port.
    pub already_running: bool,
    /// Fatal boot failure, rendered full-screen.
    pub boot_error: Option<String>,
    /// Current view.
    pub screen: Screen,
    /// Registered games.
    pub games: Vec<Game>,
    /// The game being managed.
    pub selected_game: Option<GameId>,
    /// Profiles of the selected game.
    pub profiles: Vec<Profile>,
    /// Its active profile.
    pub active_profile: Option<Profile>,
    /// All staged mods of the selected game.
    pub mods: Vec<Mod>,
    /// Enabled mods in load order.
    pub order: Vec<Mod>,
    /// Download snapshots, newest first.
    pub downloads: Vec<DownloadStatus>,
    /// Install phase of each completed download.
    pub outcomes: HashMap<DownloadId, InstallOutcome>,
    /// Download ids whose outcome was already notified.
    pub outcome_seen: HashSet<DownloadId>,
    /// Selected mod rows (mass actions).
    pub selection: HashSet<ModId>,
    /// Load-order drag source, while a drag is in flight.
    pub drag: Option<usize>,
    /// Conflict summary for the active profile.
    pub conflicts: Vec<ConflictRow>,
    /// Notification center entries, newest first.
    pub notes: Vec<Note>,
    /// Notifications not yet seen.
    pub unread: usize,
    /// Whether the notification panel is open.
    pub notes_open: bool,
    /// The FOMOD wizard, when open.
    pub wizard: Option<Wizard>,
    /// FOMOD mods awaiting their first options review (opened one at a time).
    pub wizard_queue: Vec<ModId>,
    /// Mods already offered a wizard this session (no re-nagging).
    pub prompted: HashSet<ModId>,
    /// A slow engine action is in flight.
    pub busy: bool,
    /// Selectable game definitions.
    pub defs: Vec<DefChoice>,
    /// Form fields.
    pub form: Forms,
    /// The engine's resolved on-disk locations (shown in Settings).
    pub paths: Option<Paths>,
    /// The fixed dark theme.
    pub theme: Theme,
}

/// Everything the boot task hands back.
#[derive(Debug, Clone)]
pub struct Booted {
    service: Service,
    link: Option<Link>,
    defs: Vec<DefChoice>,
    paths: Paths,
}

/// All UI events.
#[derive(Debug, Clone)]
pub enum Message {
    /// The service finished (or failed) starting.
    Booted(Result<Booted, String>),
    /// Switch view.
    Navigate(Screen),
    /// Periodic refresh.
    Tick,
    /// Sidebar game picker.
    GamePicked(GameChoice),
    /// Profile picker.
    ProfilePicked(String),
    /// New-profile name input.
    ProfileNameChanged(String),
    /// Create the named profile.
    CreateProfile,
    /// Enable/disable a mod in the active profile.
    ToggleMod(ModId, bool),
    /// Toggle a mod row's selection highlight.
    RowClicked(ModId),
    /// Clear the row selection.
    ClearSelection,
    /// Enable every currently disabled mod.
    EnableAll,
    /// Enable/disable all selected mods.
    SetSelectedEnabled(bool),
    /// Delete all selected mods.
    DeleteSelected,
    /// Reinstall all selected mods from their archives.
    ReinstallSelected,
    /// Move an enabled mod one slot up/down in the load order.
    MoveMod(usize, i8),
    /// Begin dragging the load-order row at this index.
    DragStart(usize),
    /// The cursor entered another row while dragging.
    DragOver(usize),
    /// The drag ended; commit the current order.
    DragEnd,
    /// Deploy the active profile.
    Deploy,
    /// Undeploy (purge) the active profile.
    Purge,
    /// Verify the deployment.
    Verify,
    /// A background engine action finished.
    ActionFinished(Result<String, String>),
    /// Definition picker.
    DefPicked(DefChoice),
    /// Definition path input.
    DefPathChanged(String),
    /// Install path input.
    InstallPathChanged(String),
    /// Register the game.
    AddGame,
    /// Open the file picker for local archives.
    PickFiles,
    /// Files chosen in the picker.
    FilesPicked(Vec<PathBuf>),
    /// A file was dropped onto the window.
    FileDropped(PathBuf),
    /// Cancel a download.
    CancelDownload(DownloadId),
    /// Copy text to the clipboard.
    CopyText(String),
    /// Open/close the notification panel.
    ToggleNotes,
    /// Empty the notification center.
    ClearNotes,
    /// Open the FOMOD wizard for a mod.
    Configure(ModId),
    /// Wizard: toggle a plugin.
    WizardPick {
        /// Step index (absolute).
        step: usize,
        /// Group index.
        group: usize,
        /// Plugin index.
        plugin: usize,
    },
    /// Wizard: next page.
    WizardNext,
    /// Wizard: previous page.
    WizardBack,
    /// Wizard: apply the chosen options.
    WizardFinish,
    /// Wizard: close without applying.
    WizardCancel,
}

/// A long-running engine action executed off the UI thread.
#[derive(Debug, Clone)]
enum Action {
    Deploy(ProfileId),
    Undeploy(ProfileId),
    Verify(ProfileId),
    Install {
        game: GameId,
        path: PathBuf,
    },
    ReinstallMany(Vec<ModId>),
    ApplyWizard {
        mod_id: ModId,
        staged: PathBuf,
        installer: Box<fomod::Installer>,
        selections: fomod::Selections,
    },
}

/// The screen to open on. `MODMAN_GUI_SCREEN` overrides for development and
/// screenshot tooling; users just get the dashboard.
fn initial_screen() -> Screen {
    match std::env::var("MODMAN_GUI_SCREEN").as_deref() {
        Ok("games") => Screen::Games,
        Ok("mods") => Screen::Mods,
        Ok("loadorder") => Screen::LoadOrder,
        Ok("downloads") => Screen::Downloads,
        Ok("settings") => Screen::Settings,
        _ => Screen::Dashboard,
    }
}

/// Initial state + the boot task.
pub fn boot() -> (App, Task<Message>) {
    let app = App {
        service: None,
        link: None,
        already_running: false,
        boot_error: None,
        screen: initial_screen(),
        games: Vec::new(),
        selected_game: None,
        profiles: Vec::new(),
        active_profile: None,
        mods: Vec::new(),
        order: Vec::new(),
        downloads: Vec::new(),
        outcomes: HashMap::new(),
        outcome_seen: HashSet::new(),
        selection: HashSet::new(),
        drag: None,
        conflicts: Vec::new(),
        notes: Vec::new(),
        unread: 0,
        notes_open: false,
        wizard: None,
        wizard_queue: Vec::new(),
        prompted: HashSet::new(),
        busy: false,
        defs: Vec::new(),
        form: Forms::default(),
        paths: None,
        theme: theme::app_theme(),
    };
    (
        app,
        Task::perform(async { start_service() }, Message::Booted),
    )
}

/// Ticks for live refresh + window file drops.
pub fn subscription(app: &App) -> Subscription<Message> {
    let drops = iced::event::listen_with(|event, _status, _window| match event {
        iced::Event::Window(iced::window::Event::FileDropped(path)) => {
            Some(Message::FileDropped(path))
        }
        _ => None,
    });
    if app.service.is_some() {
        Subscription::batch([
            drops,
            iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
        ])
    } else {
        drops
    }
}

/// Open the engine, start the download manager, and bind the loopback
/// listener so browser hand-offs reach this window. Must run on the
/// executor (the listener and stager are spawned onto it).
fn start_service() -> Result<Booted, String> {
    let paths = Paths::resolve().map_err(|e| e.to_string())?;
    let engine = Engine::open(&paths).map_err(|e| e.to_string())?;
    let lockfile = paths.instance_lock();
    let defs = discover_defs(&paths);
    let service = Service::new(engine, MAX_CONCURRENT).map_err(|e| format!("{e:#}"))?;
    let link = match service
        .bind(&lockfile, modman_ipc::DEFAULT_PORT)
        .map_err(|e| format!("{e:#}"))?
    {
        Binding::Primary { port, token, .. } => Some(Link { port, token }),
        Binding::AlreadyRunning => None,
    };
    Ok(Booted {
        service,
        link,
        defs,
        paths,
    })
}

/// Built-in definitions plus any `game.toml` under `<config>/games/`.
fn discover_defs(paths: &Paths) -> Vec<DefChoice> {
    let mut defs: Vec<DefChoice> = BUILTIN_DEFS
        .iter()
        .filter_map(|text| choice_from_toml(text))
        .collect();
    let dir = paths.config_dir().join("games");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return defs;
    };
    for entry in entries.flatten().take(MAX_DEF_SCAN) {
        let path = entry.path();
        let file = if path.is_dir() {
            path.join("game.toml")
        } else {
            path
        };
        if file.extension().is_some_and(|e| e == "toml")
            && let Ok(text) = std::fs::read_to_string(&file)
            && let Some(choice) = choice_from_toml(&text)
            && !defs.contains(&choice)
        {
            defs.push(choice);
        }
    }
    defs
}

fn choice_from_toml(text: &str) -> Option<DefChoice> {
    let def = GameDef::from_toml_str(text, std::path::Path::new("<definition>")).ok()?;
    Some(DefChoice {
        name: def.name,
        toml: text.to_owned(),
    })
}

/// The update loop: route each message to its handler.
pub fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::Booted(result) => app.on_booted(result),
        Message::Navigate(screen) => app.on_navigate(screen),
        Message::Tick => app.refresh(),
        Message::GamePicked(choice) => {
            app.selected_game = Some(choice.id);
            app.selection.clear();
            app.refresh();
        }
        Message::ProfilePicked(name) => app.on_profile_picked(&name),
        Message::ProfileNameChanged(name) => app.form.profile_name = name,
        Message::CreateProfile => app.on_create_profile(),
        Message::ToggleMod(id, on) => app.on_toggle_mod(id, on),
        Message::RowClicked(id) => {
            if !app.selection.remove(&id) {
                app.selection.insert(id);
            }
        }
        Message::ClearSelection => app.selection.clear(),
        Message::EnableAll => app.on_enable_all(),
        Message::SetSelectedEnabled(on) => app.on_set_selected(on),
        Message::DeleteSelected => app.on_delete_selected(),
        Message::ReinstallSelected => return app.on_reinstall_selected(),
        Message::MoveMod(index, delta) => app.on_move_mod(index, delta),
        Message::DragStart(index) => app.drag = Some(index),
        Message::DragOver(index) => app.on_drag_over(index),
        Message::DragEnd => app.on_drag_end(),
        Message::Deploy => return app.on_profile_action(Action::Deploy),
        Message::Purge => return app.on_profile_action(Action::Undeploy),
        Message::Verify => return app.on_profile_action(Action::Verify),
        Message::ActionFinished(result) => app.on_action_finished(result),
        Message::DefPicked(choice) => app.form.def_choice = Some(choice),
        Message::DefPathChanged(path) => app.form.def_path = path,
        Message::InstallPathChanged(path) => app.form.install_path = path,
        Message::AddGame => app.on_add_game(),
        Message::PickFiles => return pick_files(),
        Message::FilesPicked(paths) => return app.on_install_files(paths),
        Message::FileDropped(path) => return app.on_install_files(vec![path]),
        Message::CancelDownload(id) => app.on_cancel_download(id),
        Message::CopyText(text) => return iced::clipboard::write(text),
        ref other => return update_overlay(app, other),
    }
    Task::none()
}

/// Notification-center and wizard messages, split out of [`update`].
fn update_overlay(app: &mut App, message: &Message) -> Task<Message> {
    match *message {
        Message::ToggleNotes => {
            app.notes_open = !app.notes_open;
            app.unread = 0;
        }
        Message::ClearNotes => {
            app.notes.clear();
            app.unread = 0;
            app.notes_open = false;
        }
        Message::Configure(id) => app.on_configure(id),
        Message::WizardPick {
            step,
            group,
            plugin,
        } => app.on_wizard_pick(step, group, plugin),
        Message::WizardNext => app.on_wizard_step(1),
        Message::WizardBack => app.on_wizard_step(-1),
        Message::WizardFinish => return app.on_wizard_finish(),
        Message::WizardCancel => app.on_wizard_cancel(),
        _ => {}
    }
    Task::none()
}

fn pick_files() -> Task<Message> {
    Task::perform(
        async {
            rfd::AsyncFileDialog::new()
                .add_filter("mod archives", &["zip", "7z", "rar", "tar", "gz"])
                .set_title("Add mods")
                .pick_files()
                .await
                .unwrap_or_default()
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        },
        Message::FilesPicked,
    )
}

impl App {
    fn on_booted(&mut self, result: Result<Booted, String>) {
        match result {
            Ok(booted) => {
                self.already_running = booted.link.is_none();
                self.link = booted.link;
                self.defs = booted.defs;
                self.paths = Some(booted.paths);
                self.service = Some(booted.service);
                self.refresh();
            }
            Err(error) => self.boot_error = Some(error),
        }
    }

    fn on_navigate(&mut self, screen: Screen) {
        self.screen = screen;
        self.notes_open = false;
        if screen == Screen::LoadOrder {
            self.refresh_conflicts();
        }
    }

    /// Re-pull downloads and engine state. Never blocks: if the engine is
    /// mid-deploy the refresh silently waits for the next tick.
    fn refresh(&mut self) {
        if let Some(service) = &self.service {
            let mut downloads = service.manager().list();
            downloads.sort_by_key(|d| std::cmp::Reverse(d.id));
            self.outcomes = downloads
                .iter()
                .filter(|d| d.state == DownloadState::Complete)
                .filter_map(|d| service.install_outcome(d.id).map(|o| (d.id, o)))
                .collect();
            self.downloads = downloads;
            self.notify_new_outcomes();
        }
        self.refresh_engine();
    }

    /// Turn newly finished installs into notifications.
    fn notify_new_outcomes(&mut self) {
        let new: Vec<(DownloadId, InstallOutcome)> = self
            .outcomes
            .iter()
            .filter(|(id, _)| !self.outcome_seen.contains(*id))
            .map(|(id, o)| (*id, o.clone()))
            .collect();
        for (id, outcome) in new {
            self.outcome_seen.insert(id);
            match outcome {
                InstallOutcome::Installed { name, configurable } => {
                    let text = if configurable {
                        format!("{name} installed · options set to defaults")
                    } else {
                        format!("{name} installed")
                    };
                    self.note(Tone::Ok, text);
                }
                InstallOutcome::Failed(error) => {
                    self.note(Tone::Error, format!("Install failed: {error}"));
                }
                InstallOutcome::NoGame => {
                    self.note(Tone::Info, "Download kept: no game matched".to_owned());
                }
            }
        }
    }

    fn refresh_engine(&mut self) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let Ok(engine) = service.engine().try_lock() else {
            return;
        };
        let games = match engine.games() {
            Ok(games) => games,
            Err(error) => {
                let text = error.to_string();
                drop(engine);
                self.note(Tone::Error, text);
                return;
            }
        };
        self.selected_game = self
            .selected_game
            .filter(|id| games.iter().any(|g| g.id == *id))
            .or_else(|| games.first().map(|g| g.id));
        self.games = games;
        let Some(game) = self.selected_game else {
            self.profiles = Vec::new();
            self.active_profile = None;
            self.mods = Vec::new();
            self.order = Vec::new();
            return;
        };
        self.profiles = engine.profiles(game).unwrap_or_default();
        self.active_profile = engine.active_profile(game).ok();
        self.mods = engine.mods(game).unwrap_or_default();
        self.order = match &self.active_profile {
            Some(profile) => engine.enabled_mods(profile.id).unwrap_or_default(),
            None => Vec::new(),
        };
        self.selection.retain(|id| self.mods.iter().any(|m| m.id == *id));
        self.queue_unreviewed_fomods();
    }

    /// Offer the options wizard once for every FOMOD mod whose defaults were
    /// applied automatically (`fomod-auto`) - the popup the installer earned.
    fn queue_unreviewed_fomods(&mut self) {
        let pending: Vec<ModId> = self
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

    /// Pop the next queued mod into the wizard, if none is open.
    fn open_next_wizard(&mut self) {
        while self.wizard.is_none() && !self.wizard_queue.is_empty() {
            let id = self.wizard_queue.remove(0);
            self.on_configure(id);
        }
    }

    /// Closing the wizard without applying accepts the defaults: mark the mod
    /// reviewed so it is not offered again, then continue the queue.
    fn on_wizard_cancel(&mut self) {
        if let Some(wizard) = self.wizard.take() {
            let id = wizard.mod_id;
            let _ = self.with_engine(|e| e.set_install_state(id, "fomod"));
        }
        self.open_next_wizard();
    }

    /// Recompute the conflict summary (plan the active profile).
    fn refresh_conflicts(&mut self) {
        let Some(profile) = self.active_profile.clone() else {
            self.conflicts = Vec::new();
            return;
        };
        let plan = self.with_engine(|e| e.plan(profile.id));
        let Some(plan) = plan else {
            return;
        };
        self.conflicts = summarize_conflicts(plan.conflicts(), &self.mods);
    }

    /// Run a quick engine call, reporting failures as notifications.
    fn with_engine<T>(
        &mut self,
        act: impl FnOnce(&Engine) -> modman_core::Result<T>,
    ) -> Option<T> {
        let outcome = {
            let service = self.service.as_ref()?;
            match service.engine().try_lock() {
                Ok(engine) => act(&engine).map_err(|e| e.to_string()),
                Err(_) => Err("engine busy, try again".to_owned()),
            }
        };
        match outcome {
            Ok(value) => Some(value),
            Err(error) => {
                self.note(Tone::Error, error);
                None
            }
        }
    }

    fn on_profile_picked(&mut self, name: &str) {
        let Some(profile) = self.profiles.iter().find(|p| p.name == name).cloned() else {
            return;
        };
        if self
            .with_engine(|e| e.set_active_profile(profile.id))
            .is_some()
        {
            self.refresh();
        }
    }

    fn on_create_profile(&mut self) {
        let name = self.form.profile_name.trim().to_owned();
        let Some(game) = self.selected_game else {
            return;
        };
        if name.is_empty() {
            return;
        }
        if let Some(profile) = self.with_engine(|e| e.create_profile(game, &name)) {
            self.note(Tone::Ok, format!("Profile “{}” created", profile.name));
            self.form.profile_name.clear();
            self.refresh();
        }
    }

    fn on_toggle_mod(&mut self, id: ModId, on: bool) {
        let Some(profile) = self.active_profile.clone() else {
            return;
        };
        if self
            .with_engine(|e| e.set_enabled(profile.id, id, on))
            .is_some()
        {
            self.refresh();
            self.refresh_conflicts();
        }
    }

    fn on_enable_all(&mut self) {
        let Some(profile) = self.active_profile.clone() else {
            return;
        };
        let disabled: Vec<ModId> = self
            .mods
            .iter()
            .filter(|m| !self.order.iter().any(|e| e.id == m.id))
            .map(|m| m.id)
            .collect();
        let count = disabled.len();
        let done = self.with_engine(|e| {
            for id in disabled {
                e.set_enabled(profile.id, id, true)?;
            }
            Ok(())
        });
        if done.is_some() {
            self.note(Tone::Ok, format!("{count} mods enabled"));
            self.refresh();
            self.refresh_conflicts();
        }
    }

    fn on_set_selected(&mut self, on: bool) {
        let Some(profile) = self.active_profile.clone() else {
            return;
        };
        let ids: Vec<ModId> = self.selection.iter().copied().collect();
        let count = ids.len();
        let done = self.with_engine(|e| {
            for id in ids {
                e.set_enabled(profile.id, id, on)?;
            }
            Ok(())
        });
        if done.is_some() {
            let verb = if on { "enabled" } else { "disabled" };
            self.note(Tone::Ok, format!("{count} mods {verb}"));
            self.refresh();
            self.refresh_conflicts();
        }
    }

    fn on_delete_selected(&mut self) {
        let ids: Vec<ModId> = self.selection.iter().copied().collect();
        let count = ids.len();
        let done = self.with_engine(|e| {
            for id in ids {
                e.delete_mod(id)?;
            }
            Ok(())
        });
        if done.is_some() {
            self.note(Tone::Ok, format!("{count} mods deleted"));
            self.selection.clear();
            self.refresh();
        }
    }

    fn on_reinstall_selected(&mut self) -> Task<Message> {
        let ids: Vec<ModId> = self.selection.iter().copied().collect();
        if ids.is_empty() {
            return Task::none();
        }
        self.selection.clear();
        self.spawn_action(Action::ReinstallMany(ids))
    }

    fn on_move_mod(&mut self, index: usize, delta: i8) {
        let target = if delta < 0 {
            index.checked_sub(1)
        } else {
            index.checked_add(1)
        };
        let Some(target) = target.filter(|t| *t < self.order.len() && index < self.order.len())
        else {
            return;
        };
        self.order.swap(index, target);
        self.commit_order();
    }

    fn on_drag_over(&mut self, index: usize) {
        let Some(from) = self.drag else {
            return;
        };
        if from == index || from >= self.order.len() || index >= self.order.len() {
            return;
        }
        let moved = self.order.remove(from);
        self.order.insert(index, moved);
        self.drag = Some(index);
    }

    fn on_drag_end(&mut self) {
        if self.drag.take().is_some() {
            self.commit_order();
        }
    }

    fn commit_order(&mut self) {
        let Some(profile) = self.active_profile.clone() else {
            return;
        };
        let ids: Vec<ModId> = self.order.iter().map(|m| m.id).collect();
        if self
            .with_engine(|e| e.set_load_order(profile.id, &ids))
            .is_some()
        {
            self.refresh();
            self.refresh_conflicts();
        }
    }

    fn on_profile_action(&mut self, make: impl FnOnce(ProfileId) -> Action) -> Task<Message> {
        let Some(profile) = self.active_profile.clone() else {
            return Task::none();
        };
        self.spawn_action(make(profile.id))
    }

    fn on_install_files(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        let Some(game) = self.selected_game else {
            self.note(Tone::Error, "Register a game first".to_owned());
            return Task::none();
        };
        let tasks: Vec<Task<Message>> = paths
            .into_iter()
            .filter(|p| p.exists())
            .map(|path| self.spawn_action(Action::Install { game, path }))
            .collect();
        Task::batch(tasks)
    }

    /// Run a slow engine action on a worker thread so the UI stays live.
    fn spawn_action(&mut self, action: Action) -> Task<Message> {
        let Some(service) = self.service.clone() else {
            return Task::none();
        };
        self.busy = true;
        let engine = Arc::clone(service.engine());
        Task::perform(
            async move {
                match tokio::task::spawn_blocking(move || run_action(&engine, &action)).await {
                    Ok(result) => result,
                    Err(error) => Err(format!("engine worker crashed: {error}")),
                }
            },
            Message::ActionFinished,
        )
    }

    fn on_action_finished(&mut self, result: Result<String, String>) {
        self.busy = false;
        match result {
            Ok(text) => self.note(Tone::Ok, text),
            Err(text) => self.note(Tone::Error, text),
        }
        self.refresh();
        if self.screen == Screen::LoadOrder {
            self.refresh_conflicts();
        }
    }

    fn on_add_game(&mut self) {
        let install = PathBuf::from(self.form.install_path.trim());
        if !install.is_dir() {
            return self.note(Tone::Error, format!("{} is not a directory", install.display()));
        }
        let def = match self.load_chosen_def() {
            Ok(def) => def,
            Err(error) => return self.note(Tone::Error, error),
        };
        if let Some(game) = self.with_engine(|e| e.add_game(&def, &install, "manual")) {
            self.note(Tone::Ok, format!("{} registered", game.name));
            self.selected_game = Some(game.id);
            self.form.install_path.clear();
            self.form.def_path.clear();
            self.refresh();
        }
    }

    fn load_chosen_def(&self) -> Result<GameDef, String> {
        let explicit = self.form.def_path.trim();
        if !explicit.is_empty() {
            return GameDef::from_file(std::path::Path::new(explicit)).map_err(|e| e.to_string());
        }
        match &self.form.def_choice {
            Some(choice) => {
                GameDef::from_toml_str(&choice.toml, std::path::Path::new("<built-in>"))
                    .map_err(|e| e.to_string())
            }
            None => Err("Pick a game definition".to_owned()),
        }
    }

    fn on_cancel_download(&mut self, id: DownloadId) {
        let Some(service) = &self.service else {
            return;
        };
        if let Err(error) = service.manager().cancel(id) {
            self.note(Tone::Error, error.to_string());
        }
        self.refresh();
    }

    fn on_configure(&mut self, id: ModId) {
        let Some(m) = self.mods.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        match fomod::parse(&m.staged_path) {
            Ok(Some(installer)) => {
                let selections = fomod::defaults(&installer);
                self.wizard = Some(Wizard {
                    mod_id: m.id,
                    mod_name: m.name,
                    installer,
                    selections,
                    step: 0,
                });
            }
            Ok(None) => self.note(Tone::Info, "No installer options in this mod".to_owned()),
            Err(error) => self.note(Tone::Error, error.to_string()),
        }
    }

    fn on_wizard_pick(&mut self, step: usize, group: usize, plugin: usize) {
        let Some(wizard) = &mut self.wizard else {
            return;
        };
        let Some(kind) = wizard
            .installer
            .steps
            .get(step)
            .and_then(|s| s.groups.get(group))
            .map(|g| g.kind)
        else {
            return;
        };
        let Some(sel) = wizard
            .selections
            .get_mut(step)
            .and_then(|s| s.get_mut(group))
        else {
            return;
        };
        match kind {
            fomod::GroupKind::All => {}
            fomod::GroupKind::ExactlyOne => {
                sel.clear();
                sel.insert(plugin);
            }
            fomod::GroupKind::AtMostOne => {
                let had = sel.remove(&plugin);
                sel.clear();
                if !had {
                    sel.insert(plugin);
                }
            }
            fomod::GroupKind::Any | fomod::GroupKind::AtLeastOne => {
                if !sel.remove(&plugin) {
                    sel.insert(plugin);
                }
            }
        }
    }

    fn on_wizard_step(&mut self, delta: i8) {
        let Some(wizard) = &mut self.wizard else {
            return;
        };
        let pages = wizard.visible_steps().len().max(1);
        let next = if delta < 0 {
            wizard.step.saturating_sub(1)
        } else {
            wizard.step.saturating_add(1)
        };
        wizard.step = next.min(pages.saturating_sub(1));
    }

    fn on_wizard_finish(&mut self) -> Task<Message> {
        let Some(wizard) = self.wizard.take() else {
            return Task::none();
        };
        let Some(m) = self.mods.iter().find(|m| m.id == wizard.mod_id).cloned() else {
            return Task::none();
        };
        self.spawn_action(Action::ApplyWizard {
            mod_id: wizard.mod_id,
            staged: m.staged_path,
            installer: Box::new(wizard.installer),
            selections: wizard.selections,
        })
    }

    fn note(&mut self, tone: Tone, text: String) {
        self.notes.insert(0, Note { tone, text });
        self.notes.truncate(MAX_NOTES);
        if !self.notes_open {
            self.unread = self.unread.saturating_add(1);
        }
    }
}

/// Aggregate raw file conflicts into per-mod-pair rows.
fn summarize_conflicts(conflicts: &[Conflict], mods: &[Mod]) -> Vec<ConflictRow> {
    let name_of = |id: ModId| {
        mods.iter()
            .find(|m| m.id == id)
            .map_or_else(|| id.to_string(), |m| m.name.clone())
    };
    let mut pairs: HashMap<(ModId, ModId), usize> = HashMap::new();
    for conflict in conflicts {
        for loser in &conflict.shadowed {
            let count = pairs.entry((conflict.winner, *loser)).or_insert(0);
            *count = count.saturating_add(1);
        }
    }
    let mut rows: Vec<ConflictRow> = pairs
        .into_iter()
        .map(|((winner, loser), files)| ConflictRow {
            winner: name_of(winner),
            loser: name_of(loser),
            files,
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.files));
    rows
}

fn run_install(engine: &Engine, game: GameId, path: &std::path::Path) -> Result<String, String> {
    match modman_service::install_file(engine, game, path) {
        Ok(InstallOutcome::Installed { name, configurable }) => Ok(if configurable {
            format!("{name} installed · options set to defaults")
        } else {
            format!("{name} installed")
        }),
        Ok(other) => Err(format!("install did not finish: {other:?}")),
        Err(error) => Err(format!("{error:#}")),
    }
}

/// Execute one slow action while holding the engine lock (worker thread).
fn run_action(engine: &Mutex<Engine>, action: &Action) -> Result<String, String> {
    let engine = engine
        .lock()
        .map_err(|_| "engine lock poisoned".to_owned())?;
    match action {
        Action::Deploy(profile) => engine
            .deploy(*profile)
            .map(|r| {
                format!(
                    "Deployed · {} added · {} removed · {} unchanged",
                    r.added(),
                    r.removed(),
                    r.unchanged()
                )
            })
            .map_err(|e| e.to_string()),
        Action::Undeploy(profile) => engine
            .undeploy(*profile)
            .map(|r| format!("Purged · {} files restored", r.removed()))
            .map_err(|e| e.to_string()),
        Action::Verify(profile) => engine
            .verify(*profile)
            .map(|r| {
                if r.is_clean() {
                    format!("Verified · {} files healthy", r.checked())
                } else {
                    format!("Verify: {} of {} files changed", r.issues().len(), r.checked())
                }
            })
            .map_err(|e| e.to_string()),
        Action::Install { game, path } => run_install(&engine, *game, path),
        Action::ReinstallMany(ids) => {
            let mut done: usize = 0;
            for id in ids {
                engine.reinstall_mod(*id).map_err(|e| e.to_string())?;
                done = done.saturating_add(1);
            }
            Ok(format!("{done} mods reinstalled"))
        }
        Action::ApplyWizard {
            mod_id,
            staged,
            installer,
            selections,
        } => {
            let ops = fomod::resolve(installer, selections);
            let placed = fomod::apply(staged, &ops).map_err(|e| e.to_string())?;
            engine
                .set_install_state(*mod_id, "fomod")
                .map_err(|e| e.to_string())?;
            Ok(format!("Options applied · {placed} files · deploy to update"))
        }
    }
}
