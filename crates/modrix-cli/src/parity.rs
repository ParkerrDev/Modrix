// SPDX-License-Identifier: GPL-2.0-only
//! Full engine parity: every capability a GUI user has, as subcommands -
//! conflicts and rules, plugin (.esp) load order, health, external mods,
//! capabilities, FOMOD configuration, and live-instance downloads. Built for
//! both humans and agents: `--json` wraps results in a stable
//! `{"ok":true,"data":…}` envelope.

use std::io::Write;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use modrix_core::{Engine, Game, ModId};
use modrix_plugin::fomod;

use crate::output::emit;
use crate::{Cli, resolve_game, resolve_mod};

/// Plugin (.esp/.esm/.esl) load-order operations.
#[derive(Subcommand)]
pub enum PluginsCmd {
    /// Show the profile's plugin load order.
    List,
    /// Set the full order: plugin names first-to-last (disabled keep state).
    Order {
        /// Plugin filenames in the desired order.
        names: Vec<String>,
    },
    /// Auto-sort (masters before dependents, master tier first).
    AutoSort,
    /// Rewrite the game's Plugins.txt from the current order.
    Sync,
}

/// Conflict rules between mods.
#[derive(Subcommand)]
pub enum RuleCmd {
    /// List the profile's rules.
    List,
    /// Add/replace a rule: `winner`'s files override `loser`'s.
    Set {
        /// The overridden mod (id or name).
        loser: String,
        /// The overriding mod (id or name).
        winner: String,
    },
    /// Remove the rule between two mods.
    Clear {
        /// One mod (id or name).
        a: String,
        /// The other mod (id or name).
        b: String,
    },
}

/// Per-file overrides (pin one contested path to a provider).
#[derive(Subcommand)]
pub enum OverrideCmd {
    /// Pin a target path to a providing mod.
    Set {
        /// Target path relative to the deploy root.
        target: String,
        /// The providing mod (id or name).
        provider: String,
    },
    /// Return a pinned path to rule-based resolution.
    Clear {
        /// Target path relative to the deploy root.
        target: String,
    },
}

/// Downloads on the live Modrix instance (GUI or `modrix serve`).
#[derive(Subcommand)]
pub enum DownloadsCmd {
    /// List all downloads.
    List,
    /// One download's status.
    Status {
        /// The download id.
        id: u64,
    },
    /// Cancel a download.
    Cancel {
        /// The download id.
        id: u64,
    },
}

/// FOMOD installer options.
#[derive(Subcommand)]
pub enum FomodCmd {
    /// Show a mod's installer steps, groups, and options (with defaults).
    Show {
        /// Mod id or name.
        module: String,
    },
    /// Apply choices: JSON `[{"step":0,"group":0,"picks":[0,2]}, …]`;
    /// unspecified groups keep their defaults.
    Apply {
        /// Mod id or name.
        module: String,
        /// The choices document (JSON text).
        #[arg(long)]
        choices: String,
    },
}

/// `game detect|active|set-active|capabilities`.
pub fn game_extra(action: &str, cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    match action {
        "detect" => {
            let found: Vec<serde_json::Value> = modrix_core::defcat::discover_defs(engine.paths())
                .iter()
                .filter_map(|entry| {
                    modrix_core::detect::detect_install(&entry.def).map(|path| {
                        serde_json::json!({
                            "id": entry.def.id,
                            "name": entry.def.name,
                            "install": path,
                            "steam_appid": entry.def.steam_appid,
                        })
                    })
                })
                .collect();
            emit(out, cli.json, &found, |out, found| {
                for game in found {
                    writeln!(
                        out,
                        "{}\t{}",
                        game["id"].as_str().unwrap_or("?"),
                        game["install"].as_str().unwrap_or("?")
                    )?;
                }
                Ok(())
            })
        }
        "active" => {
            let active = engine.active_game()?;
            emit(out, cli.json, &active, |out, active| {
                match active {
                    Some(game) => writeln!(out, "{}\t{}\t{}", game.id, game.plugin_id, game.name)?,
                    None => writeln!(out, "no active game")?,
                }
                Ok(())
            })
        }
        "set-active" => {
            let game = resolve_game(cli, engine)?;
            engine.set_active_game(game.id)?;
            emit(out, cli.json, &game, |out, game| {
                writeln!(out, "active game: {}", game.name)?;
                Ok(())
            })
        }
        "capabilities" => {
            let game = resolve_game(cli, engine)?;
            let caps = engine.capabilities(game.id)?;
            emit(out, cli.json, &caps, |out, caps| {
                writeln!(
                    out,
                    "load_order: {}\nexternal_scan: {}\nhealth_checks: {}",
                    caps.load_order, caps.external_scan, caps.health_checks
                )?;
                Ok(())
            })
        }
        other => bail!("unknown game action `{other}`"),
    }
}

/// `plugins …` - the .esp load order.
pub fn plugins_cmd(
    cmd: &PluginsCmd,
    cli: &Cli,
    engine: &Engine,
    out: &mut dyn Write,
) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    match cmd {
        PluginsCmd::List => {
            let plugins = engine.plugins(profile.id)?;
            emit(out, cli.json, &plugins, |out, plugins| {
                for (i, p) in plugins.iter().enumerate() {
                    let mark = if p.enabled { "*" } else { " " };
                    let missing = if p.missing_masters.is_empty() {
                        String::new()
                    } else {
                        format!("\tMISSING: {}", p.missing_masters.join(", "))
                    };
                    writeln!(out, "{mark} {i:>3} {}\t({}){missing}", p.name, p.mod_name)?;
                }
                Ok(())
            })
        }
        PluginsCmd::Order { names } => {
            let current = engine.plugins(profile.id)?;
            let enabled: std::collections::HashMap<String, bool> = current
                .iter()
                .map(|p| (p.name.to_ascii_lowercase(), p.enabled))
                .collect();
            let order: Vec<(String, bool)> = names
                .iter()
                .map(|n| {
                    let on = enabled
                        .get(&n.to_ascii_lowercase())
                        .copied()
                        .unwrap_or(true);
                    (n.clone(), on)
                })
                .collect();
            engine.set_plugin_order(profile.id, &order)?;
            crate::output::ack(out, cli.json, &format!("{} plugins ordered", order.len()))
        }
        PluginsCmd::AutoSort => {
            let sorted = engine.auto_sort_plugins(profile.id)?;
            emit(out, cli.json, &sorted, |out, sorted| {
                writeln!(out, "sorted {} plugins", sorted.len())?;
                Ok(())
            })
        }
        PluginsCmd::Sync => {
            let dir = engine.sync_plugins_txt(profile.id)?;
            let data = serde_json::json!({ "written_to": dir });
            emit(out, cli.json, &data, |out, _| {
                match &dir {
                    Some(dir) => writeln!(out, "wrote {}", dir.display())?,
                    None => writeln!(out, "this game has no load-order file")?,
                }
                Ok(())
            })
        }
    }
}

/// `mod conflicts` - pairwise conflicts with rule state.
pub fn conflicts(cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    let conflicts = engine.mod_conflicts(profile.id)?;
    let mods = engine.mods(game.id)?;
    let name = |id: ModId| {
        mods.iter()
            .find(|m| m.id == id)
            .map_or_else(|| id.to_string(), |m| m.name.clone())
    };
    emit(out, cli.json, &conflicts, |out, conflicts| {
        for c in conflicts {
            let state = if c.resolved() {
                "resolved"
            } else {
                "UNRESOLVED"
            };
            writeln!(
                out,
                "{state}\t{} <-> {}\t{} file(s)",
                name(c.first),
                name(c.second),
                c.files.len()
            )?;
        }
        Ok(())
    })
}

/// `mod rule …`
pub fn rule_cmd(cmd: &RuleCmd, cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    match cmd {
        RuleCmd::List => {
            let rules = engine.mod_rules(profile.id)?;
            emit(out, cli.json, &rules, |out, rules| {
                for rule in rules {
                    writeln!(out, "{} loses to {}", rule.loser, rule.winner)?;
                }
                Ok(())
            })
        }
        RuleCmd::Set { loser, winner } => {
            let loser = resolve_mod(engine, &game, loser)?;
            let winner = resolve_mod(engine, &game, winner)?;
            engine.set_mod_rule(profile.id, loser.id, winner.id)?;
            crate::output::ack(
                out,
                cli.json,
                &format!("{} now overrides {}", winner.name, loser.name),
            )
        }
        RuleCmd::Clear { a, b } => {
            let a = resolve_mod(engine, &game, a)?;
            let b = resolve_mod(engine, &game, b)?;
            engine.clear_mod_rule(profile.id, a.id, b.id)?;
            crate::output::ack(out, cli.json, "rule cleared")
        }
    }
}

/// `mod override …`
pub fn override_cmd(
    cmd: &OverrideCmd,
    cli: &Cli,
    engine: &Engine,
    out: &mut dyn Write,
) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    match cmd {
        OverrideCmd::Set { target, provider } => {
            let provider = resolve_mod(engine, &game, provider)?;
            engine.set_file_override(profile.id, target, Some(provider.id))?;
            crate::output::ack(
                out,
                cli.json,
                &format!("{target} pinned to {}", provider.name),
            )
        }
        OverrideCmd::Clear { target } => {
            engine.set_file_override(profile.id, target, None)?;
            crate::output::ack(out, cli.json, &format!("{target} unpinned"))
        }
    }
}

/// `health` - issues + whether deploy is blocked.
pub fn health(cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    let issues = engine.health(profile.id)?;
    emit(out, cli.json, &issues, |out, issues| {
        if issues.is_empty() {
            writeln!(out, "healthy")?;
        }
        for issue in issues {
            let severity = match issue.severity {
                modrix_core::Severity::Error => "ERROR",
                modrix_core::Severity::Warning => "WARN",
                modrix_core::Severity::Info => "INFO",
            };
            let blocking = if issue.blocking {
                " [blocks deploy]"
            } else {
                ""
            };
            writeln!(out, "{severity}{blocking}\t{}", issue.message)?;
        }
        Ok(())
    })
}

/// `external list` - unmanaged mods in the game directory.
pub fn external(cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let externals = engine.external_mods(game.id)?;
    emit(out, cli.json, &externals, |out, externals| {
        for m in externals {
            writeln!(out, "{}\t{}\t{} file(s)", m.label, m.name, m.files)?;
        }
        Ok(())
    })
}

/// `mod hash <archive>` - content hash + already-installed check.
pub fn mod_hash(
    archive: &std::path::Path,
    cli: &Cli,
    engine: &Engine,
    out: &mut dyn Write,
) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let hash = modrix_core::sha256_file(archive).context("hashing the archive")?;
    let existing = engine.find_by_archive_hash(game.id, &hash)?;
    let data = serde_json::json!({ "sha256": hash, "already_installed": existing });
    emit(out, cli.json, &data, |out, _| {
        writeln!(out, "{hash}")?;
        for m in &existing {
            writeln!(out, "already installed as: {} ({})", m.name, m.id)?;
        }
        Ok(())
    })
}

/// `downloads …` - forwarded to the live instance over loopback IPC.
pub fn downloads_cmd(
    cmd: &DownloadsCmd,
    cli: &Cli,
    engine: &Engine,
    out: &mut dyn Write,
) -> Result<()> {
    let lockfile = engine.paths().instance_lock();
    let secondary = modrix_ipc::secondary_from_lock(&lockfile)
        .context("no running Modrix instance (open the GUI or `modrix serve`)")?;
    let path = match cmd {
        DownloadsCmd::List => "/downloads".to_owned(),
        DownloadsCmd::Status { id } => format!("/download/{id}"),
        DownloadsCmd::Cancel { id } => format!("/download/{id}/cancel"),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let reply = runtime.block_on(secondary.send(&path, ""))?;
    if reply.status != 200 {
        bail!("instance replied {}: {}", reply.status, reply.body);
    }
    if cli.json {
        // The service already speaks JSON; wrap it in the standard envelope.
        writeln!(out, "{{\"ok\":true,\"data\":{}}}", reply.body)?;
    } else {
        writeln!(out, "{}", reply.body)?;
    }
    Ok(())
}

/// `fomod …`
pub fn fomod_cmd(cmd: &FomodCmd, cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    match cmd {
        FomodCmd::Show { module } => fomod_show(module, cli, engine, out),
        FomodCmd::Apply { module, choices } => fomod_apply(module, choices, cli, engine, out),
    }
}

/// Resolve `(mod, installer, present-files)` for the selected game's mod.
fn parsed_installer(
    cli: &Cli,
    engine: &Engine,
    module: &str,
) -> Result<(modrix_core::Mod, fomod::Installer, fomod::Present)> {
    let game: Game = resolve_game(cli, engine)?;
    let m = resolve_mod(engine, &game, module)?;
    let installer = fomod::parse(&m.staged_path)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .context("this mod has no FOMOD installer")?;
    let present = modrix_service::present_files(engine, game.id);
    Ok((m, installer, present))
}

fn fomod_show(module: &str, cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let (_m, installer, present) = parsed_installer(cli, engine, module)?;
    let defaults = fomod::defaults(&installer, &present);
    let steps: Vec<serde_json::Value> = installer
        .steps
        .iter()
        .enumerate()
        .map(|(si, step)| {
            let groups: Vec<serde_json::Value> = step
                .groups
                .iter()
                .enumerate()
                .map(|(gi, group)| {
                    let plugins: Vec<serde_json::Value> = group
                        .plugins
                        .iter()
                        .enumerate()
                        .map(|(pi, plugin)| {
                            let selected = defaults
                                .get(si)
                                .and_then(|g| g.get(gi))
                                .is_some_and(|set| set.contains(&pi));
                            serde_json::json!({
                                "index": pi, "name": plugin.name, "default": selected,
                            })
                        })
                        .collect();
                    serde_json::json!({ "index": gi, "name": group.name, "plugins": plugins })
                })
                .collect();
            serde_json::json!({ "index": si, "name": step.name, "groups": groups })
        })
        .collect();
    let data = serde_json::json!({ "steps": steps });
    emit(out, cli.json, &data, |out, data| {
        write_fomod_tree(out, data)
    })
}

fn write_fomod_tree(out: &mut dyn Write, data: &serde_json::Value) -> Result<()> {
    for step in data["steps"].as_array().map_or(&[][..], Vec::as_slice) {
        writeln!(
            out,
            "step {}: {}",
            step["index"],
            step["name"].as_str().unwrap_or("")
        )?;
        for group in step["groups"].as_array().map_or(&[][..], Vec::as_slice) {
            writeln!(
                out,
                "  group {}: {}",
                group["index"],
                group["name"].as_str().unwrap_or("")
            )?;
            for plugin in group["plugins"].as_array().map_or(&[][..], Vec::as_slice) {
                let mark = if plugin["default"].as_bool() == Some(true) {
                    "[x]"
                } else {
                    "[ ]"
                };
                writeln!(
                    out,
                    "    {mark} {} {}",
                    plugin["index"],
                    plugin["name"].as_str().unwrap_or("")
                )?;
            }
        }
    }
    Ok(())
}

fn fomod_apply(
    module: &str,
    choices: &str,
    cli: &Cli,
    engine: &Engine,
    out: &mut dyn Write,
) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Choice {
        step: usize,
        group: usize,
        picks: Vec<usize>,
    }
    let (m, installer, present) = parsed_installer(cli, engine, module)?;
    let mut selections = fomod::defaults(&installer, &present);
    let choices: Vec<Choice> = serde_json::from_str(choices).context("parsing --choices JSON")?;
    for choice in choices {
        let slot = selections
            .get_mut(choice.step)
            .and_then(|groups| groups.get_mut(choice.group))
            .with_context(|| format!("no step {} group {}", choice.step, choice.group))?;
        *slot = choice.picks.into_iter().collect();
    }
    let ops = fomod::resolve(&installer, &selections, &present);
    let placed = fomod::apply(&m.staged_path, &ops).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    engine.set_install_state(m.id, "fomod")?;
    crate::output::ack(
        out,
        cli.json,
        &format!("{placed} files placed - deploy to update the game"),
    )
}
