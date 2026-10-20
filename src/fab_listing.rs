//! Public Fab listing dates and library presentation rules. Network work runs off the UI thread.
use crate::{
    epic,
    fab::{Item, SearchMetadata},
    model::EngineVersion,
    storage,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Read,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Sort {
    #[default]
    LatestUpdate,
    LatestAdded,
    NameAscending,
    NameDescending,
}
impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Self::LatestUpdate => "Latest update",
            Self::LatestAdded => "Latest added",
            Self::NameAscending => "Name A - Z",
            Self::NameDescending => "Name Z - A",
        }
    }
}
pub fn sort(items: &mut [Item], order: Sort) {
    items.sort_by(|a, b| {
        let name = a
            .title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.key().cmp(&b.key()));
        match order {
            Sort::LatestUpdate => b.listing_updated_at.cmp(&a.listing_updated_at).then(name),
            Sort::LatestAdded => b.acquired_at.cmp(&a.acquired_at).then(name),
            Sort::NameAscending => name,
            Sort::NameDescending => name.reverse(),
        }
    });
}
pub fn supports_engine(item: &Item, branch: &str) -> bool {
    branch.is_empty() || item.compatible(branch).is_some()
}
pub fn engine_branches<'a>(items: impl IntoIterator<Item = &'a Item>) -> Vec<String> {
    let branches: BTreeSet<_> = items
        .into_iter()
        .flat_map(|i| &i.project_versions)
        .filter(|v| {
            v.target_platforms.is_empty() || v.target_platforms.iter().any(|p| p == "Windows")
        })
        .flat_map(|v| &v.engine_versions)
        .filter_map(|raw| EngineVersion::parse(raw))
        .map(|v| (v.major, v.minor))
        .collect();
    branches
        .into_iter()
        .rev()
        .map(|(major, minor)| format!("{major}.{minor}"))
        .collect()
}
pub fn listing_id(item: &Item) -> Option<String> {
    listing_url_id(item.url.as_deref()?)
}
/// The owned Unreal catalog also includes legacy Marketplace entitlements.
/// Keep those for installed-package matching, but do not present them as Fab
/// listings. Epic's launcher-distributed Fab integration is the one exception.
pub fn visible_in_library(item: &Item) -> bool {
    listing_id(item).is_some()
        || (item.plugin()
            && item.asset_namespace == "ue"
            && item.asset_id == "c5c30696e4e0482f9c0a170873975230")
}
pub(crate) fn listing_url_id(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("www.fab.com")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let id = url
        .path()
        .trim_end_matches('/')
        .strip_prefix("/listings/")?;
    if id.len() != 36
        || !id.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
    {
        return None;
    }
    Some(id.to_ascii_lowercase())
}
fn parse_date(value: &serde_json::Value) -> Option<i64> {
    time::OffsetDateTime::parse(
        value.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()
    .map(|d| d.unix_timestamp())
}
fn latest_date(value: &serde_json::Value) -> Option<i64> {
    // publishedAt is the current listing publication; firstPublishedAt is its original release.
    std::iter::once(&value["publishedAt"])
        .chain(
            value["changelogs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| &c["publishedAt"]),
        )
        .filter_map(parse_date)
        .max()
}
pub fn date_label(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(|t| time::OffsetDateTime::from_unix_timestamp(t).ok())
        .map(|d| format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day()))
        .unwrap_or_else(|| "Unavailable".into())
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct CachedDate {
    #[serde(default)]
    search: Option<SearchMetadata>,
    checked_at: u64,
    updated_at: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    failed_at: u64,
}
const CACHE_SECONDS: u64 = 6 * 60 * 60;
const RETRY_SECONDS: u64 = 5 * 60;
fn fresh(entry: &CachedDate, now: u64) -> bool {
    entry.search.is_some() && now >= entry.checked_at && now - entry.checked_at < CACHE_SECONDS
}
fn needs_refresh(entry: Option<&CachedDate>, now: u64, retry: bool) -> bool {
    entry.is_none_or(|entry| {
        !fresh(entry, now)
            && (retry
                || entry.last_error.is_none()
                || now < entry.failed_at
                || now - entry.failed_at >= RETRY_SECONDS)
    })
}
pub enum DateEvent {
    Batch(Vec<(String, Option<i64>, Option<SearchMetadata>)>),
    Progress(usize, usize),
    Finished(Vec<ListingIssue>),
}
#[derive(Clone)]
pub struct ListingIssue {
    pub key: String,
    pub title: String,
    pub reason: String,
    pub retryable: bool,
}
fn listing_issue(
    item: &Item,
    cache: &HashMap<String, CachedDate>,
    errors: &HashMap<String, String>,
) -> Option<ListingIssue> {
    let id = listing_id(item);
    let entry = id.as_ref().and_then(|id| cache.get(id));
    if entry.is_some_and(|e| e.search.is_some() && e.updated_at.is_some()) {
        return None;
    }
    let reason = if id.is_none() {
        "The owned library supplies a legacy Marketplace link or no public Fab listing link.".into()
    } else if entry.is_some_and(|e| e.search.is_some()) {
        "Fab supplies no update date. Description and tag search are available.".into()
    } else {
        errors
            .get(id.as_ref().unwrap())
            .or_else(|| entry.and_then(|e| e.last_error.as_ref()))
            .cloned()
            .unwrap_or_else(|| {
                "Detail loading paused after repeated request failures. Retry later.".into()
            })
    };
    Some(ListingIssue {
        key: item.key(),
        title: item.title.clone(),
        reason,
        retryable: id.is_some(),
    })
}
fn fetch_date(
    client: &reqwest::blocking::Client,
    id: &str,
) -> Result<(Option<i64>, SearchMetadata), String> {
    let response = client
        .get(format!("https://www.fab.com/i/listings/{id}"))
        .send()
        .map_err(|_| "Fab listing request failed or timed out")?;
    if !response.status().is_success() {
        return Err(format!(
            "Fab detail request returned HTTP {}. Cached library information is still available.",
            response.status().as_u16()
        ));
    }
    let mut bytes = vec![];
    response
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read Fab listing")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Fab listing too large".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid Fab listing")?;
    if value["uid"]
        .as_str()
        .is_none_or(|returned| !returned.eq_ignore_ascii_case(id))
    {
        return Err("Fab listing identity mismatch".into());
    }
    Ok((latest_date(&value), search_metadata(&value)))
}
fn search_metadata(value: &serde_json::Value) -> SearchMetadata {
    SearchMetadata {
        description: value["description"].as_str().unwrap_or_default().to_owned(),
        publisher: value["user"]["sellerName"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        tags: value["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tag| tag.as_str().or_else(|| tag["name"].as_str()))
            .map(str::to_owned)
            .collect(),
    }
}
/// Cache access and HTTP requests belong on a background thread. No Epic credentials are sent.
pub fn refresh_dates(items: &[Item], retry: bool, notify: impl Fn(DateEvent)) {
    let path = storage::data_dir().join("library/listing-dates.json");
    let mut cache: HashMap<String, CachedDate> = storage::read_json(&path).unwrap_or_default();
    let now = epic::now();
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    for item in items {
        if let Some(id) = listing_id(item) {
            groups.entry(id).or_default().push(item.key());
        }
    }
    notify(DateEvent::Batch(
        items
            .iter()
            .filter_map(|i| {
                let date = cache.get(&listing_id(i)?)?;
                Some((i.key(), date.updated_at, date.search.clone()))
            })
            .collect(),
    ));
    let jobs: Vec<_> = groups
        .keys()
        .filter(|id| needs_refresh(cache.get(*id), now, retry))
        .cloned()
        .collect();
    let total = jobs.len();
    let mut updated = Vec::new();
    let mut errors = HashMap::new();
    notify(DateEvent::Progress(0, total));
    if let Ok(client) = reqwest::blocking::Client::builder()
        .user_agent(concat!("DoNothingAndWin/", env!("CARGO_PKG_VERSION")))
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert(
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("application/json"),
            );
            h
        })
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .build()
    {
        let next = AtomicUsize::new(0);
        let failures = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::channel();
            for _ in 0..3.min(total) {
                let (tx, jobs, next, failures, client) =
                    (tx.clone(), &jobs, &next, &failures, &client);
                scope.spawn(move || {
                    loop {
                        // Avoid repeatedly hitting an unavailable or rate-limited service.
                        if failures.load(Ordering::Relaxed) >= 3 {
                            break;
                        }
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(id) = jobs.get(index) else {
                            break;
                        };
                        let result = fetch_date(client, id);
                        if result.is_err() {
                            failures.fetch_add(1, Ordering::Relaxed);
                        } else {
                            failures.store(0, Ordering::Relaxed);
                        }
                        if tx.send((id.clone(), result)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx);
            for (index, (id, result)) in rx.into_iter().enumerate() {
                if let Err(error) = &result {
                    errors.insert(id.clone(), error.clone());
                    let entry = cache.entry(id.clone()).or_default();
                    entry.last_error = Some(error.clone());
                    entry.failed_at = epic::now();
                }
                if let Ok((updated_at, search)) = result {
                    cache.insert(
                        id.clone(),
                        CachedDate {
                            search: Some(search.clone()),
                            checked_at: now,
                            updated_at,
                            ..Default::default()
                        },
                    );
                    updated.extend(
                        groups[&id]
                            .iter()
                            .map(|k| (k.clone(), updated_at, Some(search.clone()))),
                    );
                    // Publish metadata promptly; one slow listing must not hold every
                    // successful date/search result until the entire catalog finishes.
                    if updated.len() >= 12 {
                        notify(DateEvent::Batch(std::mem::take(&mut updated)));
                        // Preserve completed work if the launcher exits during a long refresh.
                        let _ = storage::write_json(&path, &cache);
                    }
                }
                notify(DateEvent::Progress(index + 1, total));
            }
        });
    }
    notify(DateEvent::Batch(updated));
    // A follow-up catalog pass must not immediately retry jobs paused by the
    // service failure threshold, including jobs that never reached a worker.
    for id in &jobs {
        if !errors.contains_key(id) && needs_refresh(cache.get(id), epic::now(), false) {
            let entry = cache.entry(id.clone()).or_default();
            entry.last_error =
                Some("Detail loading paused after repeated request failures. Retry later.".into());
            entry.failed_at = epic::now();
        }
    }
    if total > 0 {
        let _ = storage::write_json(&path, &cache);
    }
    let unavailable = items
        .iter()
        .filter_map(|i| listing_issue(i, &cache, &errors))
        .collect();
    notify(DateEvent::Finished(unavailable));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn latest_added_uses_acquisition_not_publication_and_puts_unknown_dates_last() {
        let mut old = item("Old acquisition", Some(999));
        old.acquired_at = Some(10);
        let mut newer = item("New acquisition", Some(1));
        newer.acquired_at = Some(20);
        let unknown = item("A missing date", Some(1000));
        let mut items = vec![unknown, old, newer];
        sort(&mut items, Sort::LatestAdded);
        assert_eq!(
            items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["New acquisition", "Old acquisition", "A missing date"]
        );
        sort(&mut items, Sort::LatestUpdate);
        assert_eq!(items[0].title, "A missing date");
    }
    #[test]
    fn library_visibility_keeps_fab_listings_and_only_the_official_integration() {
        let mut legacy = item("Legacy plugin", None);
        legacy.distribution_method = "CODE_PLUGIN".into();
        assert!(!visible_in_library(&legacy));
        legacy.url = Some("https://www.unrealengine.com/marketplace/en/product/old-plugin".into());
        assert!(!visible_in_library(&legacy));
        let mut listing = legacy.clone();
        listing.url =
            Some("https://www.fab.com/listings/907039cf-f267-489e-afe4-7e74768e9b7a".into());
        // Missing optional metadata is not evidence that a listing is unavailable.
        assert!(listing.listing_search.is_none());
        assert!(visible_in_library(&listing));
        let mut fab = legacy.clone();
        fab.url = None;
        fab.asset_namespace = "ue".into();
        fab.asset_id = "c5c30696e4e0482f9c0a170873975230".into();
        assert!(visible_in_library(&fab));
        let mut impostor = fab.clone();
        impostor.asset_namespace = "another-publisher".into();
        assert!(!visible_in_library(&impostor));
        let catalog = [legacy, listing, fab, impostor];
        let visible: Vec<_> = catalog
            .iter()
            .filter(|item| visible_in_library(item))
            .collect();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible.iter().filter(|item| item.plugin()).count(), 2);
        assert_eq!(
            catalog.len(),
            4,
            "raw entitlements remain available for installed-plugin matching"
        );
    }
    #[test]
    fn failed_details_back_off_but_manual_retry_bypasses_cooldown() {
        let entry = CachedDate {
            last_error: Some("Fab detail request returned HTTP 403".into()),
            failed_at: 100,
            ..Default::default()
        };
        assert!(!needs_refresh(Some(&entry), 100, false));
        assert!(!needs_refresh(Some(&entry), 100 + RETRY_SECONDS - 1, false));
        assert!(needs_refresh(Some(&entry), 100 + RETRY_SECONDS, false));
        assert!(needs_refresh(Some(&entry), 101, true));
        assert!(needs_refresh(Some(&entry), 99, false));
        assert!(needs_refresh(None, 101, false));
        let restored: CachedDate =
            serde_json::from_slice(&serde_json::to_vec(&entry).unwrap()).unwrap();
        assert!(!needs_refresh(Some(&restored), 101, false));
        let mut item = item("Unavailable", None);
        item.url = Some("https://www.fab.com/listings/907039cf-f267-489e-afe4-7e74768e9b7a".into());
        let cache = HashMap::from([(listing_id(&item).unwrap(), restored)]);
        assert!(
            listing_issue(&item, &cache, &HashMap::new())
                .unwrap()
                .reason
                .contains("403")
        );
    }
    #[test]
    fn retry_preserves_usable_cached_metadata_and_skips_fresh_successes() {
        let entry = CachedDate {
            search: Some(SearchMetadata::default()),
            checked_at: 100,
            updated_at: Some(50),
            ..Default::default()
        };
        assert!(!needs_refresh(Some(&entry), 101, true));
        let failed = CachedDate {
            last_error: Some("Timeout".into()),
            failed_at: 100 + CACHE_SECONDS,
            ..entry
        };
        assert!(!needs_refresh(Some(&failed), 101 + CACHE_SECONDS, false));
        assert!(needs_refresh(Some(&failed), 101 + CACHE_SECONDS, true));
        assert!(failed.search.is_some());
        assert_eq!(failed.updated_at, Some(50));
    }
    #[test]
    fn missing_details_explain_legacy_links_request_failures_and_date_only_gaps() {
        let mut i = item("Example", None);
        let mut cache = HashMap::new();
        let mut errors = HashMap::new();
        let legacy = listing_issue(&i, &cache, &errors).unwrap();
        assert!(!legacy.retryable);
        assert!(legacy.reason.contains("legacy Marketplace"));
        let id = "907039cf-f267-489e-afe4-7e74768e9b7a";
        i.url = Some(format!("https://www.fab.com/listings/{id}"));
        errors.insert(id.into(), "Fab detail request returned HTTP 403".into());
        assert!(
            listing_issue(&i, &cache, &errors)
                .unwrap()
                .reason
                .contains("403")
        );
        cache.insert(
            id.into(),
            CachedDate {
                search: Some(SearchMetadata::default()),
                checked_at: 1,
                updated_at: None,
                ..Default::default()
            },
        );
        assert!(
            listing_issue(&i, &cache, &errors)
                .unwrap()
                .reason
                .contains("tag search are available")
        );
        cache.get_mut(id).unwrap().updated_at = Some(1);
        assert!(listing_issue(&i, &cache, &errors).is_none());
    }

    #[test]
    fn actual_grux_listing_matches_monster_tag() {
        let value =
            serde_json::from_str(include_str!("../tests/fixtures/fab-grux-search.json")).unwrap();
        let mut item = item("Paragon: Grux", None);
        let metadata = search_metadata(&value);
        assert_eq!(metadata.publisher, "Epic Games");
        assert!(metadata.tags.iter().any(|tag| tag == "Monster"));
        item.listing_search = Some(metadata);
        assert_eq!(
            crate::fab_search::SearchQuery::new("monster")
                .rank(&crate::fab_search::SearchDocument::new(&item)),
            Some(1)
        );
    }

    #[test]
    fn date_only_cache_is_refreshed_for_search_metadata() {
        let old: CachedDate =
            serde_json::from_value(json!({"checked_at":100,"updated_at":50})).unwrap();
        assert!(!fresh(&old, 101));
        assert_eq!(old.updated_at, Some(50));
    }

    fn item(title: &str, date: Option<i64>) -> Item {
        serde_json::from_value(json!({
            "assetId": title, "assetNamespace": "test", "title": title,
            "listingUpdatedAt": date
        }))
        .unwrap()
    }

    #[test]
    fn newest_dates_first_unknown_last_with_stable_name_ties() {
        let mut items = vec![
            item("Unknown", None),
            item("Beta", Some(200)),
            item("Old", Some(100)),
            item("alpha", Some(200)),
        ];
        sort(&mut items, Sort::LatestUpdate);
        assert_eq!(
            items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["alpha", "Beta", "Old", "Unknown"]
        );
        sort(&mut items, Sort::NameDescending);
        assert_eq!(
            items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["Unknown", "Old", "Beta", "alpha"]
        );
        sort(&mut items, Sort::NameAscending);
        assert_eq!(items[0].title, "alpha");
    }

    #[test]
    fn publication_and_changelog_dates_not_original_creation() {
        let listing = json!({
            "createdAt": "2030-01-01T00:00:00Z",
            "firstPublishedAt": "2014-08-20T23:24:56.338000Z",
            "publishedAt": "2025-03-12T19:37:36.138032Z",
            "changelogs": [{"publishedAt":"invalid"},
                {"publishedAt":"2026-01-03T13:56:35.415307+01:00"}]
        });
        assert_eq!(
            latest_date(&listing),
            parse_date(&json!("2026-01-03T12:56:35Z"))
        );
        assert_eq!(date_label(latest_date(&listing)), "2026-01-03");
        assert_eq!(
            latest_date(&json!({"createdAt":"2025-01-01T00:00:00Z"})),
            None
        );
        assert_eq!(date_label(None), "Unavailable");
    }

    #[test]
    fn engine_filter_matches_exact_windows_branch_and_sorts_numerically() {
        let item: Item = serde_json::from_value(json!({
            "assetId":"test", "assetNamespace":"test", "title":"Test",
            "projectVersions":[
                {"artifactId":"a", "engineVersions":["5.9", "5.10", "5.10.2"], "targetPlatforms":["Windows"]},
                {"artifactId":"b", "engineVersions":["5.8"], "targetPlatforms":["Mac"]},
                {"artifactId":"c", "engineVersions":["4.26", "invalid"]}
            ]
        })).unwrap();
        assert!(supports_engine(&item, ""));
        assert!(supports_engine(&item, "5.10.1"));
        assert!(supports_engine(&item, "4.26"));
        assert!(!supports_engine(&item, "5.8"));
        assert!(!supports_engine(&item, "5.11"));
        assert_eq!(engine_branches(&[item]), ["5.10", "5.9", "4.26"]);
    }

    #[test]
    fn listing_urls_only_resolve_valid_fab_ids() {
        let mut item = item("Test", None);
        let id = "98ff449d-79db-4f54-9303-75486c4fb9d9";
        item.url = Some(format!("https://www.fab.com/listings/{id}/"));
        assert_eq!(listing_id(&item).as_deref(), Some(id));
        for url in [
            format!("https://example.com/listings/{id}"),
            format!("http://www.fab.com/listings/{id}"),
            format!("https://user@www.fab.com/listings/{id}"),
            "https://www.fab.com/listings/not-an-id".into(),
        ] {
            item.url = Some(url);
            assert_eq!(listing_id(&item), None);
        }
    }

    #[test]
    fn cache_expiry_and_older_library_cache_are_supported() {
        let entry = CachedDate {
            search: Some(SearchMetadata::default()),
            checked_at: 100,
            updated_at: Some(50),
            ..Default::default()
        };
        assert!(fresh(&entry, 100));
        assert!(fresh(&entry, 100 + CACHE_SECONDS - 1));
        assert!(!fresh(&entry, 100 + CACHE_SECONDS));
        assert!(!fresh(&entry, 99));
        let old: Item = serde_json::from_value(json!({
            "assetId":"test", "assetNamespace":"test", "title":"Test"
        }))
        .unwrap();
        assert!(old.url.is_none());
        assert!(old.listing_updated_at.is_none());
    }
}
