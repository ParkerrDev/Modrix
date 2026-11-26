// SPDX-License-Identifier: GPL-2.0-only
//! `modrix interop` - generate the managed reference assemblies an IL2CPP game
//! needs before a mod can be compiled against it.
//!
//! This is what makes an IL2CPP game buildable without any third-party loader.
//! Both inputs already sit in the game directory: `global-metadata.dat` names
//! every type, and `GameAssembly.dll` carries the type table those names index
//! into. Nothing is downloaded and nothing else has to be installed.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use modrix_core::{Engine, Game};
use modrix_il2cpp::{GameBinary, Metadata};
use modrix_interop_gen::{GameModel, emit_all};

use crate::Cli;
use crate::output;

/// Where the metadata lives, relative to the install directory, per game.
/// The data folder is named after the executable, so it cannot be derived
/// generically - but it is discoverable by looking for the one `*_Data` folder
/// that contains an `il2cpp_data` subdirectory.
const METADATA_TAIL: &str = "il2cpp_data/Metadata/global-metadata.dat";

/// Interop subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum InteropCmd {
    /// Generate reference assemblies into the loader's interop directory.
    Generate {
        /// Write here instead of the loader's own interop directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Look up types in the game's metadata: members, and enum values.
    ///
    /// Reads `global-metadata.dat` directly, so it answers from the shipped
    /// game rather than from generated assemblies - which matters when the
    /// question is "what integer does this enum member actually have", since
    /// that is the kind of constant a mod otherwise has to hardcode and guess.
    Inspect {
        /// Case-insensitive substring matched against `Namespace.Name`.
        pattern: String,
        /// Also list each matching type's methods.
        #[arg(long)]
        methods: bool,
        /// Stop after this many types.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

/// What a generation run produced.
#[derive(serde::Serialize)]
struct GenerateReport {
    /// Where the assemblies were written.
    output: PathBuf,
    /// How many assemblies were emitted.
    assemblies: usize,
    /// Total bytes written.
    bytes: u64,
    /// The game's IL2CPP metadata version.
    metadata_version: i32,
}

/// Run an `interop` subcommand.
///
/// # Errors
///
/// Fails if the game is not IL2CPP, its files cannot be read, or the output
/// directory cannot be written.
pub fn interop_cmd(
    cmd: &InteropCmd,
    cli: &Cli,
    engine: &Engine,
    out: &mut dyn Write,
) -> Result<()> {
    let game = crate::resolve_game(cli, engine)?;
    match cmd {
        InteropCmd::Generate { out: dest } => generate(&game, engine, dest.as_deref(), cli, out),
        InteropCmd::Inspect {
            pattern,
            methods,
            limit,
        } => inspect(&game, pattern, *methods, *limit, out),
    }
}

fn generate(
    game: &Game,
    engine: &Engine,
    dest: Option<&Path>,
    cli: &Cli,
    out: &mut dyn Write,
) -> Result<()> {
    let install = Path::new(&game.install_path);
    let metadata_path = find_metadata(install)
        .with_context(|| format!("{} does not look like an IL2CPP game", game.name))?;
    let binary_path = install.join("GameAssembly.dll");
    if !binary_path.exists() {
        bail!("GameAssembly.dll is missing from {}", install.display());
    }

    let target = match dest {
        Some(path) => path.to_path_buf(),
        None => interop_dir(game, engine)?,
    };

    let metadata = Metadata::open(&metadata_path)
        .with_context(|| format!("reading {}", metadata_path.display()))?;
    let type_count = metadata.type_count()?;
    let binary = GameBinary::open(&binary_path, type_count)
        .with_context(|| format!("reading {}", binary_path.display()))?;
    let model = GameModel::build(&metadata, &binary)?;
    let emitted = emit_all(&model, &metadata)?;

    std::fs::create_dir_all(&target).with_context(|| format!("creating {}", target.display()))?;
    let mut bytes = 0_u64;
    for (name, body) in &emitted {
        let path = target.join(format!("{name}.dll"));
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        bytes = bytes.saturating_add(u64::try_from(body.len()).unwrap_or(u64::MAX));
    }
    // Stamp what the assemblies were generated from, so a game update can be
    // detected as "these are stale" rather than surfacing as mystery compile
    // errors against renamed types.
    let stamp = target.join("generated-from.txt");
    let _ = std::fs::write(
        &stamp,
        format!(
            "metadata_version={}\ntypes={}\nassemblies={}\n",
            metadata.version(),
            type_count,
            emitted.len()
        ),
    );

    let report = GenerateReport {
        output: target,
        assemblies: emitted.len(),
        bytes,
        metadata_version: metadata.version(),
    };
    output::emit(out, cli.json, &report, |out, report| {
        // Integer megabytes: the exact byte count is in the JSON envelope, and
        // a float cast here trips the precision lint for no reader benefit.
        let megabytes = report.bytes.saturating_div(1_048_576);
        writeln!(
            out,
            "Generated {} reference assemblies ({megabytes} MB) from IL2CPP metadata v{}",
            report.assemblies, report.metadata_version
        )?;
        writeln!(out, "  -> {}", report.output.display())?;
        writeln!(
            out,
            "\nMods can now be compiled against this game with no third-party loader."
        )?;
        Ok(())
    })
}

/// The loader's interop directory, from the game definition.
fn interop_dir(game: &Game, engine: &Engine) -> Result<PathBuf> {
    let def = engine.game_def(game.id)?;
    let loader = def
        .mod_loader
        .as_ref()
        .context("this game declares no mod loader, so there is nowhere to put interop")?;
    let relative = loader
        .interop_dir
        .as_deref()
        .context("this game's loader declares no interop directory")?;
    Ok(Path::new(&game.install_path).join(relative))
}

/// Locate `global-metadata.dat` under whichever `*_Data` folder holds it.
fn find_metadata(install: &Path) -> Result<PathBuf> {
    let entries =
        std::fs::read_dir(install).with_context(|| format!("reading {}", install.display()))?;
    for entry in entries.flatten().take(4096) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let candidate = path.join(METADATA_TAIL);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("no {METADATA_TAIL} found under {}", install.display())
}

/// Print every type whose `Namespace.Name` contains `pattern`.
///
/// # Errors
///
/// Fails when the game is not IL2CPP or its metadata cannot be read.
fn inspect(
    game: &Game,
    pattern: &str,
    with_methods: bool,
    limit: usize,
    out: &mut dyn Write,
) -> Result<()> {
    let install = Path::new(&game.install_path);
    let metadata_path = find_metadata(install)
        .with_context(|| format!("{} does not look like an IL2CPP game", game.name))?;
    let metadata = Metadata::open(&metadata_path)
        .with_context(|| format!("reading {}", metadata_path.display()))?;

    // Constants are stored in one table keyed by field index, so it is read once
    // rather than per field - the alternative is a linear scan per enum member.
    let defaults = metadata.default_values()?;
    let needle = pattern.to_lowercase();
    let total = metadata.type_count()?;

    let mut shown = 0_usize;
    for index in 0..total {
        if shown >= limit {
            writeln!(
                out,
                "... stopping at {limit} types; narrow the pattern or raise --limit"
            )?;
            break;
        }
        let type_def = metadata.type_def(index)?;
        let full = if type_def.namespace.is_empty() {
            type_def.name.clone()
        } else {
            format!("{}.{}", type_def.namespace, type_def.name)
        };
        if !full.to_lowercase().contains(&needle) {
            continue;
        }
        shown = shown.saturating_add(1);
        writeln!(out, "{full}")?;
        write_fields(&metadata, &type_def, &defaults, out)?;
        if with_methods {
            write_methods(&metadata, &type_def, out)?;
        }
    }

    if shown == 0 {
        writeln!(out, "no type matching '{pattern}' in {}", game.name)?;
    }
    Ok(())
}

/// Print a type's fields, with the constant value when it has one.
fn write_fields(
    metadata: &Metadata,
    type_def: &modrix_il2cpp::TypeDef,
    defaults: &std::collections::HashMap<usize, modrix_il2cpp::DefaultValue>,
    out: &mut dyn Write,
) -> Result<()> {
    let Some(start) = type_def.fields.start else {
        return Ok(());
    };
    let count = type_def.fields.count;
    for offset in 0..count {
        let index = start.saturating_add(offset);
        let field = metadata.field_def(index)?;
        match defaults.get(&index) {
            // An enum member or a const: the value is the whole point of asking.
            Some(default) => writeln!(out, "    {} = {}", field.name, default.value)?,
            None => writeln!(out, "    {}", field.name)?,
        }
    }
    Ok(())
}

/// Print a type's methods with their parameter names.
fn write_methods(
    metadata: &Metadata,
    type_def: &modrix_il2cpp::TypeDef,
    out: &mut dyn Write,
) -> Result<()> {
    /// `MethodAttributes.Static`.
    const STATIC: u16 = 0x0010;

    let Some(start) = type_def.methods.start else {
        return Ok(());
    };
    let count = type_def.methods.count;
    for offset in 0..count {
        let index = start.saturating_add(offset);
        let method = metadata.method_def(index)?;
        let params = method_parameters(metadata, &method)?;
        let modifier = if method.flags & STATIC != 0 {
            "static "
        } else {
            ""
        };
        writeln!(out, "    {modifier}{}({})", method.name, params.join(", "))?;
    }
    Ok(())
}

/// Parameter names for one method, in declaration order.
fn method_parameters(
    metadata: &Metadata,
    method: &modrix_il2cpp::MethodDef,
) -> Result<Vec<String>> {
    let Some(start) = method.parameters.start else {
        return Ok(Vec::new());
    };
    let count = method.parameters.count;
    let mut names = Vec::with_capacity(count);
    for offset in 0..count {
        let param = metadata.param_def(start.saturating_add(offset))?;
        names.push(param.name);
    }
    Ok(names)
}
