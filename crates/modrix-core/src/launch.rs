// SPDX-License-Identifier: GPL-2.0-only
//! Starting a game, managing its mod loader, and reading its log.
//!
//! Everything here is **fire-and-forget**. Modrix starts a game and stops
//! caring: no PID tracking, no restart watchdog, no supervision tree. Those
//! bring their own failure modes (orphaned children, zombie reaping, a manager
//! that thinks a game is running when it crashed) and nothing Modrix does
//! needs them. The log tail is a plain bounded read, polled by whoever wants
//! it, so a frontend that dies mid-tail leaves nothing behind.
//!
//! ## Why the environment matters
//!
//! A mod loader for a Windows game injects through a *proxy DLL* - a library
//! named after one the game already imports (`version.dll`), placed beside the
//! executable so the loader runs before the game does. On Windows the normal
//! DLL search order picks it up for free.
//!
//! Under Proton/Wine it does not: Wine resolves `version.dll` to its own
//! builtin unless `WINEDLLOVERRIDES` explicitly asks for the native file
//! first. Without that variable the loader is inert, the game starts perfectly,
//! and *no mods load* - with no error anywhere. That silent failure is the
//! single most confusing thing about modding a Windows game on Linux, so
//! [`launch_readiness`] reports it as a first-class condition rather than
//! letting a user discover it themselves.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::gamedef::{LaunchDef, LoaderDef};

/// Maximum bytes read from a log in one [`tail_log`] call. A game log grows
/// without bound; a caller polling it must never be handed an arbitrarily
/// large buffer.
const MAX_LOG_READ: u64 = 1 << 20;

/// Maximum lines returned in one [`tail_log`] call, whatever the caller asks.
const MAX_LOG_LINES: usize = 2000;

/// Whether a game's mod loader is installed, and whether anything is in its way.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoaderStatus {
    /// The loader's declared identity (`modrix-sons`, `redloader`, …).
    pub kind: String,
    /// Whether the loader's sentinel file is present.
    pub installed: bool,
    /// The installed version, when the loader records one.
    pub version: Option<String>,
    /// A rival loader already owning the proxy slot. `Some` means installing
    /// would brick both, so installation must refuse.
    pub conflict: Option<String>,
    /// The loader's own directory.
    pub install_root: PathBuf,
    /// Whether generated interop assemblies are present. IL2CPP games cannot
    /// even *compile* a mod until this is true.
    pub interop_ready: bool,
}

/// Whether a launch would actually produce a modded game.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LaunchReadiness {
    /// The environment the game needs, rendered as Steam launch options.
    pub required_launch_options: Option<String>,
    /// Whether that environment reaches the game on this platform without
    /// help. True on Windows (native DLL search order) and whenever the
    /// definition asks for no environment at all.
    pub env_reaches_game: bool,
    /// A human-readable explanation when it does not.
    pub warning: Option<String>,
}

/// A bounded window onto a game log.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogTail {
    /// The log file read.
    pub path: PathBuf,
    /// Byte offset to pass as `since` next call to continue where this stopped.
    pub next_cursor: u64,
    /// The log restarted since `since` (it shrank). Loaders truncate on open,
    /// so this is how a caller notices a new session rather than silently
    /// reading garbage from the middle of a line.
    pub truncated: bool,
    /// The lines read.
    pub lines: Vec<String>,
}

/// What a launch attempt did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LaunchOutcome {
    /// The command Modrix ran, for the record.
    pub command: String,
    /// Whether a running instance was stopped first.
    pub stopped_existing: bool,
    /// Readiness at the moment of launch.
    pub readiness: LaunchReadiness,
}

/// Resolve a definition-relative path against the install directory.
///
/// Definition paths are always `/`-separated; `Path::join` handles that on
/// Windows too, but a leading `/` or a `..` component would escape the install
/// directory, so both are refused.
fn resolve_in(install: &Path, rel: &str) -> Result<PathBuf> {
    let trimmed = rel.trim();
    if trimmed.is_empty() {
        return Err(Error::NotFound {
            kind: "loader path",
            key: rel.to_owned(),
        });
    }
    let candidate = Path::new(trimmed);
    let escapes = candidate.is_absolute()
        || candidate
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
    if escapes {
        return Err(Error::PathEscape {
            path: candidate.to_path_buf(),
        });
    }
    Ok(install.join(candidate))
}

/// Report whether `def`'s loader is installed in `install`, and whether a
/// rival loader is in the way.
///
/// Read-only: this never touches the game directory.
///
/// # Errors
///
/// Returns [`Error::PathEscape`] if the definition names a path outside the
/// install directory.
pub fn loader_status(install: &Path, def: &LoaderDef) -> Result<LoaderStatus> {
    let sentinel = resolve_in(install, &def.sentinel)?;
    let installed = sentinel.exists();
    let install_root = resolve_in(install, &def.install_root)?;

    let version = def
        .version_file
        .as_deref()
        .and_then(|rel| resolve_in(install, rel).ok())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let interop_ready = def
        .interop_dir
        .as_deref()
        .and_then(|rel| resolve_in(install, rel).ok())
        .is_some_and(|dir| dir_has_entries(&dir));

    Ok(LoaderStatus {
        kind: def.kind.clone(),
        installed,
        version,
        // Our own presence is not a conflict with ourselves.
        conflict: if installed {
            None
        } else {
            first_conflict(install, def)
        },
        install_root,
        interop_ready,
    })
}

/// Whether a directory exists and holds at least one entry.
fn dir_has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut it| it.next().is_some())
}

/// The first declared conflict present in the install directory.
///
/// A game has exactly one proxy-DLL slot, so two loaders can never coexist:
/// whichever is loaded second never runs, and the user gets a game with half
/// its mods missing and no diagnostic. Refusing is the only safe answer.
fn first_conflict(install: &Path, def: &LoaderDef) -> Option<String> {
    def.conflicts
        .iter()
        .find(|rel| resolve_in(install, rel).is_ok_and(|p| p.exists()))
        .cloned()
}

/// Render a definition's environment as a Steam launch-options string.
///
/// `BTreeMap` iteration is ordered, so the same definition always renders the
/// same string - which is what lets a caller compare it against what Steam
/// already has and skip a needless rewrite.
fn render_launch_options(env: &BTreeMap<String, String>) -> Option<String> {
    if env.is_empty() {
        return None;
    }
    let mut out = String::new();
    for (key, value) in env {
        out.push_str(key);
        out.push_str("=\"");
        out.push_str(value);
        out.push_str("\" ");
    }
    out.push_str("%command%");
    Some(out)
}

/// Whether a launch on this platform will actually reach the game with the
/// environment the definition asks for.
#[must_use]
pub fn launch_readiness(def: &LaunchDef) -> LaunchReadiness {
    let options = render_launch_options(&def.env);
    // On Windows the proxy DLL is found by the normal search order and no
    // environment is needed. Everywhere else the game is a Windows binary under
    // Proton/Wine, and Steam owns the process - so Modrix cannot inject the
    // variable by spawning, only the user's launch options can.
    let native = cfg!(windows);
    let needs_help = !native && options.is_some();
    LaunchReadiness {
        warning: needs_help.then(|| {
            let opts = options.clone().unwrap_or_default();
            format!(
                "This game runs under Proton/Wine, where the mod loader's proxy DLL is ignored \
                 unless Steam passes it through. Set the game's Steam launch options to:  {opts}  \
                 Without it the game starts normally and no mods load, with no error."
            )
        }),
        env_reaches_game: !needs_help,
        required_launch_options: options,
    }
}

/// Stop a running instance by image name. Best-effort: a game that is not
/// running is not an error.
fn stop_process(name: &str) -> bool {
    use std::process::{Command, Stdio};
    // Match on the image name only. A path substring would also match an
    // editor with a file open from the game directory, or Modrix itself.
    let mut command = if cfg!(windows) {
        let mut c = Command::new("taskkill");
        c.args(["/F", "/IM", name]);
        c
    } else {
        let mut c = Command::new("pkill");
        c.args(["-f", name]);
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Build the platform command that opens a `steam://` URL.
fn steam_url_command(url: &str) -> std::process::Command {
    use std::process::Command;
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    }
}

/// Start the game.
///
/// Returns as soon as the launch has been handed off; it does not wait for the
/// game to appear, because there is no reliable cross-platform signal for that
/// and a caller who needs one should poll [`tail_log`].
///
/// # Errors
///
/// Returns [`Error::Launch`] if the launch command could not be spawned, or
/// [`Error::PathEscape`] if the definition names an executable outside the
/// install directory.
pub fn launch(install: &Path, def: &LaunchDef, steam_appid: Option<i64>) -> Result<LaunchOutcome> {
    use std::process::Stdio;

    let readiness = launch_readiness(def);
    let stopped = def.process.as_deref().is_some_and(stop_process);

    let (mut command, shown) = match def.mode.as_str() {
        "steam" => {
            let appid = steam_appid.ok_or_else(|| Error::Launch {
                message: "this game declares a Steam launch but has no steam_appid".to_owned(),
            })?;
            let url = format!("steam://rungameid/{appid}");
            (steam_url_command(&url), url)
        }
        "exe" => {
            let rel = def.exe.as_deref().ok_or_else(|| Error::Launch {
                message: "this game declares an exe launch but names no exe".to_owned(),
            })?;
            let exe = resolve_in(install, rel)?;
            let mut c = std::process::Command::new(&exe);
            c.args(&def.args).current_dir(install);
            // Spawning directly DOES let us pass the environment through.
            for (key, value) in &def.env {
                c.env(key, value);
            }
            (c, exe.display().to_string())
        }
        other => {
            return Err(Error::Launch {
                message: format!("unknown launch mode `{other}`"),
            });
        }
    };

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::Launch {
            message: format!("could not start `{shown}`: {e}"),
        })?;

    Ok(LaunchOutcome {
        command: shown,
        stopped_existing: stopped,
        readiness,
    })
}

/// Read up to `max_lines` lines from `path`, starting at byte offset `since`.
///
/// Detects truncation (loaders open their log with create/truncate, so every
/// launch restarts it at zero) and rewinds rather than reading from the middle
/// of a line in a fresh, shorter file.
///
/// # Errors
///
/// Returns [`Error::Io`] if the file cannot be read. A *missing* log is not an
/// error - a game that has never run has no log - and yields an empty tail.
pub fn tail_log(path: &Path, since: u64, max_lines: usize) -> Result<LogTail> {
    use std::io::{Read, Seek, SeekFrom};

    let empty = |cursor: u64, truncated: bool| LogTail {
        path: path.to_path_buf(),
        next_cursor: cursor,
        truncated,
        lines: Vec::new(),
    };

    let Ok(mut file) = std::fs::File::open(path) else {
        return Ok(empty(0, false));
    };
    let len = file.metadata().map_err(|e| Error::io(path, e))?.len();

    // A shorter file than the caller's cursor means a new session.
    let truncated = len < since;
    let start = if truncated { 0 } else { since };
    if len <= start {
        return Ok(empty(len, truncated));
    }

    // Read the tail-most window, never the whole file.
    let available = len.saturating_sub(start);
    let take = available.min(MAX_LOG_READ);
    let from = len.saturating_sub(take);
    file.seek(SeekFrom::Start(from))
        .map_err(|e| Error::io(path, e))?;

    let mut buf = Vec::new();
    file.take(take)
        .read_to_end(&mut buf)
        .map_err(|e| Error::io(path, e))?;

    let text = String::from_utf8_lossy(&buf);
    let cap = max_lines.clamp(1, MAX_LOG_LINES);
    let all: Vec<&str> = text.lines().collect();
    // Skip a leading partial line when the window started mid-file.
    let skip_partial = usize::from(from > 0 && !buf.starts_with(b"\n"));
    let body = all.get(skip_partial..).unwrap_or(&[]);
    let keep = body.len().saturating_sub(cap);
    let lines = body
        .get(keep..)
        .unwrap_or(&[])
        .iter()
        .map(|s| (*s).to_owned())
        .collect();

    Ok(LogTail {
        path: path.to_path_buf(),
        next_cursor: len,
        truncated,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loader_def() -> LoaderDef {
        LoaderDef {
            kind: "modrix-sons".to_owned(),
            install_root: "_Modrix".to_owned(),
            sentinel: "_Modrix/net6/Modrix.Loader.dll".to_owned(),
            proxy: Some("version.dll".to_owned()),
            interop_dir: Some("_Modrix/interop".to_owned()),
            version_file: Some("_Modrix/version.txt".to_owned()),
            conflicts: vec!["MelonLoader".to_owned(), "_Redloader".to_owned()],
            source: None,
        }
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn reports_a_missing_loader() {
        let tmp = tempfile::tempdir().unwrap();
        let status = loader_status(tmp.path(), &loader_def()).unwrap();
        assert!(!status.installed);
        assert!(status.conflict.is_none());
        assert!(!status.interop_ready);
        assert!(status.version.is_none());
    }

    #[test]
    fn reports_an_installed_loader_with_its_version() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("_Modrix/net6/Modrix.Loader.dll"));
        std::fs::write(tmp.path().join("_Modrix/version.txt"), "1.2.3\n").unwrap();
        touch(&tmp.path().join("_Modrix/interop/Sons.dll"));

        let status = loader_status(tmp.path(), &loader_def()).unwrap();
        assert!(status.installed);
        assert_eq!(status.version.as_deref(), Some("1.2.3"));
        assert!(status.interop_ready, "interop assemblies were generated");
    }

    #[test]
    fn detects_a_rival_loader_owning_the_proxy_slot() {
        let tmp = tempfile::tempdir().unwrap();
        // MelonLoader is installed; ours is not. Installing over it would
        // leave two loaders fighting for one version.dll and brick both.
        std::fs::create_dir_all(tmp.path().join("MelonLoader")).unwrap();

        let status = loader_status(tmp.path(), &loader_def()).unwrap();
        assert!(!status.installed);
        assert_eq!(status.conflict.as_deref(), Some("MelonLoader"));
    }

    #[test]
    fn an_installed_loader_is_not_its_own_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("_Modrix/net6/Modrix.Loader.dll"));
        std::fs::create_dir_all(tmp.path().join("MelonLoader")).unwrap();
        let status = loader_status(tmp.path(), &loader_def()).unwrap();
        assert!(status.installed);
        assert!(
            status.conflict.is_none(),
            "conflicts only gate a fresh install"
        );
    }

    #[test]
    fn refuses_a_definition_path_that_escapes_the_install_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let mut def = loader_def();
        def.sentinel = "../../etc/passwd".to_owned();
        assert!(matches!(
            loader_status(tmp.path(), &def),
            Err(Error::PathEscape { .. })
        ));
    }

    #[test]
    fn renders_launch_options_deterministically() {
        let mut env = BTreeMap::new();
        env.insert("WINEDLLOVERRIDES".to_owned(), "version=n,b".to_owned());
        env.insert("MANGOHUD".to_owned(), "1".to_owned());
        // BTreeMap ordering: MANGOHUD before WINEDLLOVERRIDES, every time.
        assert_eq!(
            render_launch_options(&env).as_deref(),
            Some("MANGOHUD=\"1\" WINEDLLOVERRIDES=\"version=n,b\" %command%")
        );
        assert!(render_launch_options(&BTreeMap::new()).is_none());
    }

    #[test]
    fn tails_a_log_from_a_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("Latest.log");
        std::fs::write(&log, "one\ntwo\nthree\n").unwrap();

        let first = tail_log(&log, 0, 100).unwrap();
        assert_eq!(first.lines, ["one", "two", "three"]);
        assert!(!first.truncated);

        // Nothing new yet.
        let idle = tail_log(&log, first.next_cursor, 100).unwrap();
        assert!(
            idle.lines.is_empty(),
            "expected empty, got {:?}",
            idle.lines
        );
        assert_eq!(idle.next_cursor, first.next_cursor);
    }

    #[test]
    fn detects_a_truncated_log_as_a_new_session() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("Latest.log");
        std::fs::write(&log, "old session, quite long indeed\n").unwrap();
        let first = tail_log(&log, 0, 100).unwrap();

        // The loader truncates on open, so the next session starts at zero.
        std::fs::write(&log, "new\n").unwrap();
        let second = tail_log(&log, first.next_cursor, 100).unwrap();
        assert!(
            second.truncated,
            "a shrunk log is a relaunch, not a corrupt read"
        );
        assert_eq!(second.lines, ["new"]);
    }

    #[test]
    fn a_missing_log_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let tail = tail_log(&tmp.path().join("never-ran.log"), 0, 10).unwrap();
        assert!(
            tail.lines.is_empty(),
            "expected empty, got {:?}",
            tail.lines
        );
        assert_eq!(tail.next_cursor, 0);
    }

    #[test]
    fn caps_the_number_of_lines_returned() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("Latest.log");
        let body = (0..500).fold(String::new(), |mut acc, i| {
            use std::fmt::Write as _;
            let _ = writeln!(acc, "line {i}");
            acc
        });
        std::fs::write(&log, body).unwrap();

        let tail = tail_log(&log, 0, 10).unwrap();
        assert_eq!(tail.lines.len(), 10, "the newest lines, bounded");
        assert_eq!(tail.lines.last().map(String::as_str), Some("line 499"));
    }

    #[test]
    fn launch_readiness_flags_the_proton_override() {
        let mut env = BTreeMap::new();
        env.insert("WINEDLLOVERRIDES".to_owned(), "version=n,b".to_owned());
        let def = LaunchDef {
            mode: "steam".to_owned(),
            exe: None,
            args: Vec::new(),
            env,
            log: Some("_Modrix/Latest.log".to_owned()),
            process: Some("SonsOfTheForest.exe".to_owned()),
        };
        let readiness = launch_readiness(&def);
        assert!(readiness.required_launch_options.is_some());
        // On Windows the proxy loads natively; elsewhere Steam must pass it.
        assert_eq!(readiness.env_reaches_game, cfg!(windows));
        assert_eq!(readiness.warning.is_some(), !cfg!(windows));
    }
}
