// SPDX-License-Identifier: GPL-2.0-only
//! Install detection: turn a `game.toml`'s declared probe strategies into an
//! install path by reading on-disk store metadata.
//!
//! Tier-1 games carry `install_probe = ["steam", "gog", …]` (see `gamedef.rs`);
//! this module implements those strategies so a frontend can offer "this
//! supported game is installed - add it" without the user hunting for the
//! directory. It is best-effort and side-effect-free: a miss returns `None`,
//! never an error, and nothing here writes or networks. A resolved directory is
//! accepted only if it holds the definition's `required_files`.
//!
//! `steam` is cross-platform: pure filesystem reading of Valve's `KeyValues`
//! files (`libraryfolders.vdf`, `appmanifest_<id>.acf`) - no Steam API, no
//! network. The other stores read Windows-only sources and resolve to `None`
//! elsewhere: `gog`, `xbox`, `uplay` and the generic `registry` probe read the
//! registry (via `winreg`); `epic` and `origin` parse the launchers' on-disk
//! manifests. `path-hint` is still a stub.
//!
//! Every read is size-capped and every scan bounded (Power of Ten §9.3): a
//! malformed or hostile metadata file can waste no more than a bounded effort.

use std::path::{Path, PathBuf};

use crate::gamedef::{GameDef, RegistryKeyDef};

/// Cap on bytes read from any Valve metadata file. These are a few KB in the
/// wild; the bound stops a huge or hostile file from being read whole.
const MAX_VDF_BYTES: u64 = 1 << 20; // 1 MiB

/// Cap on library roots scanned per Steam install (a heavy setup has a handful).
const MAX_LIBRARIES: usize = 64;

/// Cap on lines scanned in one metadata file.
const MAX_VDF_LINES: usize = 100_000;

/// Cap on quoted tokens pulled from one line (real lines have two).
const MAX_TOKENS_PER_LINE: usize = 8;

/// Locate a game's install directory using its declared probe strategies, in
/// declared order. Returns the first existing directory found, or `None` when
/// nothing detects (the caller should then offer manual entry).
#[must_use]
pub fn detect_install(def: &GameDef) -> Option<PathBuf> {
    for strategy in &def.install_probe {
        // A probe that resolves a directory only wins if that directory holds
        // the game's `required_files` (when any are declared) - a stale Steam
        // manifest or a same-vendor registry key can otherwise point at the
        // wrong install.
        if let Some(dir) = probe(def, strategy)
            && install_confirmed(&dir, def)
        {
            return Some(dir);
        }
    }
    None
}

/// Resolve one declared probe strategy to a candidate install directory.
fn probe(def: &GameDef, strategy: &str) -> Option<PathBuf> {
    match strategy {
        "steam" => def.steam_appid.and_then(steam_install),
        "gog" => def.gog_id.as_deref().and_then(gog_install),
        "epic" => def.epic_id.as_deref().and_then(epic_install),
        "xbox" => def.xbox_id.as_deref().and_then(xbox_install),
        "origin" => def.origin_id.as_deref().and_then(origin_install),
        "uplay" => def.uplay_id.as_deref().and_then(uplay_install),
        "registry" => registry_install(&def.registry_keys),
        // `path-hint` is not implemented yet; skip cleanly so a later
        // implemented strategy in the list still gets its turn.
        _ => None,
    }
}

/// Whether `dir` holds every file the definition declares as required. An
/// empty `required_files` accepts any directory. Public so a frontend can
/// validate a manually-entered directory against the same rule detection uses.
#[must_use]
pub fn install_confirmed(dir: &Path, def: &GameDef) -> bool {
    def.required_files.iter().all(|rel| dir.join(rel).exists())
}

/// Find where Steam installed the app with this id, if anywhere on this box.
#[must_use]
pub fn steam_install(appid: i64) -> Option<PathBuf> {
    detect_in_roots(&steam_roots(), appid)
}

/// The install-dir search over an explicit set of Steam roots - the testable
/// core of [`steam_install`], with the environment factored out.
fn detect_in_roots(roots: &[PathBuf], appid: i64) -> Option<PathBuf> {
    for root in roots {
        for library in libraries(root).into_iter().take(MAX_LIBRARIES) {
            if let Some(dir) = app_install_dir(&library, appid) {
                return Some(dir);
            }
        }
    }
    None
}

/// Existing, de-duplicated Steam root directories for this platform. Paths are
/// canonicalized so the `~/.steam/steam` symlink and its `~/.local/share/Steam`
/// target collapse to one entry (and are scanned once). Public so frontends
/// can locate Steam's local artwork cache (`appcache/librarycache/<appid>`).
pub fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        // Linux native installs (both the historical symlink names) …
        push_dir(&mut roots, &home.join(".steam/steam"));
        push_dir(&mut roots, &home.join(".steam/root"));
        push_dir(&mut roots, &home.join(".local/share/Steam"));
        // … Flatpak Steam …
        push_dir(
            &mut roots,
            &home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
        );
        // … and macOS.
        push_dir(&mut roots, &home.join("Library/Application Support/Steam"));
    }
    // Windows default install locations (registry probe not implemented).
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
            push_dir(&mut roots, &base.join("Steam"));
        }
    }
    roots
}

/// Add `p`'s canonical form to `roots` if it is an existing directory not
/// already present. `canonicalize` fails for a missing path, so this also
/// filters out candidates that do not exist.
fn push_dir(roots: &mut Vec<PathBuf>, p: &Path) {
    if let Ok(real) = p.canonicalize()
        && real.is_dir()
        && !roots.contains(&real)
    {
        roots.push(real);
    }
}

/// The library roots registered with a Steam install: the root itself plus
/// every `path` in its `libraryfolders.vdf` (games may live on other drives).
fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![root.to_path_buf()];
    // Current Steam keeps the index under `steamapps/`; older builds under
    // `config/`. Read whichever exists (both, if both do).
    for rel in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
        let Some(text) = read_capped(&root.join(rel)) else {
            continue;
        };
        for value in vdf_values(&text, "path") {
            let p = PathBuf::from(value);
            if p.is_dir() && !libs.contains(&p) {
                libs.push(p);
            }
        }
    }
    libs
}

/// If `library` holds the app, resolve its install directory from the app
/// manifest's `installdir` (relative to `steamapps/common/`).
fn app_install_dir(library: &Path, appid: i64) -> Option<PathBuf> {
    let manifest = library.join(format!("steamapps/appmanifest_{appid}.acf"));
    let text = read_capped(&manifest)?;
    let installdir = vdf_values(&text, "installdir").into_iter().next()?;
    // `installdir` is a bare directory name; reject a value that tries to
    // escape `common/` (defence-in-depth against a doctored manifest).
    if installdir.is_empty() || Path::new(&installdir).components().count() != 1 {
        return None;
    }
    let dir = library.join("steamapps/common").join(installdir);
    dir.is_dir().then_some(dir)
}

/// Read a small metadata file, bounded to [`MAX_VDF_BYTES`]. Returns `None` on
/// any error, a missing file, or an implausibly large one.
fn read_capped(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_VDF_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Every scalar value whose key equals `key`, in file order. A VDF scalar entry
/// is `"key" "value"` on one line; block headers (`"apps"` then `{`) have no
/// second token and are skipped, so a flat line scan suffices for the two keys
/// we read and never confuses a nested block for a value.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().take(MAX_VDF_LINES) {
        let tokens = quoted_tokens(line);
        if tokens.first().map(String::as_str) == Some(key)
            && let Some(value) = tokens.get(1)
        {
            out.push(value.clone());
        }
    }
    out
}

/// The double-quoted tokens on one line, unescaped, capped at
/// [`MAX_TOKENS_PER_LINE`] so a pathological line cannot allocate unboundedly.
fn quoted_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut token = String::new();
        let mut escaped = false;
        for ch in chars.by_ref() {
            match (escaped, ch) {
                (true, other) => {
                    token.push(unescape(other));
                    escaped = false;
                }
                (false, '\\') => escaped = true,
                (false, '"') => break,
                (false, other) => token.push(other),
            }
        }
        tokens.push(token);
        if tokens.len() >= MAX_TOKENS_PER_LINE {
            break;
        }
    }
    tokens
}

/// Resolve a VDF backslash escape. Notably `\\` -> `\`, which un-doubles the
/// separators in a Windows library path; unknown escapes keep the raw char.
fn unescape(ch: char) -> char {
    match ch {
        'n' => '\n',
        't' => '\t',
        other => other,
    }
}

// --- non-Steam stores ------------------------------------------------------
//
// GOG, Xbox and Uplay live entirely in the Windows registry; Epic and Origin
// keep on-disk manifests (JSON / query-string) but are rooted through the
// registry or a Windows-only ProgramData path. So the store-specific probes
// resolve to `None` on non-Windows platforms (only Steam is cross-platform).
// The manifest *parsers* are platform-independent and unit-tested everywhere.

/// Find a game's install directory from an Epic `Manifests/*.item` directory.
/// Each `.item` is JSON with `AppName` (the catalog codename) and
/// `InstallLocation`; returns the location of the manifest whose `AppName`
/// matches, when that directory exists.
#[cfg(any(windows, test))]
fn epic_install_in(manifests: &Path, appname: &str) -> Option<PathBuf> {
    const MAX_MANIFESTS: usize = 4096;
    let entries = std::fs::read_dir(manifests).ok()?;
    for entry in entries.flatten().take(MAX_MANIFESTS) {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "item") {
            continue;
        }
        let Some(text) = read_capped(&path) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if json.get("AppName").and_then(serde_json::Value::as_str) != Some(appname) {
            continue;
        }
        if let Some(loc) = json
            .get("InstallLocation")
            .and_then(serde_json::Value::as_str)
        {
            let dir = PathBuf::from(loc);
            if dir.is_dir() {
                return Some(dir);
            }
        }
    }
    None
}

/// Find a game's install directory under an Origin `LocalContent/` directory.
/// Each game has a `<Game>/<something>.mfst` file whose body is a URL query
/// string; the `id` field identifies the game and `dipinstallpath` is the
/// install directory.
#[cfg(any(windows, test))]
fn origin_install_in(localcontent: &Path, id: &str) -> Option<PathBuf> {
    const MAX_ORIGIN_DIRS: usize = 4096;
    let entries = std::fs::read_dir(localcontent).ok()?;
    for entry in entries.flatten().take(MAX_ORIGIN_DIRS) {
        let sub = entry.path();
        if !sub.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&sub) else {
            continue;
        };
        for file in files.flatten().take(MAX_ORIGIN_DIRS) {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "mfst") {
                continue;
            }
            if let Some(text) = read_capped(&path)
                && let Some(dir) = origin_mfst_dir(&text, id)
            {
                return Some(dir);
            }
        }
    }
    None
}

/// Parse one `.mfst` query string; return its `dipinstallpath` when the `id`
/// field matches `want` and the path exists.
#[cfg(any(windows, test))]
fn origin_mfst_dir(text: &str, want: &str) -> Option<PathBuf> {
    const MAX_QUERY_PAIRS: usize = 256;
    let query = text.trim().trim_start_matches('?');
    let mut id_matches = false;
    let mut install_path = None;
    for pair in query.split('&').take(MAX_QUERY_PAIRS) {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "id" => id_matches = percent_decode(value) == want,
            "dipinstallpath" => install_path = Some(percent_decode(value)),
            _ => {}
        }
    }
    if !id_matches {
        return None;
    }
    let dir = PathBuf::from(install_path?);
    dir.is_dir().then_some(dir)
}

/// Decode `%XX` percent-escapes in a query-string value (EA manifests encode
/// the install path this way). Invalid escapes are kept verbatim; no `v[i]`
/// indexing (Power of Ten).
#[cfg(any(windows, test))]
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let mut peek = chars.clone();
            let hi = peek.next().and_then(|c| c.to_digit(16));
            let lo = peek.next().and_then(|c| c.to_digit(16));
            if let (Some(h), Some(l)) = (hi, lo)
                && let Some(byte) = h
                    .checked_mul(16)
                    .and_then(|v| v.checked_add(l))
                    .and_then(|v| u8::try_from(v).ok())
            {
                out.push(char::from(byte));
                chars = peek;
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(windows)]
fn read_reg_string(hive: &str, key: &str, value: &str) -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS};
    let root = match hive {
        "HKLM" => RegKey::predef(HKEY_LOCAL_MACHINE),
        "HKCU" => RegKey::predef(HKEY_CURRENT_USER),
        "HKCR" => RegKey::predef(HKEY_CLASSES_ROOT),
        "HKU" => RegKey::predef(HKEY_USERS),
        _ => return None,
    };
    let subkey = root.open_subkey(key).ok()?;
    subkey.get_value::<String, _>(value).ok()
}

#[cfg(windows)]
fn gog_install(id: &str) -> Option<PathBuf> {
    let key = format!("SOFTWARE\\WOW6432Node\\GOG.com\\Games\\{id}");
    let dir = PathBuf::from(read_reg_string("HKLM", &key, "path")?);
    dir.is_dir().then_some(dir)
}

#[cfg(windows)]
fn uplay_install(id: &str) -> Option<PathBuf> {
    let key = format!("SOFTWARE\\WOW6432Node\\Ubisoft\\Launcher\\Installs\\{id}");
    let dir = PathBuf::from(read_reg_string("HKLM", &key, "InstallDir")?);
    dir.is_dir().then_some(dir)
}

#[cfg(windows)]
fn registry_install(keys: &[RegistryKeyDef]) -> Option<PathBuf> {
    const MAX_REGISTRY_PROBES: usize = 16;
    for probe in keys.iter().take(MAX_REGISTRY_PROBES) {
        if let Some(path) = read_reg_string(&probe.hive, &probe.key, &probe.value) {
            let mut dir = PathBuf::from(path);
            if !probe.subdir.is_empty() {
                dir.push(&probe.subdir);
            }
            if dir.is_dir() {
                return Some(dir);
            }
        }
    }
    None
}

#[cfg(windows)]
fn epic_install(appname: &str) -> Option<PathBuf> {
    let data = read_reg_string(
        "HKLM",
        "SOFTWARE\\WOW6432Node\\Epic Games\\EpicGamesLauncher",
        "AppDataPath",
    )
    .map(PathBuf::from)
    .filter(|p| p.is_dir())
    .or_else(|| {
        std::env::var_os("ProgramData")
            .map(|pd| PathBuf::from(pd).join("Epic\\EpicGamesLauncher\\Data"))
    })?;
    epic_install_in(&data.join("Manifests"), appname)
}

#[cfg(windows)]
fn origin_install(id: &str) -> Option<PathBuf> {
    let pd = std::env::var_os("ProgramData")?;
    origin_install_in(&PathBuf::from(pd).join("Origin\\LocalContent"), id)
}

#[cfg(windows)]
fn xbox_install(pkg: &str) -> Option<PathBuf> {
    const MAX_PACKAGES: usize = 8192;
    use winreg::RegKey;
    use winreg::enums::HKEY_CLASSES_ROOT;
    let repo = RegKey::predef(HKEY_CLASSES_ROOT)
        .open_subkey(
            "Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\Repository\\Packages",
        )
        .ok()?;
    // A package key is `<appid>_<version>_<arch>__<publisher>`; match on the
    // appid segment before the first underscore.
    for name in repo.enum_keys().flatten().take(MAX_PACKAGES) {
        if name.split('_').next() != Some(pkg) {
            continue;
        }
        if let Ok(sub) = repo.open_subkey(&name)
            && let Ok(root) = sub.get_value::<String, _>("PackageRootFolder")
        {
            let dir = PathBuf::from(root);
            if dir.is_dir() {
                return Some(dir);
            }
        }
    }
    None
}

// Non-Windows: these stores have no registry / manifest layout to read.
#[cfg(not(windows))]
fn gog_install(_id: &str) -> Option<PathBuf> {
    None
}
#[cfg(not(windows))]
fn uplay_install(_id: &str) -> Option<PathBuf> {
    None
}
#[cfg(not(windows))]
fn registry_install(_keys: &[RegistryKeyDef]) -> Option<PathBuf> {
    None
}
#[cfg(not(windows))]
fn epic_install(_appname: &str) -> Option<PathBuf> {
    None
}
#[cfg(not(windows))]
fn origin_install(_id: &str) -> Option<PathBuf> {
    None
}
#[cfg(not(windows))]
fn xbox_install(_pkg: &str) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    /// A filesystem path as Steam writes it into a VDF: backslashes escaped
    /// (`\` -> `\\`). On Unix this is a no-op; on Windows it produces the same
    /// escaped form the real parser round-trips through.
    fn vdf_path(path: &Path) -> String {
        path.display().to_string().replace('\\', "\\\\")
    }

    #[test]
    fn vdf_values_pulls_scalar_pairs_and_skips_block_headers() {
        let text = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/games/lib\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"264710\"\t\t\"7\"\n\t\t}\n\t}\n}\n";
        assert_eq!(vdf_values(text, "path"), vec!["/games/lib".to_owned()]);
        // `apps` and `libraryfolders` are block headers: no scalar value.
        assert!(vdf_values(text, "apps").is_empty());
    }

    #[test]
    fn vdf_unescapes_windows_backslashes() {
        let text = "\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n";
        assert_eq!(
            vdf_values(text, "path"),
            vec!["D:\\SteamLibrary".to_owned()]
        );
    }

    #[test]
    fn detects_a_game_in_the_root_library() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write(
            &root.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                vdf_path(&root)
            ),
        );
        write(
            &root.join("steamapps/appmanifest_264710.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"264710\"\n\t\"installdir\"\t\t\"Subnautica\"\n}\n",
        );
        std::fs::create_dir_all(root.join("steamapps/common/Subnautica")).unwrap();

        let got = detect_in_roots(std::slice::from_ref(&root), 264_710);
        assert_eq!(got, Some(root.join("steamapps/common/Subnautica")));
    }

    #[test]
    fn detects_a_game_in_a_secondary_library() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("steam");
        let lib = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&lib).unwrap();
        write(
            &root.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                vdf_path(&root),
                vdf_path(&lib)
            ),
        );
        // The manifest lives only in the secondary library.
        write(
            &lib.join("steamapps/appmanifest_489830.acf"),
            "\"AppState\"\n{\n\t\"installdir\"\t\t\"Skyrim Special Edition\"\n}\n",
        );
        std::fs::create_dir_all(lib.join("steamapps/common/Skyrim Special Edition")).unwrap();

        let got = detect_in_roots(&[root], 489_830);
        assert_eq!(
            got,
            Some(lib.join("steamapps/common/Skyrim Special Edition"))
        );
    }

    #[test]
    fn missing_app_yields_none() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write(
            &root.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                vdf_path(&root)
            ),
        );
        assert_eq!(detect_in_roots(&[root], 999_999), None);
    }

    #[test]
    fn manifest_without_the_install_dir_on_disk_is_not_reported() {
        // A manifest can linger after a move/uninstall; only report a real dir.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write(
            &root.join("steamapps/appmanifest_264710.acf"),
            "\"AppState\"\n{\n\t\"installdir\"\t\t\"Subnautica\"\n}\n",
        );
        // No steamapps/common/Subnautica directory created.
        assert_eq!(detect_in_roots(&[root], 264_710), None);
    }

    #[test]
    fn a_manifest_installdir_cannot_escape_common() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write(
            &root.join("steamapps/appmanifest_1.acf"),
            "\"AppState\"\n{\n\t\"installdir\"\t\t\"../../../../etc\"\n}\n",
        );
        assert_eq!(detect_in_roots(&[root], 1), None);
    }

    fn def_with(body: &str) -> GameDef {
        GameDef::from_toml_str(
            &format!("api_version = 2\nid = \"g\"\nname = \"G\"\n{body}"),
            Path::new("<test>"),
        )
        .unwrap()
    }

    #[test]
    fn install_confirmed_gates_on_required_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let def = def_with("required_files = [\"bin/game.exe\"]\n");
        assert!(
            !install_confirmed(dir, &def),
            "missing required file rejects"
        );
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/game.exe"), b"x").unwrap();
        assert!(
            install_confirmed(dir, &def),
            "present required file accepts"
        );
        // No required_files → any directory is accepted.
        assert!(install_confirmed(dir, &def_with("")));
    }

    #[test]
    fn epic_manifest_resolves_by_appname() {
        let tmp = tempfile::tempdir().unwrap();
        let manifests = tmp.path().join("Manifests");
        let install = tmp.path().join("Games/MyGame");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&manifests).unwrap();
        let loc = serde_json::to_string(&install.to_string_lossy().into_owned()).unwrap();
        std::fs::write(
            manifests.join("abc.item"),
            format!("{{\"AppName\":\"Flour\",\"InstallLocation\":{loc}}}"),
        )
        .unwrap();
        assert_eq!(epic_install_in(&manifests, "Flour"), Some(install));
        assert_eq!(epic_install_in(&manifests, "NotHere"), None);
    }

    #[test]
    fn origin_mfst_matches_id_and_decodes_path() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("EA Games/Dragon Age");
        std::fs::create_dir_all(&install).unwrap();
        let enc = install.to_string_lossy().replace(' ', "%20");
        let body = format!("?state=kReady&id=OFB-EAST%3A12345&dipinstallpath={enc}&x=1");
        assert_eq!(origin_mfst_dir(&body, "OFB-EAST:12345"), Some(install));
        assert!(origin_mfst_dir(&body, "OTHER-ID").is_none());
    }

    #[test]
    fn origin_install_in_walks_localcontent() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("Games/DragonAge");
        std::fs::create_dir_all(&install).unwrap();
        let content = tmp.path().join("LocalContent/Dragon Age");
        std::fs::create_dir_all(&content).unwrap();
        let enc = install.to_string_lossy().replace(' ', "%20");
        std::fs::write(
            content.join("dragonage.mfst"),
            format!("?id=DR%3A11111&dipinstallpath={enc}"),
        )
        .unwrap();
        let root = tmp.path().join("LocalContent");
        assert_eq!(origin_install_in(&root, "DR:11111"), Some(install));
        assert_eq!(origin_install_in(&root, "DR:99999"), None);
    }

    #[test]
    fn percent_decode_handles_escapes_and_invalid_sequences() {
        assert_eq!(percent_decode("C%3A%5CGames"), "C:\\Games");
        assert_eq!(percent_decode("plain path"), "plain path");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
        assert_eq!(percent_decode("trailing%"), "trailing%");
        assert_eq!(percent_decode("x%2"), "x%2");
    }
}
