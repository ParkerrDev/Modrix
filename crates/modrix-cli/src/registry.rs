// SPDX-License-Identifier: GPL-2.0-only
//! `modrix registry …` and `modrix plugin validate` - the community plugin
//! registry from the command line.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use modrix_core::Engine;
use modrix_registry::{RegistryClient, RegistrySource};

/// Registry operations.
#[derive(Subcommand)]
pub enum RegistryCmd {
    /// List installed plugins.
    List,
    /// Search the registry index.
    Search {
        /// Substring to match against id, name, or Nexus domain.
        query: Option<String>,
        /// Refetch the index even if the cache is fresh.
        #[arg(long)]
        refresh: bool,
    },
    /// Show one registry entry in detail.
    Info {
        /// The plugin id.
        id: String,
    },
    /// Install (or update) a plugin from the registry.
    Install {
        /// The plugin id.
        id: String,
    },
    /// Remove an installed plugin (refused while a registered game uses it).
    Uninstall {
        /// The plugin id.
        id: String,
    },
    /// Update every installed plugin that the registry has a newer version of.
    Update,
    /// Remove installed plugins no registered game references.
    Gc,
}

/// Plugin-development operations.
#[derive(Subcommand)]
pub enum PluginCmd {
    /// Validate a plugin directory (manifest, definition, hashes, game.lua) -
    /// what the registry's PR gate runs.
    Validate {
        /// The plugin directory (holding plugin.toml + game.toml).
        dir: PathBuf,
    },
    /// Print the sha256 + size of every file in a plugin directory, in the
    /// registry index's format - for filling in a submission.
    Hash {
        /// The plugin directory.
        dir: PathBuf,
    },
}

/// Run one registry command (spins a small runtime for the async fetches).
pub fn registry_cmd(cmd: &RegistryCmd, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let client = RegistryClient::new(RegistrySource::resolve(), engine.paths())
        .context("initializing the registry client")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;
    match cmd {
        RegistryCmd::List => {
            for p in client.installed() {
                writeln!(out, "{}\t{}\t{}", p.id, p.version, p.name)?;
            }
            Ok(())
        }
        RegistryCmd::Search { query, refresh } => {
            let index = runtime.block_on(client.index(*refresh))?;
            search_out(&client, &index, query.as_deref().unwrap_or(""), out)
        }
        RegistryCmd::Info { id } => {
            let index = runtime.block_on(client.index(false))?;
            info_out(find(&index, id)?, out)
        }
        RegistryCmd::Install { id } => {
            let index = runtime.block_on(client.index(false))?;
            let manifest = runtime.block_on(client.install(find(&index, id)?))?;
            writeln!(out, "installed {} {}", manifest.id, manifest.version)?;
            Ok(())
        }
        RegistryCmd::Uninstall { id } => {
            client.uninstall(id, &referenced(engine)?)?;
            writeln!(out, "removed {id}")?;
            Ok(())
        }
        RegistryCmd::Update => {
            let index = runtime.block_on(client.index(true))?;
            update_all(&runtime, &client, &index, out)
        }
        RegistryCmd::Gc => {
            let removed = client.gc(&referenced(engine)?)?;
            match removed.len() {
                0 => writeln!(out, "nothing to remove")?,
                n => writeln!(out, "removed {n} unused plugin(s): {}", removed.join(", "))?,
            }
            Ok(())
        }
    }
}

fn find<'a>(
    index: &'a modrix_registry::Index,
    id: &str,
) -> Result<&'a modrix_registry::IndexEntry> {
    index
        .plugins
        .iter()
        .find(|p| p.id == id)
        .with_context(|| format!("plugin `{id}` not found in the registry"))
}

fn search_out(
    client: &RegistryClient,
    index: &modrix_registry::Index,
    query: &str,
    out: &mut dyn Write,
) -> Result<()> {
    let installed = client.installed();
    for entry in RegistryClient::search(index, query) {
        let mark = match installed.iter().find(|p| p.id == entry.id) {
            Some(p) if p.version == entry.version => "installed",
            Some(_) => "update available",
            None => "",
        };
        writeln!(
            out,
            "{}\t{}\t{}\t{mark}",
            entry.id, entry.version, entry.name
        )?;
    }
    Ok(())
}

fn info_out(entry: &modrix_registry::IndexEntry, out: &mut dyn Write) -> Result<()> {
    writeln!(out, "id: {}", entry.id)?;
    writeln!(out, "name: {}", entry.name)?;
    writeln!(out, "version: {}", entry.version)?;
    writeln!(out, "api_version: {}", entry.api_version)?;
    writeln!(out, "lua: {}\tskills: {}", entry.has_lua, entry.has_skill)?;
    for file in &entry.files {
        writeln!(out, "  {} ({} bytes)", file.path, file.size)?;
    }
    Ok(())
}

fn update_all(
    runtime: &tokio::runtime::Runtime,
    client: &RegistryClient,
    index: &modrix_registry::Index,
    out: &mut dyn Write,
) -> Result<()> {
    let mut updated = 0_usize;
    for installed in client.installed() {
        let newer = index
            .plugins
            .iter()
            .find(|e| e.id == installed.id && e.version != installed.version);
        if let Some(entry) = newer {
            runtime.block_on(client.install(entry))?;
            writeln!(out, "updated {} -> {}", installed.id, entry.version)?;
            updated = updated.saturating_add(1);
        }
    }
    writeln!(out, "{updated} plugin(s) updated")?;
    Ok(())
}

/// Plugin ids of currently registered games (they anchor their plugins).
fn referenced(engine: &Engine) -> Result<Vec<String>> {
    Ok(engine.games()?.into_iter().map(|g| g.plugin_id).collect())
}

/// Validate a plugin directory: manifest parses, ids agree, the definition is
/// valid at its declared `api_version`, and any `game.lua` loads in the
/// sandbox. This is what the registry's PR gate runs.
pub fn plugin_cmd(cmd: &PluginCmd, out: &mut dyn Write) -> Result<()> {
    let dir = match cmd {
        PluginCmd::Hash { dir } => {
            for (path, sha256, size) in hash_dir(dir) {
                writeln!(
                    out,
                    "{{ path = \"{path}\", sha256 = \"{sha256}\", size = {size} }}"
                )?;
            }
            return Ok(());
        }
        PluginCmd::Validate { dir } => dir,
    };
    let manifest_path = dir.join("plugin.toml");
    let manifest: modrix_registry::PluginManifest =
        toml::from_str(&std::fs::read_to_string(&manifest_path).context("reading plugin.toml")?)
            .context("parsing plugin.toml")?;
    let def =
        modrix_core::GameDef::from_file(&dir.join("game.toml")).context("validating game.toml")?;
    check(manifest.id == def.id, "plugin.toml id matches game.toml id")?;
    check(
        manifest.api_version == def.api_version,
        "plugin.toml api_version matches game.toml",
    )?;
    check(!manifest.version.trim().is_empty(), "version is set")?;
    if dir.join("game.lua").exists() {
        let logic = modrix_plugin::lua::LuaGameLogic::load(dir, &def)
            .context("loading game.lua in the sandbox")?;
        check(logic.is_some(), "game.lua loads")?;
        // A dry detect() proves the script parses and runs under budget.
        if let Some(logic) = logic {
            use modrix_core::logic::GameLogic as _;
            logic.detect().context("running detect() under budget")?;
        }
    }
    writeln!(out, "ok: {} {} validates", manifest.id, manifest.version)?;
    Ok(())
}

fn check(condition: bool, what: &str) -> Result<()> {
    if !condition {
        bail!("validation failed: {what}");
    }
    Ok(())
}

/// Compute the `[files]`/index records for a plugin dir - used by
/// `scripts/gen_index.py` parity tests and local tooling.
#[must_use]
pub fn hash_dir(dir: &Path) -> Vec<(String, String, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten().take(256) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(dir)
                && let Ok(hash) = modrix_core::sha256_file(&path)
            {
                let size = std::fs::metadata(&path).map_or(0, |m| m.len());
                out.push((rel.to_string_lossy().replace('\\', "/"), hash, size));
            }
        }
    }
    out.sort();
    out
}
