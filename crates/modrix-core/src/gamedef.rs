// SPDX-License-Identifier: GPL-2.0-only
//! Declarative game definitions (`game.toml`).
//!
//! Tier 1 of the plugin system (see `docs/ARCHITECTURE.md` §5.1): most games
//! are pure data - an id, a name, where mods deploy, and (since `api_version` 2)
//! the game's *capabilities*: its load-order strategy, which directories count
//! as mod content when normalizing archives, how to find hand-installed mods,
//! and game-specific health checks. Core dispatches on this data and carries
//! **no game-specific logic of its own**; anything a definition does not
//! declare simply does not run. The Lua tier (`game.lua`) adds logic on top in
//! `modrix-plugin`; this stays a small, dependency-light parser so the engine
//! can drive a data-defined game without linking the Lua host.

use std::path::Path;

use crate::error::{Error, Result};

/// The `mod_loader.kind` that means "Modrix supplies this loader itself" - there
/// is nothing to download and no third-party project to track.
pub const NATIVE_LOADER_KIND: &str = "modrix-sons";

/// The oldest `game.toml` schema version this build understands.
pub const MIN_API_VERSION: u32 = 1;
/// The newest `game.toml` schema version this build understands.
///
/// v3 adds `[launch]`, `[mod_loader]` and `park_root_binaries`: a definition can
/// now declare how to start the game, which mod loader it needs, and that its
/// mod root must not have top-level binaries parked away from it. Every new
/// field is `#[serde(default)]`, so v1/v2 definitions parse unchanged.
pub const MAX_API_VERSION: u32 = 3;

/// A parsed, validated game definition.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GameDef {
    /// Definition schema version; must be within
    /// [`MIN_API_VERSION`]..=[`MAX_API_VERSION`].
    pub api_version: u32,
    /// Stable identifier (e.g. `skyrimse`), used as the plugin id.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Steam AppID, when the game is on Steam.
    #[serde(default)]
    pub steam_appid: Option<i64>,
    /// The Nexus Mods game domain (e.g. `skyrimspecialedition`), used to route
    /// incoming `nxm://` links to this game.
    #[serde(default)]
    pub nexus_domain: Option<String>,
    /// GOG product id (the numeric `HKLM\…\GOG.com\Games\<id>` subkey), when
    /// the game is on GOG. Used by the `gog` install probe.
    #[serde(default)]
    pub gog_id: Option<String>,
    /// Epic Games Store `AppName` (the catalog codename in a `*.item`
    /// manifest), when the game is on Epic. Used by the `epic` install probe.
    #[serde(default)]
    pub epic_id: Option<String>,
    /// Xbox / Microsoft Store package identity name (the appid segment of a
    /// package family name), when the game is on the Microsoft Store. Used by
    /// the `xbox` install probe.
    #[serde(default)]
    pub xbox_id: Option<String>,
    /// Origin / EA offer id (the `id=` field of a `.mfst` manifest), when the
    /// game is on EA/Origin. Used by the `origin` install probe.
    #[serde(default)]
    pub origin_id: Option<String>,
    /// Ubisoft Connect / Uplay install id (the `HKLM\…\Ubisoft\Launcher\
    /// Installs\<id>` subkey), when the game is on Uplay. Used by the `uplay`
    /// install probe.
    #[serde(default)]
    pub uplay_id: Option<String>,
    /// Where mods deploy, relative to the install path (e.g. `Data`). May be
    /// empty to deploy at the install root. Interpreted relative to
    /// [`GameDef::mod_base`].
    #[serde(default)]
    pub mod_root: String,
    /// What `mod_root` is anchored to when resolving the deploy target:
    /// `install` (default), `documents`, `local_appdata`, or `roaming_appdata`.
    /// Non-install bases deploy into the user's profile folder rather than the
    /// game directory - some games (The Sims, Dragon Age, Baldur's Gate 3,
    /// Factorio, …) keep mods there. On Linux/macOS the base lives inside the
    /// game's Proton prefix; on Windows it is the real user folder. Absent =
    /// `install` (the game directory), byte-identical to pre-`mod_base` behavior.
    #[serde(default)]
    pub mod_base: Option<String>,
    /// Deploy strategy hint (`link` or `copy`); the applier always falls back
    /// link → copy, so this only forces copy when a game cannot tolerate links.
    #[serde(default)]
    pub deploy: Option<String>,
    /// Ordered install-probe strategies, tried in order until one resolves an
    /// existing directory: `steam`, `gog`, `epic`, `xbox`, `origin`, `uplay`,
    /// `registry` (uses `registry_keys`), `path-hint`. Unknown/unimplemented
    /// strategies are skipped cleanly so a later one still gets its turn.
    #[serde(default)]
    pub install_probe: Vec<String>,
    /// Windows registry values that name an install directory, consulted by the
    /// `registry` probe in order. Generalizes the vendor keys Bethesda, CD
    /// Projekt, Maxis, etc. write (e.g. `Installed Path`). Windows-only; ignored
    /// on other platforms. Empty = the `registry` probe finds nothing.
    #[serde(default)]
    pub registry_keys: Vec<RegistryKeyDef>,
    /// Files (relative to the install directory) that must all exist for a
    /// detected or manually-entered directory to be accepted as this game.
    /// Disambiguates when a probe returns a plausible-but-wrong directory
    /// (Vortex's `requiredFiles`). Empty = accept any directory a probe yields.
    #[serde(default)]
    pub required_files: Vec<String>,
    /// The game's load-order strategy, when it has one. v1 defs use a bare
    /// name (`"plugins_txt"`, parameters from a legacy preset); v2 defs use a
    /// table with explicit parameters. Absent = the game has no load-order
    /// file (e.g. BepInEx orders by declared dependencies).
    #[serde(default)]
    pub load_order: Option<LoadOrderDef>,
    /// Directory names that ARE mod content at the root of an archive, never
    /// a packaging wrapper to hoist away (e.g. Bethesda's `meshes`,
    /// `textures`; BepInEx's `bepinex`). Compared case-insensitively. Empty =
    /// use the conservative built-in default list (v1 compatibility).
    #[serde(default)]
    pub content_dirs: Vec<String>,
    /// Lowercased file names the base game ships in its deploy root (vanilla
    /// plugins, DLC). A trailing `*` matches a prefix (`cc*` for Creation
    /// Club). Used to classify unowned files as vanilla rather than foreign.
    #[serde(default)]
    pub base_files: Vec<String>,
    /// How to find hand-installed (external) mods in the game directory.
    /// Empty on a v1 def = the built-in default detectors; empty on a v2 def
    /// = the game declares no external-mod layout and nothing is scanned.
    #[serde(default)]
    pub external_scan: Vec<ExternalScanDef>,
    /// Game-specific health checks (script-extender loader, known mod
    /// pairings). Absent = none run.
    #[serde(default)]
    pub health: Option<HealthDef>,
    /// Whether a top-level `.exe`/`.dll` in a staged archive is a *loader*
    /// that belongs beside the game binary (parked into `.root/`) rather than
    /// the mod payload itself.
    ///
    /// Absent = infer from the shape of `mod_root`, which is right for
    /// Bethesda (`Data` sits beside the binary, so a loose DLL is SKSE) and
    /// for nested roots (`BepInEx/plugins` IS the plugin container). It is
    /// WRONG for a loader whose mod directory is a single component holding
    /// bare payload DLLs - Sons of the Forest deploys `Mods/<Name>.dll`, and
    /// inferring would park every mod to the install root where nothing loads
    /// it. Such a game sets this to `false` explicitly.
    #[serde(default)]
    pub park_root_binaries: Option<bool>,
    /// How to start the game (and where it writes its log). Absent = Modrix
    /// cannot launch this game and frontends hide the control.
    #[serde(default)]
    pub launch: Option<LaunchDef>,
    /// The mod loader this game needs before any mod will load. Absent =
    /// mods drop straight into the game and no loader is managed.
    ///
    /// Named `mod_loader` rather than `loader` because `loader` is already a
    /// nested key under `[health]`, and the misplaced-key guard below relies
    /// on top-level names never being valid inside a table.
    #[serde(default)]
    pub mod_loader: Option<LoaderDef>,
    /// The community mod store this game has, when one exists. Absent = mods
    /// arrive only through the browser hand-off and frontends hide the
    /// browse-and-install screen entirely.
    #[serde(default)]
    pub store: Option<StoreDef>,
}

/// A community mod store a game's mods can be browsed and installed from.
///
/// Deliberately just an identity plus an origin: `kind` selects the client that
/// knows the site's JSON shape (each store's API differs, so there is no useful
/// generic schema to declare here), and `base_url` is the only thing that ever
/// varies per deployment. Core stores and validates this; it does not speak
/// HTTP - the client lives in `modrix-service`, keeping core free of any
/// site-specific code.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoreDef {
    /// Which store client handles this API (e.g. `sotf-mods`). An unknown kind
    /// is not an error here - a newer definition may name a client this build
    /// does not have yet, and the frontend simply finds no handler.
    pub kind: String,
    /// The API origin, with no trailing slash (e.g. `https://api.sotf-mods.com`).
    /// Must be `https://` - a store install runs unattended and ends in code
    /// deployed into the game directory, so the transport is never optional.
    pub base_url: String,
}

/// How to start a game, and where to read its log afterwards.
///
/// Deliberately fire-and-forget: Modrix starts the game and stops caring.
/// Supervising a game process (PID tracking, restart watchdogs) is a different
/// problem with its own failure modes, and nothing here needs it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LaunchDef {
    /// `"steam"` (via `steam://rungameid/<steam_appid>`) or `"exe"` (run
    /// `exe` from the install directory).
    pub mode: String,
    /// For `mode = "exe"`: the executable, relative to the install directory.
    #[serde(default)]
    pub exe: Option<String>,
    /// Extra command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables the game needs. On Linux/macOS a Windows-only
    /// title runs under Proton/Wine, where a loader's proxy DLL is inert
    /// unless `WINEDLLOVERRIDES` names it - that is what this carries.
    /// Ignored on Windows, where the proxy loads natively.
    ///
    /// A `BTreeMap` rather than a `HashMap` so the rendered launch options are
    /// byte-stable across runs; Modrix compares them to decide whether a
    /// Steam config actually needs rewriting.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// The game/loader log, relative to the install directory.
    #[serde(default)]
    pub log: Option<String>,
    /// Process image name used to stop a running instance before relaunching
    /// (e.g. `SonsOfTheForest.exe`). Absent = never kill anything.
    #[serde(default)]
    pub process: Option<String>,
}

/// A mod loader that must be installed into the game before mods will load.
///
/// This is *not* a mod: it is infrastructure that lives beside the game binary
/// and is deliberately kept out of the deployment manifest, so `verify` and
/// `undeploy` never treat it as owned content.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoaderDef {
    /// Loader identity. `modrix-sons` is Modrix's own native loader (built in,
    /// nothing to download); anything else names a third-party loader that
    /// `source` says how to fetch.
    pub kind: String,
    /// The loader's own directory, relative to the install dir (`_Modrix`).
    pub install_root: String,
    /// A file that exists if and only if the loader is installed, relative to
    /// the install dir. This is the installed/not-installed test.
    pub sentinel: String,
    /// The proxy DLL the loader injects through, relative to the install dir
    /// (`version.dll`). Games have exactly one such slot, which is why two
    /// loaders can never coexist.
    #[serde(default)]
    pub proxy: Option<String>,
    /// Where generated interop assemblies land, relative to the install dir.
    /// IL2CPP games only; mods compile against these.
    #[serde(default)]
    pub interop_dir: Option<String>,
    /// A file holding the installed version string, relative to the install
    /// dir. Absent = version is unknown but installation is still detectable.
    #[serde(default)]
    pub version_file: Option<String>,
    /// Paths (relative to the install dir) whose presence means a *rival*
    /// loader already owns the proxy slot. Installing over one of these
    /// bricks both, so Modrix refuses rather than guessing.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// Where to fetch a third-party loader. Absent = `kind` is native and
    /// Modrix supplies it.
    #[serde(default)]
    pub source: Option<LoaderSourceDef>,
}

/// Where a third-party loader's release archive comes from.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoaderSourceDef {
    /// `owner/repo` whose latest GitHub release carries the loader.
    #[serde(default)]
    pub github_repo: Option<String>,
    /// Release asset name, matched case-insensitively (asset casing drifts
    /// between releases, so this is never an exact match).
    #[serde(default)]
    pub asset: Option<String>,
    /// A direct download URL, used instead of `github_repo`.
    #[serde(default)]
    pub url: Option<String>,
    /// Expected SHA-256 of the archive, when it is pinned.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// A load-order strategy declaration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum LoadOrderDef {
    /// v1 shorthand: a strategy name; parameters come from the legacy preset
    /// table in `loadorder.rs` (kept only for pre-v2 definitions).
    Named(String),
    /// v2: an explicit strategy table.
    PluginsTxt(PluginsTxtDef),
}

/// Parameters of the `plugins_txt` strategy (Bethesda's activation file).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginsTxtDef {
    /// Strategy discriminator; must be `"plugins_txt"`.
    pub strategy: String,
    /// The game's folder under the user's local app data (e.g.
    /// `Skyrim Special Edition`), where `Plugins.txt` lives.
    pub appdata_dir: String,
}

/// A Windows registry value naming a game's install directory.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RegistryKeyDef {
    /// The hive: `HKLM`, `HKCU`, `HKCR`, or `HKU`.
    pub hive: String,
    /// The key path under the hive (backslash-separated, no leading slash).
    pub key: String,
    /// The value name to read (its string data is the install directory).
    pub value: String,
    /// A subdirectory to append to the read path (e.g. some vendors point at a
    /// parent). Empty = use the read path verbatim.
    #[serde(default)]
    pub subdir: String,
}

/// One external-mod detector: how hand-installed content appears on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExternalScanDef {
    /// `"file"` (each matching file is one external mod) or `"folder"` (each
    /// subdirectory is one external mod).
    pub kind: String,
    /// Human label for the kind (e.g. `SKSE plugin`, `BepInEx plugin`).
    pub label: String,
    /// Directory to scan, relative to the deploy root (matched
    /// case-insensitively). Empty = the deploy root itself.
    #[serde(default)]
    pub dir: String,
    /// For `kind = "file"`: extensions to report (lowercase, no dot).
    #[serde(default)]
    pub exts: Vec<String>,
    /// For `kind = "file"`: skip names in the definition's `base_files`
    /// (vanilla content is not an external mod).
    #[serde(default)]
    pub skip_base: bool,
}

/// Game-specific health checks, all data-driven.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthDef {
    /// A script-extender-style loader requirement.
    #[serde(default)]
    pub loader: Option<LoaderCheckDef>,
    /// Known "part A needs part B" mod pairings.
    #[serde(default)]
    pub recommended: Vec<RecommendDef>,
}

/// "Mods ship `<plugins_dir>` content, so a loader binary must be present."
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoaderCheckDef {
    /// Staged-tree-relative directory whose presence in any mod means the
    /// loader is required (e.g. `SKSE/Plugins`).
    pub plugins_dir: String,
    /// A root-parked binary (`.root/<name>`) starting with this prefix
    /// (case-insensitive) satisfies the requirement (e.g. `skse`).
    pub root_prefix: String,
    /// The issue text shown when the loader is missing.
    pub message: String,
}

/// "If a mod matches, it also needs this root-parked file."
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecommendDef {
    /// Trigger: a staged file at this relative path in any mod.
    #[serde(default)]
    pub if_file: Option<String>,
    /// Trigger: a mod whose name contains this (case-insensitive).
    #[serde(default)]
    pub if_name_contains: Option<String>,
    /// Required: `.root/<name>` present in any mod.
    pub requires_root_file: String,
    /// The issue text shown when the requirement is missing.
    pub message: String,
}

/// Every key that belongs at the top level of a `game.toml`. None of these
/// names is also valid inside a nested table, so finding one there is
/// unambiguously a misplaced key rather than a field from a newer schema
/// (unknown nested keys stay tolerated, so definitions written for a later
/// build still load).
const TOP_LEVEL_KEYS: [&str; 25] = [
    "api_version",
    "id",
    "name",
    "steam_appid",
    "nexus_domain",
    "gog_id",
    "epic_id",
    "xbox_id",
    "origin_id",
    "uplay_id",
    "mod_root",
    "mod_base",
    "deploy",
    "install_probe",
    "registry_keys",
    "required_files",
    "load_order",
    "content_dirs",
    "base_files",
    "external_scan",
    "health",
    "park_root_binaries",
    "launch",
    "mod_loader",
    "store",
];

/// Most nodes the misplaced-key walk will visit (bounded loop, no recursion).
const MAX_WALK_NODES: usize = 1024;

/// Find a top-level key that was written inside a nested table. Returns the
/// offending key and the table it landed in.
fn find_misplaced_key(text: &str) -> Option<(String, String)> {
    let root: toml::Value = toml::from_str(text).ok()?;
    // Seed the worklist with the root's nested values; the root's own keys are
    // legitimately top-level.
    let mut work: Vec<(String, &toml::Value)> = Vec::new();
    for (name, value) in root.as_table()? {
        if value.is_table() || value.is_array() {
            work.push((name.clone(), value));
        }
    }
    let mut visited = 0_usize;
    while let Some((path, value)) = work.pop() {
        visited = visited.saturating_add(1);
        if visited > MAX_WALK_NODES {
            break;
        }
        if let Some(table) = value.as_table() {
            for (key, child) in table {
                if TOP_LEVEL_KEYS.contains(&key.as_str()) {
                    return Some((key.clone(), path));
                }
                if child.is_table() || child.is_array() {
                    work.push((format!("{path}.{key}"), child));
                }
            }
        } else if let Some(array) = value.as_array() {
            for item in array {
                if item.is_table() || item.is_array() {
                    work.push((path.clone(), item));
                }
            }
        }
    }
    None
}

impl GameDef {
    /// Parse and validate a `game.toml` from its text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GameDef`] if the TOML is malformed, the `api_version` is
    /// unsupported, or required fields are empty.
    pub fn from_toml_str(text: &str, source: &Path) -> Result<Self> {
        let def: Self = toml::from_str(text).map_err(|e| Error::GameDef {
            path: source.to_path_buf(),
            message: e.to_string(),
        })?;
        // TOML scopes every key after a `[table]` header into that table, so a
        // top-level key written below one is silently swallowed (serde ignores
        // the unknown field) and its capability is lost without a parse error.
        // Catch that here rather than shipping a definition whose
        // `content_dirs`/`base_files` never took effect.
        if let Some((key, table)) = find_misplaced_key(text) {
            return Err(Error::GameDef {
                path: source.to_path_buf(),
                message: format!(
                    "`{key}` is a top-level key but appears inside the `[{table}]` table - \
                     move it above the first table header (TOML scopes keys to the preceding \
                     header, so it is otherwise ignored)"
                ),
            });
        }
        def.validate(source)?;
        Ok(def)
    }

    /// Load and validate a `game.toml` from disk.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the file cannot be read, or [`Error::GameDef`]
    /// as for [`GameDef::from_toml_str`].
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        Self::from_toml_str(&text, path)
    }

    /// Serialize back to JSON (how the engine persists the def it registered
    /// a game with, so capabilities survive without re-reading files).
    ///
    /// # Errors
    ///
    /// Returns [`Error::GameDef`] if serialization fails (practically never).
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| Error::GameDef {
            path: std::path::PathBuf::from("<def>"),
            message: e.to_string(),
        })
    }

    /// Parse a def previously persisted with [`GameDef::to_json`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::GameDef`] on malformed JSON.
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).map_err(|e| Error::GameDef {
            path: std::path::PathBuf::from("<def-json>"),
            message: e.to_string(),
        })
    }

    fn validate(&self, source: &Path) -> Result<()> {
        let complaint = if !(MIN_API_VERSION..=MAX_API_VERSION).contains(&self.api_version) {
            Some(format!(
                "unsupported api_version {} (this build supports {MIN_API_VERSION}..={MAX_API_VERSION})",
                self.api_version
            ))
        } else if self.id.trim().is_empty() {
            Some("`id` must not be empty".to_owned())
        } else if self.name.trim().is_empty() {
            Some("`name` must not be empty".to_owned())
        } else {
            self.validate_details()
        };
        match complaint {
            Some(message) => Err(Error::GameDef {
                path: source.to_path_buf(),
                message,
            }),
            None => Ok(()),
        }
    }

    fn validate_details(&self) -> Option<String> {
        if let Some(LoadOrderDef::PluginsTxt(t)) = &self.load_order {
            if t.strategy != "plugins_txt" {
                return Some(format!("unknown load_order strategy `{}`", t.strategy));
            }
            if t.appdata_dir.trim().is_empty() {
                return Some("load_order.appdata_dir must not be empty".to_owned());
            }
        }
        for scan in &self.external_scan {
            if scan.kind != "file" && scan.kind != "folder" {
                return Some(format!(
                    "external_scan kind `{}` is not `file` or `folder`",
                    scan.kind
                ));
            }
            if scan.kind == "file" && scan.exts.is_empty() {
                return Some("a `file` external_scan needs `exts`".to_owned());
            }
        }
        if let Some(base) = &self.mod_base
            && !matches!(
                base.as_str(),
                "install" | "documents" | "local_appdata" | "roaming_appdata"
            )
        {
            return Some(format!(
                "unknown mod_base `{base}` (install/documents/local_appdata/roaming_appdata)"
            ));
        }
        for probe in &self.registry_keys {
            if !matches!(probe.hive.as_str(), "HKLM" | "HKCU" | "HKCR" | "HKU") {
                return Some(format!(
                    "registry_keys hive `{}` is not HKLM/HKCU/HKCR/HKU",
                    probe.hive
                ));
            }
            if probe.key.trim().is_empty() || probe.value.trim().is_empty() {
                return Some("a registry_keys entry needs a `key` and a `value`".to_owned());
            }
        }
        self.validate_launch()
            .or_else(|| self.validate_loader())
            .or_else(|| self.validate_store())
    }

    /// Check the `[launch]` table. Split out of [`Self::validate_details`] to
    /// keep each validator under the workspace's 60-line ceiling.
    fn validate_launch(&self) -> Option<String> {
        let launch = self.launch.as_ref()?;
        match launch.mode.as_str() {
            "steam" => {
                if self.steam_appid.is_none() {
                    return Some("launch.mode = \"steam\" needs a `steam_appid`".to_owned());
                }
            }
            "exe" => {
                let named = launch.exe.as_ref().is_some_and(|e| !e.trim().is_empty());
                if !named {
                    return Some("launch.mode = \"exe\" needs an `exe`".to_owned());
                }
            }
            other => return Some(format!("unknown launch mode `{other}` (steam/exe)")),
        }
        if launch.log.as_ref().is_some_and(|l| l.trim().is_empty()) {
            return Some("launch.log must not be empty when present".to_owned());
        }
        None
    }

    /// Check the `[loader]` table.
    fn validate_loader(&self) -> Option<String> {
        let loader = self.mod_loader.as_ref()?;
        if loader.kind.trim().is_empty() {
            return Some("mod_loader.kind must not be empty".to_owned());
        }
        if loader.install_root.trim().is_empty() {
            return Some("mod_loader.install_root must not be empty".to_owned());
        }
        if loader.sentinel.trim().is_empty() {
            return Some("mod_loader.sentinel must not be empty".to_owned());
        }
        // A non-native loader has to say where it comes from, or `install`
        // would have nothing to do and would silently report success.
        if loader.kind != NATIVE_LOADER_KIND {
            let src = loader.source.as_ref();
            let fetchable = src.is_some_and(|s| s.github_repo.is_some() || s.url.is_some());
            if !fetchable {
                return Some(format!(
                    "mod_loader.kind `{}` is not native, so [mod_loader.source] needs a `github_repo` or `url`",
                    loader.kind
                ));
            }
        }
        None
    }

    /// Check the `[store]` table. Split out for the same reason as
    /// [`Self::validate_launch`]: the 60-line ceiling is per function.
    fn validate_store(&self) -> Option<String> {
        let store = self.store.as_ref()?;
        if store.kind.trim().is_empty() {
            return Some("store.kind must not be empty".to_owned());
        }
        // Plain http would let anyone on the path choose which archive gets
        // extracted into the game directory, so it is rejected outright rather
        // than warned about.
        if !store.base_url.starts_with("https://") {
            return Some(format!(
                "store.base_url `{}` must be an https:// origin",
                store.base_url
            ));
        }
        // A trailing slash would produce `…//api/mods` once a client joins a
        // path onto it. Some servers 404 that, so normalize by refusing it.
        if store.base_url.ends_with('/') {
            return Some("store.base_url must not end with a slash".to_owned());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn inline() -> PathBuf {
        PathBuf::from("<inline>")
    }

    const SAMPLE_V1: &str = r#"
        api_version = 1
        id = "skyrimse"
        name = "Skyrim Special Edition"
        steam_appid = 489_830
        mod_root = "Data"
        deploy = "link"
        install_probe = ["steam", "registry"]
        load_order = "plugins_txt"
    "#;

    #[test]
    fn parses_a_valid_v1_definition() {
        let def = GameDef::from_toml_str(SAMPLE_V1, &inline()).unwrap();
        assert_eq!(def.id, "skyrimse");
        assert_eq!(def.steam_appid, Some(489_830));
        assert_eq!(def.mod_root, "Data");
        assert!(matches!(
            def.load_order,
            Some(LoadOrderDef::Named(ref n)) if n == "plugins_txt"
        ));
    }

    #[test]
    fn parses_a_v2_definition_with_capabilities() {
        let text = r#"
            api_version = 2
            id = "testgame"
            name = "Test"
            mod_root = "Data"
            content_dirs = ["meshes", "textures"]
            base_files = ["base.esm", "cc*"]

            [load_order]
            strategy = "plugins_txt"
            appdata_dir = "Test Game"

            [[external_scan]]
            kind = "file"
            label = "plugin"
            exts = ["esp", "esm"]
            skip_base = true

            [[external_scan]]
            kind = "folder"
            label = "BepInEx plugin"
            dir = "BepInEx/plugins"

            [health.loader]
            plugins_dir = "SKSE/Plugins"
            root_prefix = "skse"
            message = "loader missing"

            [[health.recommended]]
            if_file = "SKSE/Plugins/EngineFixes.dll"
            requires_root_file = "d3dx9_42.dll"
            message = "part 2 missing"
        "#;
        let def = GameDef::from_toml_str(text, &inline()).unwrap();
        assert!(matches!(
            def.load_order,
            Some(LoadOrderDef::PluginsTxt(ref t)) if t.appdata_dir == "Test Game"
        ));
        assert_eq!(def.content_dirs, vec!["meshes", "textures"]);
        assert_eq!(def.external_scan.len(), 2);
        assert_eq!(def.external_scan[1].kind, "folder");
        let health = def.health.unwrap();
        assert_eq!(health.loader.unwrap().root_prefix, "skse");
        assert_eq!(health.recommended.len(), 1);
    }

    #[test]
    fn shipped_definitions_are_valid_v2() {
        for (path, text) in [
            (
                "games/skyrimse/game.toml",
                include_str!("../../../games/skyrimse/game.toml"),
            ),
            (
                "games/subnautica/game.toml",
                include_str!("../../../games/subnautica/game.toml"),
            ),
        ] {
            let def = GameDef::from_toml_str(text, std::path::Path::new(path)).unwrap();
            assert_eq!(def.api_version, 2, "{path} should be api_version 2");
        }
    }

    #[test]
    fn shipped_subnautica_definition_declares_bepinex() {
        // Subnautica is a Unity/BepInEx game: mods deploy into the plugin
        // container (a nested mod root) and there is no load-order strategy -
        // BepInEx orders plugins by their declared dependencies.
        let text = include_str!("../../../games/subnautica/game.toml");
        let def = GameDef::from_toml_str(text, std::path::Path::new("games/subnautica/game.toml"))
            .unwrap();
        assert_eq!(def.id, "subnautica");
        assert_eq!(def.steam_appid, Some(264_710));
        assert_eq!(def.mod_root, "BepInEx/plugins");
        assert!(def.load_order.is_none());
        assert!(def.external_scan.iter().any(|s| s.kind == "folder"));
    }

    #[test]
    fn def_round_trips_through_json() {
        let def = GameDef::from_toml_str(SAMPLE_V1, &inline()).unwrap();
        let json = def.to_json().unwrap();
        let back = GameDef::from_json(&json).unwrap();
        assert_eq!(back.id, def.id);
        assert!(matches!(back.load_order, Some(LoadOrderDef::Named(_))));
    }

    #[test]
    fn rejects_unsupported_api_version() {
        let text = "api_version = 999\nid = \"x\"\nname = \"X\"\n";
        let err = GameDef::from_toml_str(text, &inline()).unwrap_err();
        assert!(matches!(err, Error::GameDef { .. }));
    }

    #[test]
    fn parses_multi_store_identity_and_registry_probes() {
        let text = r#"
            api_version = 2
            id = "witcher3"
            name = "The Witcher 3"
            steam_appid = 292030
            gog_id = "1207664663"
            epic_id = "cabe2a"
            install_probe = ["steam", "gog", "epic", "registry"]
            required_files = ["bin/x64/witcher3.exe"]

            [[registry_keys]]
            hive  = "HKLM"
            key   = "SOFTWARE\\WOW6432Node\\CD Projekt Red\\The Witcher 3"
            value = "InstallFolder"
        "#;
        let def = GameDef::from_toml_str(text, &inline()).unwrap();
        assert_eq!(def.gog_id.as_deref(), Some("1207664663"));
        assert_eq!(def.epic_id.as_deref(), Some("cabe2a"));
        assert_eq!(def.required_files, vec!["bin/x64/witcher3.exe"]);
        assert_eq!(def.registry_keys.len(), 1);
        assert_eq!(def.registry_keys[0].hive, "HKLM");
        assert_eq!(def.registry_keys[0].value, "InstallFolder");
    }

    #[test]
    fn rejects_a_top_level_key_swallowed_by_a_table() {
        // TOML scopes `content_dirs` into `[[registry_keys]]` here, so serde
        // silently dropped it and the game shipped with no content dirs. This
        // must be a loud parse error, not a silent capability loss.
        let text = "api_version = 2\nid = \"x\"\nname = \"X\"\n\
                    [[registry_keys]]\nhive = \"HKLM\"\nkey = \"a\\\\b\"\nvalue = \"Path\"\n\
                    content_dirs = [\"meshes\"]\n";
        let err = GameDef::from_toml_str(text, &inline()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("content_dirs"), "got: {message}");
        assert!(message.contains("registry_keys"), "got: {message}");

        // The same keys placed above the table header are fine.
        let good = "api_version = 2\nid = \"x\"\nname = \"X\"\n\
                    content_dirs = [\"meshes\"]\n\
                    [[registry_keys]]\nhive = \"HKLM\"\nkey = \"a\\\\b\"\nvalue = \"Path\"\n";
        let def = GameDef::from_toml_str(good, &inline()).unwrap();
        assert_eq!(def.content_dirs, vec!["meshes"]);
        assert_eq!(def.registry_keys.len(), 1);
    }

    #[test]
    fn unknown_nested_keys_stay_tolerated_for_forward_compatibility() {
        // A definition authored for a newer build may carry nested fields this
        // one does not know; those must still load (only *top-level* names in a
        // nested table are an error).
        let text = "api_version = 2\nid = \"x\"\nname = \"X\"\n\
                    [load_order]\nstrategy = \"plugins_txt\"\nappdata_dir = \"X\"\n\
                    plugin_exts = [\"esp\"]\n";
        let def = GameDef::from_toml_str(text, &inline()).unwrap();
        assert!(def.load_order.is_some());
    }

    #[test]
    fn starfield_activates_plugins_through_plugins_txt() {
        // Regression: Starfield's plugin list lives at
        // %LOCALAPPDATA%\Starfield\Plugins.txt like the rest of the Creation
        // Engine family. Without this block a deployed mod never activates.
        let text = include_str!("../../../games/starfield/game.toml");
        let def = GameDef::from_toml_str(text, std::path::Path::new("games/starfield/game.toml"))
            .unwrap();
        let Some(LoadOrderDef::PluginsTxt(ref t)) = def.load_order else {
            panic!("starfield must declare the plugins_txt strategy");
        };
        assert_eq!(t.appdata_dir, "Starfield");
        // The structural fix above must also hold: these are real capabilities.
        assert!(def.content_dirs.contains(&"meshes".to_owned()));
        assert!(def.base_files.contains(&"starfield.esm".to_owned()));
    }

    #[test]
    fn rejects_an_unknown_registry_hive() {
        let text = "api_version = 2\nid = \"x\"\nname = \"X\"\n\
                    [[registry_keys]]\nhive = \"HKWAT\"\nkey = \"a\\\\b\"\nvalue = \"Path\"\n";
        assert!(GameDef::from_toml_str(text, &inline()).is_err());
    }

    #[test]
    fn rejects_a_bad_scan_kind() {
        let text = "api_version = 2\nid = \"x\"\nname = \"X\"\n\
                    [[external_scan]]\nkind = \"weird\"\nlabel = \"x\"\n";
        assert!(GameDef::from_toml_str(text, &inline()).is_err());
    }

    #[test]
    fn rejects_empty_id() {
        let text = "api_version = 1\nid = \"\"\nname = \"X\"\n";
        assert!(GameDef::from_toml_str(text, &inline()).is_err());
    }

    #[test]
    fn defaults_optional_fields() {
        let text = "api_version = 1\nid = \"x\"\nname = \"X\"\n";
        let def = GameDef::from_toml_str(text, &inline()).unwrap();
        assert_eq!(def.mod_root, "");
        assert!(def.steam_appid.is_none());
        assert!(
            def.install_probe.is_empty(),
            "expected empty, got {:?}",
            def.install_probe
        );
        assert!(
            def.content_dirs.is_empty(),
            "expected empty, got {:?}",
            def.content_dirs
        );
        assert!(def.external_scan.is_empty());
        assert!(def.health.is_none());
        assert!(def.gog_id.is_none());
        assert!(def.epic_id.is_none());
        assert!(def.registry_keys.is_empty());
        assert!(
            def.required_files.is_empty(),
            "expected empty, got {:?}",
            def.required_files
        );
        // v3 fields default to absent, which is what keeps every v1/v2
        // definition parsing unchanged.
        assert!(def.park_root_binaries.is_none());
        assert!(def.launch.is_none());
        assert!(def.mod_loader.is_none());
        assert!(def.store.is_none());
    }

    #[test]
    fn parses_a_v3_definition_with_launch_and_loader() {
        let text = "\
api_version = 3
id = \"g\"
name = \"G\"
steam_appid = 1
park_root_binaries = false

[launch]
mode = \"steam\"
log = \"_Modrix/Latest.log\"
process = \"G.exe\"

[launch.env]
WINEDLLOVERRIDES = \"version=n,b\"

[mod_loader]
kind = \"modrix-sons\"
install_root = \"_Modrix\"
sentinel = \"_Modrix/net6/Modrix.Loader.dll\"
proxy = \"version.dll\"

[store]
kind = \"sotf-mods\"
base_url = \"https://api.sotf-mods.com\"
";
        let def = GameDef::from_toml_str(text, &inline()).unwrap();
        assert_eq!(def.park_root_binaries, Some(false));
        let launch = def.launch.expect("launch table parsed");
        assert_eq!(launch.mode, "steam");
        assert_eq!(
            launch.env.get("WINEDLLOVERRIDES").map(String::as_str),
            Some("version=n,b"),
            "the Proton override is what makes a loader work off-Windows"
        );
        let loader = def.mod_loader.expect("mod_loader table parsed");
        assert_eq!(loader.kind, NATIVE_LOADER_KIND);
        assert!(loader.source.is_none(), "a native loader needs no source");
        let store = def.store.expect("store table parsed");
        assert_eq!(store.kind, "sotf-mods");
        assert_eq!(store.base_url, "https://api.sotf-mods.com");
    }

    #[test]
    fn store_tables_are_rejected_when_incoherent() {
        let base = "api_version = 3\nid = \"g\"\nname = \"G\"\n";
        // No kind: nothing selects a client, so the screen would show but never
        // load.
        let text = format!("{base}\n[store]\nkind = \"\"\nbase_url = \"https://x.example\"\n");
        assert!(GameDef::from_toml_str(&text, &inline()).is_err());
        // Plain http: a store install ends in code inside the game directory.
        let text =
            format!("{base}\n[store]\nkind = \"sotf-mods\"\nbase_url = \"http://x.example\"\n");
        assert!(GameDef::from_toml_str(&text, &inline()).is_err());
        // A trailing slash would build `…//api/mods`.
        let text =
            format!("{base}\n[store]\nkind = \"sotf-mods\"\nbase_url = \"https://x.example/\"\n");
        assert!(GameDef::from_toml_str(&text, &inline()).is_err());
    }

    #[test]
    fn v3_tables_are_rejected_when_incoherent() {
        let base = "api_version = 3\nid = \"g\"\nname = \"G\"\n";
        // steam launch without an appid has nothing to launch.
        let text = format!("{base}\n[launch]\nmode = \"steam\"\n");
        assert!(GameDef::from_toml_str(&text, &inline()).is_err());
        // exe launch without an exe likewise.
        let text = format!("{base}\n[launch]\nmode = \"exe\"\n");
        assert!(GameDef::from_toml_str(&text, &inline()).is_err());
        // An unknown mode is a typo, not a forward-compatible extension.
        let text = format!("{base}\n[launch]\nmode = \"magic\"\n");
        assert!(GameDef::from_toml_str(&text, &inline()).is_err());
        // A non-native loader with no source would silently install nothing.
        let text = format!(
            "{base}\n[mod_loader]\nkind = \"redloader\"\ninstall_root = \"_Redloader\"\n\
             sentinel = \"_Redloader/net6/RedLoader.dll\"\n"
        );
        assert!(GameDef::from_toml_str(&text, &inline()).is_err());
    }

    #[test]
    fn v3_is_the_newest_supported_schema() {
        let text = "api_version = 4\nid = \"g\"\nname = \"G\"\n";
        assert!(
            GameDef::from_toml_str(text, &inline()).is_err(),
            "a definition from a newer build must be rejected, not half-read"
        );
    }

    #[test]
    fn shipped_sonsoftheforest_definition_opts_out_of_parking() {
        // Regression guard for a silent misdeploy: `Mods` is a single-component
        // mod root, so without the explicit opt-out every payload DLL is parked
        // to `.root/` and deployed to the game install directory, where the
        // loader never looks. Deploy reports success; nothing loads.
        let text = include_str!("../../../games/sonsoftheforest/game.toml");
        let path = std::path::Path::new("games/sonsoftheforest/game.toml");
        let def = GameDef::from_toml_str(text, path).unwrap();
        assert_eq!(def.api_version, 3);
        assert_eq!(def.mod_root, "Mods");
        assert_eq!(
            def.park_root_binaries,
            Some(false),
            "removing this silently breaks every Sons of the Forest mod"
        );
        let loader = def.mod_loader.expect("declares a loader");
        assert_eq!(loader.kind, NATIVE_LOADER_KIND);
        assert!(
            loader.conflicts.iter().any(|c| c == "MelonLoader"),
            "must refuse to install over a rival proxy owner"
        );
        // The Mod Store tab is gated on this table alone, so losing it would
        // silently remove the only in-app way to install a Sons mod.
        let store = def.store.expect("declares a mod store");
        assert_eq!(store.kind, "sotf-mods");
        assert_eq!(store.base_url, "https://api.sotf-mods.com");
    }
}
