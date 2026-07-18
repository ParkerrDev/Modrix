// SPDX-License-Identifier: GPL-2.0-only
//! Embed every bundled game definition into the binary.
//!
//! Scans the workspace `games/<id>/` directory at build time and generates
//! `BUILTIN_GAME_DEFS`: the `game.toml` text of each game plus its optional
//! `game.lua`. `defcat.rs` includes the generated file, so dropping a new
//! `games/<id>/game.toml` into the tree ships it out of the box - no code
//! change and no per-game `include_str!` list to maintain.

use std::path::PathBuf;

fn main() {
    let Ok(crate_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let games_dir = PathBuf::from(&crate_dir).join("../../games");
    let generated = PathBuf::from(&out_dir).join("builtin_defs.rs");

    // Rebuild when a game is added or removed (directory listing changes).
    println!("cargo:rerun-if-changed={}", games_dir.display());

    let mut subdirs: Vec<PathBuf> = match std::fs::read_dir(&games_dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(_) => Vec::new(),
    };
    // Deterministic embed order (a Modrix invariant).
    subdirs.sort();

    let mut entries: Vec<String> = Vec::new();
    for sub in &subdirs {
        let toml_path = sub.join("game.toml");
        let Ok(toml_body) = std::fs::read_to_string(&toml_path) else {
            continue;
        };
        // Rebuild when this game's files change.
        println!("cargo:rerun-if-changed={}", sub.display());
        println!("cargo:rerun-if-changed={}", toml_path.display());
        let lua_path = sub.join("game.lua");
        let lua_lit = match std::fs::read_to_string(&lua_path) {
            Ok(body) => {
                println!("cargo:rerun-if-changed={}", lua_path.display());
                format!("Some({body:?})")
            }
            Err(_) => "None".to_owned(),
        };
        // `{:?}` on a &str emits a valid, fully-escaped Rust string literal, so
        // arbitrary TOML/Lua content embeds safely regardless of platform.
        entries.push(format!(
            "    BuiltinDef {{ toml: {toml_body:?}, lua: {lua_lit} }},"
        ));
    }

    let body = format!(
        "struct BuiltinDef {{ toml: &'static str, lua: Option<&'static str> }}\n\
         static BUILTIN_GAME_DEFS: &[BuiltinDef] = &[\n{}\n];\n",
        entries.join("\n")
    );
    let _ = std::fs::write(&generated, body);
}
