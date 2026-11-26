// SPDX-License-Identifier: GPL-2.0-only
//! L3: the real oracle. Compile C# against the emitted DLL with the actual
//! Roslyn compiler from an installed .NET SDK.
//!
//! Everything else in this crate is self-referential - the writer and the
//! reader can agree on a wrong answer. Roslyn cannot be talked around: it is
//! the consumer the output exists for, and its diagnostics are the ground
//! truth for whether an `Assembly` row, a `Constant` row or a coded-index
//! width is right.
//!
//! The test skips itself when no SDK is installed, so CI without .NET is
//! still green.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::fixtures::full_surface_targeting;
use super::scale::synthetic_targeting;
use crate::builder::AssemblyBuilder;

/// Where a probe compilation happens and what it needs.
struct Toolchain {
    dotnet: PathBuf,
    csc: PathBuf,
    reference_pack: PathBuf,
    framework_major: u16,
}

/// Locates an SDK, or `None` when this machine has no .NET installed.
fn toolchain() -> Option<Toolchain> {
    let root = dotnet_root()?;
    let dotnet = which_dotnet()?;
    let sdk = newest_child(&root.join("sdk"))?;
    let csc = sdk.join("Roslyn/bincore/csc.dll");
    if !csc.is_file() {
        return None;
    }
    let preferred = version_key(&sdk).first().copied().unwrap_or(0);
    let (reference_pack, framework_major) = reference_pack(
        &root.join("packs/Microsoft.NETCore.App.Ref"),
        u16::try_from(preferred).unwrap_or(0),
    )?;
    Some(Toolchain {
        dotnet,
        csc,
        reference_pack,
        framework_major,
    })
}

fn dotnet_root() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(root) = std::env::var("DOTNET_ROOT") {
        candidates.push(PathBuf::from(root));
    }
    candidates.push(PathBuf::from("/usr/share/dotnet"));
    candidates.push(PathBuf::from("/usr/lib/dotnet"));
    candidates.push(PathBuf::from("/usr/local/share/dotnet"));
    candidates
        .into_iter()
        .find(|path| path.join("sdk").is_dir())
}

fn which_dotnet() -> Option<PathBuf> {
    let path = PathBuf::from("dotnet");
    Command::new(&path)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|_| path)
}

/// The highest-versioned child directory, compared numerically - `10.0.110`
/// sorts *below* `9.0.119` as a string, which would silently pick the wrong SDK.
fn newest_child(directory: &Path) -> Option<PathBuf> {
    let mut entries: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|path| (version_key(&path), path))
        .collect();
    entries.sort();
    entries.pop().map(|(_, path)| path)
}

/// Splits a directory name such as `10.0.110` into comparable components.
fn version_key(path: &Path) -> Vec<u64> {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
        .split(['.', '-'])
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

/// Finds a reference pack and the framework major version it targets, so the
/// emitted `AssemblyRef` version matches and no CS1701 warning appears.
fn reference_pack(packs: &Path, preferred_major: u16) -> Option<(PathBuf, u16)> {
    let mut found: Vec<(u16, PathBuf)> = Vec::new();
    for version in std::fs::read_dir(packs).ok()?.filter_map(Result::ok) {
        let ref_dir = version.path().join("ref");
        let Ok(entries) = std::fs::read_dir(&ref_dir) else {
            continue;
        };
        for moniker in entries.filter_map(Result::ok) {
            let name = moniker.file_name().to_string_lossy().to_string();
            if let Some(major) = framework_major(&name) {
                found.push((major, moniker.path()));
            }
        }
    }
    if let Some((major, path)) = found.iter().find(|(major, _)| *major == preferred_major) {
        return Some((path.clone(), *major));
    }
    found.sort_by_key(|(major, _)| *major);
    found.pop().map(|(major, path)| (path, major))
}

/// `net9.0` -> `9`.
fn framework_major(moniker: &str) -> Option<u16> {
    let rest = moniker.strip_prefix("net")?;
    let (major, _) = rest.split_once('.')?;
    major.parse().ok()
}

/// Runs `csc` over `source`, referencing `emitted` plus the whole reference
/// pack, and returns `(success, combined output)`.
fn compile(tools: &Toolchain, workspace: &Path, emitted: &Path, source: &Path) -> (bool, String) {
    let mut command = Command::new(&tools.dotnet);
    command
        .arg("exec")
        .arg(&tools.csc)
        .arg("-nologo")
        .arg("-nostdlib+")
        .arg("-target:library")
        .arg(format!("-out:{}", workspace.join("probe.dll").display()))
        .arg(format!("-r:{}", emitted.display()));
    for entry in std::fs::read_dir(&tools.reference_pack)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "dll") {
            command.arg(format!("-r:{}", path.display()));
        }
    }
    command.arg(source);
    let output = command.output().expect("csc must be runnable");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

/// Emits `builder`, writes `probe`, compiles, and asserts csc exited 0.
#[expect(
    clippy::print_stdout,
    reason = "the compiler's own transcript is the result this test exists to produce"
)]
fn expect_compiles(tools: &Toolchain, builder: &AssemblyBuilder, name: &str, probe: &str) {
    let workspace =
        std::env::temp_dir().join(format!("modrix-ecma335-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).expect("temp directory");
    let emitted = workspace.join("Emitted.dll");
    // L0 runs on the csc fixtures too, so a failure here is attributed to the
    // writer before Roslyn is ever started.
    let (bytes, _) = super::fixtures::emit_and_verify(builder);
    std::fs::write(&emitted, &bytes).expect("write dll");
    let source = workspace.join("probe.cs");
    std::fs::write(&source, probe).expect("write probe");

    let (ok, output) = compile(tools, &workspace, &emitted, &source);
    println!("--- csc transcript for `{name}` ---");
    println!("emitted: {}", emitted.display());
    println!("exit ok: {ok}");
    println!(
        "{}",
        if output.is_empty() {
            "(no output)"
        } else {
            &output
        }
    );
    // Setting MODRIX_ECMA335_KEEP leaves the emitted DLL and the probe on disk
    // so a failure can be pulled apart with an external metadata dumper.
    if std::env::var_os("MODRIX_ECMA335_KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&workspace);
    }
    assert!(ok, "csc rejected the emitted assembly:\n{output}");
    assert!(
        output.is_empty(),
        "csc accepted the assembly but emitted diagnostics:\n{output}"
    );
}

const FULL_SURFACE_PROBE: &str = r#"using System;

public static class Probe
{
    public static string Run()
    {
        Type a = typeof(MyLib.IThing);
        Type b = typeof(MyLib.Widget);
        Type c = typeof(MyLib.Box<int>);

        MyLib.Widget w = new MyLib.Widget();
        w.Go();
        MyLib.IThing t = w;
        t.Go();
        w.Value = 7;
        w.Tags = new string[] { "tag" };
        w.DoThing("s");
        w.Count = 5;
        int count = w.Count;
        int sum = MyLib.Widget.Add(1, 2);

        int x = 1, y = 2;
        MyLib.Widget.Swap<int>(ref x, ref y);

        MyLib.Box<string> boxed = MyLib.Box<string>.Make("inner");
        string got = boxed.Get();
        MyLib.Box<int> ints = w.IntBox;

        MyLib.Widget.Inner nested = new MyLib.Widget.Inner();
        nested.Z = 3;

        MyLib.Point p = new MyLib.Point();
        p.X = 1;
        p.Y = 2;

        MyLib.Color colour = MyLib.Color.Blue;
        MyLib.Color other = MyLib.Color.Red;

        return string.Concat(
            a, b, c, count, sum, x, y, got, ints, nested.Z, p.X, p.Y, colour, other, w.Tags[0]);
    }
}
"#;

const SCALE_PROBE: &str = r"using System;

public static class ScaleProbe
{
    public static string Run()
    {
        Type first = typeof(Bulk.T000000);
        Type last = typeof(Bulk.T020005);
        Bulk.T000000 a = new Bulk.T000000();
        Bulk.T020005 b = new Bulk.T020005();
        a.F000000 = 1;
        b.F020005 = 2;
        a.M000000();
        b.M020005();
        return string.Concat(first, last, a.F000000, b.F020005);
    }
}
";

/// The headline check: a full-surface reference assembly compiles clean.
#[test]
fn roslyn_accepts_the_full_surface_assembly() {
    let Some(tools) = toolchain() else {
        return;
    };
    let builder = full_surface_targeting(tools.framework_major, false);
    expect_compiles(&tools, &builder, "full", FULL_SURFACE_PROBE);
}

/// The same assembly with the two reference-assembly marker attributes, which
/// adds the `MemberRef` and `CustomAttribute` tables to the mix.
#[test]
fn roslyn_accepts_the_marked_reference_assembly() {
    let Some(tools) = toolchain() else {
        return;
    };
    let builder = full_surface_targeting(tools.framework_major, true);
    expect_compiles(&tools, &builder, "marked", FULL_SURFACE_PROBE);
}

/// The width oracle: 20 007 types force 4-byte `TypeDefOrRef` coded indices
/// and a `#Strings` heap over 64 KB. Roslyn resolving both the first and the
/// last type is the only external proof that the threshold is right.
#[test]
fn roslyn_resolves_the_first_and_last_type_past_the_coded_index_threshold() {
    let Some(tools) = toolchain() else {
        return;
    };
    let builder = synthetic_targeting(20_007, 0, tools.framework_major);
    expect_compiles(&tools, &builder, "scale", SCALE_PROBE);
}
