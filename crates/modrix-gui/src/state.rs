// SPDX-License-Identifier: GPL-2.0-only
//! The whole GUI state. Engine-derived data arrives from the worker thread
//! as snapshots (see [`crate::bus`]); everything here is plain data the view
//! reads, so a frame never touches the disk or the engine lock.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use modrix_core::plugins::GamePlugin;
use modrix_core::{
    ExternalMod, Game, GameCapabilities, GameId, Issue, Mod, ModConflict, ModId, Paths, Profile,
    ProgressSnapshot,
};
use modrix_download::{DownloadId, DownloadStatus};
use modrix_plugin::fomod;
use modrix_service::{InstallOutcome, Service};
use rune::TextEdit;

use crate::art::ArtSet;
use crate::bus::{Bus, Event, Job};
use crate::prefs::Appearance;

/// Notifications kept before the oldest are dropped.
pub const MAX_NOTES: usize = 200;
/// Seconds an event notification lives before it expires on its own.
pub const NOTE_TTL: f64 = 300.0;
/// Seconds a toast stays on screen.
pub const TOAST_TTL: f64 = 4.5;

/// Which main view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    /// Overview of the selected game.
    Dashboard,
    /// The mod library.
    Mods,
    /// Plugin load order.
    LoadOrder,
    /// Conflict rules and per-file pins.
    Conflicts,
    /// Browser hand-off downloads.
    Downloads,
    /// Registered, detected and installable games.
    Games,
    /// Appearance, paths, pairing, plugins, updates.
    Settings,
}

impl Screen {
    /// Every screen in navigation order.
    pub const ALL: [Screen; 7] = [
        Screen::Dashboard,
        Screen::Mods,
        Screen::LoadOrder,
        Screen::Conflicts,
        Screen::Downloads,
        Screen::Games,
        Screen::Settings,
    ];

    /// The navigation label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Screen::Dashboard => "Overview",
            Screen::Mods => "Mods",
            Screen::LoadOrder => "Load order",
            Screen::Conflicts => "Conflicts",
            Screen::Downloads => "Downloads",
            Screen::Games => "Games",
            Screen::Settings => "Settings",
        }
    }

    /// Parse the `MODRIX_GUI_SCREEN` development override.
    #[must_use]
    pub fn from_env(value: &str) -> Option<Screen> {
        match value {
            "dashboard" => Some(Screen::Dashboard),
            "mods" => Some(Screen::Mods),
            "loadorder" => Some(Screen::LoadOrder),
            "conflicts" => Some(Screen::Conflicts),
            "downloads" => Some(Screen::Downloads),
            "games" => Some(Screen::Games),
            "settings" => Some(Screen::Settings),
            _ => None,
        }
    }
}

/// A sortable column of the Mods table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// Install time, the default.
    Installed,
    /// Alphabetical.
    Name,
    /// Enabled first.
    Enabled,
    /// Version string.
    Version,
    /// Source (nexus, local).
    Source,
}

/// Which list owns the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// The Mods table.
    Mods,
    /// The plugin load order.
    Plugins,
}

/// A multi-selection: selected indices, a fixed anchor for Shift ranges and
/// a moving cursor.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Selected indices.
    pub items: BTreeSet<usize>,
    /// The fixed end of a Shift range.
    pub anchor: Option<usize>,
    /// The moving end.
    pub cursor: Option<usize>,
}

impl Selection {
    /// Clear everything.
    pub fn clear(&mut self) {
        self.items.clear();
        self.anchor = None;
        self.cursor = None;
    }

    /// Select exactly `index`.
    pub fn only(&mut self, index: usize) {
        self.items.clear();
        self.items.insert(index);
        self.anchor = Some(index);
        self.cursor = Some(index);
    }

    /// Toggle `index`; it becomes the new anchor.
    pub fn toggle(&mut self, index: usize) {
        if !self.items.remove(&index) {
            self.items.insert(index);
        }
        self.anchor = Some(index);
        self.cursor = Some(index);
    }

    /// Extend from the anchor to `index`.
    pub fn range_to(&mut self, index: usize) {
        let anchor = self.anchor.or(self.cursor).unwrap_or(index);
        let (lo, hi) = (anchor.min(index), anchor.max(index));
        self.items = (lo..=hi).collect();
        self.anchor = Some(anchor);
        self.cursor = Some(index);
    }

    /// Select `0..len`.
    pub fn all(&mut self, len: usize) {
        self.items = (0..len).collect();
        self.anchor = (len > 0).then_some(0);
        self.cursor = len.checked_sub(1);
    }

    /// Keep only indices below `len` (after the list shrinks).
    pub fn retain_below(&mut self, len: usize) {
        self.items.retain(|i| *i < len);
        self.anchor = self.anchor.filter(|a| *a < len);
        self.cursor = self.cursor.filter(|c| *c < len);
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

/// One event in the notification center.
#[derive(Debug, Clone)]
pub struct Note {
    /// Stable id (drives toast keys).
    pub id: u64,
    /// Severity.
    pub tone: Tone,
    /// The message.
    pub text: String,
    /// Frame-clock seconds when it happened.
    pub at: f64,
}

/// A selectable game definition (built-in, user, or installed plugin).
#[derive(Debug, Clone)]
pub struct DefChoice {
    /// Stable id (`skyrimse`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// The raw `game.toml` text.
    pub toml: String,
}

/// A supported game found installed on disk.
#[derive(Debug, Clone)]
pub struct Detected {
    /// The definition that matched.
    pub def: DefChoice,
    /// The install directory.
    pub install: PathBuf,
}

/// One community plugin from the registry index.
#[derive(Debug, Clone)]
pub struct RegistryChoice {
    /// Plugin id (= game id).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Registry version.
    pub version: String,
    /// The full index entry (needed to install).
    pub entry: modrix_registry::IndexEntry,
}

/// An installed registry plugin.
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    /// Plugin id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Installed version.
    pub version: String,
}

/// A duplicate-install prompt.
#[derive(Debug, Clone)]
pub struct DupPrompt {
    /// The archive the user tried to install.
    pub path: PathBuf,
    /// The installed mod with the same content.
    pub existing: ModId,
    /// Its display name.
    pub existing_name: String,
}

/// The loopback pairing the browser extension needs.
#[derive(Debug, Clone)]
pub struct Link {
    /// Bound port.
    pub port: u16,
    /// Per-session token.
    pub token: String,
}

/// The in-app updater's state.
#[derive(Debug, Clone, Default)]
pub enum UpdateState {
    /// Up to date, offline, or not yet checked.
    #[default]
    Idle,
    /// A check is running.
    Checking,
    /// A newer release is offered.
    Available(modrix_update::UpdateInfo),
    /// Its installer is downloading or launching.
    Installing(modrix_update::UpdateInfo),
}

/// The FOMOD options wizard.
#[derive(Debug, Clone)]
pub struct Wizard {
    /// The mod being configured.
    pub mod_id: ModId,
    /// Its display name.
    pub mod_name: String,
    /// Its staged tree.
    pub staged_path: PathBuf,
    /// The parsed installer.
    pub installer: fomod::Installer,
    /// Current selections.
    pub selections: fomod::Selections,
    /// Position within the visible steps.
    pub step: usize,
    /// The option last touched (step, group, plugin): drives the preview.
    pub focus: Option<(usize, usize, usize)>,
    /// Files present in the install (fileDependency conditions).
    pub present: fomod::Present,
}

impl Wizard {
    /// Indices of steps visible under the current selections.
    #[must_use]
    pub fn visible_steps(&self) -> Vec<usize> {
        let flags = fomod::flags_of(&self.installer, &self.selections, &self.present);
        self.installer
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.visible
                    .as_ref()
                    .is_none_or(|d| d.eval(&flags, &self.present))
            })
            .map(|(i, _)| i)
            .collect()
    }
}

/// Text fields, kept apart from engine data so a refresh never clobbers typing.
#[derive(Debug, Default)]
pub struct Forms {
    /// Mods table filter.
    pub search: TextEdit,
    /// Picked definition id for manual registration.
    pub def_id: Option<String>,
    /// Explicit `game.toml` path (overrides the pick).
    pub def_path: TextEdit,
    /// Install directory for manual registration.
    pub install_path: TextEdit,
    /// New profile name.
    pub profile_name: TextEdit,
}

/// A load-order drag in progress.
#[derive(Debug, Clone, Copy)]
pub struct PluginDrag {
    /// The row being dragged.
    pub from: usize,
    /// Pointer travel in points.
    pub dy: f32,
}

/// Engine-derived data for the selected game, replaced wholesale per snapshot.
#[derive(Debug, Clone, Default)]
pub struct Library {
    /// Registered games.
    pub games: Vec<Game>,
    /// The game being managed.
    pub game: Option<GameId>,
    /// Its profiles.
    pub profiles: Vec<Profile>,
    /// Its active profile.
    pub profile: Option<Profile>,
    /// Every staged mod, sorted per [`App::mod_sort`].
    pub mods: Vec<Mod>,
    /// Enabled mods in load order.
    pub order: Vec<Mod>,
    /// Enabled mod ids (fast lookup).
    pub enabled: HashSet<ModId>,
    /// What the game supports.
    pub capabilities: Option<GameCapabilities>,
    /// Plugin load order (heavy pass).
    pub plugins: Vec<GamePlugin>,
    /// Setup health issues, worst first (heavy pass).
    pub health: Vec<Issue>,
    /// Pairwise conflicts (heavy pass).
    pub conflicts: Vec<ModConflict>,
    /// Mods in the game dir Modrix did not deploy (heavy pass).
    pub external: Vec<ExternalMod>,
    /// Plugins each mod ships (heavy pass).
    pub plugin_counts: HashMap<ModId, usize>,
}

/// Which popup surface is open (at most one at a time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    /// The sidebar game switcher.
    GameMenu,
    /// The toolbar profile picker.
    ProfileMenu,
    /// Deploy split-button menu.
    DeployMenu,
    /// The notification center.
    Notes,
    /// The manual-registration definition picker.
    DefPicker,
    /// A per-file pin picker in Conflicts (index into the expanded pair's files).
    PinPicker(usize),
}

/// The whole GUI state.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags of the one GUI state, not a hidden enum"
)]
pub struct App {
    /// Background events (snapshots, finished jobs, art, boot).
    pub inbox: Receiver<Event>,
    /// Where workers post events (holds the sender half).
    pub bus: Bus,
    /// Engine worker queue, once booted.
    pub jobs: Option<std::sync::mpsc::Sender<(u64, Job)>>,
    /// The async runtime for network work (None in headless snaps).
    pub rt: Option<tokio::runtime::Handle>,
    /// The embedded hand-off service.
    pub service: Option<Service>,
    /// Extension pairing when this process is primary.
    pub link: Option<Link>,
    /// True when another instance holds the port.
    pub already_running: bool,
    /// Fatal boot failure.
    pub boot_error: Option<String>,
    /// Boot finished.
    pub booted: bool,
    /// The engine's progress sink.
    pub progress: Arc<modrix_core::Progress>,
    /// Latest progress snapshot.
    pub op: Option<ProgressSnapshot>,
    /// Slow actions in flight.
    pub busy: u32,
    /// Current view.
    pub screen: Screen,
    /// Engine data.
    pub lib: Library,
    /// The game the user asked for (snapshots for any other game are stale).
    pub want_game: Option<GameId>,
    /// Sequence number of the last job queued.
    pub job_seq: u64,
    /// A light refresh is already queued.
    pub refresh_queued: bool,
    /// Frame-clock time of the last refresh request.
    pub last_refresh: f64,
    /// Latest frame clock seen by `poll`.
    pub now: f64,
    /// Download snapshots, newest first.
    pub downloads: Vec<DownloadStatus>,
    /// Install phase of finished downloads.
    pub outcomes: HashMap<DownloadId, InstallOutcome>,
    /// Outcomes already notified.
    pub outcome_seen: HashSet<DownloadId>,
    /// Mods table selection (indices into the filtered view).
    pub mod_sel: Selection,
    /// Load-order selection.
    pub plugin_sel: Selection,
    /// Which list owns the keyboard.
    pub focus_pane: Pane,
    /// Mods sort (column, ascending).
    pub mod_sort: (SortKey, bool),
    /// Load-order drag.
    pub drag: Option<PluginDrag>,
    /// Expanded conflict pair.
    pub expanded_conflict: Option<(ModId, ModId)>,
    /// Artwork per game.
    pub art: HashMap<GameId, ArtSet>,
    /// Games whose art was requested.
    pub art_requested: HashSet<GameId>,
    /// Notification center, newest first.
    pub notes: Vec<Note>,
    /// Next note id.
    pub note_seq: u64,
    /// Unseen notifications.
    pub unread: usize,
    /// Open popup, if any.
    pub popup: Option<Popup>,
    /// Updater state.
    pub update: UpdateState,
    /// Pending duplicate prompts (the first is shown).
    pub dup_queue: Vec<DupPrompt>,
    /// The open wizard.
    pub wizard: Option<Wizard>,
    /// FOMOD mods waiting for their first review.
    pub wizard_queue: Vec<ModId>,
    /// Mods already offered a wizard.
    pub prompted: HashSet<ModId>,
    /// Files are being dragged over the window.
    pub drop_hover: bool,
    /// Selectable definitions.
    pub defs: Vec<DefChoice>,
    /// Supported games found on disk.
    pub detected: Vec<Detected>,
    /// Registry plugins.
    pub registry: Vec<RegistryChoice>,
    /// Registry index is loading.
    pub registry_loading: bool,
    /// Installed registry plugins.
    pub installed_plugins: Vec<InstalledPlugin>,
    /// Text fields.
    pub form: Forms,
    /// Resolved on-disk locations.
    pub paths: Option<Paths>,
    /// Appearance preference.
    pub appearance: Appearance,
    /// Confirm before deleting the selected mods.
    pub confirm_delete: bool,
    /// Bumped whenever `lib.mods` is replaced or reordered.
    pub lib_gen: u64,
    /// The filtered row list, keyed by search text and `lib_gen`: the table
    /// asks for it once per drawn row, so it must not refilter each time.
    visible_cache: std::cell::RefCell<Option<VisibleCache>>,
}

/// One computed filter result for the Mods table.
#[derive(Debug)]
struct VisibleCache {
    needle: String,
    lib_gen: u64,
    rows: std::rc::Rc<[usize]>,
}

impl App {
    /// Fresh state with an unbooted service.
    #[must_use]
    pub fn new(rt: Option<tokio::runtime::Handle>) -> App {
        let (bus, inbox) = Bus::channel();
        App {
            inbox,
            bus,
            jobs: None,
            rt,
            service: None,
            link: None,
            already_running: false,
            boot_error: None,
            booted: false,
            progress: Arc::new(modrix_core::Progress::default()),
            op: None,
            busy: 0,
            screen: initial_screen(),
            lib: Library::default(),
            want_game: None,
            job_seq: 0,
            refresh_queued: false,
            last_refresh: 0.0,
            now: 0.0,
            downloads: Vec::new(),
            outcomes: HashMap::new(),
            outcome_seen: HashSet::new(),
            mod_sel: Selection::default(),
            plugin_sel: Selection::default(),
            focus_pane: Pane::Mods,
            mod_sort: (SortKey::Installed, false),
            drag: None,
            expanded_conflict: None,
            art: HashMap::new(),
            art_requested: HashSet::new(),
            notes: Vec::new(),
            note_seq: 0,
            unread: 0,
            popup: None,
            update: UpdateState::Idle,
            dup_queue: Vec::new(),
            wizard: None,
            wizard_queue: Vec::new(),
            prompted: HashSet::new(),
            drop_hover: false,
            defs: Vec::new(),
            detected: Vec::new(),
            registry: Vec::new(),
            registry_loading: false,
            installed_plugins: Vec::new(),
            form: Forms::default(),
            paths: None,
            appearance: crate::prefs::load(),
            confirm_delete: false,
            lib_gen: 0,
            visible_cache: std::cell::RefCell::new(None),
        }
    }

    /// The selected game record.
    #[must_use]
    pub fn game(&self) -> Option<&Game> {
        let id = self.lib.game?;
        self.lib.games.iter().find(|g| g.id == id)
    }

    /// The selected game's artwork.
    #[must_use]
    pub fn game_art(&self) -> Option<&ArtSet> {
        self.lib.game.and_then(|id| self.art.get(&id))
    }

    /// A mod's display name.
    #[must_use]
    pub fn mod_name(&self, id: ModId) -> String {
        self.lib
            .mods
            .iter()
            .find(|m| m.id == id)
            .map_or_else(|| id.to_string(), |m| m.name.clone())
    }

    /// Whether a screen applies to the selected game.
    #[must_use]
    pub fn screen_visible(&self, screen: Screen) -> bool {
        let load_order = self.lib.capabilities.is_none_or(|c| c.load_order);
        match screen {
            Screen::LoadOrder => load_order,
            Screen::Conflicts => {
                load_order
                    || !self.lib.conflicts.is_empty()
                    || self.lib.health.iter().any(|i| i.blocking)
            }
            _ => true,
        }
    }

    /// Issues that block a deploy.
    pub fn blockers(&self) -> impl Iterator<Item = &Issue> {
        self.lib.health.iter().filter(|i| i.blocking)
    }

    /// Indices into `lib.mods` that pass the search filter, in display order.
    #[must_use]
    pub fn visible_mods(&self) -> std::rc::Rc<[usize]> {
        let needle = self.form.search.text.trim();
        if let Ok(cache) = self.visible_cache.try_borrow()
            && let Some(hit) = cache.as_ref()
            && hit.lib_gen == self.lib_gen
            && hit.needle == needle
        {
            return std::rc::Rc::clone(&hit.rows);
        }
        let lower = needle.to_lowercase();
        let rows: std::rc::Rc<[usize]> = self
            .lib
            .mods
            .iter()
            .enumerate()
            .filter(|(_, m)| lower.is_empty() || m.name.to_lowercase().contains(&lower))
            .map(|(i, _)| i)
            .collect();
        if let Ok(mut cache) = self.visible_cache.try_borrow_mut() {
            *cache = Some(VisibleCache {
                needle: needle.to_owned(),
                lib_gen: self.lib_gen,
                rows: std::rc::Rc::clone(&rows),
            });
        }
        rows
    }

    /// The mod ids currently selected in the Mods table.
    #[must_use]
    pub fn selected_mod_ids(&self) -> Vec<ModId> {
        let visible = self.visible_mods();
        self.mod_sel
            .items
            .iter()
            .filter_map(|i| visible.get(*i))
            .filter_map(|i| self.lib.mods.get(*i).map(|m| m.id))
            .collect()
    }

    /// Unresolved conflict pairs.
    #[must_use]
    pub fn unresolved_conflicts(&self) -> usize {
        self.lib.conflicts.iter().filter(|c| !c.resolved()).count()
    }

    /// Downloads still transferring or queued.
    #[must_use]
    pub fn active_downloads(&self) -> usize {
        use modrix_download::DownloadState;
        self.downloads
            .iter()
            .filter(|d| matches!(d.state, DownloadState::Active | DownloadState::Queued))
            .count()
    }
}

/// The screen to open on (`MODRIX_GUI_SCREEN` overrides for development).
fn initial_screen() -> Screen {
    std::env::var("MODRIX_GUI_SCREEN")
        .ok()
        .and_then(|v| Screen::from_env(&v))
        .unwrap_or(Screen::Dashboard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_range_keeps_anchor() {
        let mut sel = Selection::default();
        sel.only(3);
        sel.range_to(6);
        assert_eq!(sel.items.len(), 4);
        sel.range_to(1);
        assert_eq!(sel.items.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(sel.anchor, Some(3));
    }

    #[test]
    fn selection_toggle_and_retain() {
        let mut sel = Selection::default();
        sel.toggle(2);
        sel.toggle(5);
        sel.toggle(2);
        assert_eq!(sel.items.iter().copied().collect::<Vec<_>>(), vec![5]);
        sel.retain_below(4);
        assert!(sel.items.is_empty());
        assert_eq!(sel.cursor, Some(2), "a cursor still inside the list stays");
        sel.retain_below(2);
        assert_eq!(sel.cursor, None);
    }

    #[test]
    fn selection_all_handles_empty() {
        let mut sel = Selection::default();
        sel.all(0);
        assert!(sel.items.is_empty());
        assert_eq!(sel.cursor, None);
        sel.all(3);
        assert_eq!(sel.cursor, Some(2));
    }
}
