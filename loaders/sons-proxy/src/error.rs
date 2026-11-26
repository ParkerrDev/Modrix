// SPDX-License-Identifier: GPL-2.0-only
//! The one error type the proxy uses.
//!
//! Hand-rolled rather than `thiserror`, because this crate ships inside the
//! game process and deliberately depends on nothing but `windows-sys`. Every
//! variant carries enough context to identify the failing step from a single
//! log line, since a failure here happens on a user's machine under Proton with
//! no debugger attached.

use core::fmt;
use std::path::PathBuf;

/// Everything that can go wrong between `DLL_PROCESS_ATTACH` and the managed
/// entrypoint returning.
#[derive(Debug)]
pub enum Error {
    /// A Win32 call failed. `op` names the call; `code` is `GetLastError()`.
    Win32 {
        /// The Win32 function that failed, e.g. `"LoadLibraryW"`.
        op: &'static str,
        /// The value `GetLastError()` returned immediately afterwards.
        code: u32,
    },
    /// A file the loader needs is not on disk.
    Missing {
        /// What the file is for, e.g. `"coreclr"`.
        what: &'static str,
        /// Where it was looked for.
        path: PathBuf,
    },
    /// A module was loaded but did not export a symbol we require.
    MissingExport {
        /// Module file name, e.g. `"coreclr.dll"`.
        module: &'static str,
        /// The symbol that was absent.
        symbol: &'static str,
    },
    /// A string destined for a C API contained an interior NUL byte.
    InteriorNul(&'static str),
    /// A CoreCLR hosting call returned a non-zero HRESULT.
    CoreClr {
        /// The hosting function, e.g. `"coreclr_initialize"`.
        op: &'static str,
        /// The HRESULT it returned.
        hr: u32,
    },
    /// A bounded wait expired.
    Timeout {
        /// What was being waited for.
        what: &'static str,
        /// How long the wait lasted, in seconds.
        secs: u64,
    },
    /// The process path could not be turned into a game directory.
    NoGameRoot(PathBuf),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win32 { op, code } => write!(f, "{op} failed, GetLastError = {code}"),
            Self::Missing { what, path } => write!(f, "{what} not found at {}", path.display()),
            Self::MissingExport { module, symbol } => {
                write!(f, "{module} does not export {symbol}")
            }
            Self::InteriorNul(what) => write!(f, "{what} contains an interior NUL byte"),
            Self::CoreClr { op, hr } => write!(f, "{op} returned 0x{hr:08x}"),
            Self::Timeout { what, secs } => write!(f, "timed out after {secs}s waiting for {what}"),
            Self::NoGameRoot(path) => {
                write!(f, "cannot derive the game root from {}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::Error;
    use std::path::PathBuf;

    #[test]
    fn every_variant_renders_its_context() {
        let cases = [
            Error::Win32 {
                op: "LoadLibraryW",
                code: 126,
            },
            Error::Missing {
                what: "coreclr",
                path: PathBuf::from("/x/coreclr.dll"),
            },
            Error::MissingExport {
                module: "coreclr.dll",
                symbol: "coreclr_initialize",
            },
            Error::InteriorNul("APP_PATHS"),
            Error::CoreClr {
                op: "coreclr_initialize",
                hr: 0x8007_000e,
            },
            Error::Timeout {
                what: "GameAssembly.dll",
                secs: 120,
            },
            Error::NoGameRoot(PathBuf::from("/")),
        ];
        for case in cases {
            let rendered = case.to_string();
            assert!(!rendered.is_empty());
            // A bare Debug-style rendering would be useless in a user's log.
            assert!(!rendered.starts_with('{'));
        }
    }

    #[test]
    fn win32_error_names_the_call_and_code() {
        let msg = Error::Win32 {
            op: "LoadLibraryW",
            code: 126,
        }
        .to_string();
        assert!(msg.contains("LoadLibraryW"));
        assert!(msg.contains("126"));
    }

    #[test]
    fn coreclr_hresult_is_hex_padded() {
        let msg = Error::CoreClr {
            op: "coreclr_initialize",
            hr: 0x8007_000e,
        }
        .to_string();
        assert!(msg.contains("0x8007000e"), "{msg}");
    }
}
