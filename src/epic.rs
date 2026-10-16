use crate::model::{EngineVersion, Release, Snapshot};
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

// Public launcher OAuth client, independently documented by Legendary/egs-api.
// These identify the client; users supply their OWN one-time authorization code.
const CLIENT_ID: &str = "34a02cf8f4414e29b15921876da36f9a";
const CLIENT_SECRET: &str = "daafbccc737745039dffe53d94fc76cf";
const AUTH: &str = "https://account-public-service-prod03.ol.epicgames.com/account/api/oauth/token";
const CATALOG: &str =
    "https://catalog-public-service-prod06.ol.epicgames.com/catalog/api/shared/namespace/ue/items";
const BUILDS: &str =
    "https://launcher-public-service-prod06.ol.epicgames.com/launcher/api/public/assets/Windows";
pub const SIGN_IN_URL: &str = "https://www.epicgames.com/id/login?redirectUrl=https%3A%2F%2Fwww.epicgames.com%2Fid%2Fapi%2Fredirect%3FclientId%3D34a02cf8f4414e29b15921876da36f9a%26responseType%3Dcode";
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub refresh_token: String,
    pub display_name: String,
    pub account_id: String,
}
// Intentionally no Debug: authentication data must never enter logs.
pub struct Epic {
    client: Client,
}
pub(crate) fn session_lock() -> Result<std::fs::File, String> {
    std::fs::create_dir_all(crate::storage::data_dir()).map_err(|e| e.to_string())?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(crate::storage::data_dir().join("epic-session.lock"))
        .map_err(|e| e.to_string())?;
    fs2::FileExt::lock_exclusive(&f).map_err(|e| e.to_string())?;
    Ok(f)
}
// Access tokens stay in memory and are tied to the current saved session.
// Fab HTTP requests do not hold the cross-process refresh-token lock.
struct FabAccessToken {
    account: String,
    refresh_hash: [u8; 20],
    token: String,
    until: std::time::Instant,
}
impl FabAccessToken {
    fn matches(&self, session: &Session, now: std::time::Instant) -> bool {
        self.account == session.account_id
            && self.refresh_hash == crate::buildpatch::digest(session.refresh_token.as_bytes())
            && now < self.until
    }
}
static FAB_ACCESS: std::sync::Mutex<Option<FabAccessToken>> = std::sync::Mutex::new(None);
impl Epic {
    pub fn new() -> Result<Self, String> {
        static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
        if let Some(client) = CLIENT.get() {
            return Ok(Self {
                client: client.clone(),
            });
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(35))
            .connect_timeout(Duration::from_secs(12))
            .user_agent(concat!("DoNothingAndWin/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client: CLIENT.get_or_init(|| client).clone(),
        })
    }
    fn decode(r: Response) -> Result<Value, String> {
        let status = r.status();
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 => "Epic sign-in expired. Please reconnect your account.".into(),
                403 => {
                    "Epic denied access. Sign in with an account entitled to Unreal Engine.".into()
                }
                429 => format!(
                    "Epic rate limited this check. Retry after {} seconds.",
                    r.headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("900")
                ),
                _ => format!(
                    "Epic returned HTTP {}. The previous successful snapshot is retained.",
                    status.as_u16()
                ),
            });
        }
        r.json()
            .map_err(|_| "Epic returned an unexpected response. Verification is incomplete.".into())
    }
    fn authenticate(&self, grant: &str, value: Option<&str>) -> Result<Value, String> {
        let mut form = vec![("grant_type", grant), ("token_type", "eg1")];
        if let Some(v) = value {
            form.push((
                if grant == "refresh_token" {
                    "refresh_token"
                } else {
                    "code"
                },
                v,
            ));
        }
        Self::decode(
            self.client
                .post(AUTH)
                .basic_auth(CLIENT_ID, Some(CLIENT_SECRET))
                .form(&form)
                .send()
                .map_err(|_| {
                    "Could not reach Epic authentication. Check your connection.".to_string()
                })?,
        )
    }
    fn session(v: &Value) -> Result<Session, String> {
        Ok(Session {
            refresh_token: v["refresh_token"]
                .as_str()
                .ok_or("Epic did not provide a renewable session.")?
                .into(),
            display_name: v["displayName"].as_str().unwrap_or("Developer").into(),
            account_id: v["account_id"]
                .as_str()
                .ok_or("Epic did not provide an account identity.")?
                .into(),
        })
    }
    pub fn sign_in(&self, code: &str) -> Result<Session, String> {
        let code = code.trim();
        let parsed: Option<Value> = serde_json::from_str(code).ok();
        let code = parsed
            .as_ref()
            .and_then(|v| v["authorizationCode"].as_str())
            .unwrap_or(code);
        if code.is_empty() {
            return Err("Epic returned an empty sign-in response.".into());
        }
        Self::session(&self.authenticate("authorization_code", Some(code))?)
    }
    fn fab_access_token(&self, session: &Session) -> Result<String, String> {
        let mut cached = FAB_ACCESS
            .lock()
            .map_err(|_| "Authentication cache unavailable")?;
        let current = crate::storage::load_session()
            .filter(|s| s.account_id == session.account_id)
            .ok_or("Epic account changed")?;
        if let Some(token) = cached
            .as_ref()
            .filter(|t| t.matches(&current, std::time::Instant::now()))
        {
            return Ok(token.token.clone());
        }
        let _lock = session_lock()?;
        let current = crate::storage::load_session()
            .filter(|s| s.account_id == session.account_id)
            .ok_or("Epic account changed")?;
        let auth = self.authenticate("refresh_token", Some(&current.refresh_token))?;
        let renewed = Self::session(&auth)?;
        crate::storage::save_session(&renewed)?;
        let token = auth["access_token"]
            .as_str()
            .ok_or("Missing access token")?
            .to_owned();
        let lifetime = auth["expires_in"]
            .as_u64()
            .unwrap_or(0)
            .min(3600)
            .saturating_sub(60);
        *cached = Some(FabAccessToken {
            account: renewed.account_id,
            refresh_hash: crate::buildpatch::digest(renewed.refresh_token.as_bytes()),
            token: token.clone(),
            until: std::time::Instant::now() + Duration::from_secs(lifetime),
        });
        Ok(token)
    }
    pub fn fab_request(
        &self,
        session: &Session,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, String> {
        let url = format!("https://www.fab.com/e/{path}");
        for attempt in 0..2 {
            let token = self.fab_access_token(session)?;
            let request = if let Some(body) = body {
                self.client.post(&url).json(body)
            } else {
                self.client.get(&url)
            };
            let response = request
                .bearer_auth(&token)
                .send()
                .map_err(|_| "Fab request failed")?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                let mut cached = FAB_ACCESS
                    .lock()
                    .map_err(|_| "Authentication cache unavailable")?;
                if cached.as_ref().is_some_and(|t| t.token == token) {
                    *cached = None;
                }
                continue;
            }
            return Self::decode(response);
        }
        Err("Epic sign-in expired. Please reconnect your account.".into())
    }
    pub fn owned_assets(&self, session: &Session) -> Result<Vec<Value>, String> {
        let _lock = session_lock()?;
        let current = crate::storage::load_session()
            .filter(|s| s.account_id == session.account_id)
            .ok_or("Epic account changed; reconnect")?;
        let auth = self.authenticate("refresh_token", Some(&current.refresh_token))?;
        crate::storage::save_session(&Self::session(&auth)?)?;
        let token = auth["access_token"].as_str().ok_or("No access token")?;
        let value = Self::decode(
            self.client
                .get(BUILDS)
                .bearer_auth(token)
                .query(&[("label", "Live")])
                .send()
                .map_err(|_| "Could not check owned plugin builds")?,
        )?;
        Ok(value
            .as_array()
            .ok_or("Invalid owned build response")?
            .iter()
            .filter(|v| v["namespace"] == "ue")
            .cloned()
            .collect())
    }
    pub fn manifest_locations(
        &self,
        session: &Session,
        release: &Release,
    ) -> Result<Vec<reqwest::Url>, String> {
        if ![&release.app, &release.catalog_id, &release.channel]
            .iter()
            .all(|s| {
                s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
            })
        {
            return Err("Invalid engine identity".into());
        }
        let _lock = session_lock()?;
        let current = crate::storage::load_session()
            .filter(|s| s.account_id == session.account_id)
            .ok_or("Your Epic account changed or was signed out. Please retry from the current account.")?;
        let auth = self.authenticate("refresh_token", Some(&current.refresh_token))?;
        crate::storage::save_session(&Self::session(&auth)?)?;
        let token = auth["access_token"].as_str().ok_or("No access token")?;
        let url = format!(
            "https://launcher-public-service-prod06.ol.epicgames.com/launcher/api/public/assets/v2/platform/Windows/namespace/ue/catalogItem/{}/app/{}/label/{}",
            release.catalog_id, release.app, release.channel
        );
        let value = Self::decode(
            self.client
                .get(url)
                .bearer_auth(token)
                .send()
                .map_err(|_| "Could not fetch engine manifest locations")?,
        )?;
        let elements = value["elements"]
            .as_array()
            .ok_or("Missing manifest elements")?;
        let mut urls = Vec::new();
        for element in elements {
            if element["appName"]
                .as_str()
                .is_some_and(|a| a != release.app)
            {
                continue;
            }
            if let Some(manifests) = element["manifests"].as_array() {
                for entry in manifests {
                    let Some(raw) = entry["uri"].as_str() else {
                        continue;
                    };
                    let mut url =
                        reqwest::Url::parse(raw).map_err(|_| "Invalid manifest location")?;
                    if url.scheme() == "http" {
                        url.set_scheme("https")
                            .map_err(|_| "Invalid manifest scheme")?;
                    }
                    if let Some(params) = entry["queryParams"].as_array() {
                        for p in params {
                            if let (Some(k), Some(v)) = (p["name"].as_str(), p["value"].as_str()) {
                                url.query_pairs_mut().append_pair(k, v);
                            }
                        }
                    }
                    urls.push(url);
                }
            }
        }
        if urls.is_empty() {
            return Err(
                "Epic did not return a downloadable manifest for this engine/account".into(),
            );
        }
        Ok(urls)
    }
    pub fn check(&self, session: Option<&Session>) -> Result<(Snapshot, Option<Session>), String> {
        let _lock = session_lock()?;
        let current = session
            .map(|original| {
                crate::storage::load_session()
                    .filter(|s| s.account_id == original.account_id)
                    .ok_or("Your Epic account changed or was signed out. Please check again.")
            })
            .transpose()?;
        let session = current.as_ref();
        let auth = self.authenticate(
            if session.is_some() {
                "refresh_token"
            } else {
                "client_credentials"
            },
            session.map(|s| s.refresh_token.as_str()),
        )?;
        let token = auth["access_token"]
            .as_str()
            .ok_or("Epic did not provide an access token.")?;
        let renewed = if session.is_some() {
            Some(Self::session(&auth)?)
        } else {
            None
        };
        // Persist rotated refresh token BEFORE any subsequent request can fail.
        if let Some(s) = &renewed {
            crate::storage::save_session(s)?;
        }
        let mut catalog = Vec::new();
        let mut start = 0usize;
        let mut expected = None;
        loop {
            let v = Self::decode(
                self.client
                    .get(CATALOG)
                    .bearer_auth(token)
                    .query(&[
                        ("category", "engines"),
                        ("start", &start.to_string()),
                        ("count", "100"),
                    ])
                    .send()
                    .map_err(|_| {
                        "Engine catalog request failed. Cached data is unchanged.".to_string()
                    })?,
            )?;
            let (items, total) = parse_catalog_page(&v, start)?;
            if expected.is_some_and(|n| n != total) {
                return Err("Engine catalog changed during pagination. Retry the check.".into());
            }
            expected = Some(total);
            start += items.len();
            catalog.extend(items);
            if start >= total {
                break;
            }
            if start > 10000 {
                return Err("Unexpectedly large engine catalog. Verification stopped.".into());
            }
        }
        let assets = if session.is_some() {
            Some(Self::decode(
                self.client
                    .get(BUILDS)
                    .bearer_auth(token)
                    .query(&[("label", "Live")])
                    .send()
                    .map_err(|_| {
                        "Engine build feed failed. Previous results are retained.".to_string()
                    })?,
            )?)
        } else {
            None
        };
        let mut snapshot = combine(&catalog, assets.as_ref())?;
        if session.is_some() && !snapshot.missing_builds.is_empty() {
            return Err(format!(
                "Epic's build feed is incomplete: {} catalog branches have no build metadata. Previous results are retained.",
                snapshot.missing_builds.len()
            ));
        }
        snapshot.checked_at = now();
        Ok((snapshot, renewed))
    }
}

pub fn parse_catalog_page(v: &Value, start: usize) -> Result<(Vec<Value>, usize), String> {
    let items = v["elements"]
        .as_array()
        .ok_or("Missing engine catalog elements")?;
    let total = v["paging"]["total"]
        .as_u64()
        .ok_or("Missing catalog pagination total")? as usize;
    let actual = v["paging"]["start"]
        .as_u64()
        .ok_or("Missing catalog page offset")? as usize;
    if actual != start || (items.is_empty() && start < total) || start + items.len() > total {
        return Err("Incomplete or inconsistent engine catalog pagination".into());
    }
    for item in items {
        if !item["categories"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["path"].as_str() == Some("engines")))
        {
            return Err("Epic ignored the engine-only catalog filter".into());
        }
    }
    Ok((items.clone(), total))
}
pub fn combine(catalog: &[Value], assets: Option<&Value>) -> Result<Snapshot, String> {
    let mut releases = BTreeMap::new();
    for item in catalog {
        let id = item["id"]
            .as_str()
            .ok_or("Engine catalog item has no identity")?;
        let info = item["releaseInfo"]
            .as_array()
            .ok_or("Engine catalog item has no release information")?;
        for r in info {
            if !r["platform"]
                .as_array()
                .is_some_and(|a| a.iter().any(|p| p.as_str() == Some("Windows")))
            {
                continue;
            }
            let app = r["appId"]
                .as_str()
                .ok_or("Engine release has no app identity")?;
            if !app.starts_with("UE_") {
                continue;
            }
            releases.insert(
                app.to_owned(),
                Release {
                    app: app.into(),
                    catalog_id: id.into(),
                    channel: "Live".into(),
                    platform: "Windows".into(),
                    version: None,
                    build: None,
                },
            );
        }
    }
    if releases.is_empty() {
        return Err("No engine releases were returned; verification is incomplete.".into());
    }
    let catalog_count = releases.len();
    if let Some(assets) = assets {
        let items = assets.as_array().ok_or("Unexpected build-feed schema")?;
        let mut engine_count = 0;
        for a in items {
            if a["namespace"].as_str() != Some("ue") {
                continue;
            }
            let Some(app) = a["appName"].as_str().filter(|s| s.starts_with("UE_")) else {
                continue;
            };
            let build = a["buildVersion"]
                .as_str()
                .filter(|b| !b.is_empty())
                .ok_or("Engine build has no build version")?;
            let id = a["catalogItemId"]
                .as_str()
                .ok_or("Engine build has no catalog identity")?;
            releases.insert(
                app.into(),
                Release {
                    app: app.into(),
                    catalog_id: id.into(),
                    channel: "Live".into(),
                    platform: "Windows".into(),
                    version: EngineVersion::parse(build),
                    build: Some(build.into()),
                },
            );
            engine_count += 1;
        }
        if engine_count == 0 {
            return Err(
                "Authenticated feed contained no engines. Verification is incomplete.".into(),
            );
        }
    }
    let missing_builds = releases
        .values()
        .filter(|r| r.build.is_none())
        .map(|r| r.app.clone())
        .collect();
    let mut releases: Vec<_> = releases.into_values().collect();
    releases.sort_by(|a, b| {
        EngineVersion::parse(&b.app)
            .cmp(&EngineVersion::parse(&a.app))
            .then(a.app.cmp(&b.app))
    });
    Ok(Snapshot {
        releases,
        catalog_count,
        missing_builds,
        authenticated: assets.is_some(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Value {
        serde_json::from_str(
            include_str!("../tests/fixtures/catalog.json").trim_start_matches('\u{feff}'),
        )
        .unwrap()
    }
    #[test]
    fn live_catalog_and_build_fixture_cover_all_branches() {
        let c = catalog();
        let (items, total) = parse_catalog_page(&c, 0).unwrap();
        assert_eq!(items.len(), total);
        let a: Value = serde_json::from_str(
            include_str!("../tests/fixtures/builds.json").trim_start_matches('\u{feff}'),
        )
        .unwrap();
        let s = combine(&items, Some(&a)).unwrap();
        assert!(s.verified());
        assert_eq!(s.releases.len(), 39);
        let newest = s.releases.iter().find(|r| r.app == "UE_5.8").unwrap();
        assert_eq!(newest.version.as_ref().unwrap().to_string(), "5.8.3");
    }
    #[test]
    fn empty_page_is_failure() {
        let v = serde_json::json!({"elements":[],"paging":{"start":0,"total":13}});
        assert!(parse_catalog_page(&v, 0).is_err());
    }
    #[test]
    fn filter_ignored_is_failure() {
        let v = serde_json::json!({"elements":[{"categories":[{"path":"assets"}]}],"paging":{"start":0,"total":1}});
        assert!(parse_catalog_page(&v, 0).is_err());
    }
    #[test]
    fn failed_auth_not_empty_success() {
        assert!(
            combine(
                catalog()["elements"].as_array().unwrap(),
                Some(&serde_json::json!({"errorCode":"expired"}))
            )
            .is_err()
        );
    }
    #[test]
    fn catalog_without_account_is_partial() {
        let s = combine(catalog()["elements"].as_array().unwrap(), None).unwrap();
        assert!(!s.verified());
        assert!(!s.missing_builds.is_empty());
    }
}

#[cfg(test)]
mod fab_token_tests {
    use super::*;
    #[test]
    fn access_token_is_invalid_after_expiry_account_switch_or_session_rotation() {
        let session = Session {
            account_id: "a".into(),
            refresh_token: "test-session".into(),
            display_name: String::new(),
        };
        let now = std::time::Instant::now();
        let token = FabAccessToken {
            account: session.account_id.clone(),
            refresh_hash: crate::buildpatch::digest(session.refresh_token.as_bytes()),
            token: "test-access".into(),
            until: now + Duration::from_secs(60),
        };
        assert!(token.matches(&session, now));
        assert!(!token.matches(&session, now + Duration::from_secs(60)));
        assert!(!token.matches(
            &Session {
                account_id: "b".into(),
                ..session.clone()
            },
            now
        ));
        assert!(!token.matches(
            &Session {
                refresh_token: "rotated".into(),
                ..session
            },
            now
        ));
    }
}
