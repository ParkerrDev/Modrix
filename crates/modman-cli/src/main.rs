// SPDX-License-Identifier: GPL-2.0-only
//! ModManager command-line frontend (`modman`).
//!
//! A thin `clap` layer over [`modman_core::Engine`]. The CLI is built first
//! because it proves the engine works headless: everything the GUI and TUI can
//! do is reachable here. In Phase 0 it opens the engine (creating the data
//! directory and database) and reports the resolved locations.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use modman_core::{Engine, Paths};

/// ModManager - a fast, native, cross-platform mod manager.
#[derive(Parser)]
#[command(name = "modman", version, about, long_about = None)]
struct Cli {
    /// Root directory for ModManager's own data (overrides platform defaults).
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print the resolved config/data/cache locations and the database path.
    Paths,
}

fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    let paths = match cli.data_dir.as_ref() {
        Some(dir) => Paths::rooted_at(dir),
        None => Paths::resolve().context("resolving platform directories")?,
    };
    let engine = Engine::open(&paths).context("opening the ModManager engine")?;

    match cli.command.unwrap_or(Command::Paths) {
        Command::Paths => print_paths(engine.paths()).context("writing output")?,
    }
    Ok(())
}

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

/// Report the resolved locations. Writes to a locked stdout handle (rather than
/// the `println!` macro) so a broken pipe surfaces as an error, not a panic.
fn print_paths(paths: &Paths) -> std::io::Result<()> {
    let out = std::io::stdout();
    let mut out = out.lock();
    writeln!(out, "config:   {}", paths.config_dir().display())?;
    writeln!(out, "data:     {}", paths.data_dir().display())?;
    writeln!(out, "cache:    {}", paths.cache_dir().display())?;
    writeln!(out, "database: {}", paths.database_file().display())?;
    Ok(())
}
