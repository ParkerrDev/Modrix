// SPDX-License-Identifier: GPL-2.0-only
//! Snapshot scenes: made-up libraries that put every screen in a known
//! state, for `--snap` renders and visual review. Nothing here touches disk.

use std::path::PathBuf;

use modrix_core::health::Severity;
use modrix_core::plugins::GamePlugin;
use modrix_core::{
    ConflictFile, ExternalMod, Game, GameCapabilities, GameId, Issue, Mod, ModConflict, ModId,
    ModRule, Profile, ProfileId,
};
use modrix_download::{DownloadId, DownloadState, DownloadStatus};

use crate::prefs::Appearance;
use crate::state::{App, Link, Screen, Tone};

/// A scene: its `--scene` name and the state it sets up.
type Scene = (&'static str, fn(&mut App));

/// Named scenes for `--scene`.
pub const SCENES: &[Scene] = &[
    ("dashboard", |s| scene(s, Screen::Dashboard)),
    ("mods", mods),
    ("loadorder", |s| scene(s, Screen::LoadOrder)),
    ("conflicts", conflicts),
    ("downloads", |s| scene(s, Screen::Downloads)),
    ("games", |s| scene(s, Screen::Games)),
    ("settings", |s| scene(s, Screen::Settings)),
    ("wizard", wizard),
    ("light", light),
    ("empty", empty),
];

fn id<T: serde::de::DeserializeOwned>(raw: i64) -> Option<T> {
    serde_json::from_str(&raw.to_string()).ok()
}

#[expect(
    clippy::expect_used,
    reason = "an i64 always deserializes into an id newtype"
)]
fn mod_id(raw: i64) -> ModId {
    id(raw).expect("an i64 always deserializes into an id newtype")
}

#[expect(
    clippy::expect_used,
    reason = "an i64 always deserializes into an id newtype"
)]
fn game_id(raw: i64) -> GameId {
    id(raw).expect("an i64 always deserializes into an id newtype")
}

#[expect(
    clippy::expect_used,
    reason = "an i64 always deserializes into an id newtype"
)]
fn profile_id(raw: i64) -> ProfileId {
    id(raw).expect("an i64 always deserializes into an id newtype")
}

/// Mod names, versions, days ago.
const MODS: &[(&str, &str, i64)] = &[
    ("SKSE64", "2.2.6", 40),
    ("SkyUI", "5.2SE", 39),
    ("Unofficial Skyrim Special Edition Patch", "4.3.2", 39),
    ("Address Library for SKSE Plugins", "11", 38),
    ("Static Mesh Improvement Mod", "2.08", 30),
    ("Noble Skyrim Texture Overhaul", "4.0", 30),
    ("Immersive Armors", "8.1", 22),
    ("Cutting Room Floor", "3.1.9", 21),
    ("Alternate Start: Live Another Life", "4.2.4", 14),
    ("Enhanced Lights and FX", "5.4", 9),
    ("Obsidian Weathers and Seasons", "1.03", 4),
    ("Immersive Citizens: AI Overhaul", "0.4.0", 1),
];

fn game(raw: i64, name: &str, appid: i64, path: &str) -> Game {
    Game {
        id: game_id(raw),
        plugin_id: name.to_lowercase().replace(' ', ""),
        name: name.to_owned(),
        install_path: PathBuf::from(path),
        mod_root: "Data".to_owned(),
        mod_base: "install".to_owned(),
        store: "steam".to_owned(),
        steam_appid: Some(appid),
        nexus_domain: None,
        staging_root: PathBuf::from("/home/player/.local/share/modrix/staging"),
    }
}

fn games(s: &mut App) {
    s.lib.games = vec![
        game(
            1,
            "Skyrim Special Edition",
            489_830,
            "/home/player/.steam/steam/steamapps/common/Skyrim Special Edition",
        ),
        game(
            2,
            "Subnautica",
            264_710,
            "/home/player/.steam/steam/steamapps/common/Subnautica",
        ),
        game(
            3,
            "Stardew Valley",
            413_150,
            "/home/player/.steam/steam/steamapps/common/Stardew Valley",
        ),
    ];
    for g in &s.lib.games {
        if let Some(appid) = g.steam_appid {
            s.art.insert(
                g.id,
                crate::art::ArtSet::from_data(crate::art::local(appid)),
            );
        }
    }
}

fn library(s: &mut App) {
    let skyrim = game_id(1);
    games(s);
    s.lib.game = Some(skyrim);
    s.lib.profiles = ["Default", "Survival", "Vanilla plus"]
        .iter()
        .zip(1..)
        .map(|(name, raw)| Profile {
            id: profile_id(raw),
            game_id: skyrim,
            name: (*name).to_owned(),
            is_active: raw == 1,
        })
        .collect();
    s.lib.profile = s.lib.profiles.first().cloned();
    s.lib.capabilities = Some(GameCapabilities {
        load_order: true,
        external_scan: true,
        health_checks: true,
        launch: true,
        mod_loader: false,
        store: false,
    });
    s.lib_gen = s.lib_gen.wrapping_add(1);
    s.lib.mods = MODS
        .iter()
        .zip(1..)
        .map(|(m, raw)| demo_mod(s.now, skyrim, raw, *m))
        .collect();
    s.lib.order = s.lib.mods.clone();
    s.lib.enabled = s
        .lib
        .mods
        .iter()
        .filter(|m| m.id.get() % 5 != 0)
        .map(|m| m.id)
        .collect();
    plugins(s);
    findings(s);
}

fn findings(s: &mut App) {
    s.lib.health = vec![
        Issue {
            severity: Severity::Warning,
            message:
                "Cutting Room Floor expects Unofficial Skyrim Special Edition Patch to load first"
                    .to_owned(),
            blocking: false,
        },
        Issue {
            severity: Severity::Info,
            message: "Two plugins could be flagged as light to save load-order slots".to_owned(),
            blocking: false,
        },
    ];
    s.lib.conflicts = conflict_list();
    s.lib.external = vec![ExternalMod {
        name: "ENB Series".to_owned(),
        label: "Found in the game folder".to_owned(),
        path: PathBuf::from("d3d11.dll"),
        files: 14,
    }];
    s.booted = true;
    s.link = Some(Link {
        port: 47_761,
        token: "c0ffee0ddba11f00d5ca1ab1e5eed".to_owned(),
    });
    s.sort_mods();
}

fn demo_mod(now: f64, game: GameId, raw: i64, (name, version, days): (&str, &str, i64)) -> Mod {
    #[expect(clippy::cast_possible_truncation, reason = "a demo clock fits in i64")]
    let base = now as i64;
    let at = 1_795_600_000_i64
        .max(base)
        .saturating_sub(days.saturating_mul(86_400));
    Mod {
        id: mod_id(raw),
        game_id: game,
        name: name.to_owned(),
        version: Some(version.to_owned()),
        source: "archive".to_owned(),
        staged_path: PathBuf::from(format!("/staging/{raw}")),
        install_state: "installed".to_owned(),
        archive_path: None,
        nexus_mod_id: None,
        created_at: Some(at),
        archive_sha256: None,
    }
}

fn plugins(s: &mut App) {
    let list: &[(&str, i64, bool, bool, &[&str])] = &[
        ("Skyrim.esm", 0, true, false, &[]),
        ("Update.esm", 0, true, false, &[]),
        (
            "Unofficial Skyrim Special Edition Patch.esp",
            3,
            true,
            false,
            &[],
        ),
        ("SkyUI_SE.esp", 2, false, false, &[]),
        ("Immersive Armors.esp", 7, false, false, &[]),
        ("Cutting Room Floor.esp", 8, false, false, &[]),
        (
            "Alternate Start - Live Another Life.esp",
            9,
            false,
            false,
            &[],
        ),
        ("EnhancedLightsandFX.esp", 10, false, true, &[]),
        ("Obsidian Weathers.esp", 11, false, true, &[]),
        (
            "Immersive Citizens - AI Overhaul.esp",
            12,
            false,
            false,
            &["Dawnguard.esm"],
        ),
    ];
    s.lib.plugins = list
        .iter()
        .map(|(name, owner, master, light, missing)| GamePlugin {
            name: (*name).to_owned(),
            mod_id: mod_id(*owner),
            mod_name: s
                .lib
                .mods
                .iter()
                .find(|m| m.id.get() == *owner)
                .map_or_else(|| "Base game".to_owned(), |m| m.name.clone()),
            is_master: *master,
            is_light: *light,
            masters: Vec::new(),
            missing_masters: missing.iter().map(|m| (*m).to_owned()).collect(),
            enabled: *name != "SkyUI_SE.esp",
        })
        .collect();
    s.lib.plugin_counts =
        s.lib
            .plugins
            .iter()
            .fold(std::collections::HashMap::new(), |mut acc, p| {
                let n: &mut usize = acc.entry(p.mod_id).or_default();
                *n = n.saturating_add(1);
                acc
            });
}

fn conflict_list() -> Vec<ModConflict> {
    let files = |winner: i64, names: &[&str]| -> Vec<ConflictFile> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| ConflictFile {
                target: (*n).to_owned(),
                winner: mod_id(winner),
                overridden: i == 1,
            })
            .collect()
    };
    vec![
        ModConflict {
            first: mod_id(5),
            second: mod_id(6),
            rule: Some(ModRule {
                loser: mod_id(5),
                winner: mod_id(6),
            }),
            files: files(
                6,
                &[
                    "textures/architecture/whiterun/wrwoodplank01.dds",
                    "textures/architecture/whiterun/wrwoodplank01_n.dds",
                ],
            ),
        },
        ModConflict {
            first: mod_id(10),
            second: mod_id(11),
            rule: None,
            files: files(
                11,
                &[
                    "meshes/effects/fxambsunshafts01.nif",
                    "shaders/weather/skyrimclear.fx",
                    "textures/effects/fxsunglare.dds",
                    "textures/sky/skyrimcloudsupper01.dds",
                ],
            ),
        },
    ]
}

fn scene(s: &mut App, screen: Screen) {
    library(s);
    downloads(s);
    notes(s);
    s.screen = screen;
    s.appearance = Appearance::Dark;
}

fn downloads(s: &mut App) {
    let items = [
        (
            1,
            DownloadState::Active,
            412_000_000,
            Some(1_024_000_000),
            8,
            "Skyrim 202X 10.0 - Architecture.7z.part",
        ),
        (
            2,
            DownloadState::Queued,
            0,
            None,
            0,
            "Ordinator - Perks of Skyrim.zip.part",
        ),
        (
            3,
            DownloadState::Complete,
            18_400_000,
            Some(18_400_000),
            0,
            "SkyUI_5_2_SE.7z",
        ),
        (
            4,
            DownloadState::Failed,
            2_000_000,
            Some(90_000_000),
            0,
            "Lux - Lighting.7z.part",
        ),
    ];
    s.downloads = items
        .into_iter()
        .map(
            |(raw, state, done, total, connections, file)| DownloadStatus {
                id: DownloadId::from_u64(raw),
                state,
                done,
                total,
                connections,
                file: PathBuf::from(file),
            },
        )
        .collect();
    s.outcomes.insert(
        DownloadId::from_u64(3),
        modrix_service::InstallOutcome::Installed {
            name: "SkyUI".to_owned(),
            configurable: false,
        },
    );
}

fn notes(s: &mut App) {
    s.note(
        Tone::Ok,
        "Deployed: 1,284 added, 12 removed, 40,212 unchanged",
    );
    s.note(Tone::Info, "Installed SkyUI");
    s.unread = 1;
    s.notes.iter_mut().for_each(|n| n.at = -60.0);
}

fn mods(s: &mut App) {
    scene(s, Screen::Mods);
    s.mod_sel.only(1);
    s.mod_sel.toggle(2);
}

fn conflicts(s: &mut App) {
    scene(s, Screen::Conflicts);
    s.expanded_conflict = Some((mod_id(10), mod_id(11)));
}

fn light(s: &mut App) {
    scene(s, Screen::Dashboard);
    s.appearance = Appearance::Light;
}

fn empty(s: &mut App) {
    s.booted = true;
    s.screen = Screen::Dashboard;
    s.appearance = Appearance::Dark;
}

/// Options as (name, kind, description).
type Options<'a> = &'a [(&'a str, modrix_plugin::fomod::PluginKind, &'a str)];

fn group(
    name: &str,
    kind: modrix_plugin::fomod::GroupKind,
    options: Options<'_>,
) -> modrix_plugin::fomod::Group {
    use modrix_plugin::fomod::{Plugin, TypeDescriptor};
    modrix_plugin::fomod::Group {
        name: name.to_owned(),
        kind,
        plugins: options
            .iter()
            .map(|(name, kind, description)| Plugin {
                name: (*name).to_owned(),
                description: (*description).to_owned(),
                image: None,
                files: Vec::new(),
                flags: Vec::new(),
                kind: TypeDescriptor::Simple(*kind),
            })
            .collect(),
    }
}

fn installer() -> modrix_plugin::fomod::Installer {
    use modrix_plugin::fomod::{GroupKind, Installer, PluginKind, Step};
    let weather: Options<'_> = &[
        (
            "Balanced",
            PluginKind::Recommended,
            "Seasons shift every month. Storms are rarer in summer and fog settles on the coast in autumn.",
        ),
        (
            "Harsh",
            PluginKind::Optional,
            "Longer blizzards, darker nights and colder light.",
        ),
        (
            "Cinematic",
            PluginKind::Optional,
            "Brighter skies with dramatic clouds, made for screenshots.",
        ),
    ];
    let patches: Options<'_> = &[
        (
            "Enhanced Lights and FX",
            PluginKind::Recommended,
            "Matches interior light to the weather outside.",
        ),
        (
            "Climates of Tamriel",
            PluginKind::NotUsable,
            "Climates of Tamriel is not installed.",
        ),
        (
            "Wet and Cold",
            PluginKind::Optional,
            "Breath and wet skin follow the season.",
        ),
    ];
    Installer {
        module_name: "Obsidian Weathers and Seasons".to_owned(),
        info_name: Some("Obsidian Weathers and Seasons".to_owned()),
        info_version: Some("1.03".to_owned()),
        required: Vec::new(),
        steps: vec![Step {
            name: "Core files".to_owned(),
            visible: None,
            groups: vec![
                group("Weather set", GroupKind::ExactlyOne, weather),
                group("Patches", GroupKind::Any, patches),
            ],
        }],
        conditional: Vec::new(),
        module_image: None,
    }
}

fn wizard(s: &mut App) {
    scene(s, Screen::Mods);
    let installer = installer();
    let present = modrix_plugin::fomod::Present::new();
    let selections = modrix_plugin::fomod::defaults(&installer, &present);
    s.wizard = Some(crate::state::Wizard {
        mod_id: mod_id(11),
        mod_name: "Obsidian Weathers and Seasons".to_owned(),
        staged_path: PathBuf::from("/staging/11"),
        installer,
        selections,
        step: 0,
        focus: Some((0, 0, 0)),
        present,
    });
}
