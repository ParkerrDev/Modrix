// SPDX-License-Identifier: GPL-2.0-only
//! ModManager command-line frontend (`modman`).
//!
//! A thin `clap` layer over [`modman_core::Engine`]. The CLI is built first
//! because it proves the engine works headless: everything the GUI and TUI can
//! do is reachable here. All business logic lives in the engine - this file only
//! parses arguments, calls engine methods, and formats the result.

mod output;
mod serve;

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use modman_core::{Engine, Game, GameDef, Mod, Paths, Profile};

/// ModManager - a fast, native, cross-platform mod manager.
#[derive(Parser)]
#[command(name = "modman", version, about, long_about = None)]
struct Cli {
    /// Root directory for ModManager's own data (overrides platform defaults).
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Which game to act on: its plugin id or numeric id. Optional when only
    /// one game is registered.
    #[arg(long, global = true, value_name = "GAME")]
    game: Option<String>,

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
        #[arg(long, default_value_t = modman_ipc::DEFAULT_PORT)]
        port: u16,
    },
}

#[derive(Subcommand)]
enum GameCmd {
    /// Register a game from a `game.toml` definition.
    Add {
        /// Path to the `game.toml` definition.
        #[arg(long, value_name = "FILE")]
        def: PathBuf,
        /// The game's install directory.
        #[arg(long, value_name = "DIR")]
        install: PathBuf,
        /// The store the install came from.
        #[arg(long, default_value = "manual")]
        store: String,
    },
    /// List registered games.
    List,
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
    let engine = Engine::open(&paths).context("opening the ModManager engine")?;

    // `serve` runs its own async runtime and owns the engine; everything else is
    // a quick synchronous action.
    if let Command::Serve { port } = &cli.command {
        return serve::run(engine, *port);
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    dispatch(&cli, &engine, &mut out)
}

fn dispatch(cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    match &cli.command {
        Command::Paths => output::paths(engine.paths(), out),
        Command::Game { cmd } => game_cmd(cmd, engine, out),
        Command::Mod { cmd } => mod_cmd(cmd, cli, engine, out),
        Command::Profile { cmd } => profile_cmd(cmd, cli, engine, out),
        Command::Loadorder { mods } => loadorder(cli, engine, mods, out),
        Command::Deploy { dry_run } => deploy(cli, engine, *dry_run, out),
        Command::Undeploy => undeploy(cli, engine, out),
        Command::Verify => verify(cli, engine, out),
        // Handled in `main` before dispatch (needs to own the engine).
        Command::Serve { .. } => Ok(()),
    }
}

fn game_cmd(cmd: &GameCmd, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    match cmd {
        GameCmd::Add {
            def,
            install,
            store,
        } => {
            let def = GameDef::from_file(def).context("loading the game definition")?;
            let game = engine
                .add_game(&def, install, store)
                .context("registering the game")?;
            writeln!(out, "added game {} ({})", game.name, game.plugin_id)?;
            Ok(())
        }
        GameCmd::List => {
            for game in engine.games()? {
                writeln!(out, "{}\t{}\t{}", game.id, game.plugin_id, game.name)?;
            }
            Ok(())
        }
    }
}

fn mod_cmd(cmd: &ModCmd, cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    match cmd {
        ModCmd::Add { source, name } => {
            let game = resolve_game(cli, engine)?;
            // An explicit name skips detection; otherwise the shared install
            // path handles naming, versioning, and the FOMOD default pass.
            if let Some(name) = name.as_deref() {
                let m = engine
                    .stage(game.id, name, source)
                    .context("staging the mod")?;
                writeln!(out, "staged mod {} ({})", m.name, m.id)?;
                return Ok(());
            }
            match modman_service::install_file(engine, game.id, source)? {
                modman_service::InstallOutcome::Installed { name, configurable } => {
                    let extra = if configurable { " [fomod defaults]" } else { "" };
                    writeln!(out, "installed {name}{extra}")?;
                }
                other => writeln!(out, "install did not finish: {other:?}")?,
            }
            Ok(())
        }
        ModCmd::List => {
            let game = resolve_game(cli, engine)?;
            let enabled = engine.enabled_mods(engine.active_profile(game.id)?.id)?;
            for m in engine.mods(game.id)? {
                let mark = if enabled.iter().any(|e| e.id == m.id) {
                    "*"
                } else {
                    " "
                };
                writeln!(out, "{mark} {}\t{}", m.id, m.name)?;
            }
            Ok(())
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
            let configurable = modman_service::fomod_pass(engine, &fresh)?;
            let profile = engine.active_profile(game.id)?;
            engine.set_enabled(profile.id, fresh.id, true)?;
            let extra = if configurable { " [fomod defaults]" } else { "" };
            writeln!(out, "reinstalled {}{extra}", fresh.name)?;
            Ok(())
        }
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
        for (index, m) in engine.enabled_mods(profile.id)?.iter().enumerate() {
            writeln!(out, "{index}\t{}", m.name)?;
        }
        return Ok(());
    }
    let mut ids = Vec::with_capacity(mods.len());
    for module in mods {
        ids.push(resolve_mod(engine, &game, module)?.id);
    }
    engine.set_load_order(profile.id, &ids)?;
    writeln!(out, "load order set ({} mods)", ids.len())?;
    Ok(())
}

fn deploy(cli: &Cli, engine: &Engine, dry_run: bool, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    if dry_run {
        let plan = engine.plan(profile.id)?;
        output::plan(&plan, out)
    } else {
        let report = engine.deploy(profile.id).context("deploying")?;
        output::report(&report, out)
    }
}

fn undeploy(cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    let report = engine.undeploy(profile.id).context("undeploying")?;
    writeln!(out, "removed {} files", report.removed())?;
    Ok(())
}

fn verify(cli: &Cli, engine: &Engine, out: &mut dyn Write) -> Result<()> {
    let game = resolve_game(cli, engine)?;
    let profile = engine.active_profile(game.id)?;
    let report = engine.verify(profile.id)?;
    output::verify(&report, out)
}

// --- resolution helpers ----------------------------------------------------

/// Resolve the game to act on from `--game` (or the sole game if unambiguous).
fn resolve_game(cli: &Cli, engine: &Engine) -> Result<Game> {
    let games = engine.games()?;
    if games.is_empty() {
        bail!("no games registered - add one with `modman game add`");
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
