// SPDX-License-Identifier: GPL-2.0-only
//! Modrix plugin host.
//!
//! Two tiers: declarative `game.toml` game definitions (the ~80% case, no
//! code; parsed in `modrix-core`) and [`lua`] - `game.lua` scripts under a
//! locked-down sandbox for games that need custom logic, implementing core's
//! [`modrix_core::logic::GameLogic`] seam. Also hosts the [`fomod`] installer
//! engine: parsing `ModuleConfig.xml`, computing default selections, and
//! materializing chosen options into a staged tree.

pub mod fomod;
pub mod lua;

/// Discover every installed definition that ships a `game.lua` and register
/// its sandboxed logic with the engine. Call at boot, before the engine is
/// shared. Scripts that fail to load are skipped with a warning - a broken
/// plugin must not take the whole application down.
pub fn register_lua_logic(engine: &mut modrix_core::Engine) {
    let entries = modrix_core::defcat::discover_defs(engine.paths());
    for entry in entries {
        let Some(source) = entry.source.as_deref() else {
            continue; // Built-ins are compiled in and ship no script.
        };
        let Some(dir) = source.parent() else {
            continue;
        };
        match lua::LuaGameLogic::load(dir, &entry.def) {
            Ok(Some(logic)) => {
                tracing::info!(plugin = %entry.def.id, "registered game.lua logic");
                engine.register_logic(&entry.def.id, std::sync::Arc::new(logic));
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(plugin = %entry.def.id, %error, "skipping broken game.lua");
            }
        }
    }
}

/// Plugin-host errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A FOMOD installer could not be parsed or applied.
    #[error("fomod {path}: {message}")]
    Fomod {
        /// The offending file or tree.
        path: std::path::PathBuf,
        /// What went wrong.
        message: String,
    },
}

/// Plugin-host result.
pub type Result<T> = std::result::Result<T, Error>;
