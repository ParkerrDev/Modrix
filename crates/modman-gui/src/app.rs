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
use modman_core::plugins::GamePlugin;
use modman_download::{DownloadId, DownloadState, DownloadStatus};
use modman_plugin::fomod;
use modman_service::{Binding, InstallOutcome, Service};

use crate::theme;

/// A sortable column of the Mods table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// Install order (mod id) - the default.
    Installed,
    /// Alphabetical by name.
    Name,
    /// Enabled first.
    Enabled,
    /// By version string.
    Version,
    /// By source (nexus/local).
    Source,
}

/// Which list has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// The Mods table.
    Mods,
    /// The Load Order (plugins) list.
    Plugins,
}

/// A multi-selection over a list: selected indices, a fixed **anchor** (where
/// a Shift-range starts), and a moving **cursor** (where it currently ends).
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Selected indices.
    pub items: std::collections::BTreeSet<usize>,
    /// The fixed end of a Shift-range (set by plain clicks/moves).
    pub anchor: Option<usize>,
    /// The moving end (last row touched).
    pub cursor: Option<usize>,
}

impl Selection {
    /// Clear everything.
    pub fn clear(&mut self) {
        self.items.clear();
        self.anchor = None;
        self.cursor = None;
    }

    /// Select exactly `index` (plain click / arrow move).
    pub fn only(&mut self, index: usize) {
        self.items.clear();
        self.items.insert(index);
        self.anchor = Some(index);
        self.cursor = Some(index);
    }

    /// Toggle `index` (Ctrl semantics); it becomes the new anchor.
    pub fn toggle(&mut self, index: usize) {
        if !self.items.remove(&index) {
            self.items.insert(index);
        }
        self.anchor = Some(index);
        self.cursor = Some(index);
    }

    /// Extend from the fixed anchor to `index` (Shift semantics). The anchor
    /// does not move, so repeated extensions grow the same range.
    pub fn range_to(&mut self, index: usize) {
        let anchor = self.anchor.or(self.cursor).unwrap_or(index);
        let (lo, hi) = (anchor.min(index), anchor.max(index));
        self.items = (lo..=hi).collect();
        self.anchor = Some(anchor);
        self.cursor = Some(index);
    }

    /// Keep only indices below `len` (after the list shrinks).
    pub fn retain_below(&mut self, len: usize) {
        self.items.retain(|i| *i < len);
        self.anchor = self.anchor.filter(|a| *a < len);
        self.cursor = self.cursor.filter(|c| *c < len);
    }
}

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
    /// Its staged tree (image lookups resolve against it).
    pub staged_path: PathBuf,
    /// The parsed installer.
    pub installer: fomod::Installer,
    /// Current selections.
    pub selections: fomod::Selections,
    /// Position within the *visible* steps.
    pub step: usize,
    /// The option under the cursor / last touched - drives the preview pane.
    pub focus: Option<(usize, usize, usize)>,
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
    /// The profile's plugin (.esp/.esm/.esl) load order.
    pub plugins: Vec<GamePlugin>,
    /// Download snapshots, newest first.
    pub downloads: Vec<DownloadStatus>,
    /// Install phase of each completed download.
    pub outcomes: HashMap<DownloadId, InstallOutcome>,
    /// Download ids whose outcome was already notified.
    pub outcome_seen: HashSet<DownloadId>,
    /// Selected mod rows in the Mods table (indices into `mods`).
    pub mod_sel: Selection,
    /// Selected plugin rows in the Load Order (indices into `plugins`).
    pub plugin_sel: Selection,
    /// Which list owns the keyboard, set by the last click.
    pub focus_pane: Pane,
    /// Live modifier-key state (drives Shift/Ctrl-click selection).
    pub modifiers: iced::keyboard::Modifiers,
    /// Mods table sort: (column, ascending).
    pub mod_sort: (SortKey, bool),
    /// Mods list viewport: (scroll offset, visible height).
    pub mods_view: Option<(f32, f32)>,
    /// Plugins list viewport: (scroll offset, visible height).
    pub plugins_view: Option<(f32, f32)>,
    /// Load-order drag: (source index, current hover index).
    pub drag: Option<(usize, usize)>,
    /// Conflict summary for the active profile.
    pub conflicts: Vec<ConflictRow>,
    /// Setup health issues, worst-first.
    pub health: Vec<modman_core::Issue>,
    /// The engine's live progress sink (shared before the engine exists so
    /// boot-time crash recovery reports too).
    pub progress: std::sync::Arc<modman_core::Progress>,
    /// Latest progress snapshot (polled every tick).
    pub op: Option<modman_core::ProgressSnapshot>,
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
    /// A list row was clicked - updates the selection using the live
    /// modifier state (Shift = range, Ctrl = toggle).
    RowClick {
        /// Which list.
        pane: Pane,
        /// Row index.
        index: usize,
    },
    /// Click-away: clear the active list's selection.
    ClearSelection,
    /// A key was pressed while a list had focus.
    Key(iced::keyboard::Key, iced::keyboard::Modifiers),
    /// The modifier-key state changed.
    Modifiers(iced::keyboard::Modifiers),
    /// Sort the Mods table by a column (clicking again flips direction).
    SortBy(SortKey),
    /// A list was scrolled: (pane, offset y, viewport height).
    Scrolled(Pane, f32, f32),
    /// Enable every currently disabled mod.
    EnableAll,
    /// Enable/disable all selected mods.
    SetSelectedEnabled(bool),
    /// Delete all selected mods.
    DeleteSelected,
    /// Reinstall all selected mods from their archives.
    ReinstallSelected,
    /// Toggle a plugin's activation.
    TogglePlugin(usize),
    /// Auto-sort the plugin load order (LOOT-style).
    AutoSort,
    /// Move the current selection up/down by one (arrow buttons/keys).
    MoveSelection {
        /// Which list.
        pane: Pane,
        /// -1 up, +1 down.
        delta: i8,
    },
    /// Begin dragging a row.
    DragStart(Pane, usize),
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
    /// Wizard: preview an option (hover).
    WizardHover {
        /// Step index (absolute).
        step: usize,
        /// Group index.
        group: usize,
        /// Plugin index.
        plugin: usize,
    },
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
    AutoSortPlugins(ProfileId),
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
        plugins: Vec::new(),
        downloads: Vec::new(),
        outcomes: HashMap::new(),
        outcome_seen: HashSet::new(),
        mod_sel: Selection::default(),
        plugin_sel: Selection::default(),
        focus_pane: Pane::Mods,
        modifiers: iced::keyboard::Modifiers::default(),
        mod_sort: (SortKey::Installed, true),
        mods_view: None,
        plugins_view: None,
        drag: None,
        conflicts: Vec::new(),
        health: Vec::new(),
        progress: std::sync::Arc::new(modman_core::Progress::default()),
        op: None,
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
    let progress = std::sync::Arc::clone(&app.progress);
    (
        app,
        Task::perform(async move { start_service(&progress) }, Message::Booted),
    )
}

/// Ticks for live refresh + window file drops.
pub fn subscription(app: &App) -> Subscription<Message> {
    // `listen_with` takes a plain fn pointer (no captures); `on_key` ignores
    // navigation keys while the wizard is open.
    let events = iced::event::listen_with(|event, _status, _window| match event {
        iced::Event::Window(iced::window::Event::FileDropped(path)) => {
            Some(Message::FileDropped(path))
        }
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            Some(Message::Key(key, modifiers))
        }
        iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
            Some(Message::Modifiers(modifiers))
        }
        _ => None,
    });
    // The tick runs from the very start: it drives the boot/recovery
    // progress display before the service exists. Faster while an operation
    // is live so the bar and status line feel alive.
    let period = if app.op.is_some() || app.service.is_none() {
        Duration::from_millis(150)
    } else {
        Duration::from_secs(1)
    };
    Subscription::batch([events, iced::time::every(period).map(|_| Message::Tick)])
}

/// The stable scrollable id for a pane's list.
fn scroll_id(pane: Pane) -> iced::widget::scrollable::Id {
    match pane {
        Pane::Mods => iced::widget::scrollable::Id::new("mods-list"),
        Pane::Plugins => iced::widget::scrollable::Id::new("plugins-list"),
    }
}

/// Case-insensitive name ordering.
fn compare_names(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
}

/// Step an index by `delta` (clamped to `0..len`).
fn step(index: usize, delta: i8, len: usize) -> usize {
    if delta < 0 {
        index.saturating_sub(1)
    } else {
        index.saturating_add(1).min(len.saturating_sub(1))
    }
}

/// Open the engine, start the download manager, and bind the loopback
/// listener so browser hand-offs reach this window. Must run on the
/// executor (the listener and stager are spawned onto it).
fn start_service(progress: &std::sync::Arc<modman_core::Progress>) -> Result<Booted, String> {
    let paths = Paths::resolve().map_err(|e| e.to_string())?;
    let engine = Engine::open_with_progress(&paths, std::sync::Arc::clone(progress))
        .map_err(|e| e.to_string())?;
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
        Message::Tick => {
            app.op = app.progress.snapshot();
            app.refresh();
        }
        Message::GamePicked(choice) => {
            app.selected_game = Some(choice.id);
            app.mod_sel.clear();
            app.plugin_sel.clear();
            app.refresh();
        }
        Message::ProfilePicked(name) => app.on_profile_picked(&name),
        Message::ProfileNameChanged(name) => app.form.profile_name = name,
        Message::CreateProfile => app.on_create_profile(),
        Message::ToggleMod(id, on) => app.on_toggle_mod(id, on),
        Message::RowClick { pane, index } => {
            let (ctrl, shift) = (app.modifiers.control(), app.modifiers.shift());
            app.on_row_click(pane, index, ctrl, shift);
        }
        Message::ClearSelection => app.active_selection().clear(),
        Message::Key(key, mods) => {
            app.modifiers = mods;
            return app.on_key(&key, mods);
        }
        Message::Modifiers(mods) => app.modifiers = mods,

        Message::EnableAll => app.on_enable_all(),
        Message::SetSelectedEnabled(on) => app.on_set_selected(on),
        Message::DeleteSelected => app.on_delete_selected(),
        Message::ReinstallSelected => return app.on_reinstall_selected(),
        Message::TogglePlugin(index) => app.on_toggle_plugin(index),
        Message::AutoSort => return app.on_auto_sort(),
        Message::MoveSelection { pane, delta } => {
            app.on_move_selection(pane, delta);
            return app.ensure_visible(pane);
        }
        Message::DragStart(pane, index) => {
            app.focus_pane = pane;
            app.drag = Some((index, index));
        }
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

/// Notification-center, wizard, sort, and scroll messages, split out of
/// [`update`].
fn update_overlay(app: &mut App, message: &Message) -> Task<Message> {
    match *message {
        Message::SortBy(key) => app.on_sort_by(key),
        Message::Scrolled(pane, offset, height) => match pane {
            Pane::Mods => app.mods_view = Some((offset, height)),
            Pane::Plugins => app.plugins_view = Some((offset, height)),
        },
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
        Message::WizardHover {
            step,
            group,
            plugin,
        } => {
            if let Some(wizard) = &mut app.wizard {
                wizard.focus = Some((step, group, plugin));
            }
        }
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
                self.refresh_heavy();
            }
            Err(error) => self.boot_error = Some(error),
        }
    }

    fn on_navigate(&mut self, screen: Screen) {
        self.screen = screen;
        self.notes_open = false;
        if screen == Screen::LoadOrder {
            self.refresh_heavy();
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
        drop(engine);
        self.apply_mod_sort();
        self.mod_sel.retain_below(self.mods.len());
        self.queue_unreviewed_fomods();
    }

    /// Recompute the plugin load order, conflicts, and health - the
    /// expensive pass (it parses plugin headers and walks staged trees), so
    /// it runs only after a mutation or a Load Order visit, never per tick.
    fn refresh_heavy(&mut self) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let Some(profile) = self.active_profile.as_ref().map(|p| p.id) else {
            self.plugins = Vec::new();
            self.health = Vec::new();
            self.conflicts = Vec::new();
            return;
        };
        let Ok(engine) = service.engine().try_lock() else {
            return;
        };
        self.plugins = engine.plugins(profile).unwrap_or_default();
        self.health = engine.health(profile).unwrap_or_default();
        let conflicts = engine
            .plan(profile)
            .map(|p| summarize_conflicts(p.conflicts(), &self.mods))
            .unwrap_or_default();
        drop(engine);
        self.conflicts = conflicts;
        self.plugin_sel.retain_below(self.plugins.len());
        self.push_health_notes();
    }

    /// Raise a red/amber notification for each health issue not seen this
    /// refresh cycle (deduped by message text so ticks don't spam).
    fn push_health_notes(&mut self) {
        let fresh: Vec<modman_core::Issue> = self
            .health
            .iter()
            .filter(|i| !self.notes.iter().any(|n| n.text == i.message))
            .cloned()
            .collect();
        for issue in fresh {
            let tone = match issue.severity {
                modman_core::Severity::Error => Tone::Error,
                modman_core::Severity::Warning | modman_core::Severity::Info => Tone::Info,
            };
            self.note(tone, issue.message);
        }
    }

    /// The selection for whichever list currently has focus.
    fn active_selection(&mut self) -> &mut Selection {
        match self.focus_pane {
            Pane::Mods => &mut self.mod_sel,
            Pane::Plugins => &mut self.plugin_sel,
        }
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
            self.refresh_heavy();
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
            self.refresh_heavy();
        }
    }

    /// The mod ids currently selected in the Mods table.
    fn selected_mod_ids(&self) -> Vec<ModId> {
        self.mod_sel
            .items
            .iter()
            .filter_map(|i| self.mods.get(*i).map(|m| m.id))
            .collect()
    }

    fn on_set_selected(&mut self, on: bool) {
        let Some(profile) = self.active_profile.clone() else {
            return;
        };
        let ids = self.selected_mod_ids();
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
            self.refresh_heavy();
        }
    }

    fn on_delete_selected(&mut self) {
        let ids = self.selected_mod_ids();
        let count = ids.len();
        let done = self.with_engine(|e| {
            for id in ids {
                e.delete_mod(id)?;
            }
            Ok(())
        });
        if done.is_some() {
            self.note(Tone::Ok, format!("{count} mods deleted"));
            self.mod_sel.clear();
            self.refresh();
        }
    }

    fn on_reinstall_selected(&mut self) -> Task<Message> {
        let ids = self.selected_mod_ids();
        if ids.is_empty() {
            return Task::none();
        }
        self.mod_sel.clear();
        self.spawn_action(Action::ReinstallMany(ids))
    }

    /// A row was clicked: plain = select only, Ctrl = toggle, Shift = range.
    fn on_row_click(&mut self, pane: Pane, index: usize, ctrl: bool, shift: bool) {
        self.focus_pane = pane;
        let sel = self.active_selection();
        if shift {
            sel.range_to(index);
        } else if ctrl {
            sel.toggle(index);
        } else {
            sel.only(index);
        }
    }

    /// Keyboard control for the visible list. Arrows move the cursor,
    /// Shift extends, Ctrl(+Shift) moves the selected plugin(s), Ctrl+A
    /// selects everything, Escape clears. Scrolls to keep the cursor
    /// visible.
    fn on_key(
        &mut self,
        key: &iced::keyboard::Key,
        mods: iced::keyboard::Modifiers,
    ) -> Task<Message> {
        use iced::keyboard::{Key, key::Named};
        // The wizard owns the keyboard; other screens have text inputs.
        let pane = match (self.wizard.is_some(), self.screen) {
            (false, Screen::Mods) => Pane::Mods,
            (false, Screen::LoadOrder) => Pane::Plugins,
            _ => return Task::none(),
        };
        self.focus_pane = pane;
        if mods.control()
            && let Key::Character(c) = key
            && c.as_str().eq_ignore_ascii_case("a")
        {
            let len = self.list_len(pane);
            let sel = self.active_selection();
            sel.items = (0..len).collect();
            sel.anchor = Some(0);
            sel.cursor = len.checked_sub(1);
            return Task::none();
        }
        let delta: i8 = match key {
            Key::Named(Named::ArrowUp) => -1,
            Key::Named(Named::ArrowDown) => 1,
            Key::Named(Named::Escape) => {
                self.active_selection().clear();
                return Task::none();
            }
            _ => return Task::none(),
        };
        if mods.control() && pane == Pane::Plugins {
            // Ctrl moves the selected plugins; Ctrl+Shift moves a block.
            self.on_move_selection(Pane::Plugins, delta);
        } else if mods.shift() {
            self.extend_cursor(delta);
        } else {
            self.move_cursor(delta);
        }
        self.ensure_visible(pane)
    }

    /// Scroll the pane so its cursor row stays inside the viewport.
    fn ensure_visible(&self, pane: Pane) -> Task<Message> {
        use iced::widget::scrollable;
        let (row_height, view, cursor) = match pane {
            Pane::Mods => (40.0_f32, self.mods_view, self.mod_sel.cursor),
            Pane::Plugins => (50.0_f32, self.plugins_view, self.plugin_sel.cursor),
        };
        let Some(cursor) = cursor else {
            return Task::none();
        };
        #[expect(clippy::cast_precision_loss, reason = "list indices are small")]
        let top = cursor as f32 * row_height;
        let target = match view {
            None => Some(top.max(0.0)),
            Some((offset, _)) if top < offset => Some(top),
            Some((offset, height)) if top + row_height > offset + height => {
                Some((top + row_height - height).max(0.0))
            }
            Some(_) => None,
        };
        match target {
            Some(y) => scrollable::scroll_to(
                scroll_id(pane),
                scrollable::AbsoluteOffset { x: 0.0, y },
            ),
            None => Task::none(),
        }
    }

    fn on_sort_by(&mut self, key: SortKey) {
        let (current, ascending) = self.mod_sort;
        self.mod_sort = if current == key {
            (key, !ascending)
        } else {
            (key, true)
        };
        self.mod_sel.clear();
        self.apply_mod_sort();
    }

    /// Sort `self.mods` by the chosen column (stable; name tiebreak).
    fn apply_mod_sort(&mut self) {
        let enabled: std::collections::HashSet<ModId> =
            self.order.iter().map(|m| m.id).collect();
        let (key, ascending) = self.mod_sort;
        self.mods.sort_by(|a, b| {
            let ord = match key {
                SortKey::Installed => a.id.cmp(&b.id),
                SortKey::Name => compare_names(&a.name, &b.name),
                SortKey::Enabled => enabled
                    .contains(&b.id)
                    .cmp(&enabled.contains(&a.id))
                    .then_with(|| compare_names(&a.name, &b.name)),
                SortKey::Version => a
                    .version
                    .cmp(&b.version)
                    .then_with(|| compare_names(&a.name, &b.name)),
                SortKey::Source => a
                    .source
                    .cmp(&b.source)
                    .then_with(|| compare_names(&a.name, &b.name)),
            };
            if ascending { ord } else { ord.reverse() }
        });
    }

    fn list_len(&self, pane: Pane) -> usize {
        match pane {
            Pane::Mods => self.mods.len(),
            Pane::Plugins => self.plugins.len(),
        }
    }

    fn move_cursor(&mut self, delta: i8) {
        let len = self.list_len(self.focus_pane);
        if len == 0 {
            return;
        }
        let sel = self.active_selection();
        let next = step(sel.cursor.unwrap_or(0), delta, len);
        sel.only(next);
    }

    fn extend_cursor(&mut self, delta: i8) {
        let len = self.list_len(self.focus_pane);
        if len == 0 {
            return;
        }
        let sel = self.active_selection();
        let next = step(sel.cursor.unwrap_or(0), delta, len);
        sel.range_to(next);
    }

    fn on_toggle_plugin(&mut self, index: usize) {
        let Some(plugin) = self.plugins.get_mut(index) else {
            return;
        };
        plugin.enabled = !plugin.enabled;
        self.commit_plugin_order();
    }

    fn on_auto_sort(&mut self) -> Task<Message> {
        match self.active_profile.clone() {
            Some(profile) => self.spawn_action(Action::AutoSortPlugins(profile.id)),
            None => Task::none(),
        }
    }

    /// Move the focused list's current selection by `delta`, keeping the
    /// moved rows selected and the cursor on them (so the pointer stays put
    /// relative to the row for arrow-button clicks).
    fn on_move_selection(&mut self, pane: Pane, delta: i8) {
        self.focus_pane = pane;
        match pane {
            Pane::Mods => {} // mod order is derived from the plugin order now
            Pane::Plugins => self.move_plugins(delta),
        }
    }

    fn move_plugins(&mut self, delta: i8) {
        let len = self.plugins.len();
        let mut indices: Vec<usize> = self.plugin_sel.items.iter().copied().collect();
        if indices.is_empty()
            && let Some(c) = self.plugin_sel.cursor
        {
            indices.push(c);
        }
        if indices.is_empty() || len == 0 {
            return;
        }
        // Moving down processes bottom-first; up processes top-first.
        if delta > 0 {
            indices.sort_by(|a, b| b.cmp(a));
        } else {
            indices.sort_unstable();
        }
        // Bail if a block edge is already at the boundary.
        let at_edge = indices.iter().any(|i| {
            (delta < 0 && *i == 0) || (delta > 0 && *i >= len.saturating_sub(1))
        });
        if at_edge {
            return;
        }
        let mut moved = std::collections::BTreeSet::new();
        for i in indices {
            let target = if delta < 0 {
                i.saturating_sub(1)
            } else {
                i.saturating_add(1)
            };
            self.plugins.swap(i, target);
            moved.insert(target);
        }
        let cursor = self.plugin_sel.cursor.map(|c| step(c, delta, len));
        self.plugin_sel.items = moved;
        self.plugin_sel.cursor = cursor;
        self.commit_plugin_order();
    }

    fn on_drag_over(&mut self, index: usize) {
        if let Some((from, _)) = self.drag
            && index < self.plugins.len()
        {
            self.drag = Some((from, index));
        }
    }

    fn on_drag_end(&mut self) {
        let Some((from, to)) = self.drag.take() else {
            return;
        };
        if from != to && from < self.plugins.len() && to < self.plugins.len() {
            let moved = self.plugins.remove(from);
            self.plugins.insert(to, moved);
            self.plugin_sel.only(to);
            self.commit_plugin_order();
        }
    }

    fn commit_plugin_order(&mut self) {
        let Some(profile) = self.active_profile.clone() else {
            return;
        };
        let order: Vec<(String, bool)> = self
            .plugins
            .iter()
            .map(|p| (p.name.clone(), p.enabled))
            .collect();
        if self
            .with_engine(|e| e.set_plugin_order(profile.id, &order))
            .is_some()
        {
            // Re-pull so master-tier partitioning and health re-evaluate,
            // but keep the user's cursor.
            let cursor = self.plugin_sel.cursor;
            self.refresh();
            self.plugin_sel.cursor = cursor.filter(|c| *c < self.plugins.len());
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
            self.refresh_heavy();
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
                    staged_path: m.staged_path,
                    installer,
                    selections,
                    step: 0,
                    focus: None,
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
        wizard.focus = Some((step, group, plugin));
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
            fomod::GroupKind::Any => {
                if !sel.remove(&plugin) {
                    sel.insert(plugin);
                }
            }
            fomod::GroupKind::AtLeastOne => {
                // The group cannot become empty (Vortex parity).
                if sel.contains(&plugin) {
                    if sel.len() > 1 {
                        sel.remove(&plugin);
                    }
                } else {
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
                let fresh = engine.reinstall_mod(*id).map_err(|e| e.to_string())?;
                modman_service::fomod_pass(&engine, &fresh).map_err(|e| format!("{e:#}"))?;
                if let Ok(profile) = engine.active_profile(fresh.game_id) {
                    let _ = engine.set_enabled(profile.id, fresh.id, true);
                }
                done = done.saturating_add(1);
            }
            Ok(format!("{done} mods reinstalled"))
        }
        Action::AutoSortPlugins(profile) => {
            let plugins = engine.auto_sort_plugins(*profile).map_err(|e| e.to_string())?;
            Ok(format!("Load order sorted · {} plugins", plugins.len()))
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
