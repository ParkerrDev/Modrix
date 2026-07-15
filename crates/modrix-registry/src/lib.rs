// SPDX-License-Identifier: GPL-2.0-only
//! The community plugin registry client.
//!
//! Plugins (game-support definitions, optional `game.lua`, skill files) live
//! in a curated Git repository. Its `index.json` is one fetch away and
//! answers search; installing a plugin fetches each listed file, verifies
//! its SHA-256 and size against the index, and lands it **atomically** under
//! `<data>/plugins/<id>/` - exactly where core's definition catalog
//! ([`modrix_core::defcat`]) already looks, so an installed plugin is
//! immediately a registerable game.
//!
//! "Only keep locally what is needed": plugins are fetched on demand,
//! uninstallable, and [`RegistryClient::gc`] removes any whose game is no
//! longer registered.
//!
//! Sources: an HTTP base URL (the public registry) or a local directory (a
//! clone - used for development and while the repository is private). Both
//! go through the same verification; the network is never trusted.

use std::path::{Path, PathBuf};

use modrix_core::Paths;
use sha2::Digest as _;

/// Largest `index.json` accepted.
const MAX_INDEX_BYTES: usize = 1024 * 1024;
/// Most files one plugin may ship.
const MAX_PLUGIN_FILES: usize = 64;
/// Largest single plugin file.
const MAX_FILE_BYTES: usize = 1024 * 1024;
/// How long a cached index stays fresh.
const INDEX_TTL: std::time::Duration = std::time::Duration::from_mins(15);

/// The default public registry (raw file access to the main branch).
pub const DEFAULT_REGISTRY: &str =
    "https://raw.githubusercontent.com/ParkerrDev/modrix-plugins/main";

/// The environment variable overriding the registry source (a URL or a
/// local directory path) - how a private/dev registry is used.
pub const REGISTRY_ENV: &str = "MODRIX_REGISTRY";

/// Registry-client errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A network or transport failure.
    #[error("registry transport: {0}")]
    Transport(String),
    /// The index or a manifest could not be parsed.
    #[error("registry data: {0}")]
    Data(String),
    /// A fetched file failed its integrity check.
    #[error("integrity: {0}")]
    Integrity(String),
    /// A filesystem operation failed.
    #[error("i/o at `{path}`: {source}")]
    Io {
        /// The path the failing operation targeted.
        path: PathBuf,
        /// The underlying OS error.
        source: std::io::Error,
    },
    /// The plugin is still referenced by a registered game.
    #[error("plugin `{0}` is in use by a registered game")]
    InUse(String),
    /// No such plugin in the index.
    #[error("plugin `{0}` not found in the registry")]
    NotFound(String),
}

impl Error {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Registry-client result.
pub type Result<T> = std::result::Result<T, Error>;

/// Where the registry lives.
#[derive(Debug, Clone)]
pub enum RegistrySource {
    /// An HTTP(S) base URL serving the repository's files.
    Http {
        /// Base URL without a trailing slash.
        base: String,
    },
    /// A local checkout of the registry repository.
    Local {
        /// The repository root.
        root: PathBuf,
    },
}

impl RegistrySource {
    /// Resolve the source: the [`REGISTRY_ENV`] override (URL or directory
    /// path), else the public default.
    #[must_use]
    pub fn resolve() -> Self {
        match std::env::var(REGISTRY_ENV) {
            Ok(value) if value.starts_with("http://") || value.starts_with("https://") => {
                Self::Http {
                    base: value.trim_end_matches('/').to_owned(),
                }
            }
            Ok(value) if !value.trim().is_empty() => Self::Local {
                root: PathBuf::from(value),
            },
            _ => Self::Http {
                base: DEFAULT_REGISTRY.to_owned(),
            },
        }
    }
}

/// The one-fetch search index at the registry root.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Index {
    /// Index schema version.
    pub index_version: u32,
    /// When the index was generated (informational).
    #[serde(default)]
    pub generated_at: String,
    /// Every plugin in the registry.
    pub plugins: Vec<IndexEntry>,
}

/// One plugin in the index.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexEntry {
    /// Stable plugin id (the game id for game-support plugins).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Plugin version (semver; bumped on any file change).
    pub version: String,
    /// The `game.toml` `api_version` it targets.
    pub api_version: u32,
    /// Steam AppID, when the plugin supports a Steam game.
    #[serde(default)]
    pub steam_appid: Option<i64>,
    /// Nexus domain, when applicable.
    #[serde(default)]
    pub nexus_domain: Option<String>,
    /// Whether the plugin ships Tier-2 logic (`game.lua`).
    #[serde(default)]
    pub has_lua: bool,
    /// Whether the plugin ships agent skill files.
    #[serde(default)]
    pub has_skill: bool,
    /// Repository-relative directory (e.g. `plugins/skyrimse`).
    pub path: String,
    /// Every file the plugin ships, with integrity data.
    pub files: Vec<IndexFile>,
}

/// One file of a plugin.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexFile {
    /// Path relative to the plugin directory.
    pub path: String,
    /// Lowercase hex SHA-256 of the contents.
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
}

/// A locally installed plugin (parsed from its `plugin.toml`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginManifest {
    /// Stable plugin id.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Installed version.
    pub version: String,
    /// The `game.toml` `api_version` it targets.
    pub api_version: u32,
    /// Plugin authors.
    #[serde(default)]
    pub authors: Vec<String>,
}

/// The registry client.
pub struct RegistryClient {
    source: RegistrySource,
    paths: Paths,
    http: Option<modrix_download::http::HttpClient>,
}

impl RegistryClient {
    /// Build a client over `source`, installing into the data dir of `paths`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] if the HTTP stack cannot initialize.
    pub fn new(source: RegistrySource, paths: &Paths) -> Result<Self> {
        let http = match &source {
            RegistrySource::Http { .. } => Some(
                modrix_download::http::HttpClient::new()
                    .map_err(|e| Error::Transport(e.to_string()))?,
            ),
            RegistrySource::Local { .. } => None,
        };
        Ok(Self {
            source,
            paths: paths.clone(),
            http,
        })
    }

    /// Fetch (or reuse a fresh cached copy of) the registry index.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`]/[`Error::Data`] on fetch or parse failure.
    pub async fn index(&self, refresh: bool) -> Result<Index> {
        let cache = self.paths.cache_dir().join("registry-index.json");
        if !refresh && let Some(index) = read_fresh_cache(&cache) {
            return Ok(index);
        }
        let bytes = self.fetch("index.json", MAX_INDEX_BYTES).await?;
        let index: Index =
            serde_json::from_slice(&bytes).map_err(|e| Error::Data(e.to_string()))?;
        if index.index_version != 1 {
            return Err(Error::Data(format!(
                "unsupported index_version {}",
                index.index_version
            )));
        }
        if let Some(parent) = cache.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&cache, &bytes);
        Ok(index)
    }

    /// Case-insensitive substring search over id, name, and Nexus domain.
    #[must_use]
    pub fn search<'a>(index: &'a Index, query: &str) -> Vec<&'a IndexEntry> {
        let needle = query.to_ascii_lowercase();
        index
            .plugins
            .iter()
            .filter(|p| {
                needle.is_empty()
                    || p.id.to_ascii_lowercase().contains(&needle)
                    || p.name.to_ascii_lowercase().contains(&needle)
                    || p.nexus_domain
                        .as_deref()
                        .is_some_and(|d| d.to_ascii_lowercase().contains(&needle))
            })
            .collect()
    }

    /// Install (or update) a plugin: fetch every listed file, verify size and
    /// SHA-256, and atomically replace `<data>/plugins/<id>/`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Integrity`] on any hash/size mismatch (nothing is
    /// installed), or transport/data/io errors.
    pub async fn install(&self, entry: &IndexEntry) -> Result<PluginManifest> {
        if entry.files.len() > MAX_PLUGIN_FILES {
            return Err(Error::Data(format!(
                "plugin lists {} files (cap {MAX_PLUGIN_FILES})",
                entry.files.len()
            )));
        }
        let plugins_root = self.paths.data_dir().join("plugins");
        let tmp = plugins_root.join(format!(".tmp-{}", entry.id));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| Error::io(&tmp, e))?;

        for file in &entry.files {
            validate_rel(&file.path)?;
            let remote = format!("{}/{}", entry.path, file.path);
            let bytes = self.fetch(&remote, MAX_FILE_BYTES).await?;
            verify(file, &bytes)?;
            let dest = tmp.join(&file.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            std::fs::write(&dest, &bytes).map_err(|e| Error::io(&dest, e))?;
        }
        let manifest = read_manifest(&tmp.join("plugin.toml"))?;
        if manifest.id != entry.id {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(Error::Data(format!(
                "manifest id `{}` does not match index id `{}`",
                manifest.id, entry.id
            )));
        }
        // Atomic swap: the old version vanishes only after the new one is
        // complete and verified.
        let dest = plugins_root.join(&entry.id);
        let old = plugins_root.join(format!(".old-{}", entry.id));
        let _ = std::fs::remove_dir_all(&old);
        if dest.exists() {
            std::fs::rename(&dest, &old).map_err(|e| Error::io(&dest, e))?;
        }
        std::fs::rename(&tmp, &dest).map_err(|e| Error::io(&tmp, e))?;
        let _ = std::fs::remove_dir_all(&old);
        tracing::info!(plugin = %entry.id, version = %entry.version, "plugin installed");
        Ok(manifest)
    }

    /// Remove an installed plugin. `referenced` carries the plugin ids of
    /// currently registered games; removing one of those is refused.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InUse`] when refused, [`Error::NotFound`] when not
    /// installed, or [`Error::Io`].
    pub fn uninstall(&self, id: &str, referenced: &[String]) -> Result<()> {
        if referenced.iter().any(|r| r == id) {
            return Err(Error::InUse(id.to_owned()));
        }
        let dir = self.paths.data_dir().join("plugins").join(id);
        if !dir.is_dir() {
            return Err(Error::NotFound(id.to_owned()));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        Ok(())
    }

    /// Every installed plugin (directories under `<data>/plugins/` with a
    /// readable `plugin.toml`).
    #[must_use]
    pub fn installed(&self) -> Vec<PluginManifest> {
        installed_at(&self.paths)
    }

    /// Remove every installed plugin no registered game references ("only
    /// keep locally what is needed"). Returns the removed ids.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on a removal failure.
    pub fn gc(&self, referenced: &[String]) -> Result<Vec<String>> {
        let mut removed = Vec::new();
        for manifest in self.installed() {
            if !referenced.contains(&manifest.id) {
                self.uninstall(&manifest.id, referenced)?;
                removed.push(manifest.id);
            }
        }
        Ok(removed)
    }

    /// Fetch one repository-relative file, capped at `cap` bytes.
    async fn fetch(&self, rel: &str, cap: usize) -> Result<Vec<u8>> {
        match &self.source {
            RegistrySource::Http { base } => {
                let url = format!("{base}/{rel}");
                let client = self
                    .http
                    .as_ref()
                    .ok_or_else(|| Error::Transport("no http client".to_owned()))?;
                let response = client
                    .get(&url, &[])
                    .await
                    .map_err(|e| Error::Transport(e.to_string()))?;
                if response.status != 200 {
                    return Err(Error::Transport(format!(
                        "GET {url} returned {}",
                        response.status
                    )));
                }
                response
                    .bytes(cap)
                    .await
                    .map_err(|e| Error::Transport(e.to_string()))
            }
            RegistrySource::Local { root } => {
                validate_rel(rel)?;
                let path = root.join(rel);
                let meta = std::fs::metadata(&path).map_err(|e| Error::io(&path, e))?;
                if meta.len() > cap as u64 {
                    return Err(Error::Data(format!("{rel} is over the size cap")));
                }
                std::fs::read(&path).map_err(|e| Error::io(&path, e))
            }
        }
    }
}

/// Every installed plugin under `<data>/plugins/` - usable without a client
/// (no network stack needed just to list a directory).
#[must_use]
pub fn installed_at(paths: &Paths) -> Vec<PluginManifest> {
    let root = paths.data_dir().join("plugins");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten().take(MAX_PLUGIN_FILES.saturating_mul(4)) {
        let dir = entry.path();
        if !dir.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if let Ok(manifest) = read_manifest(&dir.join("plugin.toml")) {
            out.push(manifest);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Read the cached index if it is younger than [`INDEX_TTL`].
fn read_fresh_cache(cache: &Path) -> Option<Index> {
    let meta = std::fs::metadata(cache).ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age > INDEX_TTL {
        return None;
    }
    let bytes = std::fs::read(cache).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn read_manifest(path: &Path) -> Result<PluginManifest> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    toml::from_str(&text).map_err(|e| Error::Data(format!("plugin.toml: {e}")))
}

/// Verify a fetched file against its index record.
fn verify(file: &IndexFile, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 != file.size {
        return Err(Error::Integrity(format!(
            "{}: size {} != declared {}",
            file.path,
            bytes.len(),
            file.size
        )));
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let got = hex(&hasher.finalize());
    if got != file.sha256.to_ascii_lowercase() {
        return Err(Error::Integrity(format!(
            "{}: sha256 mismatch (got {got})",
            file.path
        )));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Reject `..`, absolute paths, and empty components in repo-relative paths.
fn validate_rel(rel: &str) -> Result<()> {
    let p = Path::new(rel);
    let bad = rel.is_empty()
        || p.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        });
    if bad {
        return Err(Error::Data(format!("path escapes the registry: {rel}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a local registry containing one plugin and return
    /// `(registry_root, index)`.
    fn fixture(tmp: &Path) -> Index {
        let plugin_dir = tmp.join("plugins/testgame");
        std::fs::create_dir_all(plugin_dir.join("skills")).unwrap();
        let manifest =
            "id = \"testgame\"\nname = \"Test Game\"\nversion = \"1.0.0\"\napi_version = 2\n";
        let def = "api_version = 2\nid = \"testgame\"\nname = \"Test Game\"\nmod_root = \"Data\"\n";
        let skill = "# Modding Test Game\n";
        std::fs::write(plugin_dir.join("plugin.toml"), manifest).unwrap();
        std::fs::write(plugin_dir.join("game.toml"), def).unwrap();
        std::fs::write(plugin_dir.join("skills/testgame.skill.md"), skill).unwrap();
        let files = [
            ("plugin.toml", manifest),
            ("game.toml", def),
            ("skills/testgame.skill.md", skill),
        ]
        .iter()
        .map(|(path, body)| {
            let mut hasher = sha2::Sha256::new();
            hasher.update(body.as_bytes());
            IndexFile {
                path: (*path).to_owned(),
                sha256: hex(&hasher.finalize()),
                size: body.len() as u64,
            }
        })
        .collect();
        let index = Index {
            index_version: 1,
            generated_at: String::new(),
            plugins: vec![IndexEntry {
                id: "testgame".to_owned(),
                name: "Test Game".to_owned(),
                version: "1.0.0".to_owned(),
                api_version: 2,
                steam_appid: None,
                nexus_domain: None,
                has_lua: false,
                has_skill: true,
                path: "plugins/testgame".to_owned(),
                files,
            }],
        };
        std::fs::write(tmp.join("index.json"), serde_json::to_vec(&index).unwrap()).unwrap();
        index
    }

    fn client(registry_root: &Path, data_root: &Path) -> RegistryClient {
        RegistryClient::new(
            RegistrySource::Local {
                root: registry_root.to_path_buf(),
            },
            &Paths::rooted_at(data_root),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn install_verifies_and_lands_where_the_def_catalog_looks() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = tmp.path().join("registry");
        std::fs::create_dir_all(&registry).unwrap();
        fixture(&registry);
        let data = tmp.path().join("data");
        let c = client(&registry, &data);

        let index = c.index(true).await.unwrap();
        let entry = &index.plugins[0];
        let manifest = c.install(entry).await.unwrap();
        assert_eq!(manifest.version, "1.0.0");

        // The def catalog now sees the game.
        let paths = Paths::rooted_at(&data);
        let found = modrix_core::defcat::find_def(&paths, "testgame").expect("catalog hit");
        assert_eq!(found.def.name, "Test Game");
        // Skills land with it.
        assert!(
            paths
                .data_dir()
                .join("plugins/testgame/skills/testgame.skill.md")
                .is_file()
        );
    }

    #[tokio::test]
    async fn a_corrupted_file_is_rejected_with_no_partial_install() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = tmp.path().join("registry");
        std::fs::create_dir_all(&registry).unwrap();
        let index = fixture(&registry);
        // Corrupt the def on "the server" after the index was generated.
        std::fs::write(
            registry.join("plugins/testgame/game.toml"),
            "tampered bytes",
        )
        .unwrap();
        let data = tmp.path().join("data");
        let c = client(&registry, &data);

        let err = c.install(&index.plugins[0]).await.unwrap_err();
        assert!(matches!(err, Error::Integrity(_)), "got {err:?}");
        assert!(
            !Paths::rooted_at(&data)
                .data_dir()
                .join("plugins/testgame")
                .exists(),
            "nothing may be installed on integrity failure"
        );
    }

    #[tokio::test]
    async fn search_matches_id_name_and_domain() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = tmp.path().join("registry");
        std::fs::create_dir_all(&registry).unwrap();
        let index = fixture(&registry);
        assert_eq!(RegistryClient::search(&index, "test").len(), 1);
        assert_eq!(RegistryClient::search(&index, "TEST GAME").len(), 1);
        assert_eq!(RegistryClient::search(&index, "nope").len(), 0);
        assert_eq!(RegistryClient::search(&index, "").len(), 1);
    }

    #[tokio::test]
    async fn uninstall_and_gc_respect_references() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = tmp.path().join("registry");
        std::fs::create_dir_all(&registry).unwrap();
        let index = fixture(&registry);
        let data = tmp.path().join("data");
        let c = client(&registry, &data);
        c.install(&index.plugins[0]).await.unwrap();
        assert_eq!(c.installed().len(), 1);

        // Referenced: refused by uninstall AND left alone by gc.
        let refs = vec!["testgame".to_owned()];
        assert!(matches!(
            c.uninstall("testgame", &refs),
            Err(Error::InUse(_))
        ));
        assert!(c.gc(&refs).unwrap().is_empty());
        assert_eq!(c.installed().len(), 1);

        // Unreferenced: gc sweeps it.
        let removed = c.gc(&[]).unwrap();
        assert_eq!(removed, vec!["testgame"]);
        assert!(c.installed().is_empty());
    }

    #[tokio::test]
    async fn escaping_paths_in_the_index_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = tmp.path().join("registry");
        std::fs::create_dir_all(&registry).unwrap();
        let mut index = fixture(&registry);
        index.plugins[0].files[0].path = "../evil.toml".to_owned();
        let data = tmp.path().join("data");
        let c = client(&registry, &data);
        assert!(matches!(
            c.install(&index.plugins[0]).await.unwrap_err(),
            Error::Data(_)
        ));
    }
}
