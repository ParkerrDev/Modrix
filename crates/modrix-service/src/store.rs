// SPDX-License-Identifier: GPL-2.0-only
//! Client for the community mod stores a game definition can declare.
//!
//! Today that means `sotf-mods.com`, the Sons of the Forest community store: a
//! public, unauthenticated JSON API. This module knows its shape so that
//! `modrix-core` does not - core only carries `[store] { kind, base_url }` and
//! exposes it as the `store` capability.
//!
//! Installing is deliberately *not* a second download pipeline. Resolution ends
//! in a [`HandoffJob`] per mod, handed to [`Service::enqueue_job`], so a store
//! install is the same segmented, resumable, auto-staged path a browser click
//! takes.
//!
//! ## What this API does that a naive client gets wrong
//!
//! Every one of these is verified against the live service and pinned by a test
//! over a captured fixture in `fixtures/`:
//!
//! 1. `type` defaults to `Mod`, which hides every `Library`. Dependencies are
//!    almost always libraries, so `type=Both` is mandatory or dependency
//!    resolution silently finds nothing.
//! 2. Omitting `approved` **disables** the approval filter rather than
//!    defaulting it on. `approved=true` must be sent literally.
//! 3. `dependencies` is a JSON array on the list endpoint and a
//!    comma-separated string on the detail endpoint. Both are accepted here.
//! 4. `versions` on the list endpoint is a single arbitrary *old* entry, and
//!    even on the detail endpoint the latest is not necessarily first
//!    (`BuildShare` has it at index 6). The `isLatest` flag decides; index 0
//!    never does.
//!
//! [`Service::enqueue_job`]: crate::Service::enqueue_job

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use modrix_download::http::HttpClient;
use modrix_download::{GameHint, HandoffJob};

/// The `store.kind` this client handles.
pub const SOTF_MODS_KIND: &str = "sotf-mods";

/// What Modrix identifies itself as to a store API.
const USER_AGENT: &str = concat!("Modrix/", env!("CARGO_PKG_VERSION"));

/// Hard cap on any store response body. The whole catalog is ~300 KiB, so this
/// is orders of magnitude of headroom while still bounding what a hostile or
/// broken origin can make us allocate.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Catalog page size. The store has ~200 mods, so one page is the whole thing;
/// [`StoreCatalog::truncated`] reports it if that ever stops being true.
const CATALOG_LIMIT: u32 = 500;

/// Simultaneous requests this client will make. The store asks for nothing and
/// rate-limits nothing; this is politeness, not a limit being worked around.
const MAX_IN_FLIGHT: usize = 4;

/// How deep a dependency chain may go before it is treated as malformed. Real
/// chains are one or two links; this only exists so a pathological graph cannot
/// walk forever even with the cycle guard.
const MAX_DEPTH: usize = 8;

/// How many mods one install may pull in. A one-click action that queues
/// hundreds of downloads is a bug, not a feature.
const MAX_PLAN_ITEMS: usize = 32;

// --- model -------------------------------------------------------------------

/// One mod (or library, or build) as the store describes it.
///
/// Only the fields Modrix acts on are modelled; unknown fields are ignored, so
/// the store adding columns never breaks parsing.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreItem {
    /// The store's numeric row id. Not an addressable identifier: the detail
    /// endpoint is keyed by [`mod_id`](Self::mod_id), and passing this instead
    /// answers 404.
    pub id: i64,
    /// Display name (e.g. `Axel's Mod Menu`).
    pub name: String,
    /// URL slug on the website. Free-form: a real one is `axel's-mod-menu`, so
    /// it is never interpolated into a URL unencoded.
    #[serde(default)]
    pub slug: String,
    /// The stable string identifier the detail and download endpoints use, and
    /// the name other mods list as a dependency.
    #[serde(rename = "mod_id")]
    pub mod_id: String,
    /// One-line summary shown in the list.
    #[serde(default)]
    pub short_description: Option<String>,
    /// The [`mod_id`](Self::mod_id)s this mod needs. Array or comma-separated
    /// string depending on the endpoint - see the module docs.
    #[serde(default, deserialize_with = "deserialize_dependencies")]
    pub dependencies: Vec<String>,
    /// `Mod`, `Library`, `Build`, or absent on older rows.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// Whether a moderator approved it. Never assume: the API only filters on
    /// this when explicitly asked to.
    #[serde(default)]
    pub is_approved: bool,
    /// Whether it is flagged adult.
    #[serde(rename = "isNSFW", default)]
    pub is_nsfw: bool,
    /// All-time download count (the catalog's default ordering).
    #[serde(default)]
    pub downloads: u64,
    /// The current version string. Authoritative - unlike
    /// [`versions`](Self::versions) on the list endpoint.
    #[serde(default)]
    pub latest_version: Option<String>,
    /// Released versions. One stale entry on the list endpoint; the full set,
    /// in no particular order, on the detail endpoint.
    #[serde(default)]
    pub versions: Vec<StoreVersion>,
    /// The author.
    #[serde(default)]
    pub user: Option<StoreUser>,
}

/// One released version of a mod.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreVersion {
    /// The version string (e.g. `1.3.8`).
    pub version: String,
    /// Whether this is the current release. The **only** trustworthy way to
    /// pick a version; ordering is not.
    #[serde(default)]
    pub is_latest: bool,
    /// Direct CDN URL for the archive. Present on the detail endpoint only.
    #[serde(default)]
    pub download_url: Option<String>,
    /// The archive's original filename.
    #[serde(default)]
    pub filename: Option<String>,
}

/// A mod author.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct StoreUser {
    /// Display name.
    #[serde(default)]
    pub name: String,
}

/// A parsed catalog page.
#[derive(Debug, Clone)]
pub struct StoreCatalog {
    /// Installable items (approved, not adult), most-downloaded first.
    pub items: Vec<StoreItem>,
    /// True if the store reported more pages than were fetched, so dependency
    /// resolution may not see every library.
    pub truncated: bool,
}

/// What a one-click install resolved to.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    /// Everything to install, **dependencies first** and the requested mod
    /// last, so the queue installs them in a usable order.
    pub items: Vec<StoreItem>,
    /// Dependencies skipped because they are already installed.
    pub satisfied: Vec<String>,
    /// Dependencies the catalog does not contain. Usually a library that is
    /// unapproved or withdrawn - the mod will install but may not load, so the
    /// caller is expected to say so rather than pretend success.
    pub missing: Vec<String>,
}

impl StoreItem {
    /// Whether Modrix will offer this item at all.
    #[must_use]
    pub fn is_installable(&self) -> bool {
        self.is_approved && !self.is_nsfw
    }

    /// Whether this item is a library (a dependency rather than a thing a user
    /// picks). Absent `type` is treated as a plain mod, matching the store's
    /// own default.
    #[must_use]
    pub fn is_library(&self) -> bool {
        self.kind.as_deref() == Some("Library")
    }

    /// The author's display name, when the store returned one.
    #[must_use]
    pub fn author(&self) -> Option<&str> {
        self.user.as_ref().map(|u| u.name.as_str())
    }

    /// The current release's entry.
    ///
    /// Selection is by the `isLatest` flag, then by matching the top-level
    /// `latestVersion`. It deliberately never falls back to `versions[0]`: on
    /// the list endpoint that is an arbitrary old release, and on the detail
    /// endpoint the entries are unordered.
    #[must_use]
    pub fn latest(&self) -> Option<&StoreVersion> {
        self.versions
            .iter()
            .find(|v| v.is_latest)
            .or_else(|| self.latest_version.as_deref().and_then(|w| self.find(w)))
    }

    fn find(&self, version: &str) -> Option<&StoreVersion> {
        self.versions.iter().find(|v| v.version == version)
    }

    /// Case-insensitive identity key, used for cycle and duplicate detection.
    fn key(&self) -> String {
        self.mod_id.to_ascii_lowercase()
    }
}

// --- parsing -----------------------------------------------------------------

/// The `{ status, data }` envelope every endpoint wraps its payload in.
#[derive(serde::Deserialize)]
struct Envelope<T> {
    data: T,
}

/// Paging counters returned beside a list.
#[derive(serde::Deserialize)]
struct Meta {
    #[serde(default)]
    pages: u32,
}

#[derive(serde::Deserialize)]
struct ListEnvelope {
    data: Vec<StoreItem>,
    #[serde(default)]
    meta: Option<Meta>,
}

/// Accept `dependencies` as either a JSON array or a comma-separated string.
///
/// The two endpoints genuinely disagree - the list returns `["SonsAxLib"]` and
/// the detail returns `"SimpleNetworkEvents,Banking"` - so a client that models
/// only one of them either fails to parse half the API or resolves nothing.
///
/// # Errors
///
/// Returns the deserializer's error if the field is neither shape.
fn deserialize_dependencies<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Raw {
        List(Vec<String>),
        Csv(String),
    }
    use serde::Deserialize as _;
    // `Option` absorbs an explicit JSON null, which the store also sends.
    let raw = Option::<Raw>::deserialize(deserializer)?;
    let parts: Vec<String> = match raw {
        None => Vec::new(),
        Some(Raw::List(list)) => list,
        Some(Raw::Csv(text)) => text.split(',').map(str::to_owned).collect(),
    };
    Ok(parts
        .into_iter()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect())
}

/// Parse a catalog response, dropping anything not installable.
///
/// The approval and adult filters are re-applied here even though the query
/// asks for them: they are query *parameters*, and a parameter that is dropped,
/// renamed, or ignored server-side fails open.
///
/// # Errors
///
/// Returns an error if the body is not the expected JSON envelope.
pub fn parse_catalog(body: &[u8]) -> Result<StoreCatalog> {
    let parsed: ListEnvelope = serde_json::from_slice(body).context("parsing the store catalog")?;
    let truncated = parsed.meta.is_some_and(|m| m.pages > 1);
    let items = parsed
        .data
        .into_iter()
        .filter(StoreItem::is_installable)
        .collect();
    Ok(StoreCatalog { items, truncated })
}

/// Parse a single-mod detail response.
///
/// # Errors
///
/// Returns an error if the body is not the expected JSON envelope.
pub fn parse_detail(body: &[u8]) -> Result<StoreItem> {
    let parsed: Envelope<StoreItem> =
        serde_json::from_slice(body).context("parsing the store mod detail")?;
    Ok(parsed.data)
}

// --- URLs --------------------------------------------------------------------

/// The catalog URL for a store origin.
///
/// `type=Both` and `approved=true` are not defaults being restated: without the
/// first the API hides every library, and without the second it disables the
/// approval filter entirely.
#[must_use]
pub fn catalog_url(base_url: &str) -> String {
    format!(
        "{base_url}/api/mods?type=Both&approved=true&nsfw=false&limit={CATALOG_LIMIT}\
         &orderby=most_downloaded"
    )
}

/// The detail URL for one mod id.
#[must_use]
pub fn detail_url(base_url: &str, mod_id: &str) -> String {
    format!("{base_url}/api/mods/{}", percent_encode(mod_id))
}

/// Percent-encode one URL path segment, keeping only RFC 3986 unreserved
/// characters. Store identifiers and slugs are author-supplied free text -
/// `axel's-mod-menu` is a real slug - so nothing goes into a URL raw.
fn percent_encode(segment: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            // Writing to a String cannot fail, so there is no error to handle.
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

// --- dependency resolution ---------------------------------------------------

/// Resolve everything installing `mod_id` implies, from an already-fetched
/// catalog.
///
/// The whole graph is available in the catalog (every row carries its own
/// `dependencies`), so this needs no network and is a pure function of its
/// inputs - which is what makes it testable against a captured fixture.
///
/// The walk is an explicit worklist rather than recursion, and every id is
/// entered into `visited` before it is expanded. That is what makes a cycle
/// (`A` needs `B`, `B` needs `A`) terminate instead of recursing forever, and
/// it also collapses a diamond to one install. `installed` (lowercased mod ids
/// and mod names already present) is consulted per node, so re-installing a mod
/// whose library is already there queues only the mod.
///
/// # Errors
///
/// Returns an error if `mod_id` is not in the catalog, or if the graph pulls in
/// more than [`MAX_PLAN_ITEMS`] mods.
pub fn resolve_plan<S: std::hash::BuildHasher>(
    catalog: &[StoreItem],
    mod_id: &str,
    installed: &HashSet<String, S>,
) -> Result<InstallPlan> {
    let root = find_item(catalog, mod_id)
        .ok_or_else(|| anyhow!("`{mod_id}` is not in the store catalog"))?;
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(root.key());
    let mut work: VecDeque<(String, usize)> = VecDeque::new();
    for dep in &root.dependencies {
        work.push_back((dep.clone(), 1));
    }
    let mut plan = InstallPlan {
        items: Vec::new(),
        satisfied: Vec::new(),
        missing: Vec::new(),
    };
    while let Some((dep_id, depth)) = work.pop_front() {
        if plan.items.len() >= MAX_PLAN_ITEMS {
            bail!("installing `{mod_id}` would pull in more than {MAX_PLAN_ITEMS} mods");
        }
        let key = dep_id.to_ascii_lowercase();
        if !visited.insert(key.clone()) {
            continue;
        }
        if installed.contains(&key) {
            plan.satisfied.push(dep_id);
            continue;
        }
        let Some(item) = find_item(catalog, &dep_id) else {
            plan.missing.push(dep_id);
            continue;
        };
        if let Some(next) = depth.checked_add(1).filter(|d| *d <= MAX_DEPTH) {
            for dep in &item.dependencies {
                work.push_back((dep.clone(), next));
            }
        }
        plan.items.push(item.clone());
    }
    // Dependencies queue first so they are staged before the mod that needs them.
    plan.items.push(root.clone());
    Ok(plan)
}

/// Find a catalog row by mod id, case-insensitively (authors are inconsistent
/// about casing when listing a dependency).
fn find_item<'a>(catalog: &'a [StoreItem], mod_id: &str) -> Option<&'a StoreItem> {
    let want = mod_id.to_ascii_lowercase();
    catalog.iter().find(|item| item.key() == want)
}

/// Build the lookup set [`resolve_plan`] consults, from the names and ids of mods that
/// are already installed for the game.
#[must_use]
pub fn installed_keys<'a>(names: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
    names
        .into_iter()
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

// --- hand-off ----------------------------------------------------------------

/// Turn a **detail** item into the download hand-off that installs it.
///
/// `domain` is the game's Nexus domain, which is what routes the finished
/// download to the right game; `None` leaves it unrouted and the archive stays
/// in the download directory.
///
/// The direct CDN URL is strongly preferred: the store's own
/// `/api/mods/{id}/download/{version}` route answers a `Range` request with
/// `200` and the whole body (with a `Content-Range` header that contradicts
/// it), so it can only ever be a single non-resumable connection.
///
/// # Errors
///
/// Returns an error if the item carries no identifiable current version.
pub fn handoff_job(item: &StoreItem, base_url: &str, domain: Option<&str>) -> Result<HandoffJob> {
    let version = item
        .latest()
        .ok_or_else(|| anyhow!("`{}` has no current version to download", item.mod_id))?;
    let url = match version.download_url.as_deref() {
        Some(direct) if direct.starts_with("https://") => direct.to_owned(),
        _ => format!(
            "{base_url}/api/mods/{}/download/{}",
            percent_encode(&item.mod_id),
            percent_encode(&version.version)
        ),
    };
    let filename = version
        .filename
        .clone()
        .filter(|f| !f.trim().is_empty())
        .unwrap_or_else(|| format!("{}-{}.zip", item.mod_id, version.version));
    Ok(HandoffJob {
        schema_version: 1,
        url,
        filename,
        mime: Some("application/zip".to_owned()),
        referrer: None,
        user_agent: Some(USER_AGENT.to_owned()),
        cookie: None,
        headers: BTreeMap::new(),
        total_bytes: None,
        page_url: None,
        game_hint: domain.map(|d| GameHint {
            domain: Some(d.to_owned()),
            numeric_id: None,
        }),
    })
}

// --- client ------------------------------------------------------------------

/// An HTTP client for one store origin.
#[derive(Clone)]
pub struct StoreClient {
    http: HttpClient,
    base_url: String,
    /// Caps simultaneous requests at [`MAX_IN_FLIGHT`] even when several
    /// resolutions run at once.
    gate: Arc<tokio::sync::Semaphore>,
}

impl std::fmt::Debug for StoreClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl StoreClient {
    /// Build a client for a `[store]` table's `base_url`.
    ///
    /// # Errors
    ///
    /// Returns an error if `base_url` is not an https origin, or if the TLS
    /// stack cannot be initialized.
    pub fn new(base_url: &str) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_owned();
        if !base_url.starts_with("https://") {
            bail!("store base_url `{base_url}` must be an https:// origin");
        }
        Ok(Self {
            http: HttpClient::new().context("building the store HTTP client")?,
            base_url,
            gate: Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT)),
        })
    }

    /// Fetch the installable catalog, most-downloaded first.
    ///
    /// # Errors
    ///
    /// Returns an error on any transport failure, a non-200 status, an
    /// over-cap body, or a malformed response.
    pub async fn catalog(&self) -> Result<StoreCatalog> {
        let body = self.get(&catalog_url(&self.base_url)).await?;
        parse_catalog(&body)
    }

    /// Fetch one mod's detail, which is the only place a downloadable version
    /// (and its CDN URL) appears.
    ///
    /// # Errors
    ///
    /// As [`Self::catalog`], plus a 404 for an unknown mod id.
    pub async fn detail(&self, mod_id: &str) -> Result<StoreItem> {
        let body = self.get(&detail_url(&self.base_url, mod_id)).await?;
        parse_detail(&body)
    }

    /// Resolve `mod_id` and everything it needs into ready-to-queue hand-offs,
    /// dependencies first.
    ///
    /// # Errors
    ///
    /// Returns an error if the catalog cannot be fetched, `mod_id` is not in
    /// it, or no resolved mod has a downloadable version.
    pub async fn install_jobs<S: std::hash::BuildHasher>(
        &self,
        mod_id: &str,
        installed: &HashSet<String, S>,
        domain: Option<&str>,
    ) -> Result<(Vec<HandoffJob>, InstallPlan)> {
        let catalog = self.catalog().await?;
        let plan = resolve_plan(&catalog.items, mod_id, installed)?;
        let mut jobs = Vec::with_capacity(plan.items.len());
        for item in &plan.items {
            // The catalog's `versions` is a stale stub with no URL, so the
            // download always comes from a detail fetch.
            let detail = self
                .detail(&item.mod_id)
                .await
                .with_context(|| format!("looking up `{}`", item.mod_id))?;
            jobs.push(handoff_job(&detail, &self.base_url, domain)?);
        }
        Ok((jobs, plan))
    }

    /// One capped GET, identified as Modrix.
    async fn get(&self, url: &str) -> Result<Vec<u8>> {
        let _permit = self
            .gate
            .acquire()
            .await
            .map_err(|_| anyhow!("the store client was shut down"))?;
        let headers = [("User-Agent", USER_AGENT.to_owned())];
        let response = self
            .http
            .get(url, &headers)
            .await
            .with_context(|| format!("GET {url}"))?;
        if response.status != 200 {
            bail!("GET {url} returned {}", response.status);
        }
        response
            .bytes(MAX_BODY_BYTES)
            .await
            .with_context(|| format!("reading {url}"))
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;
