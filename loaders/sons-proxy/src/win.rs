// SPDX-License-Identifier: GPL-2.0-only
//! Thin, checked wrappers over the handful of Win32 calls the proxy needs.
//!
//! Everything `unsafe` in this crate that is not an export thunk lives here, so
//! there is exactly one file to audit for FFI mistakes. Each wrapper converts a
//! Win32 failure into [`Error`] with the call's name attached, because a bare
//! `GetLastError` number in a log is not actionable.
//!
//! Nothing in here allocates on behalf of Windows: buffers are Rust `Vec<u16>`
//! and every loop is bounded.

use crate::error::Error;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{GetLastError, HMODULE};
use windows_sys::Win32::System::Environment::SetEnvironmentVariableW;
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleW, GetProcAddress, LoadLibraryW,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

/// Longest path we will accept from Windows, in UTF-16 code units.
///
/// `\\?\`-prefixed paths reach 32,767; the loop below starts small and grows to
/// this, which is a hard bound rather than "until it fits".
const MAX_PATH_UNITS: usize = 32_768;

/// Encodes a Rust string as a NUL-terminated UTF-16 buffer for the `*W` APIs.
fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Encodes a path as a NUL-terminated UTF-16 buffer.
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Encodes an ASCII symbol name as a NUL-terminated byte buffer for
/// `GetProcAddress`, which is ANSI-only.
fn ansi(symbol: &str) -> Vec<u8> {
    let mut bytes = symbol.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

/// The last Win32 error, as an [`Error`] naming the call that produced it.
fn last_error(op: &'static str) -> Error {
    // SAFETY: `GetLastError` reads thread-local state, takes no arguments and
    // has no failure mode.
    let code = unsafe { GetLastError() };
    Error::Win32 { op, code }
}

/// Full path of a loaded module. Pass the `DllMain` instance handle to learn our
/// own file name, which is what decides which proxy slot we are occupying.
///
/// # Errors
///
/// Returns [`Error::Win32`] if `GetModuleFileNameW` fails or if the path is
/// longer than [`MAX_PATH_UNITS`].
pub fn module_file_name(module: HMODULE) -> Result<PathBuf, Error> {
    let mut capacity: usize = 260;
    // Bounded: capacity doubles from 260, so at most 8 iterations to 32768.
    for _ in 0..8 {
        let mut buffer = vec![0u16; capacity];
        let request = u32::try_from(capacity).unwrap_or(u32::MAX);
        // SAFETY: `buffer` owns `capacity` writable u16s and `request` is that
        // same count, so the call cannot write out of bounds.
        let written = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), request) };
        let written = usize::try_from(written).unwrap_or(0);
        if written == 0 {
            return Err(last_error("GetModuleFileNameW"));
        }
        if written < capacity {
            buffer.truncate(written);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        capacity = capacity.saturating_mul(2).min(MAX_PATH_UNITS);
    }
    Err(Error::Win32 {
        op: "GetModuleFileNameW",
        code: 0,
    })
}

/// `%SystemDirectory%`, i.e. where the genuine `version.dll` lives.
///
/// # Errors
///
/// Returns [`Error::Win32`] if `GetSystemDirectoryW` fails or reports a path
/// longer than [`MAX_PATH_UNITS`].
pub fn system_directory() -> Result<PathBuf, Error> {
    let mut buffer = vec![0u16; MAX_PATH_UNITS];
    let request = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    // SAFETY: `buffer` owns `request` writable u16s.
    let written = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), request) };
    let written = usize::try_from(written).unwrap_or(0);
    if written == 0 || written >= buffer.len() {
        return Err(last_error("GetSystemDirectoryW"));
    }
    buffer.truncate(written);
    Ok(PathBuf::from(OsString::from_wide(&buffer)))
}

/// Loads a DLL by absolute path.
///
/// # Errors
///
/// Returns [`Error::Win32`] with the `LoadLibraryW` failure code.
pub fn load_library(path: &Path) -> Result<HMODULE, Error> {
    let encoded = wide_path(path);
    // SAFETY: `encoded` is NUL-terminated and outlives the call.
    let handle = unsafe { LoadLibraryW(encoded.as_ptr()) };
    if handle.is_null() {
        Err(last_error("LoadLibraryW"))
    } else {
        Ok(handle)
    }
}

/// Handle of an already-loaded module, or `None` if it is not loaded yet.
///
/// Does not take a reference on the module and does not load it - which is
/// exactly what the IL2CPP poll needs.
#[must_use]
pub fn module_handle(name: &str) -> Option<HMODULE> {
    let encoded = wide(name);
    // SAFETY: `encoded` is NUL-terminated and outlives the call.
    let handle = unsafe { GetModuleHandleW(encoded.as_ptr()) };
    if handle.is_null() { None } else { Some(handle) }
}

/// Address of an exported symbol, or `None` when the module does not export it.
///
/// Returned as `usize` rather than a typed pointer so the caller has to name the
/// signature it is transmuting to at the point of use.
#[must_use]
pub fn proc_address(module: HMODULE, symbol: &str) -> Option<usize> {
    let encoded = ansi(symbol);
    // SAFETY: `encoded` is a NUL-terminated ANSI name that outlives the call,
    // and `module` came from `LoadLibraryW`/`GetModuleHandleW`.
    let found = unsafe { GetProcAddress(module, encoded.as_ptr()) };
    found.map(|address| address as usize)
}

/// Sets a process environment variable in the Win32 environment block.
///
/// Used rather than [`std::env::set_var`] because CoreCLR and the managed side
/// both read through `GetEnvironmentVariableW`, and because Rust's wrapper
/// carries a stricter safety contract about concurrent `getenv` that we cannot
/// honour inside a running game.
///
/// # Errors
///
/// Returns [`Error::Win32`] with the `SetEnvironmentVariableW` failure code.
pub fn set_env(name: &str, value: &str) -> Result<(), Error> {
    let name_w = wide(name);
    let value_w = wide(value);
    // SAFETY: both buffers are NUL-terminated and outlive the call. The write
    // races only with other threads reading the same variable; nothing in the
    // game reads MODRIX_* and the managed side only reads it after this returns.
    let ok = unsafe { SetEnvironmentVariableW(name_w.as_ptr(), value_w.as_ptr()) };
    if ok == 0 {
        Err(last_error("SetEnvironmentVariableW"))
    } else {
        Ok(())
    }
}

/// Wine's version string, or `None` on real Windows.
///
/// `ntdll!wine_get_version` is the same probe RedLoader uses. It is logged
/// rather than acted on: CoreCLR's behaviour under Wine is the largest unknown
/// in M0, and knowing the exact Wine/Proton build from the user's own log is the
/// difference between a diagnosis and a guess.
#[must_use]
pub fn wine_version() -> Option<String> {
    type WineGetVersion = unsafe extern "system" fn() -> *const u8;
    let ntdll = module_handle("ntdll.dll")?;
    let address = proc_address(ntdll, "wine_get_version")?;
    // SAFETY: the symbol exists in this ntdll, and Wine documents it as a
    // no-argument function returning a static NUL-terminated ASCII string.
    let get_version = unsafe { core::mem::transmute::<usize, WineGetVersion>(address) };
    // SAFETY: calling the function just resolved, with its documented signature.
    let raw = unsafe { get_version() };
    if raw.is_null() {
        return None;
    }
    // SAFETY: Wine returns a pointer to a static NUL-terminated string that
    // lives for the life of the process.
    let text = unsafe { core::ffi::CStr::from_ptr(raw.cast()) };
    Some(text.to_string_lossy().into_owned())
}
