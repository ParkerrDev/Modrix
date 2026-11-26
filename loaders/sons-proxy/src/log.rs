// SPDX-License-Identifier: GPL-2.0-only
//! `_Modrix/Latest.log` - the only diagnostic channel the proxy has.
//!
//! There is no console. Under Proton a failure inside `DllMain` closes the game
//! with no window, no stderr and no crash dump, so anything that is not written
//! to this file did not happen as far as the user is concerned. Every write is
//! therefore best-effort and never propagates an error: failing to log must not
//! be the reason the game dies.
//!
//! Native code writes the first three milestone lines; the managed entrypoint
//! appends the fourth. Both open the file in append mode, and native writing
//! stops before the managed call, so the two never interleave.

use crate::layout::PREVIOUS_LOG_FILE_NAME;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

/// Prefix every line the loader emits carries, so the log can be grepped and so
/// lines from the game's own stdout redirection are distinguishable.
pub const TAG: &str = "[modrix]";

/// Rotates the log to `Previous.log`, truncates it, and ensures its parent exists.
///
/// Called once, at `DLL_PROCESS_ATTACH`.
pub fn reset(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Keep one generation. The reason is not tidiness: when the loader takes the
    // game down, the run that explains why is the one being overwritten right
    // now - and the game is relaunched often enough (Steam, Proton, a crash
    // handler) that the evidence is gone before anyone can read it. Whatever
    // killed the previous run is in Previous.log.
    if path.is_file() {
        let previous = path.with_file_name(PREVIOUS_LOG_FILE_NAME);
        let _ = std::fs::rename(path, previous);
    }
    let _ = std::fs::write(path, b"");
}

/// Appends one tagged line. Best-effort; a failure is silently dropped.
pub fn line(path: &Path, message: &str) {
    let mut record =
        String::with_capacity(TAG.len().saturating_add(message.len()).saturating_add(2));
    let _ = writeln!(record, "{TAG} {message}");
    let opened = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    if let Ok(mut file) = opened {
        let _ = file.write_all(record.as_bytes());
        let _ = file.flush();
    }
}

/// Milestone 1: the proxy is loaded and the real system DLL is behind it.
///
/// `slot` is our own file name (`version.dll`), derived rather than assumed, so
/// the same binary dropped in as `winhttp.dll` reports the truth.
#[must_use]
pub fn proxy_attached(slot: &str, real: &Path) -> String {
    format!("proxy attached, real {slot} = {}", real.display())
}

/// Milestone 2: the game's IL2CPP module is mapped.
///
/// Deliberately reports the module base rather than an IL2CPP domain. Fetching
/// a domain means calling `il2cpp_domain_get`, and doing that from the
/// bootstrap thread aborts the game with "Collecting from unknown thread" - the
/// runtime's GC rejects calls from threads it never registered. See `il2cpp.rs`
/// for the bisection that established this.
///
/// So this proves only "the game is up far enough to host a runtime", which is
/// all the proxy needs. It does *not* prove we are ahead of the game, which is
/// what intercepting `il2cpp_init` would buy.
#[must_use]
pub fn game_assembly_mapped(base: usize) -> String {
    format!(
        "GameAssembly.dll mapped at {} (no runtime call made)",
        crate::layout::format_address(base)
    )
}

/// Milestone 3: CoreCLR initialized and a delegate to managed code exists.
#[must_use]
pub fn coreclr_up(runtime_version: &str) -> String {
    format!("coreclr up, runtime = {runtime_version}")
}

#[cfg(test)]
mod tests {
    use super::{TAG, coreclr_up, game_assembly_mapped, line, proxy_attached, reset};
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("modrix-sons-proxy-log")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("_Modrix").join("Latest.log")
    }

    #[test]
    fn reset_creates_the_loader_directory_and_empties_the_file() {
        let path = scratch("reset");
        line(&path, "written before reset");
        reset(&path);
        assert!(path.parent().unwrap().is_dir());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn reset_keeps_the_previous_run_alongside() {
        // When the loader kills the game, Steam relaunches it and the next
        // attach truncates the only record of why. One generation is kept so the
        // failing run is still readable afterwards.
        let path = scratch("rotate");
        // `reset` first, because `line` is a no-op until the directory exists -
        // in production `reset` always runs at DLL_PROCESS_ATTACH before any
        // line is written, so this mirrors the real order.
        reset(&path);
        line(&path, "the run that crashed");
        reset(&path);
        let previous = path.with_file_name("Previous.log");
        assert!(previous.is_file(), "Previous.log was not created");
        assert!(
            std::fs::read_to_string(&previous)
                .unwrap()
                .contains("the run that crashed")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn lines_are_tagged_appended_and_newline_terminated() {
        let path = scratch("append");
        reset(&path);
        line(&path, "first");
        line(&path, "second");
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, format!("{TAG} first\n{TAG} second\n"));
        std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn logging_to_an_unwritable_path_does_not_panic() {
        // A directory is never openable as a file; the write must be swallowed.
        line(Path::new("/"), "this cannot be written");
    }

    #[test]
    fn milestone_lines_match_the_documented_acceptance_text() {
        assert_eq!(
            proxy_attached("version.dll", Path::new("C:/windows/system32/version.dll")),
            "proxy attached, real version.dll = C:/windows/system32/version.dll"
        );
        assert_eq!(
            game_assembly_mapped(0x0000_7ffb_1234_5678),
            "GameAssembly.dll mapped at 0x00007ffb12345678 (no runtime call made)"
        );
        assert_eq!(coreclr_up("6.0.7"), "coreclr up, runtime = 6.0.7");
    }
}
