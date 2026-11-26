// SPDX-License-Identifier: GPL-2.0-only
//! Where everything lives on disk, derived once from the process path.
//!
//! This module is pure: no FFI, no Windows types, no I/O except the one
//! best-effort read of the runtime stamp. That is deliberate - path derivation
//! is the part most likely to be wrong under Proton (the CWD is not the game
//! directory, and Wine's case folding hides casing mistakes), so it is the part
//! that has to be unit-testable on a Linux host.
//!
//! The names below must stay in lockstep with `games/sonsoftheforest/game.toml`
//! (`install_root`, `sentinel`, `[launch] log`). If one moves, both move.

use std::path::{Path, PathBuf};

/// Loader install root, relative to the game directory.
pub const LOADER_DIR_NAME: &str = "_Modrix";
/// Vendored CoreCLR directory, relative to [`LOADER_DIR_NAME`].
pub const DOTNET_DIR_NAME: &str = "dotnet";
/// Managed host directory, relative to [`LOADER_DIR_NAME`].
pub const MANAGED_DIR_NAME: &str = "net6";
/// Generated IL2CPP interop assemblies, relative to [`LOADER_DIR_NAME`].
pub const INTEROP_DIR_NAME: &str = "interop";
/// Log file name, relative to [`LOADER_DIR_NAME`].
pub const LOG_FILE_NAME: &str = "Latest.log";
/// Previous session's log, kept so a run that killed the game is still readable.
pub const PREVIOUS_LOG_FILE_NAME: &str = "Previous.log";
/// The CoreCLR shared library we host.
pub const CORECLR_FILE_NAME: &str = "coreclr.dll";
/// BepInEx's `mini-coreclr` drops this stamp file beside `coreclr.dll`.
pub const VERSION_STAMP_NAME: &str = ".version";

/// Managed assembly name, WITHOUT extension - this is what
/// `coreclr_create_delegate` wants.
pub const MANAGED_ASSEMBLY: &str = "Modrix.Loader";
/// Fully qualified managed type hosting the entrypoint.
pub const ENTRY_TYPE: &str = "Modrix.Loader.Entrypoint";
/// Managed entrypoint method. Zero parameters, void return, non-negotiable.
pub const ENTRY_METHOD: &str = "Start";
/// The AppDomain friendly name passed to `coreclr_initialize`.
pub const DOMAIN_FRIENDLY_NAME: &str = "Modrix Domain";

/// `APP_PATHS` uses the platform path-list separator. The proxy only ever runs
/// as a Windows PE (under Wine on Linux), so this is always `;` - deriving it
/// from the *host* would be wrong when cross-compiling.
const APP_PATHS_SEPARATOR: char = ';';

/// Every path the loader needs, resolved from the game executable's own path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    process_path: PathBuf,
    game_root: PathBuf,
    loader_dir: PathBuf,
    dotnet_dir: PathBuf,
    interop_dir: PathBuf,
    managed_dir: PathBuf,
}

impl Layout {
    /// Derives the layout from the full path of the running executable.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::NoGameRoot`] when `process_path` has no
    /// parent directory, which would mean Windows handed us a bare file name.
    pub fn from_process_path(process_path: PathBuf) -> Result<Self, crate::error::Error> {
        let game_root = process_path
            .parent()
            .ok_or_else(|| crate::error::Error::NoGameRoot(process_path.clone()))?
            .to_path_buf();
        let loader_dir = game_root.join(LOADER_DIR_NAME);
        let dotnet_dir = loader_dir.join(DOTNET_DIR_NAME);
        let interop_dir = loader_dir.join(INTEROP_DIR_NAME);
        let managed_dir = loader_dir.join(MANAGED_DIR_NAME);
        Ok(Self {
            process_path,
            game_root,
            loader_dir,
            dotnet_dir,
            interop_dir,
            managed_dir,
        })
    }

    /// Full path of the running game executable.
    #[must_use]
    pub fn process_path(&self) -> &Path {
        &self.process_path
    }

    /// Directory containing the game executable.
    #[must_use]
    pub fn game_root(&self) -> &Path {
        &self.game_root
    }

    /// `<game>/_Modrix`.
    #[must_use]
    pub fn loader_dir(&self) -> &Path {
        &self.loader_dir
    }

    /// `<game>/_Modrix/dotnet`.
    #[must_use]
    pub fn dotnet_dir(&self) -> &Path {
        &self.dotnet_dir
    }

    /// `<game>/_Modrix/net6`.
    #[must_use]
    pub fn managed_dir(&self) -> &Path {
        &self.managed_dir
    }

    /// `<game>/_Modrix/Latest.log`.
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.loader_dir.join(LOG_FILE_NAME)
    }

    /// `<game>/_Modrix/dotnet/coreclr.dll`.
    #[must_use]
    pub fn coreclr_path(&self) -> PathBuf {
        self.dotnet_dir.join(CORECLR_FILE_NAME)
    }

    /// The value of the single `APP_PATHS` property CoreCLR is initialized with:
    /// the corlib directory, the host directory, then the interop directory.
    ///
    /// Doorstop passes exactly one property and the first two of these. The
    /// third is ours: a mod's own assembly references the *game's* assemblies by
    /// name (`Sons`, `UnityEngine.CoreModule`), and without the interop
    /// directory on the probing path the runtime cannot find them - the mod
    /// loads, then throws `FileNotFoundException: Could not load file or
    /// assembly 'Sons'` the moment it touches a game type.
    #[must_use]
    pub fn app_paths(&self) -> String {
        format!(
            "{}{APP_PATHS_SEPARATOR}{}{APP_PATHS_SEPARATOR}{}",
            self.dotnet_dir.display(),
            self.managed_dir.display(),
            self.interop_dir.display()
        )
    }

    /// `<game>/_Modrix/interop`, where generated reference assemblies live.
    #[must_use]
    pub fn interop_dir(&self) -> &Path {
        &self.interop_dir
    }

    /// Whether the running process is the Unity player this loader belongs to.
    ///
    /// A `version.dll` next to the game is loaded by *every* PE started from that
    /// directory, not only the game. Under Proton that is a real and frequent
    /// occurrence - Steam's helpers, Wine services and the game's own crash
    /// handler all qualify - and every one of them ran the full bootstrap:
    /// truncating `Latest.log`, then blocking 120 seconds waiting for a
    /// `GameAssembly.dll` that process will never map. The visible symptom is a
    /// loader log that keeps resetting to six lines and a `timed out waiting for
    /// GameAssembly.dll module` error while the game is plainly running, with
    /// nothing indicating a second process is involved at all.
    ///
    /// Unity's own layout settles it. A player executable always sits beside a
    /// data directory named after it - `SonsOfTheForest.exe` next to
    /// `SonsOfTheForest_Data`. No helper process satisfies that, and it holds for
    /// every Unity game rather than being specific to this one.
    #[must_use]
    pub fn owns_game(&self) -> bool {
        let Some(stem) = self.process_path.file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        self.game_root.join(format!("{stem}_Data")).is_dir()
    }

    /// The `TRUSTED_PLATFORM_ASSEMBLIES` property: every managed assembly the
    /// runtime ships, by full path.
    ///
    /// Doorstop does not pass this, and for simply *running* managed code it is
    /// not needed - `APP_PATHS` probing covers that. It is needed the moment
    /// anything asks the runtime where the platform assemblies are, because
    /// nothing else answers that question:
    ///
    /// - `AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")` is exactly this
    ///   value, and returns null when it is not passed.
    /// - Mono.Cecil resolves platform assemblies *only* from there
    ///   (`SearchTrustedPlatformAssemblies`). With it unset, Cecil cannot resolve
    ///   `System.Private.CoreLib`.
    /// - Harmony builds delegate types through Cecil, so with it unset **every
    ///   Harmony patch of an IL2CPP method fails** with `IL Compile Error
    ///   (unknown location)` - a message that says nothing about any of this.
    ///
    /// Assemblies are listed runtime-first so that a name shipped by both the
    /// runtime and the managed host resolves to the runtime's copy, which is the
    /// same precedence `APP_PATHS` gives.
    ///
    /// Unreadable directories yield an empty contribution rather than an error:
    /// a missing managed directory is already reported by the delegate lookup
    /// with a far clearer message than anything available here.
    #[must_use]
    pub fn trusted_platform_assemblies(&self) -> String {
        let mut paths: Vec<String> = Vec::new();
        for dir in [&self.dotnet_dir, &self.managed_dir] {
            let mut found = managed_assemblies_in(dir);
            // Sorted so the property is byte-stable across launches; directory
            // order is not guaranteed and a shifting value makes any
            // resolution bug look intermittent.
            found.sort();
            paths.extend(found);
        }
        paths.join(&APP_PATHS_SEPARATOR.to_string())
    }

    /// The `MODRIX_*` environment block the managed entrypoint reads instead of
    /// re-deriving any of these paths itself.
    ///
    /// `game_assembly` is the mapped base of `GameAssembly.dll`. It is deliberately
    /// NOT an IL2CPP domain pointer - obtaining one would mean calling into the
    /// runtime from an unregistered thread, which aborts the game (see
    /// `il2cpp.rs`). Managed code must attach to IL2CPP itself, on the main thread.
    /// the log line renders it so the two can be eyeballed against each other.
    #[must_use]
    pub fn env_pairs(&self, game_assembly: usize) -> Vec<(&'static str, String)> {
        vec![
            ("MODRIX_INITIALIZED", "TRUE".to_owned()),
            (
                "MODRIX_PROCESS_PATH",
                self.process_path.display().to_string(),
            ),
            ("MODRIX_GAME_ROOT", self.game_root.display().to_string()),
            ("MODRIX_LOADER_DIR", self.loader_dir.display().to_string()),
            ("MODRIX_GAME_ASSEMBLY", format_address(game_assembly)),
        ]
    }
}

/// Full paths of every `.dll` directly inside `dir`, or empty if unreadable.
///
/// Not recursive: CoreCLR's trusted list is a flat set of assemblies, and a
/// runtime layout nests only culture-specific satellites, which do not belong
/// on it.
fn managed_assemblies_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dll = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"));
        if is_dll && path.is_file() {
            found.push(path.display().to_string());
        }
    }
    found
}

/// Renders a module base for logs and for `MODRIX_GAME_ASSEMBLY`.
#[must_use]
pub fn format_address(address: usize) -> String {
    format!("0x{address:016x}")
}

/// Reads the runtime version out of `mini-coreclr`'s `.version` stamp.
///
/// The file is two lines - a commit hash then a version, e.g. `6.0.7` - so the
/// last non-empty line is the version. Returns `None` when the stamp is absent
/// or unreadable; the caller logs `unknown` rather than failing, because a
/// missing stamp does not stop the runtime from working.
#[must_use]
pub fn read_runtime_version(dotnet_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dotnet_dir.join(VERSION_STAMP_NAME)).ok()?;
    raw.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{Layout, format_address, read_runtime_version};
    use std::path::PathBuf;

    fn layout() -> Layout {
        Layout::from_process_path(PathBuf::from("/game/SonsOfTheForest.exe")).unwrap()
    }

    #[test]
    fn derives_every_directory_from_the_exe_path() {
        let l = layout();
        assert_eq!(l.game_root(), PathBuf::from("/game"));
        assert_eq!(l.loader_dir(), PathBuf::from("/game/_Modrix"));
        assert_eq!(l.dotnet_dir(), PathBuf::from("/game/_Modrix/dotnet"));
        assert_eq!(l.managed_dir(), PathBuf::from("/game/_Modrix/net6"));
        assert_eq!(l.log_path(), PathBuf::from("/game/_Modrix/Latest.log"));
        assert_eq!(
            l.coreclr_path(),
            PathBuf::from("/game/_Modrix/dotnet/coreclr.dll")
        );
    }

    #[test]
    fn log_path_matches_the_game_definition() {
        // games/sonsoftheforest/game.toml declares `[launch] log = "_Modrix/Latest.log"`.
        let l = layout();
        let relative = l
            .log_path()
            .strip_prefix(l.game_root())
            .unwrap()
            .to_path_buf();
        assert_eq!(relative, PathBuf::from("_Modrix").join("Latest.log"));
    }

    #[test]
    fn app_paths_is_corlib_then_host_then_interop() {
        let paths = layout().app_paths();
        let parts: Vec<&str> = paths.split(';').collect();
        assert_eq!(parts.len(), 3, "{paths}");
        assert!(parts.first().unwrap().ends_with("dotnet"), "{paths}");
        assert!(parts.get(1).unwrap().ends_with("net6"), "{paths}");
        // Without this a mod loads and then cannot resolve `Sons` or
        // `UnityEngine.CoreModule` the instant it touches a game type.
        assert!(parts.get(2).unwrap().ends_with("interop"), "{paths}");
    }

    #[test]
    fn trusted_platform_assemblies_lists_runtime_then_host_dlls() {
        // Without this property Cecil cannot resolve System.Private.CoreLib, and
        // every Harmony patch of an IL2CPP method fails with a message that
        // mentions neither Cecil nor this property. The ordering matters too:
        // runtime first, so a name present in both resolves to the runtime copy.
        let root = std::env::temp_dir().join("modrix-sons-proxy-tpa");
        let dotnet = root.join("_Modrix").join("dotnet");
        let net6 = root.join("_Modrix").join("net6");
        std::fs::create_dir_all(&dotnet).unwrap();
        std::fs::create_dir_all(&net6).unwrap();
        std::fs::write(dotnet.join("System.Private.CoreLib.dll"), b"x").unwrap();
        std::fs::write(dotnet.join("coreclr.dll"), b"x").unwrap();
        std::fs::write(net6.join("Modrix.Loader.dll"), b"x").unwrap();
        // Not an assembly: must not appear.
        std::fs::write(net6.join("Modrix.Loader.deps.json"), b"{}").unwrap();

        let layout = Layout::from_process_path(root.join("SonsOfTheForest.exe")).unwrap();
        let value = layout.trusted_platform_assemblies();
        let parts: Vec<&str> = value.split(';').collect();

        assert_eq!(parts.len(), 3, "{value}");
        assert!(
            parts.iter().all(|p| std::path::Path::new(p)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))),
            "{value}"
        );
        assert!(
            value.contains("System.Private.CoreLib.dll"),
            "corlib is the whole point: {value}"
        );
        assert!(!value.contains("deps.json"), "{value}");
        // Runtime entries precede host entries.
        let corelib = value.find("System.Private.CoreLib.dll").unwrap();
        let loader = value.find("Modrix.Loader.dll").unwrap();
        assert!(corelib < loader, "{value}");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn trusted_platform_assemblies_is_empty_when_nothing_is_installed() {
        let layout =
            Layout::from_process_path(PathBuf::from("/nonexistent-modrix/SonsOfTheForest.exe"))
                .unwrap();
        assert_eq!(layout.trusted_platform_assemblies(), "");
    }

    #[test]
    fn only_the_unity_player_owns_the_game() {
        // A version.dll beside the game is loaded by every PE started from that
        // directory. Letting a helper process bootstrap truncates the real run's
        // log and then blocks two minutes waiting for GameAssembly.dll.
        let root = std::env::temp_dir().join("modrix-sons-proxy-owns");
        std::fs::create_dir_all(root.join("SonsOfTheForest_Data")).unwrap();

        let player = Layout::from_process_path(root.join("SonsOfTheForest.exe")).unwrap();
        assert!(player.owns_game(), "the Unity player must own the game");

        // Same directory, no matching _Data folder: a helper, not the player.
        let helper = Layout::from_process_path(root.join("steamerrorreporter64.exe")).unwrap();
        assert!(!helper.owns_game(), "a helper process must not bootstrap");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_bare_file_name_has_no_game_root() {
        assert!(Layout::from_process_path(PathBuf::from("/")).is_err());
    }

    #[test]
    fn env_pairs_cover_the_documented_five() {
        let pairs = layout().env_pairs(0x0000_7ffb_1234_5678);
        let names: Vec<&str> = pairs.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "MODRIX_INITIALIZED",
                "MODRIX_PROCESS_PATH",
                "MODRIX_GAME_ROOT",
                "MODRIX_LOADER_DIR",
                "MODRIX_GAME_ASSEMBLY"
            ]
        );
        assert_eq!(pairs.first().unwrap().1, "TRUE");
        assert_eq!(pairs.get(4).unwrap().1, "0x00007ffb12345678");
    }

    #[test]
    fn domain_is_zero_padded_hex() {
        assert_eq!(format_address(0), "0x0000000000000000");
        assert_eq!(format_address(1), "0x0000000000000001");
    }

    #[test]
    fn runtime_version_is_the_last_non_empty_line_of_the_stamp() {
        // Byte-for-byte what `mini-coreclr-Release.zip` 6.0.7 ships in
        // `win-x64/.version`, CRLF included - checked with `od -c`. A naive
        // `read_to_string().trim()` would yield the commit hash, not the version.
        let dir = std::env::temp_dir().join("modrix-sons-proxy-version-stamp");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(super::VERSION_STAMP_NAME),
            "0ec02c8c96e2eda06dc5b5edfdbdba0f36415082\r\n6.0.7\r\n",
        )
        .unwrap();
        assert_eq!(read_runtime_version(&dir).as_deref(), Some("6.0.7"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_runtime_stamp_is_not_an_error() {
        assert_eq!(
            read_runtime_version(&PathBuf::from("/nonexistent-modrix-dir")),
            None
        );
    }
}
