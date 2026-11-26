// SPDX-License-Identifier: GPL-2.0-only
//! Finding the DLL we are standing in front of, and binding our exports to it.
//!
//! The slot name is derived from our own file name rather than hard-coded, so
//! the same binary works when dropped in as `winhttp.dll` or `winmm.dll` - both
//! of which `UnityPlayer.dll` also imports. Only the `version.dll` export set is
//! compiled in today, so another slot forwards nothing; the derived name is what
//! makes that visible in the log instead of mysterious.

use crate::error::Error;
use crate::exports::{EXPORT_NAMES, SLOTS};
use core::sync::atomic::Ordering;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::HMODULE;

/// Outcome of a successful attach.
pub struct Attached {
    /// Our own file name, e.g. `version.dll`.
    pub slot: String,
    /// The real DLL now backing our exports.
    pub real: PathBuf,
    /// Export names the real DLL did not provide. Empty in every healthy case.
    pub missing: Vec<&'static str>,
}

/// Resolves the real DLL behind this proxy and points every export thunk at it.
///
/// Search order, matching what Doorstop and MelonLoader between them establish:
/// a user-supplied `<slot>_original.dll` or `<slot>_alt.dll` in the game
/// directory first, so the proxy can be chained in front of another loader, then
/// `%SystemDirectory%\<slot>.dll`. Our own file is skipped, since loading it
/// would bind every export back to these thunks and hang the first caller.
///
/// # Errors
///
/// Returns [`Error::Win32`] if our own module path cannot be read or the real
/// DLL cannot be loaded, and [`Error::Missing`] if no candidate exists.
pub fn install(self_module: HMODULE, game_root: &Path) -> Result<Attached, Error> {
    let own = crate::win::module_file_name(self_module)?;
    let slot = own.file_name().map_or_else(
        || "version.dll".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let stem = own.file_stem().map_or_else(
        || "version".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );

    let real = resolve_real_path(&own, &stem, game_root)?;
    let handle = crate::win::load_library(&real)?;
    let missing = bind_exports(handle);
    Ok(Attached {
        slot,
        real,
        missing,
    })
}

/// Picks the first candidate that exists on disk and is not this very file.
fn resolve_real_path(own: &Path, stem: &str, game_root: &Path) -> Result<PathBuf, Error> {
    let system = crate::win::system_directory()?;
    let candidates = [
        game_root.join(format!("{stem}_original.dll")),
        game_root.join(format!("{stem}_alt.dll")),
        system.join(format!("{stem}.dll")),
    ];
    let own_key = path_key(own);
    for candidate in &candidates {
        if path_key(candidate) != own_key && candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    Err(Error::Missing {
        what: "real proxy target",
        path: system.join(format!("{stem}.dll")),
    })
}

/// Windows paths are case-insensitive; comparing raw strings would let us load
/// ourselves whenever Wine or Steam hands back different casing.
fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('/', "\\")
}

/// Writes each resolved export address into its slot, returning the misses.
fn bind_exports(handle: HMODULE) -> Vec<&'static str> {
    let mut missing = Vec::new();
    for (index, name) in EXPORT_NAMES.iter().enumerate() {
        let Some(slot) = SLOTS.get(index) else {
            continue;
        };
        match crate::win::proc_address(handle, name) {
            Some(address) => slot.store(address, Ordering::SeqCst),
            None => missing.push(*name),
        }
    }
    missing
}
