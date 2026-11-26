// SPDX-License-Identifier: GPL-2.0-only
//! Tests for the store client, over JSON captured from the live API.
//!
//! Nothing here touches the network: the fixtures in `fixtures/` are verbatim
//! (only long prose fields truncated) responses from `api.sotf-mods.com`, so
//! the four ways this API misleads a client stay pinned even offline.

use super::*;

/// `GET /api/mods?type=Both&approved=true&nsfw=false&limit=500&orderby=most_downloaded`
const CATALOG: &str = include_str!("../fixtures/sotf-catalog.json");
/// `GET /api/mods/AxelModMenu` - dependencies as a comma-separated string.
const DETAIL_AXEL: &str = include_str!("../fixtures/sotf-detail-axel.json");
/// `GET /api/mods/BuildShare` - `isLatest` at index 6, not 0.
const DETAIL_BUILDSHARE: &str = include_str!("../fixtures/sotf-detail-buildshare.json");

const BASE: &str = "https://api.sotf-mods.com";

fn catalog() -> StoreCatalog {
    parse_catalog(CATALOG.as_bytes()).unwrap()
}

fn item<'a>(catalog: &'a StoreCatalog, mod_id: &str) -> &'a StoreItem {
    catalog
        .items
        .iter()
        .find(|i| i.mod_id == mod_id)
        .unwrap_or_else(|| panic!("fixture has no `{mod_id}`"))
}

// --- gotcha 1: `type` defaults to Mod and hides every Library ----------------

#[test]
fn gotcha_1_libraries_need_type_both_and_are_kept() {
    // The default `type` is `Mod`, so a request without this parameter returns
    // no libraries at all - and dependencies are almost always libraries.
    assert!(
        catalog_url(BASE).contains("type=Both"),
        "without type=Both the catalog contains no libraries, and every \
         dependency silently fails to resolve"
    );

    let catalog = catalog();
    let lib = item(&catalog, "SonsAxLib");
    assert!(lib.is_library(), "SonsAxLib is type=Library in the capture");
    assert!(
        catalog.items.iter().any(StoreItem::is_library),
        "filtering libraries out of the catalog would break resolution"
    );

    // The proof it matters: the most-downloaded mod's only dependency IS that
    // library, so a Mod-only catalog resolves it as missing.
    let plan = resolve_plan(&catalog.items, "AxelModMenu", &HashSet::new()).unwrap();
    assert!(plan.missing.is_empty(), "{:?}", plan.missing);
    assert_eq!(
        plan.items.first().map(|i| i.mod_id.as_str()),
        Some("SonsAxLib")
    );

    let mods_only: Vec<StoreItem> = catalog
        .items
        .iter()
        .filter(|i| !i.is_library())
        .cloned()
        .collect();
    let degraded = resolve_plan(&mods_only, "AxelModMenu", &HashSet::new()).unwrap();
    assert_eq!(degraded.missing, vec!["SonsAxLib".to_owned()]);
}

// --- gotcha 2: omitting `approved` DISABLES the approval filter --------------

#[test]
fn gotcha_2_unapproved_mods_are_filtered_out() {
    assert!(
        catalog_url(BASE).contains("approved=true"),
        "omitting `approved` disables the filter rather than defaulting it on"
    );
    assert!(catalog_url(BASE).contains("nsfw=false"));

    // The fixture carries a real unapproved library (`Regi_s_Modding_Library`,
    // captured from a request WITHOUT approved=true), so the client-side guard
    // is exercised even though the query already asks for approved mods.
    let raw: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
    let rows = raw["data"].as_array().unwrap();
    assert!(
        rows.iter().any(|r| r["isApproved"] == false),
        "fixture must contain an unapproved row or this test proves nothing"
    );

    let catalog = catalog();
    assert!(
        catalog.items.iter().all(|i| i.is_approved && !i.is_nsfw),
        "an unapproved or adult mod must never reach the install path"
    );
    assert!(
        !catalog
            .items
            .iter()
            .any(|i| i.mod_id == "Regi_s_Modding_Library"),
        "the unapproved library survived the filter"
    );
}

// --- gotcha 3: `dependencies` is an array here and a CSV string there --------

#[test]
fn gotcha_3_dependencies_parse_as_array_or_csv() {
    // List endpoint: a JSON array.
    let catalog = catalog();
    assert_eq!(item(&catalog, "AxelModMenu").dependencies, ["SonsAxLib"]);
    assert_eq!(
        item(&catalog, "Shops").dependencies,
        ["SimpleNetworkEvents", "Banking"]
    );

    // Detail endpoint: the same field, comma-separated.
    let raw: serde_json::Value = serde_json::from_str(DETAIL_AXEL).unwrap();
    assert!(
        raw["data"]["dependencies"].is_string(),
        "the detail endpoint really does return a string"
    );
    let detail = parse_detail(DETAIL_AXEL.as_bytes()).unwrap();
    assert_eq!(detail.dependencies, ["SonsAxLib"]);

    // An empty CSV string is no dependencies, not one blank dependency - which
    // would resolve as a missing mod named "".
    let empty = parse_detail(DETAIL_BUILDSHARE.as_bytes()).unwrap();
    assert!(
        empty.dependencies.is_empty(),
        "expected empty, got {:?}",
        empty.dependencies
    );

    // Both shapes, plus null, whitespace, and an absent field, through the
    // deserializer directly.
    let cases = [
        (r#""dependencies":["A","B"],"#, vec!["A", "B"]),
        (r#""dependencies":"A, B ,C","#, vec!["A", "B", "C"]),
        (r#""dependencies":"","#, vec![]),
        (r#""dependencies":null,"#, vec![]),
        ("", vec![]),
    ];
    for (field, want) in cases {
        let full = format!(r#"{{{field}"id":1,"name":"n","mod_id":"m"}}"#);
        let parsed: StoreItem = serde_json::from_str(&full).unwrap();
        assert_eq!(parsed.dependencies, want, "for `{field}`");
    }
}

// --- gotcha 4: `versions` is never the way to find the latest release --------

#[test]
fn gotcha_4_latest_comes_from_is_latest_never_index_zero() {
    // On the LIST endpoint `versions` is `take: 1` ordered ASC: one arbitrary
    // old release, flagged isLatest = false.
    let catalog = catalog();
    let listed = item(&catalog, "AxelModMenu");
    assert_eq!(
        listed.versions.first().map(|v| v.version.as_str()),
        Some("1.0.0"),
        "the list endpoint returns a stale single version"
    );
    assert_eq!(listed.latest_version.as_deref(), Some("1.3.8"));
    assert!(
        listed.versions.iter().all(|v| !v.is_latest),
        "nothing on the list endpoint is flagged latest, so `latestVersion` is \
         the only usable source"
    );

    // On the DETAIL endpoint the entries are unordered: BuildShare really does
    // carry its current release at index 6.
    let detail = parse_detail(DETAIL_BUILDSHARE.as_bytes()).unwrap();
    assert_eq!(
        detail.versions.first().map(|v| v.version.as_str()),
        Some("1.0.8"),
        "versions[0] is not the latest release"
    );
    assert_eq!(detail.latest_version.as_deref(), Some("1.0.10"));
    let latest = detail.latest().unwrap();
    assert_eq!(latest.version, "1.0.10");
    assert!(latest.is_latest);

    // And the hand-off must carry that version's archive, not index 0's.
    let job = handoff_job(&detail, BASE, Some("sonsoftheforest")).unwrap();
    assert!(job.url.contains("1.0.10"), "{}", job.url);
    assert!(!job.url.contains("1.0.8"), "{}", job.url);
}

// --- URL construction --------------------------------------------------------

#[test]
fn catalog_url_sends_every_parameter_the_api_needs() {
    assert_eq!(
        catalog_url(BASE),
        "https://api.sotf-mods.com/api/mods?type=Both&approved=true&nsfw=false\
         &limit=500&orderby=most_downloaded"
    );
}

#[test]
fn identifiers_are_percent_encoded_into_the_path() {
    // A real slug on this store is `axel's-mod-menu`; ids are author-supplied
    // free text and never go into a URL raw.
    assert_eq!(
        detail_url(BASE, "axel's-mod-menu"),
        "https://api.sotf-mods.com/api/mods/axel%27s-mod-menu"
    );
    assert_eq!(
        detail_url(BASE, "a b/../c"),
        "https://api.sotf-mods.com/api/mods/a%20b%2F..%2Fc",
        "a traversal attempt must not escape the path segment"
    );
    assert_eq!(
        detail_url(BASE, "AxelModMenu"),
        format!("{BASE}/api/mods/AxelModMenu")
    );
}

// --- dependency resolution ---------------------------------------------------

/// A minimal item, for graphs the live store does not (yet) contain.
fn synthetic(mod_id: &str, deps: &[&str]) -> StoreItem {
    let deps = deps
        .iter()
        .map(|d| format!("\"{d}\""))
        .collect::<Vec<_>>()
        .join(",");
    let json = format!(
        r#"{{"id":1,"name":"{mod_id}","mod_id":"{mod_id}","isApproved":true,
            "dependencies":[{deps}],"latestVersion":"1.0.0",
            "versions":[{{"version":"1.0.0","isLatest":true,
            "downloadUrl":"https://cdn.example/{mod_id}.zip"}}]}}"#
    );
    serde_json::from_str(&json).unwrap()
}

#[test]
fn a_dependency_cycle_terminates_instead_of_recursing_forever() {
    // A needs B, B needs C, C needs A. RedManager recurses on this until it
    // dies; the visited set makes it one pass.
    let catalog = [
        synthetic("A", &["B"]),
        synthetic("B", &["C"]),
        synthetic("C", &["A"]),
    ];
    let plan = resolve_plan(&catalog, "A", &HashSet::new()).unwrap();
    let ids: Vec<&str> = plan.items.iter().map(|i| i.mod_id.as_str()).collect();
    assert_eq!(ids, ["B", "C", "A"], "deps first, root last, each once");
    assert!(
        plan.missing.is_empty(),
        "expected empty, got {:?}",
        plan.missing
    );

    // Self-dependency is the degenerate cycle.
    let selfish = [synthetic("S", &["S"])];
    let plan = resolve_plan(&selfish, "S", &HashSet::new()).unwrap();
    assert_eq!(plan.items.len(), 1);
}

#[test]
fn a_diamond_installs_each_mod_once() {
    let catalog = [
        synthetic("Top", &["Left", "Right"]),
        synthetic("Left", &["Base"]),
        synthetic("Right", &["Base"]),
        synthetic("Base", &[]),
    ];
    let plan = resolve_plan(&catalog, "Top", &HashSet::new()).unwrap();
    let ids: Vec<&str> = plan.items.iter().map(|i| i.mod_id.as_str()).collect();
    assert_eq!(ids, ["Left", "Right", "Base", "Top"]);
}

#[test]
fn an_already_installed_dependency_is_not_queued_again() {
    // RedManager has no such check and re-downloads what is already there.
    let catalog = catalog();
    let installed = installed_keys(["sonsaxlib"]);
    let plan = resolve_plan(&catalog.items, "AxelModMenu", &installed).unwrap();
    assert_eq!(plan.satisfied, ["SonsAxLib".to_owned()]);
    let ids: Vec<&str> = plan.items.iter().map(|i| i.mod_id.as_str()).collect();
    assert_eq!(ids, ["AxelModMenu"]);
}

#[test]
fn installed_keys_are_matched_case_insensitively() {
    let keys = installed_keys(["  SonsAxLib  ", "", "Axel's Mod Menu"]);
    assert!(keys.contains("sonsaxlib"));
    assert!(keys.contains("axel's mod menu"));
    assert_eq!(keys.len(), 2, "blank names must not become a key");
}

#[test]
fn an_unknown_mod_id_is_an_error_not_an_empty_install() {
    let catalog = catalog();
    assert!(resolve_plan(&catalog.items, "NoSuchMod", &HashSet::new()).is_err());
}

#[test]
fn a_dependency_missing_from_the_catalog_is_reported_not_swallowed() {
    let catalog = [synthetic("A", &["Withdrawn"])];
    let plan = resolve_plan(&catalog, "A", &HashSet::new()).unwrap();
    assert_eq!(plan.missing, ["Withdrawn".to_owned()]);
    assert_eq!(
        plan.items.len(),
        1,
        "the mod still installs, with a warning"
    );
}

// --- hand-off ----------------------------------------------------------------

#[test]
fn the_handoff_reuses_the_browser_download_pipeline() {
    let detail = parse_detail(DETAIL_AXEL.as_bytes()).unwrap();
    let job = handoff_job(&detail, BASE, Some("sonsoftheforest")).unwrap();
    assert_eq!(job.schema_version, 1, "must match the hand-off schema");
    // The direct CDN URL, not the API download route: that route answers a
    // Range request with 200 and the whole body, so it cannot be segmented.
    assert!(
        job.url.starts_with("https://r2.sotf-mods.com/"),
        "{}",
        job.url
    );
    assert!(job.url.contains("1.3.8"));
    assert_eq!(
        job.game_hint.and_then(|h| h.domain).as_deref(),
        Some("sonsoftheforest"),
        "the domain hint is what routes the finished download to the game"
    );
    assert!(job.user_agent.unwrap().starts_with("Modrix/"));

    // The job survives the same trust-boundary validation a browser job does,
    // apostrophe in the filename and all.
    let dir = std::path::Path::new("/tmp/modrix-store-test");
    let job = handoff_job(&detail, BASE, None).unwrap();
    let request = job.into_request(dir).unwrap();
    assert_eq!(
        std::path::Path::new(&request.out).extension(),
        Some(std::ffi::OsStr::new("zip")),
        "{}",
        request.out
    );
    assert!(!request.out.contains('/'));
}

#[test]
fn a_version_with_no_cdn_url_falls_back_to_the_api_download_route() {
    let json = r#"{"status":"success","data":{"id":1,"name":"n","mod_id":"My Mod",
        "isApproved":true,"latestVersion":"2.0","dependencies":null,
        "versions":[{"version":"2.0","isLatest":true}]}}"#;
    let item = parse_detail(json.as_bytes()).unwrap();
    let job = handoff_job(&item, BASE, None).unwrap();
    assert_eq!(job.url, format!("{BASE}/api/mods/My%20Mod/download/2.0"));
    assert_eq!(job.filename, "My Mod-2.0.zip");
}

#[test]
fn an_item_with_no_identifiable_release_is_an_error() {
    let json = r#"{"status":"success","data":{"id":1,"name":"n","mod_id":"m",
        "isApproved":true,"versions":[{"version":"0.1","isLatest":false}]}}"#;
    let item = parse_detail(json.as_bytes()).unwrap();
    assert!(
        handoff_job(&item, BASE, None).is_err(),
        "guessing versions[0] here is exactly the bug being avoided"
    );
}

// --- catalog shape -----------------------------------------------------------

#[test]
fn the_captured_catalog_still_parses_end_to_end() {
    let catalog = catalog();
    assert!(!catalog.truncated, "one page holds the whole store");
    assert!(catalog.items.len() >= 5);
    let axel = item(&catalog, "AxelModMenu");
    assert_eq!(axel.name, "Axel's Mod Menu");
    assert_eq!(axel.author(), Some("ImAxel"));
    assert!(axel.downloads > 0);
    // `type` is null on older rows; those are plain mods, not libraries.
    let untyped = catalog.items.iter().find(|i| i.kind.is_none());
    assert!(untyped.is_some_and(|i| !i.is_library()));
}

// --- live smoke check --------------------------------------------------------

/// Confirms the captured fixtures still describe the live service. Ignored by
/// default: `cargo test --workspace` must never depend on the network.
///
/// Run with `cargo test -p modrix-service -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "hits the live sotf-mods.com API"]
async fn live_api_still_matches_the_fixtures() {
    let client = StoreClient::new(BASE).unwrap();
    let catalog = client.catalog().await.unwrap();
    assert!(catalog.items.len() > 100, "{}", catalog.items.len());
    assert!(
        catalog.items.iter().any(StoreItem::is_library),
        "type=Both must still surface libraries"
    );
    assert!(catalog.items.iter().all(StoreItem::is_installable));

    let plan = resolve_plan(&catalog.items, "AxelModMenu", &HashSet::new()).unwrap();
    assert_eq!(plan.missing, Vec::<String>::new());
    let detail = client.detail("BuildShare").await.unwrap();
    assert_eq!(
        detail.latest().map(|v| v.version.as_str()),
        detail.latest_version.as_deref()
    );
    let job = handoff_job(&detail, BASE, Some("sonsoftheforest")).unwrap();
    assert!(job.url.starts_with("https://"), "{}", job.url);
}
