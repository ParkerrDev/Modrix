// SPDX-License-Identifier: GPL-2.0-only
//! Application state and update logic.
//!
//! The GUI is a thin face over [`modman_core::Engine`] plus the embedded
//! [`modman_service::Service`] - the same service `modman serve` hosts - so a
//! browser hand-off installs mods while the window is open. All business
//! logic stays in the engine; this module only shuttles state.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::{Subscription, Task, Theme};
use modman_core::{
    DeployReport, Engine, Game, GameDef, GameId, Mod, ModId, Paths, Profile, ProfileId,
    VerifyReport,
};
use modman_download::{DownloadId, DownloadStatus};
use modman_service::{Binding, Service};

use crate::theme;

/// Simultaneously-active downloads (matches `modman serve`).
const MAX_CONCURRENT: u8 = 4;

/// Game definitions compiled into the binary, available out of the box.
const BUILTIN_DEFS: [&str; 1] = [include_str!("../../../games/skyrimse/game.toml")];

/// Most files a definition scan will consider (bounded loop).
const MAX_DEF_SCAN: usize = 256;

/// Which main view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Overview cards.
    Dashboard,
    /// Registered games + registration form.
    Games,
    /// The mod table for the selected game.
    Mods,
    /// Reorderable enabled-mod list.
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

/// The tone of the status banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// A completed action.
    Ok,
    /// A failed action.
    Error,
}

/// A dismissible one-line result banner.
#[derive(Debug, Clone)]
pub struct StatusLine {
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
    /// Path of a local archive/directory to stage.
    pub mod_path: String,
    /// Optional name override for the staged mod.
    pub mod_name: String,
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
    /// Result banner.
    pub status: Option<StatusLine>,
    /// A deploy/purge/verify/stage is in flight.
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
    /// Move an enabled mod one slot up/down in the load order.
    MoveMod(usize, i8),
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
    /// Local mod path input.
    ModPathChanged(String),
    /// Local mod name input.
    ModNameChanged(String),
    /// Stage the local mod.
    AddLocalMod,
    /// Cancel a download.
    CancelDownload(DownloadId),
    /// Copy text to the clipboard.
    CopyText(String),
    /// Dismiss the status banner.
    DismissStatus,
}

/// A long-running engine action executed off the UI thread.
#[derive(Debug, Clone)]
enum Action {
    Deploy(ProfileId),
    Undeploy(ProfileId),
    Verify(ProfileId),
    Stage {
        game: GameId,
        name: String,
        path: PathBuf,
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
        status: None,
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

/// Poll for download progress and hand-off installs once per second.
pub fn subscription(app: &App) -> Subscription<Message> {
    if app.service.is_some() {
        iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick)
    } else {
        Subscription::none()
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
        Message::Navigate(screen) => app.screen = screen,
        Message::Tick => app.refresh(),
        Message::GamePicked(choice) => {
            app.selected_game = Some(choice.id);
            app.refresh();
        }
        Message::ProfilePicked(name) => app.on_profile_picked(&name),
        Message::ProfileNameChanged(name) => app.form.profile_name = name,
        Message::CreateProfile => app.on_create_profile(),
        Message::ToggleMod(id, on) => app.on_toggle_mod(id, on),
        Message::MoveMod(index, delta) => app.on_move_mod(index, delta),
        Message::Deploy => return app.on_profile_action(Action::Deploy),
        Message::Purge => return app.on_profile_action(Action::Undeploy),
        Message::Verify => return app.on_profile_action(Action::Verify),
        Message::ActionFinished(result) => app.on_action_finished(result),
        Message::DefPicked(choice) => app.form.def_choice = Some(choice),
        Message::DefPathChanged(path) => app.form.def_path = path,
        Message::InstallPathChanged(path) => app.form.install_path = path,
        Message::AddGame => app.on_add_game(),
        Message::ModPathChanged(path) => app.form.mod_path = path,
        Message::ModNameChanged(name) => app.form.mod_name = name,
        Message::AddLocalMod => return app.on_add_local_mod(),
        Message::CancelDownload(id) => app.on_cancel_download(id),
        Message::CopyText(text) => return iced::clipboard::write(text),
        Message::DismissStatus => app.status = None,
    }
    Task::none()
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

    /// Re-pull downloads and engine state. Never blocks: if the engine is
    /// mid-deploy the refresh silently waits for the next tick.
    fn refresh(&mut self) {
        if let Some(service) = &self.service {
            let mut downloads = service.manager().list();
            downloads.sort_by_key(|d| std::cmp::Reverse(d.id));
            self.downloads = downloads;
        }
        self.refresh_engine();
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
                self.fail(text);
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
    }

    /// Run a quick engine call, reporting failures in the status banner.
    fn with_engine<T>(
        &mut self,
        act: impl FnOnce(&Engine) -> modman_core::Result<T>,
    ) -> Option<T> {
        let outcome = {
            let Some(service) = self.service.as_ref() else {
                self.fail("the engine is still starting".to_owned());
                return None;
            };
            match service.engine().try_lock() {
                Ok(engine) => act(&engine).map_err(|e| e.to_string()),
                Err(_) => Err("the engine is busy - try again in a moment".to_owned()),
            }
        };
        match outcome {
            Ok(value) => Some(value),
            Err(error) => {
                self.fail(error);
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
            return self.fail("register a game first".to_owned());
        };
        if name.is_empty() {
            return self.fail("give the profile a name".to_owned());
        }
        if let Some(profile) = self.with_engine(|e| e.create_profile(game, &name)) {
            self.ok(format!("Created profile “{}”", profile.name));
            self.form.profile_name.clear();
            self.refresh();
        }
    }

    fn on_toggle_mod(&mut self, id: ModId, on: bool) {
        let Some(profile) = self.active_profile.clone() else {
            return self.fail("no active profile".to_owned());
        };
        if self
            .with_engine(|e| e.set_enabled(profile.id, id, on))
            .is_some()
        {
            self.refresh();
        }
    }

    fn on_move_mod(&mut self, index: usize, delta: i8) {
        let Some(profile) = self.active_profile.clone() else {
            return;
        };
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
        let ids: Vec<ModId> = self.order.iter().map(|m| m.id).collect();
        if self
            .with_engine(|e| e.set_load_order(profile.id, &ids))
            .is_some()
        {
            self.refresh();
        }
    }

    fn on_profile_action(&mut self, make: impl FnOnce(ProfileId) -> Action) -> Task<Message> {
        let Some(profile) = self.active_profile.clone() else {
            self.fail("no active profile".to_owned());
            return Task::none();
        };
        self.spawn_action(make(profile.id))
    }

    fn on_add_local_mod(&mut self) -> Task<Message> {
        let Some(game) = self.selected_game else {
            self.fail("register a game first".to_owned());
            return Task::none();
        };
        let path = PathBuf::from(self.form.mod_path.trim());
        if !path.exists() {
            self.fail(format!("{} does not exist", path.display()));
            return Task::none();
        }
        let typed = self.form.mod_name.trim();
        let name = if typed.is_empty() {
            path.file_stem()
                .map_or_else(|| "mod".to_owned(), |s| s.to_string_lossy().into_owned())
        } else {
            typed.to_owned()
        };
        self.form.mod_path.clear();
        self.form.mod_name.clear();
        self.spawn_action(Action::Stage { game, name, path })
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
            Ok(text) => self.ok(text),
            Err(text) => self.fail(text),
        }
        self.refresh();
    }

    fn on_add_game(&mut self) {
        let install = PathBuf::from(self.form.install_path.trim());
        if !install.is_dir() {
            return self.fail(format!("{} is not a directory", install.display()));
        }
        let def = match self.load_chosen_def() {
            Ok(def) => def,
            Err(error) => return self.fail(error),
        };
        if let Some(game) = self.with_engine(|e| e.add_game(&def, &install, "manual")) {
            self.ok(format!("Registered {}", game.name));
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
            None => Err("pick a game definition (or point at a game.toml)".to_owned()),
        }
    }

    fn on_cancel_download(&mut self, id: DownloadId) {
        let Some(service) = &self.service else {
            return;
        };
        if let Err(error) = service.manager().cancel(id) {
            self.fail(error.to_string());
        }
        self.refresh();
    }

    fn ok(&mut self, text: String) {
        self.status = Some(StatusLine {
            tone: Tone::Ok,
            text,
        });
    }

    fn fail(&mut self, text: String) {
        self.status = Some(StatusLine {
            tone: Tone::Error,
            text,
        });
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
            .map(|r| summarize_deploy(&r))
            .map_err(|e| e.to_string()),
        Action::Undeploy(profile) => engine
            .undeploy(*profile)
            .map(|r| format!("Purged - {} file(s) removed, originals restored", r.removed()))
            .map_err(|e| e.to_string()),
        Action::Verify(profile) => engine
            .verify(*profile)
            .map(|r| summarize_verify(&r))
            .map_err(|e| e.to_string()),
        Action::Stage { game, name, path } => engine
            .stage(*game, name, path)
            .map(|m| format!("Staged “{}” - enable it, then deploy", m.name))
            .map_err(|e| e.to_string()),
    }
}

fn summarize_deploy(report: &DeployReport) -> String {
    use std::fmt::Write as _;
    let (hard, sym, copy) = report.link_breakdown();
    let mut text = format!(
        "Deployed - {} added, {} removed, {} unchanged ({hard} hardlinks, {sym} symlinks, {copy} copies)",
        report.added(),
        report.removed(),
        report.unchanged(),
    );
    if !report.conflicts().is_empty() {
        let _ = write!(
            text,
            "; {} conflict(s) resolved by load order",
            report.conflicts().len()
        );
    }
    if report.skipped_modified() > 0 {
        let _ = write!(
            text,
            "; {} user-modified file(s) left untouched",
            report.skipped_modified()
        );
    }
    text
}

fn summarize_verify(report: &VerifyReport) -> String {
    if report.is_clean() {
        format!("Verified - {} file(s), all healthy", report.checked())
    } else {
        format!(
            "Verified - {} of {} file(s) missing or modified",
            report.issues().len(),
            report.checked()
        )
    }
}
