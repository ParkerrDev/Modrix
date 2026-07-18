// SPDX-License-Identifier: GPL-2.0-only
//! The game-definition catalog: every `game.toml` this installation can
//! register a game from.
//!
//! Three sources, in precedence order (later wins on the same id, so an
//! installed plugin can update a built-in):
//! 1. definitions compiled into the binary (`games/*/game.toml`),
//! 2. user definitions under `<config>/games/`,
//! 3. installed registry plugins under `<data>/plugins/<id>/game.toml`.
//!
//! Every frontend (GUI, CLI, TUI) enumerates supported games through this
//! module - nothing UI-side embeds definitions of its own.

use std::path::{Path, PathBuf};

use crate::gamedef::GameDef;
use crate::paths::Paths;

/// Most files a definition scan will consider (bounded loop).
const MAX_DEF_SCAN: usize = 256;

// `BuiltinDef { toml, lua }` and `BUILTIN_GAME_DEFS: &[BuiltinDef]`, generated
// by `build.rs` from every `games/<id>/{game.toml, game.lua}` in the tree.
include!(concat!(env!("OUT_DIR"), "/builtin_defs.rs"));

/// One catalog entry: the parsed definition plus its raw text (frontends
/// re-serialize or display it) and where it came from.
#[derive(Debug, Clone)]
pub struct DefEntry {
    /// The parsed, validated definition.
    pub def: GameDef,
    /// The raw `game.toml` text.
    pub toml: String,
    /// Where the definition was loaded from (`None` = compiled in).
    pub source: Option<PathBuf>,
    /// The definition's `game.lua`, when it ships one. Populated for compiled-in
    /// (built-in) games; on-disk games load their script from `source`'s
    /// directory instead, so this stays `None` for them.
    pub lua: Option<String>,
}

/// The built-in definitions only (no filesystem access).
#[must_use]
pub fn builtin_defs() -> Vec<DefEntry> {
    BUILTIN_GAME_DEFS
        .iter()
        .filter_map(|builtin| {
            let mut entry = entry_from_toml(builtin.toml, None)?;
            entry.lua = builtin.lua.map(str::to_owned);
            Some(entry)
        })
        .collect()
}

/// The full catalog: built-ins, then `<config>/games/`, then installed
/// plugins under `<data>/plugins/`. A later definition with an id already
/// seen **replaces** the earlier one (plugins update built-ins).
#[must_use]
pub fn discover_defs(paths: &Paths) -> Vec<DefEntry> {
    let mut defs = builtin_defs();
    scan_dir(&paths.config_dir().join("games"), &mut defs);
    scan_dir(&paths.data_dir().join("plugins"), &mut defs);
    defs
}

/// Find one definition by its stable id.
#[must_use]
pub fn find_def(paths: &Paths, id: &str) -> Option<DefEntry> {
    discover_defs(paths).into_iter().find(|e| e.def.id == id)
}

/// Scan a directory of definitions: `<dir>/<name>.toml` files or
/// `<dir>/<id>/game.toml` folders (the plugin layout).
fn scan_dir(dir: &Path, defs: &mut Vec<DefEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten().take(MAX_DEF_SCAN) {
        let path = entry.path();
        let file = if path.is_dir() {
            path.join("game.toml")
        } else {
            path
        };
        if file.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        if let Some(found) = entry_from_toml(&text, Some(file)) {
            upsert(defs, found);
        }
    }
}

/// Insert or replace by definition id.
fn upsert(defs: &mut Vec<DefEntry>, entry: DefEntry) {
    match defs.iter_mut().find(|e| e.def.id == entry.def.id) {
        Some(existing) => *existing = entry,
        None => defs.push(entry),
    }
}

fn entry_from_toml(text: &str, source: Option<PathBuf>) -> Option<DefEntry> {
    let origin = source
        .clone()
        .unwrap_or_else(|| PathBuf::from("<built-in>"));
    let def = GameDef::from_toml_str(text, &origin).ok()?;
    Some(DefEntry {
        def,
        toml: text.to_owned(),
        source,
        lua: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_include_the_shipped_games() {
        let ids: Vec<String> = builtin_defs().into_iter().map(|e| e.def.id).collect();
        // The catalog grows as games are added; these two are always present.
        assert!(ids.iter().any(|id| id == "skyrimse"), "got: {ids:?}");
        assert!(ids.iter().any(|id| id == "subnautica"), "got: {ids:?}");
    }

    #[test]
    fn every_builtin_definition_parses() {
        // build.rs embeds each games/<id>/game.toml verbatim; all must be valid.
        let defs = builtin_defs();
        assert!(!defs.is_empty(), "at least the shipped games must embed");
        for entry in &defs {
            assert!(!entry.def.id.trim().is_empty());
            assert!(!entry.def.name.trim().is_empty());
        }
    }

    #[test]
    fn all_bundled_game_tomls_parse_and_have_unique_ids() {
        // The catalog's builtin_defs() silently drops a malformed def (so one
        // bad plugin can't break the app at runtime); this test is the
        // dev-time gate that every shipped def is actually valid, with the
        // offending file named on failure.
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games");
        let mut ids: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&games).unwrap().flatten() {
            let toml = entry.path().join("game.toml");
            if !toml.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&toml).unwrap();
            let def = GameDef::from_toml_str(&text, &toml)
                .unwrap_or_else(|e| panic!("{}: {e}", toml.display()));
            // The folder name must match the def id (so build.rs embed order and
            // on-disk override-by-id line up).
            let folder = entry.file_name().to_string_lossy().into_owned();
            assert_eq!(def.id, folder, "id must equal folder name in {folder}");
            ids.push(def.id);
        }
        let mut seen = std::collections::HashSet::new();
        for id in &ids {
            assert!(seen.insert(id.as_str()), "duplicate game id: {id}");
        }
        assert!(ids.len() >= 2, "expected the shipped games at least");
    }

    #[test]
    fn user_and_plugin_defs_extend_and_override_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::rooted_at(tmp.path());
        // A user def adds a new game...
        let user = paths.config_dir().join("games/mygame");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            user.join("game.toml"),
            "api_version = 2\nid = \"mygame\"\nname = \"My Game\"\n",
        )
        .unwrap();
        // ...and an installed plugin overrides a built-in by id.
        let plugin = paths.data_dir().join("plugins/skyrimse");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("game.toml"),
            "api_version = 2\nid = \"skyrimse\"\nname = \"Skyrim SE (updated)\"\n",
        )
        .unwrap();

        let defs = discover_defs(&paths);
        let names: Vec<&str> = defs.iter().map(|e| e.def.name.as_str()).collect();
        assert!(names.contains(&"My Game"));
        assert!(names.contains(&"Skyrim SE (updated)"));
        // Replaced, not duplicated.
        assert_eq!(defs.iter().filter(|e| e.def.id == "skyrimse").count(), 1);
        assert!(find_def(&paths, "mygame").is_some());
    }
}
