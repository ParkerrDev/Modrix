// SPDX-License-Identifier: GPL-2.0-only
//! The sandboxed Lua host: Tier-2 game logic (`game.lua`).
//!
//! A game plugin may ship a `game.lua` beside its `game.toml` defining any of
//! the callbacks `detect()`, `mod_root(install)`, `install(ctx)`, and
//! `load_order(plugins)`. This module runs them under a strict sandbox and
//! implements [`modrix_core::logic::GameLogic`] over the results, so the
//! engine consumes plugins through the same seam whatever language they are
//! written in.
//!
//! Sandbox contract (docs/ARCHITECTURE.md §5.2):
//! - Only the `table`, `string`, and `math` standard libraries load; `io`,
//!   `os`, `debug`, `package`/`require`, and the base library's `load`/
//!   `dofile`/`loadfile` escape hatches are absent or removed.
//! - Filesystem access goes through `modrix.fs.*`, jailed to the tree the
//!   callback is about (the extracted archive), read-only, size-capped.
//! - Plugins return plans - `modrix.fs.stage(src, dest)` records intent; core
//!   validates and applies it. Nothing in here writes a file.
//! - Every invocation runs a **fresh VM** with an instruction budget, a wall
//!   clock budget, and a memory limit, so a hostile script cannot hang or
//!   exhaust the host (Power of Ten: bounded everything).

use std::cell::{Cell, RefCell};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Lua, LuaOptions, StdLib, Value, VmState};
use modrix_core::logic::{GameLogic, StageEntry, StagePlan};
use modrix_core::{Error as CoreError, GameDef};

/// Largest `game.lua` the host will load.
const MAX_SCRIPT_BYTES: u64 = 512 * 1024;
/// The hook fires every this many VM instructions.
const HOOK_EVERY: u32 = 10_000;
/// Total VM instructions one callback may execute.
const MAX_INSTRUCTIONS: u64 = 5_000_000;
/// Wall-clock budget for one callback.
const MAX_WALL: Duration = Duration::from_millis(250);
/// Lua heap ceiling.
const MAX_MEMORY: usize = 64 * 1024 * 1024;
/// Most entries `modrix.fs.read_dir` returns.
const MAX_DIR_ENTRIES: usize = 4096;
/// Largest file `modrix.fs.read_text` will read.
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

/// Tier-2 logic backed by a `game.lua` script.
pub struct LuaGameLogic {
    plugin_id: String,
    script: String,
    game: GameSummary,
}

/// The read-only `modrix.game` context exposed to scripts.
struct GameSummary {
    id: String,
    name: String,
    mod_root: String,
    steam_appid: Option<i64>,
}

impl LuaGameLogic {
    /// Load the `game.lua` beside a definition, if one exists. `None` when
    /// the plugin is purely declarative.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Plugin`] if the script exists but cannot be read
    /// or is over the size cap.
    pub fn load(dir: &Path, def: &GameDef) -> Result<Option<Self>, CoreError> {
        let path = dir.join("game.lua");
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(None);
        };
        if meta.len() > MAX_SCRIPT_BYTES {
            return Err(plugin_err(&def.id, "game.lua exceeds the size cap"));
        }
        let script = std::fs::read_to_string(&path)
            .map_err(|e| plugin_err(&def.id, &format!("reading game.lua: {e}")))?;
        Ok(Some(Self {
            plugin_id: def.id.clone(),
            script,
            game: GameSummary {
                id: def.id.clone(),
                name: def.name.clone(),
                mod_root: def.mod_root.clone(),
                steam_appid: def.steam_appid,
            },
        }))
    }

    /// Run one callback in a fresh sandboxed VM. `jail` (when given) roots
    /// `modrix.fs`; `staged` collects `modrix.fs.stage` calls.
    fn call<R: mlua::FromLuaMulti>(
        &self,
        name: &str,
        args: impl mlua::IntoLuaMulti,
        jail: Option<&Path>,
        staged: &Rc<RefCell<Vec<StageEntry>>>,
    ) -> Result<Option<R>, CoreError> {
        let lua = self
            .sandbox(jail, staged)
            .map_err(|e| plugin_err(&self.plugin_id, &e.to_string()))?;
        arm_budget(&lua);
        lua.load(&self.script)
            .set_name(format!("{}/game.lua", self.plugin_id))
            .exec()
            .map_err(|e| plugin_err(&self.plugin_id, &e.to_string()))?;
        let func: Value = lua
            .globals()
            .get(name)
            .map_err(|e| plugin_err(&self.plugin_id, &e.to_string()))?;
        let Value::Function(func) = func else {
            return Ok(None); // Callback not defined: fall back to defaults.
        };
        let out = func
            .call::<R>(args)
            .map_err(|e| plugin_err(&self.plugin_id, &format!("{name}: {e}")))?;
        Ok(Some(out))
    }

    /// A fresh VM with the sandbox applied and the `modrix` table installed.
    fn sandbox(
        &self,
        jail: Option<&Path>,
        staged: &Rc<RefCell<Vec<StageEntry>>>,
    ) -> mlua::Result<Lua> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH,
            LuaOptions::default(),
        )?;
        lua.set_memory_limit(MAX_MEMORY)?;
        // The base library always loads; remove its escape hatches. `print`
        // goes too - plugins log through modrix.log, never stdout.
        let globals = lua.globals();
        for name in ["dofile", "loadfile", "load", "require", "print"] {
            globals.set(name, Value::Nil)?;
        }
        globals.set("modrix", self.modrix_table(&lua, jail, staged)?)?;
        Ok(lua)
    }

    /// Build the `modrix` API table.
    fn modrix_table(
        &self,
        lua: &Lua,
        jail: Option<&Path>,
        staged: &Rc<RefCell<Vec<StageEntry>>>,
    ) -> mlua::Result<mlua::Table> {
        let modrix = lua.create_table()?;
        let game = lua.create_table()?;
        game.set("id", self.game.id.as_str())?;
        game.set("name", self.game.name.as_str())?;
        game.set("mod_root", self.game.mod_root.as_str())?;
        game.set("steam_appid", self.game.steam_appid)?;
        modrix.set("game", game)?;
        modrix.set("log", log_table(lua, &self.plugin_id)?)?;
        if let Some(root) = jail {
            modrix.set("fs", fs_table(lua, root, staged)?)?;
        }
        Ok(modrix)
    }
}

impl GameLogic for LuaGameLogic {
    fn detect(&self) -> modrix_core::Result<Option<PathBuf>> {
        let staged = Rc::new(RefCell::new(Vec::new()));
        let found: Option<Option<String>> = self.call("detect", (), None, &staged)?;
        Ok(found.flatten().map(PathBuf::from))
    }

    fn mod_root(&self, install: &Path) -> modrix_core::Result<Option<String>> {
        let staged = Rc::new(RefCell::new(Vec::new()));
        let out: Option<Option<String>> = self.call(
            "mod_root",
            install.to_string_lossy().into_owned(),
            Some(install),
            &staged,
        )?;
        Ok(out.flatten())
    }

    fn install(&self, archive_root: &Path) -> modrix_core::Result<Option<StagePlan>> {
        let staged = Rc::new(RefCell::new(Vec::new()));
        let handled: Option<bool> = self
            .call::<Option<bool>>("install", (), Some(archive_root), &staged)?
            .flatten();
        // The callback stages via modrix.fs.stage and returns true to take
        // over; nil/false (or no callback) falls back to normalization.
        if handled != Some(true) {
            return Ok(None);
        }
        let plan = StagePlan {
            entries: staged.borrow().clone(),
        };
        plan.validate()?;
        Ok(Some(plan))
    }

    fn load_order(&self, plugins: &[String]) -> modrix_core::Result<Option<Vec<String>>> {
        let staged = Rc::new(RefCell::new(Vec::new()));
        let out: Option<Option<Vec<String>>> =
            self.call("load_order", plugins.to_vec(), None, &staged)?;
        Ok(out.flatten())
    }
}

/// Abort the VM when the instruction, wall-clock, or (via allocator) memory
/// budget is exhausted.
fn arm_budget(lua: &Lua) {
    let start = Instant::now();
    let instructions = Cell::new(0_u64);
    lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(HOOK_EVERY),
        move |_lua, _debug| {
            instructions.set(instructions.get().saturating_add(u64::from(HOOK_EVERY)));
            if instructions.get() > MAX_INSTRUCTIONS {
                return Err(mlua::Error::RuntimeError(
                    "instruction budget exhausted".to_owned(),
                ));
            }
            if start.elapsed() > MAX_WALL {
                return Err(mlua::Error::RuntimeError(
                    "time budget exhausted".to_owned(),
                ));
            }
            Ok(VmState::Continue)
        },
    );
}

/// `modrix.log.{debug,info,warn}` - tracing, tagged with the plugin id.
fn log_table(lua: &Lua, plugin_id: &str) -> mlua::Result<mlua::Table> {
    let log = lua.create_table()?;
    for (name, level) in [("debug", 0_u8), ("info", 1), ("warn", 2)] {
        let id = plugin_id.to_owned();
        log.set(
            name,
            lua.create_function(move |_, message: String| {
                match level {
                    0 => tracing::debug!(plugin = %id, "{message}"),
                    1 => tracing::info!(plugin = %id, "{message}"),
                    _ => tracing::warn!(plugin = %id, "{message}"),
                }
                Ok(())
            })?,
        )?;
    }
    Ok(log)
}

/// `modrix.fs.{stage,exists,read_dir,read_text}` - read-only, jailed to
/// `root`, size-capped. `stage` records intent only.
fn fs_table(
    lua: &Lua,
    root: &Path,
    staged: &Rc<RefCell<Vec<StageEntry>>>,
) -> mlua::Result<mlua::Table> {
    let fs = lua.create_table()?;
    let sink = Rc::clone(staged);
    fs.set(
        "stage",
        lua.create_function(move |_, (src, dest): (String, String)| {
            let mut entries = sink.borrow_mut();
            if entries.len() >= modrix_core::logic::MAX_PLAN_ENTRIES {
                return Err(mlua::Error::RuntimeError("stage plan too large".to_owned()));
            }
            entries.push(StageEntry { src, dest });
            Ok(())
        })?,
    )?;
    let base = root.to_path_buf();
    fs.set(
        "exists",
        lua.create_function(move |_, rel: String| Ok(jailed(&base, &rel)?.exists()))?,
    )?;
    let base = root.to_path_buf();
    fs.set(
        "read_dir",
        lua.create_function(move |_, rel: String| {
            let dir = jailed(&base, &rel)?;
            let mut names = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten().take(MAX_DIR_ENTRIES) {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
            names.sort_unstable();
            Ok(names)
        })?,
    )?;
    let base = root.to_path_buf();
    fs.set(
        "read_text",
        lua.create_function(move |_, rel: String| {
            let path = jailed(&base, &rel)?;
            let size = std::fs::metadata(&path).map_or(0, |m| m.len());
            if size > MAX_TEXT_BYTES {
                return Err(mlua::Error::RuntimeError("file too large".to_owned()));
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => Ok(Some(text)),
                Err(_) => Ok(None),
            }
        })?,
    )?;
    Ok(fs)
}

/// Resolve a script-supplied relative path inside the jail; reject absolute
/// paths and parent traversal. `""` is the jail root itself.
fn jailed(root: &Path, rel: &str) -> mlua::Result<PathBuf> {
    let p = Path::new(rel);
    let escapes = p.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if escapes {
        return Err(mlua::Error::RuntimeError(format!(
            "path escapes the sandbox: {rel}"
        )));
    }
    Ok(root.join(p))
}

fn plugin_err(plugin: &str, message: &str) -> CoreError {
    CoreError::Plugin {
        plugin: plugin.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logic(script: &str) -> LuaGameLogic {
        LuaGameLogic {
            plugin_id: "testgame".to_owned(),
            script: script.to_owned(),
            game: GameSummary {
                id: "testgame".to_owned(),
                name: "Test".to_owned(),
                mod_root: "Data".to_owned(),
                steam_appid: Some(42),
            },
        }
    }

    #[test]
    fn undefined_callbacks_fall_back_to_defaults() {
        let l = logic("-- no callbacks");
        assert_eq!(l.detect().unwrap(), None);
        assert!(l.install(Path::new("/nonexistent")).unwrap().is_none());
    }

    #[test]
    fn the_sandbox_has_no_escape_hatches() {
        // Every ambient authority a script could reach must be nil.
        let l = logic(
            r#"
            function detect()
                assert(io == nil, "io leaked")
                assert(os == nil, "os leaked")
                assert(require == nil, "require leaked")
                assert(package == nil, "package leaked")
                assert(debug == nil, "debug leaked")
                assert(dofile == nil, "dofile leaked")
                assert(loadfile == nil, "loadfile leaked")
                assert(load == nil, "load leaked")
                assert(print == nil, "print leaked")
                return nil
            end
            "#,
        );
        assert_eq!(l.detect().unwrap(), None);
    }

    #[test]
    fn the_game_context_is_visible() {
        let l = logic(
            r#"
            function detect()
                if modrix.game.id == "testgame" and modrix.game.steam_appid == 42 then
                    return "/found/it"
                end
                return nil
            end
            "#,
        );
        assert_eq!(l.detect().unwrap(), Some(PathBuf::from("/found/it")));
    }

    #[test]
    fn an_infinite_loop_is_aborted_by_the_budget() {
        let l = logic("function detect() while true do end end");
        let err = l.detect().unwrap_err();
        assert!(err.to_string().contains("budget exhausted"), "got: {err}");
    }

    #[test]
    fn a_memory_bomb_is_stopped_by_the_heap_limit() {
        let l = logic(
            "function detect()\n  local t = {}\n  local i = 1\n  while true do t[i] = string.rep('x', 65536) i = i + 1 end\nend",
        );
        assert!(l.detect().is_err());
    }

    #[test]
    fn fs_is_jailed_and_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("inside.txt"), "hello").unwrap();
        let l = logic(
            r#"
            function install()
                assert(modrix.fs.exists("inside.txt"), "jail content visible")
                assert(modrix.fs.read_text("inside.txt") == "hello")
                local ok = pcall(modrix.fs.read_text, "../outside.txt")
                assert(not ok, "parent traversal must fail")
                local ok2 = pcall(modrix.fs.exists, "/etc/passwd")
                assert(not ok2, "absolute paths must fail")
                return false
            end
            "#,
        );
        assert!(l.install(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn install_collects_a_validated_stage_plan() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Wrapper")).unwrap();
        std::fs::write(tmp.path().join("Wrapper/mod.dll"), "x").unwrap();
        let l = logic(
            r#"
            function install()
                for _, name in ipairs(modrix.fs.read_dir("")) do
                    if name == "Wrapper" then
                        modrix.fs.stage("Wrapper/mod.dll", "MyMod/mod.dll")
                    end
                end
                return true
            end
            "#,
        );
        let plan = l.install(tmp.path()).unwrap().expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].dest, "MyMod/mod.dll");
    }

    #[test]
    fn a_plan_with_escaping_paths_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let l = logic(
            r#"
            function install()
                modrix.fs.stage("a.txt", "../../outside.txt")
                return true
            end
            "#,
        );
        assert!(l.install(tmp.path()).is_err());
    }

    #[test]
    fn the_engine_stages_through_a_registered_lua_plan() {
        // End to end: a plugin's install() plan shapes the staged tree via
        // core's own validated apply, replacing default normalization.
        let tmp = tempfile::tempdir().unwrap();
        let paths = modrix_core::Paths::rooted_at(tmp.path());
        let mut engine = modrix_core::Engine::open(&paths).unwrap();
        let def = GameDef::from_toml_str(
            "api_version = 2\nid = \"luagame\"\nname = \"Lua Game\"\n",
            Path::new("<test>"),
        )
        .unwrap();
        let script = r#"
            function install()
                for _, name in ipairs(modrix.fs.read_dir("")) do
                    if modrix.fs.exists(name .. "/plugin.dll") then
                        modrix.fs.stage(name .. "/plugin.dll", "Organized/" .. name .. "/plugin.dll")
                    end
                end
                return true
            end
        "#;
        let mut lua_logic = logic(script);
        lua_logic.plugin_id = "luagame".to_owned();
        engine.register_logic("luagame", std::sync::Arc::new(lua_logic));

        let install_dir = tmp.path().join("game");
        std::fs::create_dir_all(&install_dir).unwrap();
        let game = engine.add_game(&def, &install_dir, "manual").unwrap();

        let src = tmp.path().join("srcmod/SomeMod");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("plugin.dll"), b"x").unwrap();
        std::fs::write(tmp.path().join("srcmod/readme.txt"), b"r").unwrap();
        let staged = engine
            .stage(game.id, "planned", tmp.path().join("srcmod").as_path())
            .unwrap();

        assert!(
            staged
                .staged_path
                .join("Organized/SomeMod/plugin.dll")
                .is_file(),
            "the plan's layout must win"
        );
        assert!(!staged.staged_path.join("readme.txt").exists());
    }

    #[test]
    fn load_order_returns_a_reordering() {
        let l = logic(
            "
            function load_order(plugins)
                table.sort(plugins, function(a, b) return a > b end)
                return plugins
            end
            ",
        );
        let out = l
            .load_order(&["a.esp".to_owned(), "b.esp".to_owned()])
            .unwrap()
            .expect("reordered");
        assert_eq!(out, vec!["b.esp", "a.esp"]);
    }
}
