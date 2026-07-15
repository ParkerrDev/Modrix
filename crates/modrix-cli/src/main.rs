// SPDX-License-Identifier: GPL-2.0-only
//! Modrix command-line frontend (`modrix`).
//!
//! A thin `clap` layer over [`modrix_core::Engine`]. The CLI is built first
//! because it proves the engine works headless: everything the GUI and TUI can
//! do is reachable here. All business logic lives in the engine - this file only
//! parses arguments, calls engine methods, and formats the result.

mod output;
mod parity;
mod registry;
mod serve;

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use modrix_core::{Engine, Game, GameDef, Mod, Paths, Profile};

/// Modrix - a fast, native, cross-platform mod manager.
#[derive(Parser)]
#[command(name = "modrix", version, about, long_about = None)]
struct Cli {
    /// Root directory for Modrix's own data (overrides platform defaults).
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Which game to act on: its plugin id or numeric id. Optional when only
    /// one game is registered.
    #[arg(long, global = true, value_name = "GAME")]
    game: Option<String>,

    /// Machine output: one `{"ok":true,"data":…}` JSON envelope per command.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the resolved config/data/cache locations and the database path.
    Paths,
    /// Manage games.
    Game {
        #[command(subcommand)]
        cmd: GameCmd,
    },
    /// Manage staged mods.
    Mod {
        #[command(subcommand)]
        cmd: ModCmd,
    },
    /// Manage profiles.
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// Show or set the load order (enabled mods, first to last).
    Loadorder {
        /// Mods (id or name) in the desired order; omit to show the current one.
        mods: Vec<String>,
    },
    /// Deploy the active profile into the game directory.
    Deploy {
        /// Compute and print the plan without touching any files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove everything the current deployment placed, restoring originals.
    Undeploy,
    /// Verify the current deployment against the manifest.
    Verify,
    /// Run the headless background service that handles browser download
    /// hand-offs (downloads and installs mods clicked in the browser via the
    /// extension). Blocks until killed.
    Serve {
        /// Loopback port to bind (single-instance mutex).
        #[arg(long, default_value_t = modrix_ipc::DEFAULT_PORT)]
        port: u16,
    },
    /// Search, install, and manage community plugins from the registry.
    Registry {
        #[command(subcommand)]
        cmd: registry::RegistryCmd,
    },
    /// Plugin-development tools (validation for registry submissions).
    Plugin {
        #[command(subcommand)]
        cmd: registry::PluginCmd,
    },
    /// The plugin (.esp/.esm/.esl) load order.
    Plugins {
        #[command(subcommand)]
        cmd: parity::PluginsCmd,
    },
    /// Setup health: missing masters, loader checks, conflicts, foreign files.
    Health,
    /// Mods in the game directory that Modrix does not manage (read-only).
    External,
    /// Downloads on the live Modrix instance (GUI or `modrix serve`).
    Downloads {
        #[command(subcommand)]
        cmd: parity::DownloadsCmd,
    },
    /// FOMOD installer options: inspect and apply choices.
    Fomod {
        #[command(subcommand)]
        cmd: parity::FomodCmd,
    },
    /// Run the MCP server over stdio - point an AI agent's MCP client at
    /// `modrix mcp` to let it manage mods autonomously.
    Mcp,
}

#[derive(Subcommand)]
enum GameCmd {
    /// Register a game from a `game.toml` definition.
    Add {
        /// Path to the `game.toml` definition. Omit to use the def catalog
        /// entry whose id matches `--game`.
        #[arg(long, value_name = "FILE")]
        def: Option<PathBuf>,
        /// The game's install directory.
        #[arg(long, value_name = "DIR")]
        install: PathBuf,
        /// The store the install came from.
        #[arg(long, default_value = "manual")]
        store: String,
    },
    /// List registered games.
    List,
    /// Probe every known definition for installs found on disk.
    Detect,
    /// Show the active (last worked on) game.
    Active,
    /// Make `--game` the active game.
    SetActive,
    /// Show what the selected game supports (drives frontend features).
    Capabilities,
}

#[derive(Subcommand)]
enum ModCmd {
    /// Stage a mod from an extracted directory or a `.zip` archive.
    Add {
        /// Directory or `.zip` to stage.
        source: PathBuf,
        /// Name for the mod (defaults to the source's file name).
        #[arg(long)]
        name: Option<String>,
    },
    /// List staged mods for the game.
    List,
    /// Enable a mod in the active profile.
    Enable {
        /// Mod id or name.
        module: String,
    },
    /// Disable a mod in the active profile.
    Disable {
        /// Mod id or name.
        module: String,
    },
    /// Delete a mod from the library (its staged files included).
    Remove {
        /// Mod id or name.
        module: String,
    },
    /// Re-stage a mod from its recorded source archive.
    Reinstall {
        /// Mod id or name.
        module: String,
    },
    /// Pairwise mod conflicts with their rule state.
    Conflicts,
    /// Conflict rules ("winner overrides loser").
    Rule {
        #[command(subcommand)]
        cmd: parity::RuleCmd,
    },
    /// Per-file overrides (pin a contested path to one provider).
    Override {
        #[command(subcommand)]
        cmd: parity::OverrideCmd,
    },
    /// Content-hash an archive and report if it is already installed.
    Hash {
        /// The archive file.
        archive: PathBuf,
    },
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// List the game's profiles.
    List,
    /// Create a new profile.
    Create {
        /// The new profile's name.
        name: String,
    },
    /// Make a profile active.
    Switch {
        /// The profile name to activate.
        name: String,
    },
}

fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    let paths = match cli.data_dir.as_ref() {
        Some(dir) => Paths::rooted_at(dir),
        None => Paths::resolve().context("resolving platform directories")?,
    };
    let mut engine = Engine::open(&paths).context("opening the Modrix engine")?;
    // Tier-2 plugins (game.lua) hook in before any command runs.
    modrix_plugin::register_lua_logic(&mut engine);

    // `serve` and `mcp` own the engine for their whole session; everything
    // else is a quick synchronous action.
    if let Command::Serve { port } = &cli.command {
        return serve::run(engine, *port);
    }
    if matches!(cli.command, Command::Mcp) {
        let ctx = modrix_mcp::Ctx::new(engine).context("starting the MCP runtime")?;
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        modrix_mcp::serve(&ctx, &mut stdin.lock(), &mut stdout.lock())
            .context("serving MCP over stdio")?;
        return Ok(());
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let result = dispatch(&cli, &engine, &mut out);
    if cli.json
        && let Err(error) = result
    {
        // Machine mode: errors are an envelope on stderr + a nonzero exit,
        // never a human backtrace an agent has to scrape.
        let payload = serde_json::to_string(&format!("{error:#}")).unwrap_or_default();
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{{\"ok\":false,\"error\":{payload}}}");
        std::process::exit(1);
    }
    result
}

fn dispatch(cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    match &cli.command {
        Command::Paths => output::paths(engine.paths(), out),
        Command::Game { cmd } => game_cmd(cmd, cli, engine, out),
        Command::Mod { cmd } => mod_cmd(cmd, cli, engine, out),
        Command::Profile { cmd } => profile_cmd(cmd, cli, engine, out),
        Command::Loadorder { mods } => loadorder(cli, engine, mods, out),
        Command::Deploy { dry_run } => deploy(cli, engine, *dry_run, out),
        Command::Undeploy => undeploy(cli, engine, out),
        Command::Verify => verify(cli, engine, out),
        Command::Registry { cmd } => registry::registry_cmd(cmd, engine, out),
        Command::Plugin { cmd } => registry::plugin_cmd(cmd, out),
        Command::Plugins { cmd } => parity::plugins_cmd(cmd, cli, engine, out),
        Command::Health => parity::health(cli, engine, out),
        Command::External => parity::external(cli, engine, out),
        Command::Downloads { cmd } => parity::downloads_cmd(cmd, cli, engine, out),
        Command::Fomod { cmd } => parity::fomod_cmd(cmd, cli, engine, out),
        // Handled in `main` before dispatch (they own the engine).
        Command::Serve { .. } | Command::Mcp => Ok(()),
    }
}

fn game_cmd(cmd: &GameCmd, cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    match cmd {
        GameCmd::Add {
            def,
            install,
            store,
        } => {
            let def = if let Some(path) = def {
                GameDef::from_file(path).context("loading the game definition")?
            } else {
                let id = cli
                    .game
                    .as_deref()
                    .context("pass --def FILE, or --game <id> to use a catalog definition")?;
                modrix_core::defcat::find_def(engine.paths(), id)
                    .with_context(|| format!("no definition `{id}` in the catalog"))?
                    .def
            };
            let game = engine
                .add_game(&def, install, store)
                .context("registering the game")?;
            output::ack(
                out,
                cli.json,
                &format!("added game {} ({})", game.name, game.plugin_id),
            )
        }
        GameCmd::List => {
            let games = engine.games()?;
            output::emit(out, cli.json, &games, |out, games| {
                for game in games {
                    writeln!(out, "{}\t{}\t{}", game.id, game.plugin_id, game.name)?;
                }
                Ok(())
            })
        }
        GameCmd::Detect => parity::game_extra("detect", cli, engine, out),
        GameCmd::Active => parity::game_extra("active", cli, engine, out),
        GameCmd::SetActive => parity::game_extra("set-active", cli, engine, out),
        GameCmd::Capabilities => parity::game_extra("capabilities", cli, engine, out),
    }
}

fn mod_add(
    source: &std::path::Path,
    name: Option<&str>,
    cli: &Cli,
    engine: &Engine,
    out: &mut dyn Write,
) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    // An explicit name skips detection; otherwise the shared install
    // path handles naming, versioning, and the FOMOD default pass.
    if let Some(name) = name {
        let m = engine
            .stage(game.id, name, source)
            .context("staging the mod")?;
        writeln!(out, "staged mod {} ({})", m.name, m.id)?;
        return Ok(());
    }
    match modrix_service::install_file(engine, game.id, source)? {
        modrix_service::InstallOutcome::Installed { name, configurable } => {
            writeln!(out, "installed {name}{}", fomod_suffix(configurable))?;
        }
        other => writeln!(out, "install did not finish: {other:?}")?,
    }
    Ok(())
}

fn mod_cmd(cmd: &ModCmd, cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    match cmd {
        ModCmd::Add { source, name } => mod_add(source, name.as_deref(), cli, engine, out),
        ModCmd::List => {
            let game = resolve_game(cli, engine)?;
            let enabled = engine.enabled_mods(engine.active_profile(game.id)?.id)?;
            let mods = engine.mods(game.id)?;
            output::emit(out, cli.json, &mods, |out, mods| {
                for m in mods {
                    let mark = if enabled.iter().any(|e| e.id == m.id) {
                        "*"
                    } else {
                        " "
                    };
                    writeln!(out, "{mark} {}\t{}", m.id, m.name)?;
                }
                Ok(())
            })
        }
        ModCmd::Enable { module } => set_enabled(cli, engine, module, true, out),
        ModCmd::Disable { module } => set_enabled(cli, engine, module, false, out),
        ModCmd::Remove { module } => {
            let game = resolve_game(cli, engine)?;
            let m = resolve_mod(engine, &game, module)?;
            engine.delete_mod(m.id).context("deleting the mod")?;
            writeln!(out, "removed {}", m.name)?;
            Ok(())
        }
        ModCmd::Reinstall { module } => {
            let game = resolve_game(cli, engine)?;
            let m = resolve_mod(engine, &game, module)?;
            let fresh = engine.reinstall_mod(m.id).context("reinstalling the mod")?;
            let configurable = modrix_service::fomod_pass(engine, &fresh)?;
            let profile = engine.active_profile(game.id)?;
            engine.set_enabled(profile.id, fresh.id, true)?;
            writeln!(
                out,
                "reinstalled {}{}",
                fresh.name,
                fomod_suffix(configurable)
            )?;
            Ok(())
        }
        ModCmd::Conflicts => parity::conflicts(cli, engine, out),
        ModCmd::Rule { cmd } => parity::rule_cmd(cmd, cli, engine, out),
        ModCmd::Override { cmd } => parity::override_cmd(cmd, cli, engine, out),
        ModCmd::Hash { archive } => parity::mod_hash(archive, cli, engine, out),
    }
}

/// The suffix marking an install whose FOMOD options were set to defaults.
fn fomod_suffix(configurable: bool) -> &'static str {
    if configurable {
        " [fomod defaults]"
    } else {
        ""
    }
}

fn set_enabled(
    cli: &Cli,
    engine: &Engine,
    module: &str,
    on: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    let m = resolve_mod(engine, &game, module)?;
    engine.set_enabled(profile.id, m.id, on)?;
    writeln!(
        out,
        "{} {}",
        if on { "enabled" } else { "disabled" },
        m.name
    )?;
    Ok(())
}

fn profile_cmd(cmd: &ProfileCmd, cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    match cmd {
        ProfileCmd::List => {
            for p in engine.profiles(game.id)? {
                let mark = if p.is_active { "*" } else { " " };
                writeln!(out, "{mark} {}\t{}", p.id, p.name)?;
            }
            Ok(())
        }
        ProfileCmd::Create { name } => {
            let p = engine.create_profile(game.id, name)?;
            writeln!(out, "created profile {} ({})", p.name, p.id)?;
            Ok(())
        }
        ProfileCmd::Switch { name } => {
            let p = find_profile(engine, &game, name)?;
            engine.set_active_profile(p.id)?;
            writeln!(out, "switched to profile {}", p.name)?;
            Ok(())
        }
    }
}

fn loadorder(cli: &Cli, engine: &Engine, mods: &[String], out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    if mods.is_empty() {
        let enabled = engine.enabled_mods(profile.id)?;
        return output::emit(out, cli.json, &enabled, |out, enabled| {
            for (index, m) in enabled.iter().enumerate() {
                writeln!(out, "{index}\t{}", m.name)?;
            }
            Ok(())
        });
    }
    let mut ids = Vec::with_capacity(mods.len());
    for module in mods {
        ids.push(resolve_mod(engine, &game, module)?.id);
    }
    engine.set_load_order(profile.id, &ids)?;
    output::ack(
        out,
        cli.json,
        &format!("load order set ({} mods)", ids.len()),
    )
}

fn deploy(cli: &Cli, engine: &Engine, dry_run: bool, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    if dry_run {
        let plan = engine.plan(profile.id)?;
        let data = serde_json::json!({
            "to_add": plan.to_add(), "to_remove": plan.to_remove(),
            "unchanged": plan.unchanged(), "conflicts": plan.conflicts().len(),
        });
        output::emit(out, cli.json, &data, |out, _| output::plan(&plan, out))
    } else {
        let report = engine.deploy(profile.id).context("deploying")?;
        let data = serde_json::json!({
            "added": report.added(), "removed": report.removed(),
            "unchanged": report.unchanged(), "skipped_modified": report.skipped_modified(),
        });
        output::emit(out, cli.json, &data, |out, _| output::report(&report, out))
    }
}

fn undeploy(cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    let report = engine.undeploy(profile.id).context("undeploying")?;
    output::ack(
        out,
        cli.json,
        &format!("removed {} files", report.removed()),
    )
}

fn verify(cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    let report = engine.verify(profile.id)?;
    let data = serde_json::json!({
        "clean": report.is_clean(),
        "checked": report.checked(),
        "issues": report.issues(),
    });
    output::emit(out, cli.json, &data, |out, _| output::verify(&report, out))
}

// --- resolution helpers ----------------------------------------------------

/// Resolve the game to act on from `--game` (or the sole game if unambiguous).
fn resolve_game(cli: &Cli, engine: &Engine) -> Result<Game> {
    let games = engine.games()?;
    if games.is_empty() {
        bail!("no games registered - add one with `modrix game add`");
    }
    match cli.game.as_deref() {
        None if games.len() == 1 => games.into_iter().next().context("game list changed"),
        None => bail!("several games registered - pass --game <plugin-id|id>"),
        Some(sel) => games
            .into_iter()
            .find(|g| g.plugin_id == sel || g.id.to_string() == sel)
            .with_context(|| format!("no game matches `{sel}`")),
    }
}

fn resolve_mod(engine: &Engine, game: &Game, selector: &str) -> Result<Mod> {
    engine
        .mods(game.id)?
        .into_iter()
        .find(|m| m.name == selector || m.id.to_string() == selector)
        .with_context(|| format!("no mod matches `{selector}` in {}", game.plugin_id))
}

fn find_profile(engine: &Engine, game: &Game, name: &str) -> Result<Profile> {
    engine
        .profiles(game.id)?
        .into_iter()
        .find(|p| p.name == name)
        .with_context(|| format!("no profile named `{name}`"))
}

/// Derive a mod name from the source path when one is not given.
/// Install a stderr tracing subscriber honouring `RUST_LOG` (default `info`).
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // `try_init` fails only if a subscriber is already set; ignore that.
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
