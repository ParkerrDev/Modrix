// SPDX-License-Identifier: GPL-2.0-only
//! Resolving a game's absolute deploy target root.
//!
//! A game's mods deploy into `mod_root` anchored to a `mod_base`. The default
//! base, `install`, is the game directory (`install_path`) - the only behavior
//! that existed before, and byte-identical still. The other bases put mods in
//! the user's profile folder, which a handful of games require (The Sims deploy
//! to `Documents/Electronic Arts/…`, Baldur's Gate 3 to `LocalAppData/Larian
//! Studios/…`, Factorio to `RoamingAppData/Factorio/mods`, …).
//!
//! On Windows the profile bases are the real user folders. On Linux/macOS a
//! Steam game runs under Proton, so its "user profile" lives inside the game's
//! compatdata prefix (`compatdata/<appid>/pfx/drive_c/users/steamuser/…`) -
//! the same mapping `loadorder.rs` uses for `Plugins.txt`. Resolution is
//! side-effect-free and gated on the profile actually existing: a profile base
//! resolves to `None` until the game has run once (so a never-launched Proton
//! prefix is never fabricated), and the caller decides whether that is a hard
//! error (deploy) or simply "nothing there" (a scan).

use std::path::{Path, PathBuf};

use crate::model::Game;

/// The absolute directory a game's mods deploy into: its `mod_base` anchor
/// joined with `mod_root`. `None` when a non-install base cannot be resolved
/// yet (e.g. the game has never run under Proton, so its profile folder does
/// not exist). The `install` base always resolves.
#[must_use]
pub fn deploy_root(game: &Game) -> Option<PathBuf> {
    let base = base_dir(&game.mod_base, &game.install_path, game.steam_appid)?;
    Some(join_mod_root(base, &game.mod_root))
}

/// Join a (possibly empty) `mod_root` onto a resolved base directory.
fn join_mod_root(base: PathBuf, mod_root: &str) -> PathBuf {
    if mod_root.is_empty() {
        base
    } else {
        base.join(mod_root)
    }
}

/// The four anchors a `mod_root` can hang from.
#[derive(Clone, Copy)]
enum Base {
    Install,
    Documents,
    LocalAppData,
    RoamingAppData,
}

/// Parse the stored base string. `""`, `"install"`, and anything unrecognized
/// resolve to `Install` (the safe default - a definition with a base this build
/// does not understand still deploys into the game directory rather than
/// nowhere).
fn parse_base(base: &str) -> Base {
    match base {
        "documents" => Base::Documents,
        "local_appdata" => Base::LocalAppData,
        "roaming_appdata" => Base::RoamingAppData,
        _ => Base::Install,
    }
}

/// Resolve the base directory `mod_root` is relative to.
fn base_dir(base: &str, install: &Path, appid: Option<i64>) -> Option<PathBuf> {
    match parse_base(base) {
        Base::Install => Some(install.to_path_buf()),
        profile => profile_dir(install, appid, profile),
    }
}

/// The user-profile directory for a non-install base, on Windows: the real
/// Known Folders, taken from the environment (no `unsafe` FFI). These always
/// exist for a logged-in user; the applier creates `mod_root` beneath.
#[cfg(windows)]
fn profile_dir(_install: &Path, _appid: Option<i64>, base: Base) -> Option<PathBuf> {
    match base {
        Base::Documents => {
            std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Documents"))
        }
        Base::LocalAppData => std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        Base::RoamingAppData => std::env::var_os("APPDATA").map(PathBuf::from),
        // base_dir never calls this for Install.
        Base::Install => None,
    }
}

/// The user-profile directory for a non-install base, on Linux/macOS: inside
/// the game's Proton prefix. `None` unless the prefix's user home exists (the
/// game has run at least once) - we never fabricate a prefix.
#[cfg(not(windows))]
fn profile_dir(install: &Path, appid: Option<i64>, base: Base) -> Option<PathBuf> {
    let home = proton_user_home(install, appid?)?;
    let rel = match base {
        Base::Documents => "Documents",
        Base::LocalAppData => "AppData/Local",
        Base::RoamingAppData => "AppData/Roaming",
        Base::Install => return None,
    };
    Some(home.join(rel))
}

/// A Steam game's Proton prefix user home:
/// `<steamapps>/common/<Game>` -> `<steamapps>/compatdata/<appid>/pfx/drive_c/
/// users/steamuser`. `None` unless it exists (mirrors `loadorder.rs`).
#[cfg(not(windows))]
fn proton_user_home(install: &Path, appid: i64) -> Option<PathBuf> {
    let steamapps = install.parent()?.parent()?;
    let home = steamapps
        .join("compatdata")
        .join(appid.to_string())
        .join("pfx/drive_c/users/steamuser");
    home.is_dir().then_some(home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::GameId;

    fn game(install: &Path, mod_root: &str, mod_base: &str, appid: Option<i64>) -> Game {
        Game {
            id: GameId::from_raw(1),
            plugin_id: "g".to_owned(),
            name: "G".to_owned(),
            install_path: install.to_path_buf(),
            mod_root: mod_root.to_owned(),
            mod_base: mod_base.to_owned(),
            store: "steam".to_owned(),
            steam_appid: appid,
            nexus_domain: None,
            staging_root: PathBuf::from("/staging"),
        }
    }

    #[test]
    fn install_base_is_install_path_joined_with_mod_root() {
        let install = PathBuf::from("/games/Skyrim");
        assert_eq!(
            deploy_root(&game(&install, "Data", "install", Some(1))),
            Some(install.join("Data"))
        );
        // Empty base string is treated as install (v1 / legacy rows).
        assert_eq!(
            deploy_root(&game(&install, "Data", "", Some(1))),
            Some(install.join("Data"))
        );
        // Empty mod_root deploys at the base itself.
        assert_eq!(
            deploy_root(&game(&install, "", "install", Some(1))),
            Some(install)
        );
    }

    #[test]
    fn unknown_base_falls_back_to_install() {
        let install = PathBuf::from("/games/X");
        assert_eq!(
            deploy_root(&game(&install, "mods", "future_base_v9", Some(1))),
            Some(install.join("mods"))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn documents_base_resolves_inside_an_initialized_proton_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("steamapps/common/The Sims 4");
        let home = tmp
            .path()
            .join("steamapps/compatdata/1222670/pfx/drive_c/users/steamuser");
        std::fs::create_dir_all(&install).unwrap();
        // No prefix yet -> unresolved (the game has never run).
        let g = game(
            &install,
            "Electronic Arts/The Sims 4/Mods",
            "documents",
            Some(1_222_670),
        );
        assert_eq!(deploy_root(&g), None);
        // Initialized prefix -> resolves into it (the leaf need not exist; the
        // applier creates it).
        std::fs::create_dir_all(&home).unwrap();
        assert_eq!(
            deploy_root(&g),
            Some(home.join("Documents/Electronic Arts/The Sims 4/Mods"))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn appdata_bases_map_to_the_right_prefix_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("steamapps/common/BG3");
        let home = tmp
            .path()
            .join("steamapps/compatdata/1086940/pfx/drive_c/users/steamuser");
        std::fs::create_dir_all(&home).unwrap();
        let local = game(
            &install,
            "Larian Studios/BG3/Mods",
            "local_appdata",
            Some(1_086_940),
        );
        assert_eq!(
            deploy_root(&local),
            Some(home.join("AppData/Local/Larian Studios/BG3/Mods"))
        );
        let roaming = game(
            &install,
            "Factorio/mods",
            "roaming_appdata",
            Some(1_086_940),
        );
        assert_eq!(
            deploy_root(&roaming),
            Some(home.join("AppData/Roaming/Factorio/mods"))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn a_profile_base_without_a_steam_appid_is_unresolved() {
        let g = game(Path::new("/games/X"), "mods", "documents", None);
        assert_eq!(deploy_root(&g), None);
    }
}
