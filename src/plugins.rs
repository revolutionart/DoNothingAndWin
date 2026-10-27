pub mod catalogue;
use std::path::{Path, PathBuf};
pub mod backups;
mod check_cache;
pub(crate) mod package_files;
mod parallel_checks;
pub(crate) mod receipt;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
};

// Read only descriptor names, not chunk/file payloads, from our engine receipt.
// Cache by receipt metadata: inventories are refreshed frequently for card counts.
fn bundled_descriptors(engine: &Path) -> Arc<HashSet<String>> {
    type Stamp = (std::time::SystemTime, u64);
    type Cache = HashMap<PathBuf, (Stamp, Arc<HashSet<String>>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let path = engine.join(".engine-workspace-manifest.json.gz");
    let Some(stamp) = fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok().map(|time| (time, m.len())))
    else {
        return Arc::default();
    };
    let mut cache = CACHE.get_or_init(Mutex::default).lock().unwrap();
    if let Some((old, names)) = cache.get(&path) {
        if *old == stamp {
            return names.clone();
        }
    }
    #[derive(serde::Deserialize)]
    struct FileName {
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct Files {
        files: Vec<FileName>,
    }
    #[derive(serde::Deserialize)]
    struct Receipt {
        manifest: Files,
    }
    let names: HashSet<String> = fs::File::open(&path)
        .ok()
        .and_then(|file| {
            serde_json::from_reader::<_, Receipt>(flate2::read::GzDecoder::new(file)).ok()
        })
        .map(|receipt| {
            receipt
                .manifest
                .files
                .into_iter()
                .filter_map(|f| {
                    let name = f.name.replace('\\', "/").to_ascii_lowercase();
                    name.strip_prefix("engine/plugins/")
                        .filter(|n| n.ends_with(".uplugin"))
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    let names = Arc::new(names);
    cache.insert(path, (stamp, names.clone()));
    names
}

#[derive(Clone)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub descriptor: PathBuf,
    pub catalog_id: Option<String>,
    pub number: i64,
}

/// Inventory only: a local descriptor cannot establish whether a Fab update exists.
pub fn installed(engine: &Path) -> Vec<Plugin> {
    let root = engine.join("Engine/Plugins");
    let mut result = Vec::new();
    visit(&root, &root, 0, &bundled_descriptors(engine), &mut result);
    result.sort_by_key(|p| p.name.to_lowercase());
    result
}
fn visit(root: &Path, dir: &Path, depth: usize, bundled: &HashSet<String>, out: &mut Vec<Plugin>) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<_> = entries.flatten().collect();
    // Plugin discovery stops at a plugin root. Its Content, Source, Binaries and
    // Intermediate trees cannot contain another independently installed plugin.
    let plugin_root = depth > 0
        && entries.iter().any(|entry| {
            entry.path().extension().is_some_and(|x| x == "uplugin")
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        });
    for entry in entries {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        #[cfg(windows)]
        if kind.is_dir() {
            use std::os::windows::fs::MetadataExt;
            if entry
                .metadata()
                .map_or(true, |m| m.file_attributes() & 0x400 != 0)
            {
                continue;
            }
        }
        if kind.is_dir() {
            if !plugin_root {
                visit(root, &path, depth + 1, bundled, out);
            }
        } else if path.extension().is_some_and(|x| x == "uplugin") {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                continue;
            };
            let marketplace = path
                .strip_prefix(root)
                .ok()
                .is_some_and(|p| p.starts_with("Marketplace"));
            // Epic installs Fab and Bridge directly below Engine/Plugins without Installed=true.
            let epic_optional = path.strip_prefix(root).ok().is_some_and(|p| {
                p == Path::new("Fab/Fab.uplugin") || p == Path::new("Bridge/Bridge.uplugin")
            });
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase();
            // Installed=true describes packaging, not whether the user installed an add-on.
            // Epic optional integrations remain independently managed, even if in a receipt.
            let epic_builtin = !marketplace
                && v["CreatedBy"].as_str().is_some_and(|name| {
                    matches!(
                        name.trim().to_ascii_lowercase().as_str(),
                        "epic games" | "epic games, inc." | "epic games, inc" | "epic games inc."
                    )
                });
            if !epic_optional && (bundled.contains(&relative) || epic_builtin) {
                continue;
            }
            if !marketplace && !epic_optional && v["Installed"].as_bool() != Some(true) {
                continue;
            }
            out.push(Plugin {
                name: v["FriendlyName"]
                    .as_str()
                    .unwrap_or_else(|| path.file_stem().unwrap().to_str().unwrap_or("Plugin"))
                    .into(),
                version: v["VersionName"].as_str().unwrap_or("Unknown").into(),
                catalog_id: marketplace_id(&v),
                number: v["Version"].as_i64().unwrap_or(0),
                descriptor: path,
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_searches_categories_but_stops_at_plugin_roots() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("Engine/Plugins/Marketplace/Category/Real");
        let payload = plugin.join("Intermediate/PackagedCopy");
        fs::create_dir_all(&payload).unwrap();
        fs::write(
            plugin.join("Real.uplugin"),
            br#"{"FriendlyName":"Real","VersionName":"1.0"}"#,
        )
        .unwrap();
        fs::write(
            payload.join("Copy.uplugin"),
            br#"{"FriendlyName":"Build output"}"#,
        )
        .unwrap();
        let rows = installed(dir.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Real");
    }
    #[test]
    #[ignore = "Read-only live update probe; requires DNW_TEST_PLUGIN_DESCRIPTOR, DNW_TEST_PLUGIN_ENGINE and a connected Epic account"]
    fn live_descriptor_update_probe() {
        let descriptor = std::path::PathBuf::from(
            std::env::var("DNW_TEST_PLUGIN_DESCRIPTOR").expect("Supply a test descriptor"),
        );
        let version = std::env::var("DNW_TEST_PLUGIN_ENGINE").expect("Supply an engine version");
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Engine/Plugins/Marketplace/TestPlugin");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::copy(&descriptor, folder.join(descriptor.file_name().unwrap())).unwrap();
        let engine = crate::model::Installation {
            version,
            path: temp.path().to_string_lossy().into_owned(),
            association: "isolated-read-only-probe".into(),
        };
        let session = crate::storage::load_session().expect("Connect Epic first");
        let rows = check(&engine, &session).unwrap();
        assert_eq!(rows.len(), 1);
        for row in rows {
            println!(
                "{} | installed {} | available {} | {}",
                row.plugin.name,
                row.plugin.version,
                row.available_version.as_deref().unwrap_or("unknown"),
                row.status
            );
        }
    }
    #[test]
    fn discovers_epic_fab_and_bridge_without_including_builtin_plugins() {
        let d = tempfile::tempdir().unwrap();
        for name in ["Fab", "Bridge", "Builtin"] {
            let folder = d.path().join("Engine/Plugins").join(name);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(
                folder.join(format!("{name}.uplugin")),
                format!(r#"{{"FriendlyName":"{name}","VersionName":"1.0"}}"#),
            )
            .unwrap();
        }
        let names: Vec<_> = installed(d.path()).into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Bridge", "Fab"]);
    }
    #[test]
    fn bundled_plugins_are_hidden_and_cannot_be_removed_but_optional_integrations_remain() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        let descriptors = [
            "Animation/GeneSplicer/GeneSplicer.uplugin",
            "Marketplace/Bundled/Bundled.uplugin",
            "Fab/Fab.uplugin",
            "Bridge/Bridge.uplugin",
            "Marketplace/Extra/Extra.uplugin",
        ];
        for relative in descriptors {
            let path = d.path().join("Engine/Plugins").join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, r#"{"Installed":true,"Version":1}"#).unwrap();
        }
        let receipt = serde_json::json!({"manifest":{"files":descriptors[..4].iter().map(|p|serde_json::json!({"name":format!("Engine/Plugins/{p}")})).collect::<Vec<_>>()}});
        let mut zip = flate2::write::GzEncoder::new(
            fs::File::create(d.path().join(".engine-workspace-manifest.json.gz")).unwrap(),
            flate2::Compression::default(),
        );
        zip.write_all(&serde_json::to_vec(&receipt).unwrap())
            .unwrap();
        zip.finish().unwrap();
        let names: Vec<_> = installed(d.path()).into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Bridge", "Extra", "Fab"]);
        let engine = Installation {
            path: d.path().to_string_lossy().into_owned(),
            version: "5.6.1".into(),
            association: String::new(),
        };
        let plugin = Plugin {
            name: "GeneSplicer".into(),
            version: "1".into(),
            number: 1,
            catalog_id: None,
            descriptor: d.path().join("Engine/Plugins").join(descriptors[0]),
        };
        assert!(remove_plugin(&engine, &plugin).is_err());
        assert!(plugin.descriptor.exists());
    }
    #[test]
    fn epic_bundled_plugin_is_hidden_without_a_receipt() {
        let d = tempfile::tempdir().unwrap();
        let folder = d.path().join("Engine/Plugins/Animation/GeneSplicer");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("GeneSplicer.uplugin"),
            r#"{"Installed":true,"CreatedBy":"Epic Games, Inc."}"#,
        )
        .unwrap();
        assert!(installed(d.path()).is_empty());
    }
    #[test]
    fn inventories_selected_engine_and_excludes_builtins() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("Engine/Plugins/Marketplace/Terrain");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(
            p.join("Terrain.uplugin"),
            r#"{"FriendlyName":"Terrain Tools","VersionName":"1.4.0"}"#,
        )
        .unwrap();
        std::fs::write(d.path().join("Engine/Plugins/Builtin.uplugin"), "{}").unwrap();
        let found = installed(d.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Terrain Tools");
        assert!(installed(&d.path().join("OtherEngine")).is_empty());
    }
}
use crate::{
    buildpatch::{self, Manifest},
    epic::Session,
    installer::{self, Control},
    model::{EngineVersion, Installation},
};
#[cfg(test)]
use crate::{epic::Epic, model::Release};
use std::{collections::HashMap, fs};
#[derive(Clone)]
pub struct Update {
    pub version: String,
    pub number: i64,
    pub manifest: Manifest,
    pub bases: Vec<reqwest::Url>,
    pub prefix: PathBuf,
}
#[derive(Clone)]
pub struct Checked {
    pub plugin: Plugin,
    pub update: Option<Update>,
    pub available_version: Option<String>,
    pub status: String,
}
impl Checked {
    pub fn has_update(&self) -> bool {
        self.update.is_some()
            || self
                .status
                .starts_with("Epic integration update available;")
    }
    pub fn status_detail(&self) -> &str {
        if self.status == "No owned Fab listing matches this plugin" {
            return "The Fab listing ID in this installed plugin does not match a plugin listing returned by your connected account's library. Sync the library and confirm this plugin appears there. If it does, its .uplugin listing link needs investigation. Update availability is unknown.";
        }
        match self.status_label() {
            "Up to date" => {
                "The installed version matches the latest compatible version. No update is available."
            }
            "Not available through Fab" => {
                "This plugin is not available from your connected Fab library. Check the publisher for updates."
            }
            _ => &self.status,
        }
    }
    pub fn status_label(&self) -> &'static str {
        if self.update.is_some() {
            return "Update available";
        }
        match self.status.as_str() {
            s if s.starts_with("Waiting for Fab library:") => "Waiting for Fab library",
            s if s.starts_with("Package identity could not be verified") => "Identity unverified",
            s if s.starts_with("Could not match this plugin to the signed-in account") => {
                "Not available through Fab"
            }
            s if s.starts_with("Matching Fab listing has no Windows build") => {
                "No compatible build"
            }
            "Up to date" => "Up to date",
            "Publisher version matches; original package build is not recorded" => "Up to date",
            "Installed version is newer" => "Installed newer",
            "Epic integration update available; update FabLauncher through Epic Games Launcher's Fab integration settings" => {
                "Update through Epic"
            }
            "Managed by its publisher" => "Not available through Fab",
            "No verified Fab match" | "No owned Fab listing matches this plugin" => {
                "No verified Fab match"
            }
            "No entitled compatible build" => "No compatible build",
            "Version labels differ; cannot determine which is newer" => "Version unclear",
            _ => "Check failed",
        }
    }
}

pub fn check_summary(rows: &[Checked]) -> String {
    let updates = rows.iter().filter(|r| r.has_update()).count();
    let current = rows
        .iter()
        .filter(|r| matches!(r.status_label(), "Up to date" | "Installed newer"))
        .count();
    let unavailable = rows
        .iter()
        .filter(|r| r.status_label() == "Not available through Fab")
        .count();
    let unresolved = rows.len() - updates - current - unavailable;
    let mut parts = vec![
        format!(
            "{updates} {}",
            if updates == 1 { "update" } else { "updates" }
        ),
        format!("{current} up to date"),
    ];
    if unavailable > 0 {
        parts.push(format!("{unavailable} not available through Fab"));
    }
    if unresolved > 0 {
        parts.push(format!(
            "{unresolved} {} unresolved",
            if unresolved == 1 { "check" } else { "checks" }
        ));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod status_presentation_tests {
    use super::*;
    fn row(status: &str) -> Checked {
        Checked {
            plugin: Plugin {
                name: "Test".into(),
                version: "1.0".into(),
                descriptor: PathBuf::new(),
                catalog_id: None,
                number: 1,
            },
            update: None,
            available_version: Some("1.0".into()),
            status: status.into(),
        }
    }
    #[test]
    fn equal_versions_share_current_status_and_unavailable_is_not_a_failed_check() {
        let matched = row("Publisher version matches; original package build is not recorded");
        assert_eq!(matched.status_label(), "Up to date");
        let rows = vec![matched.clone(), matched.clone(), matched, row("Up to date")];
        assert_eq!(check_summary(&rows), "0 updates · 4 up to date");
        let unavailable = row(
            "Could not match this plugin to the signed-in account's Fab library; check the publisher for updates",
        );
        assert_eq!(unavailable.status_label(), "Not available through Fab");
        assert!(
            unavailable
                .status_detail()
                .contains("connected Fab library")
        );
        let failed = row("Fab request failed");
        assert_eq!(failed.status_label(), "Check failed");
        assert_eq!(
            check_summary(&[unavailable, failed]),
            "0 updates · 0 up to date · 1 not available through Fab · 1 check unresolved"
        );
    }
}

// Some publishers leave Version at 1 for every release (including PCGEx).
// Use a numeric dotted VersionName only when the internal counters tie.
// Arbitrary labels and prerelease suffixes are not safe to order by guesswork.
pub(crate) fn compare_versions(
    number: i64,
    name: &str,
    other_number: i64,
    other_name: &str,
) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    if number != other_number {
        return Some(number.cmp(&other_number));
    }
    let name = display_version(name);
    let other_name = display_version(other_name);
    if name.trim() == other_name.trim() {
        return Some(Ordering::Equal);
    }
    fn parts(name: &str) -> Option<Vec<u64>> {
        let name = name.trim().trim_start_matches(['v', 'V']);
        let mut parts: Vec<u64> = name
            .split('.')
            .map(|part| {
                if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                part.parse().ok()
            })
            .collect::<Option<_>>()?;
        if parts.len() < 2 || parts.len() > 8 {
            return None;
        }
        while parts.last() == Some(&0) {
            parts.pop();
        }
        Some(parts)
    }
    Some(parts(name)?.cmp(&parts(other_name)?))
}
/// Present SemVer's release/prerelease label while retaining package build
/// metadata in the original descriptor and all integrity records.
pub fn display_version(name: &str) -> &str {
    let trimmed = name.trim();
    let plain = trimmed.trim_start_matches(['v', 'V']);
    if semver::Version::parse(plain).is_ok() {
        if let Some((release, _)) = trimmed.split_once('+') {
            return release;
        }
    }
    trimmed
}
#[cfg(test)]
mod version_label_tests {
    use super::*;
    #[test]
    fn package_build_metadata_does_not_become_another_release_version() {
        assert_eq!(display_version("0.79.3+0.1"), "0.79.3");
        assert_eq!(display_version("v0.79.3+build.1"), "v0.79.3");
        assert_eq!(display_version("0.79.3-preview.1+0.1"), "0.79.3-preview.1");
        assert_eq!(display_version("C++ Tools + Extras"), "C++ Tools + Extras");
        assert_eq!(
            compare_versions(1, "0.79.3+0.1", 1, "0.79.3"),
            Some(std::cmp::Ordering::Equal)
        );
        assert_eq!(
            compare_versions(1, "0.79.3+0.1", 1, "0.79.2"),
            Some(std::cmp::Ordering::Greater)
        );
        assert_eq!(
            compare_versions(1, "0.79.3-preview.1+0.1", 1, "0.79.3"),
            None
        );
    }
}
// The descriptor can retain a legacy Marketplace ID after Epic changes the
// entitled catalog item ID. Never rewrite the user's descriptor to bridge it.
fn marketplace_id(descriptor: &serde_json::Value) -> Option<String> {
    descriptor["MarketplaceURL"]
        .as_str()
        .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}
fn same_catalog(asset: &serde_json::Value, id: &str) -> bool {
    asset["catalogItemId"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case(id))
}
fn legacy_app_hint(asset: &serde_json::Value, plugin: &Plugin) -> bool {
    let Some(stem) = plugin.descriptor.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let app = asset["appName"].as_str().unwrap_or("").to_ascii_lowercase();
    let stem = stem.to_ascii_lowercase();
    // Discovery hint only; candidate_package verifies the downloaded descriptor ID.
    app == stem || app.starts_with(&format!("{stem}_"))
}
fn unlinked_app_hint(asset: &serde_json::Value, plugin: &Plugin) -> bool {
    if legacy_app_hint(asset, plugin) {
        return true;
    }
    let compact = |s: &str| {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let app = compact(asset["appName"].as_str().unwrap_or(""));
    let stem = compact(
        plugin
            .descriptor
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(""),
    );
    let name = compact(&plugin.name);
    // Epic app names can abbreviate a long descriptor name before a build ID.
    // These are discovery hints only; downloaded package metadata must agree.
    (stem.len() >= 8 && app.starts_with(&stem[..8])) || (name.len() >= 5 && app.starts_with(&name))
}

fn owned_item_hint(item: &crate::fab::Item, plugin: &Plugin) -> bool {
    let words = |s: &str| {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    let name = words(&plugin.name);
    let title = words(&item.title);
    (!name.is_empty() && name.iter().all(|word| title.contains(word)))
        || item
            .project_versions
            .iter()
            .any(|v| unlinked_app_hint(&serde_json::json!({"appName": v.artifact_id}), plugin))
}

// The modern owned Fab library includes namespaces absent from Epic's legacy
// namespace=ue catalog. Names only discover candidates; package descriptors
// still have to prove identity before version comparisons or installation.
fn check_owned_plugin(
    plugin: &Plugin,
    engine: &Installation,
    items: &[crate::fab::Item],
    mut fetch: impl FnMut(&crate::fab::Item, &crate::fab::Version, bool) -> Result<Update, String>,
) -> Checked {
    let recorded = receipt::read(plugin).filter(|r| {
        items.iter().any(|i| {
            i.plugin()
                && i.project_versions
                    .iter()
                    .any(|v| v.artifact_id == r.artifact)
        })
    });
    let exact = plugin.catalog_id.as_ref().is_some_and(|id| {
        items
            .iter()
            .any(|i| i.plugin() && i.asset_id.eq_ignore_ascii_case(id))
    });
    let candidates: Vec<_> = items
        .iter()
        .filter(|i| {
            i.plugin()
                && if let Some(receipt) = &recorded {
                    i.project_versions
                        .iter()
                        .any(|v| v.artifact_id == receipt.artifact)
                } else if exact {
                    plugin
                        .catalog_id
                        .as_ref()
                        .is_some_and(|id| i.asset_id.eq_ignore_ascii_case(id))
                } else {
                    owned_item_hint(i, plugin)
                }
        })
        .collect();
    if candidates.is_empty() {
        return Checked { plugin: plugin.clone(), update: None, available_version: None, status: "Could not match this plugin to the signed-in account's Fab library; check the publisher for updates".into() };
    }
    let mut assets = Vec::new();
    for item in &candidates {
        for version in &item.project_versions {
            if (version.target_platforms.is_empty()
                || version.target_platforms.iter().any(|p| p == "Windows"))
                && version
                    .engine_versions
                    .iter()
                    .any(|v| compatible(v, &engine.version))
            {
                assets.push(serde_json::json!({"catalogItemId":item.asset_id,"namespace":item.asset_namespace,"appName":version.artifact_id,"buildVersion":engine.version}));
            }
        }
    }
    let mut checked = check_plugin_candidates(
        plugin,
        engine,
        &assets,
        |asset, _| {
            let item = candidates
                .iter()
                .find(|i| {
                    Some(i.asset_id.as_str()) == asset["catalogItemId"].as_str()
                        && Some(i.asset_namespace.as_str()) == asset["namespace"].as_str()
                })
                .ok_or("Fab item missing")?;
            let version = item
                .project_versions
                .iter()
                .find(|v| Some(v.artifact_id.as_str()) == asset["appName"].as_str())
                .ok_or("Fab artifact missing")?;
            fetch(item, version, !exact && recorded.is_none())
        },
        true,
    );
    if assets.is_empty() {
        checked.status = format!(
            "Matching Fab listing has no Windows build for Unreal Engine {}",
            engine.version
        );
    }
    checked
}

fn verify_unlinked_identity(plugin: &Plugin, remote: &serde_json::Value) -> Result<(), String> {
    let bytes = fs::read(&plugin.descriptor).map_err(|e| e.to_string())?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Oversized installed plugin descriptor".into());
    }
    let local: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid installed plugin descriptor")?;
    // An identical full descriptor can confirm a current package even for
    // content-only tools without modules or a publisher URL. It cannot authorize
    // an update with changed versions or other metadata.
    if local == *remote {
        return Ok(());
    }
    fn publisher(value: &serde_json::Value) -> Option<String> {
        let mut url = reqwest::Url::parse(value["CreatedByURL"].as_str()?.trim()).ok()?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        url.set_fragment(None);
        Some(url.as_str().trim_end_matches('/').to_owned())
    }
    fn nonempty<'a>(value: &'a serde_json::Value, field: &str) -> Option<&'a str> {
        value[field]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
    let same = |field| {
        nonempty(&local, field)
            .zip(nonempty(remote, field))
            .is_some_and(|(a, b)| a == b)
    };
    let same_publisher = publisher(&local)
        .zip(publisher(remote))
        .is_some_and(|(a, b)| a == b);
    let modules = |v: &serde_json::Value| -> HashSet<String> {
        v["Modules"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                m["Name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
            .collect()
    };
    let local_modules = modules(&local);
    let remote_modules = modules(remote);
    let stem = plugin
        .descriptor
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    // Require the plugin's primary module plus all installed module identities.
    // Added modules are allowed; removals/renames need an explicit reinstall.
    if !same_publisher
        || !same("CreatedBy")
        || !same("FriendlyName")
        || !local_modules.contains(stem)
        || !local_modules.is_subset(&remote_modules)
    {
        return Err("Package identity could not be verified: no matching listing identity was confirmed, and publisher, name, or module metadata is missing or differs. Automatic updates are unavailable; check the publisher.".into());
    }
    Ok(())
}
fn verify_legacy_identity(plugin: &Plugin, descriptor: &serde_json::Value) -> Result<(), String> {
    if plugin
        .catalog_id
        .as_ref()
        .zip(marketplace_id(descriptor))
        .is_some_and(|(local, remote)| local.eq_ignore_ascii_case(&remote))
    {
        Ok(())
    } else {
        Err("Package does not confirm this plugin's legacy Marketplace identity".into())
    }
}
fn compatible(version: &str, engine: &str) -> bool {
    EngineVersion::parse(version)
        .zip(EngineVersion::parse(engine))
        .is_some_and(|(a, b)| a.major == b.major && a.minor == b.minor)
}
// Only small descriptors are materialized in memory. Installable files stream to disk.
pub(crate) fn payload(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, String> {
    if file.size() > 16 * 1024 * 1024 {
        return Err(format!(
            "Package descriptor is unexpectedly large: {}",
            file.name
        ));
    }
    let mut out = Vec::with_capacity(file.size() as usize);
    stream_payload(m, bases, file, cache, &mut out)?;
    Ok(out)
}
fn stream_payload(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    output: &mut impl std::io::Write,
) -> Result<(), String> {
    stream_payload_progress(m, bases, file, cache, output, &mut |_, _| {})
}
fn stream_payload_progress(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    output: &mut impl std::io::Write,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(), String> {
    stream_payload_io(m, bases, file, cache, output, &mut |p| {
        progress(p.prepared, p.downloaded)
    })
}
#[derive(Clone, Copy, Debug, Default)]
struct PayloadIo {
    prepared: u64,
    downloaded: u64,
    read: u64,
    written: u64,
}
fn stream_payload_io(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    output: &mut impl std::io::Write,
    progress: &mut impl FnMut(PayloadIo),
) -> Result<(), String> {
    stream_payload_io_controlled(m, bases, file, cache, output, &Control::default(), progress)
}
fn stream_payload_io_controlled(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    output: &mut impl std::io::Write,
    control: &Control,
    progress: &mut impl FnMut(PayloadIo),
) -> Result<(), String> {
    control.check()?;
    let mut written = 0u64;
    let mut io = PayloadIo::default();
    let client = installer::client()?;
    use sha1::{Digest, Sha1};
    let mut hash = Sha1::new();
    for p in &file.parts {
        control.check()?;
        let chunk = m.chunks.get(&p.guid).ok_or("Missing plugin chunk")?;
        if !cache.contains_key(&p.guid) {
            if cache.len() >= 32 || cache.values().map(Vec::len).sum::<usize>() >= 64 * 1024 * 1024
            {
                cache.clear();
            }
            let bytes = if let Some(bytes) =
                crate::download_policy::cached_chunk_progress(chunk, &mut |n| {
                    io.read += n;
                    progress(io);
                }) {
                bytes
            } else {
                let bytes = installer::chunk_data_progress(
                    &client,
                    chunk,
                    bases,
                    m.feature,
                    control,
                    &mut |n| {
                        io.downloaded += n;
                        progress(io);
                    },
                )?;
                crate::download_policy::cache_chunk_progress(chunk, &bytes, &mut |n| {
                    io.written += n;
                    progress(io);
                });
                bytes
            };
            cache.insert(p.guid.clone(), bytes);
        }
        control.check()?;
        let bytes = &cache[&p.guid];
        let end = (p.offset as usize)
            .checked_add(p.size as usize)
            .ok_or("Invalid chunk extent")?;
        let part = bytes
            .get(p.offset as usize..end)
            .ok_or("Invalid plugin chunk extent")?;
        output
            .write_all(part)
            .map_err(|e| format!("Writing {}: {e}", file.name))?;
        hash.update(part);
        written += part.len() as u64;
        io.prepared = written;
        io.written += part.len() as u64;
        progress(io);
    }
    control.check()?;
    let actual: [u8; 20] = hash.finalize().into();
    if actual != file.sha {
        return Err(format!("File failed hash verification: {}", file.name));
    }
    Ok(())
}
#[cfg(test)]
pub(crate) fn payload_to_file(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    path: &Path,
) -> Result<(), String> {
    payload_to_file_progress(m, bases, file, cache, path, &mut |_, _| {})
}
#[cfg(test)]
pub(crate) fn payload_to_file_progress(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    path: &Path,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(), String> {
    payload_to_file_io(m, bases, file, cache, path, &mut |p| {
        progress(p.prepared, p.downloaded)
    })
}
pub(crate) fn payload_to_file_controlled(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    path: &Path,
    control: &Control,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(), String> {
    control.check()?;
    installer::no_links(path)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let result =
        stream_payload_io_controlled(m, bases, file, cache, &mut output, control, &mut |p| {
            progress(p.prepared, p.downloaded)
        })
        .and_then(|_| output.sync_all().map_err(|e| e.to_string()));
    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}
fn payload_to_file_io(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    path: &Path,
    progress: &mut impl FnMut(PayloadIo),
) -> Result<(), String> {
    installer::no_links(path)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Creating {}: {e}", path.display()))?;
    let result = stream_payload_io(m, bases, file, cache, &mut output, progress)
        .and_then(|_| output.sync_all().map_err(|e| e.to_string()));
    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}
#[cfg(test)]
fn load_package(
    session: &Session,
    asset: &serde_json::Value,
) -> Result<(Manifest, Vec<reqwest::Url>), String> {
    let release = Release {
        app: asset["appName"].as_str().ok_or("No app identity")?.into(),
        catalog_id: asset["catalogItemId"]
            .as_str()
            .ok_or("No catalog identity")?
            .into(),
        channel: "Live".into(),
        platform: "Windows".into(),
        build: asset["buildVersion"].as_str().map(str::to_owned),
        version: None,
    };
    let urls = Epic::new()?.manifest_locations(session, &release)?;
    let bases = urls
        .iter()
        .map(|u| {
            let mut b = u.join(".").map_err(|e| e.to_string())?;
            b.set_query(None);
            Ok(b)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let client = installer::client()?;
    let mut found = None;
    for url in urls {
        if let Ok(m) =
            installer::fetch(&client, url, 128 * 1024 * 1024).and_then(|b| buildpatch::parse(&b))
        {
            found = Some(m);
            break;
        }
    }
    let manifest = found.ok_or("Could not fetch plugin manifest")?;
    if manifest.app != release.app || release.build.as_ref().is_some_and(|b| b != &manifest.build) {
        return Err("Plugin build changed; check again".into());
    }
    Ok((manifest, bases))
}
fn candidate_package(
    manifest: Manifest,
    bases: Vec<reqwest::Url>,
    plugin: &Plugin,
    engine: &Installation,
    legacy: bool,
    fab_id: Option<&str>,
) -> Result<Update, String> {
    let matching: Vec<_> = manifest
        .files
        .iter()
        .filter(|f| Path::new(&f.name).file_name() == plugin.descriptor.file_name())
        .collect();
    if matching.len() != 1 {
        return Err("Plugin descriptor identity is ambiguous".into());
    }
    let descriptor = matching[0];
    if descriptor.size() > 2 * 1024 * 1024 {
        return Err("Oversized plugin descriptor".into());
    }
    let prefix = installer::relative(&descriptor.name)?
        .parent()
        .ok_or("Invalid descriptor path")?
        .to_path_buf();
    let bytes = payload(&manifest, &bases, descriptor, &mut HashMap::new())?;
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid plugin descriptor")?;
    if let Some(id) = fab_id {
        verify_fab_candidate_identity(plugin, &v, id, legacy)?;
    }
    if fab_id.is_none()
        && legacy
        && !receipt::read(plugin).is_some_and(|r| r.artifact == manifest.app)
    {
        if plugin.catalog_id.is_some() {
            verify_legacy_identity(plugin, &v)?;
        } else {
            verify_unlinked_identity(plugin, &v)?;
        }
    }
    if !compatible(v["EngineVersion"].as_str().unwrap_or(""), &engine.version) {
        return Err("No confirmed engine-compatible descriptor".into());
    }
    Ok(Update {
        version: v["VersionName"].as_str().unwrap_or("Unknown").into(),
        number: v["Version"]
            .as_i64()
            .ok_or("Missing plugin version number")?,
        manifest,
        bases,
        prefix,
    })
}
fn fab_descriptor_id(value: &serde_json::Value) -> Option<String> {
    let ids = descriptor_fab_ids(value);
    (ids.len() == 1).then(|| ids.into_iter().next().unwrap())
}
fn descriptor_fab_ids(value: &serde_json::Value) -> HashSet<String> {
    ["FabURL", "MarketplaceURL"]
        .into_iter()
        .filter_map(|field| {
            let raw = value[field].as_str()?;
            if let Some(id) = crate::fab_listing::listing_url_id(raw) {
                return Some(id);
            }
            let url = reqwest::Url::parse(raw).ok()?;
            if url.scheme() != "com.epicgames.launcher"
                || url.host_str() != Some("ue")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
            {
                return None;
            }
            let path = url.path().trim_end_matches('/');
            let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
            if parts.len() != 3
                || !parts[0].eq_ignore_ascii_case("fab")
                || !parts[1].eq_ignore_ascii_case("product")
            {
                return None;
            }
            crate::fab_listing::listing_url_id(&format!(
                "https://www.fab.com/listings/{}",
                parts[2]
            ))
        })
        .collect()
}
pub fn fab_listing_id(plugin: &Plugin) -> Option<String> {
    let bytes = fs::read(&plugin.descriptor).ok()?;
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    fab_descriptor_id(&serde_json::from_slice(&bytes).ok()?)
}
fn verify_fab_identity(value: &serde_json::Value, expected: &str) -> Result<(), String> {
    if fab_descriptor_id(value).as_deref() != Some(expected) {
        return Err("Fab package listing identity mismatch".into());
    }
    Ok(())
}
fn verify_fab_candidate_identity(
    plugin: &Plugin,
    value: &serde_json::Value,
    expected: &str,
    allow_metadata: bool,
) -> Result<(), String> {
    if descriptor_fab_ids(value).len() > 1 {
        return Err("Conflicting Fab listing IDs in package descriptor".into());
    }
    match verify_fab_identity(value, expected) {
        Ok(()) => Ok(()),
        Err(_) if allow_metadata => verify_unlinked_identity(plugin, value),
        Err(error) => Err(error),
    }
}
fn check_fab_plugin(
    plugin: &Plugin,
    engine: &Installation,
    listing_id: &str,
    items: &[crate::fab::Item],
    mut fetch: impl FnMut(&crate::fab::Item, &crate::fab::Version, bool) -> Result<Update, String>,
) -> Checked {
    let matches: Vec<_> = items
        .iter()
        .filter(|item| {
            item.plugin()
                && item
                    .url
                    .as_deref()
                    .and_then(crate::fab_listing::listing_url_id)
                    .as_deref()
                    == Some(listing_id)
        })
        .collect();
    if matches.is_empty() {
        // Public listing links can be absent or stale even for owned plugins.
        // Names/artifacts only discover candidates. The fetcher must verify the
        // package's listing ID or the installed publisher/name/module identity.
        let mut checked = check_owned_plugin(plugin, engine, items, |item, version, _| {
            fetch(item, version, true)
        });
        if checked
            .status
            .starts_with("Could not match this plugin to the signed-in account")
        {
            checked.status = "No owned Fab listing matches this plugin".into();
        }
        return checked;
    }
    if matches.len() != 1 {
        return Checked {
            plugin: plugin.clone(),
            update: None,
            available_version: None,
            status: "Multiple owned Fab items match; automatic update unavailable".into(),
        };
    }
    let item = matches[0];
    let versions: Vec<_> = item
        .project_versions
        .iter()
        .filter(|v| {
            (v.target_platforms.is_empty() || v.target_platforms.iter().any(|p| p == "Windows"))
                && v.engine_versions
                    .iter()
                    .any(|version| compatible(version, &engine.version))
        })
        .collect();
    // Use the existing version ordering/ambiguity rules after resolving the exact
    // owned Fab listing. Artifact suffixes are identifiers, not plugin versions.
    let assets: Vec<_> = versions.iter().map(|v| serde_json::json!({
        "catalogItemId": item.asset_id, "appName":v.artifact_id, "buildVersion": engine.version
    })).collect();
    let mut identified = plugin.clone();
    identified.catalog_id = Some(item.asset_id.clone());
    let mut checked = check_plugin(&identified, engine, &assets, |asset, _| {
        let version = versions
            .iter()
            .find(|v| Some(v.artifact_id.as_str()) == asset["appName"].as_str())
            .ok_or("Fab artifact missing")?;
        fetch(item, version, false)
    });
    checked.plugin = plugin.clone();
    checked
}
// Epic's shipped DefaultPortalRegions.ini selects this separate integration feed.
// FabLauncher is an export bridge, not an entitled marketplace code-plugin item.
const FAB_INTEGRATION_FEED: &str = "https://cdn.quixel.com/fab/manifest/prod/plugins_v1.json";
#[cfg(test)]
mod integration_feed_tests {
    use super::*;
    #[test]
    fn epic_integration_uses_its_exact_engine_feed_and_never_marketplace_installer() {
        let plugin = Plugin {
            name: "FabLauncher".into(),
            version: "0.5.0".into(),
            descriptor: PathBuf::from("FabLauncher.uplugin"),
            catalog_id: None,
            number: 1,
        };
        let mut engine = Installation {
            version: "5.7.4".into(),
            path: String::new(),
            association: String::new(),
        };
        let feed = serde_json::json!({"plugins": {
            "UE_5.7": {"windows": {"version":"0.5.0", "supportedVersions":["UE_5.7"]}},
            "UE_5.8": {"windows": {"version":"0.6.0", "supportedVersions":["UE_5.8"]}}
        }});
        let current = check_fab_integration(&plugin, &engine, Ok(&feed));
        assert_eq!(current.available_version.as_deref(), Some("0.5.0"));
        assert_eq!(current.status_label(), "Up to date");
        assert!(!current.has_update());
        engine.version = "5.8.3".into();
        let newer = check_fab_integration(&plugin, &engine, Ok(&feed));
        assert_eq!(newer.available_version.as_deref(), Some("0.6.0"));
        assert_eq!(newer.status_label(), "Update through Epic");
        assert!(newer.has_update());
        assert!(
            newer.update.is_none(),
            "ZIP integrations must not enter BuildPatch installation"
        );
        engine.version = "5.9".into();
        let missing = check_fab_integration(&plugin, &engine, Ok(&feed));
        assert_eq!(missing.status_label(), "Check failed");
        assert!(missing.available_version.is_none());
        assert_eq!(
            check_summary(&[current, newer, missing]),
            "1 update · 1 up to date · 1 check unresolved"
        );
        let failed = check_fab_integration(&plugin, &engine, Err("Network unavailable".into()));
        assert_eq!(failed.status, "Network unavailable");
    }
}
fn is_fab_integration(plugin: &Plugin) -> bool {
    if plugin.descriptor.file_name().and_then(|v| v.to_str()) != Some("FabLauncher.uplugin") {
        return false;
    }
    let Some(v) = crate::storage::read_json::<serde_json::Value>(&plugin.descriptor) else {
        return false;
    };
    v["CreatedBy"]
        .as_str()
        .is_some_and(|s| matches!(s, "Epic Games" | "Epic Games, Inc."))
        && v["Modules"]
            .as_array()
            .is_some_and(|m| m.iter().any(|m| m["Name"] == "FabLauncher"))
}
fn check_fab_integration(
    plugin: &Plugin,
    engine: &Installation,
    feed: Result<&serde_json::Value, String>,
) -> Checked {
    let mut row = Checked {
        plugin: plugin.clone(),
        update: None,
        available_version: None,
        status: String::new(),
    };
    let result = (|| {
        let feed = feed?;
        let branch = EngineVersion::parse(&engine.version)
            .ok_or("Unknown engine version")?
            .branch();
        let key = format!("UE_{branch}");
        let target = &feed["plugins"][&key]["windows"];
        if !target["supportedVersions"]
            .as_array()
            .is_some_and(|v| v.iter().any(|v| v.as_str() == Some(&key)))
        {
            return Err(format!(
                "Epic's integration feed has no Windows FabLauncher build for {key}"
            ));
        }
        let available = target["version"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or("Epic's integration feed has no version")?;
        row.available_version = Some(available.into());
        Ok(match compare_versions(0, available, 0, &plugin.version) {
            Some(std::cmp::Ordering::Equal) => "Up to date",
            Some(std::cmp::Ordering::Less) => "Installed version is newer",
            Some(std::cmp::Ordering::Greater) => "Epic integration update available; update FabLauncher through Epic Games Launcher's Fab integration settings",
            None => "Version labels differ; cannot determine which is newer",
        }.to_owned())
    })();
    row.status = result.unwrap_or_else(|e| e);
    row
}
pub fn check(engine: &Installation, session: &Session) -> Result<Vec<Checked>, String> {
    let items = crate::fab::library(session, |_| {});
    check_with_library(engine, session, &items)
}
/// Reopening a dialog reuses recent successful comparisons. Explicit checks bypass them.
pub fn check_cached(
    engine: &Installation,
    session: &Session,
    force: bool,
) -> Result<Vec<Checked>, String> {
    check_cached_with_progress(engine, session, force, false, |_| {})
}
pub fn check_cached_with_progress(
    engine: &Installation,
    session: &Session,
    force: bool,
    wait_for_library_builds: bool,
    mut ready: impl FnMut(&Checked),
) -> Result<Vec<Checked>, String> {
    let local = installed(Path::new(&engine.path));
    let mut rows = Vec::new();
    let mut pending = Vec::new();
    for plugin in local {
        if let Some(row) = check_cache::get(
            &session.account_id,
            engine,
            &plugin,
            force,
            wait_for_library_builds,
        ) {
            ready(&row);
            rows.push(row);
        } else {
            pending.push(plugin);
        }
    }
    if !pending.is_empty() {
        let items = crate::fab::library_cached(session, force);
        rows.extend(check_selected(
            engine,
            session,
            &items,
            pending,
            wait_for_library_builds,
            &mut ready,
        )?);
    }
    rows.sort_by_key(|r| r.plugin.name.to_lowercase());
    Ok(rows)
}
/// A multi-engine scan reuses one owned-library response instead of fetching it per engine.
pub fn check_with_library(
    engine: &Installation,
    session: &Session,
    fab_items: &Result<Vec<crate::fab::Item>, String>,
) -> Result<Vec<Checked>, String> {
    check_with_library_progress(engine, session, fab_items, false, |_| {})
}
pub fn check_with_library_progress(
    engine: &Installation,
    session: &Session,
    fab_items: &Result<Vec<crate::fab::Item>, String>,
    wait_for_library_builds: bool,
    ready: impl FnMut(&Checked),
) -> Result<Vec<Checked>, String> {
    check_selected(
        engine,
        session,
        fab_items,
        installed(Path::new(&engine.path)),
        wait_for_library_builds,
        ready,
    )
}
fn check_selected(
    engine: &Installation,
    session: &Session,
    fab_items: &Result<Vec<crate::fab::Item>, String>,
    plugins: Vec<Plugin>,
    wait_for_library_builds: bool,
    ready: impl FnMut(&Checked),
) -> Result<Vec<Checked>, String> {
    Ok(parallel_checks::run(
        plugins,
        |plugin| {
            let fingerprint = check_cache::fingerprint(&plugin);
            let row = check_one(engine, session, fab_items, plugin, wait_for_library_builds);
            check_cache::remember(
                &session.account_id,
                engine,
                &row,
                fingerprint,
                wait_for_library_builds,
            );
            row
        },
        ready,
    ))
}
fn check_one(
    engine: &Installation,
    session: &Session,
    fab_items: &Result<Vec<crate::fab::Item>, String>,
    plugin: Plugin,
    wait_for_library_builds: bool,
) -> Checked {
    let link = fab_listing_id(&plugin);
    if crate::storage::read_json::<serde_json::Value>(&plugin.descriptor)
        .is_some_and(|v| descriptor_fab_ids(&v).len() > 1)
    {
        return Checked {
            plugin,
            update: None,
            available_version: None,
            status:
                "Conflicting Fab listing IDs in the plugin descriptor; automatic update unavailable"
                    .into(),
        };
    }
    if is_fab_integration(&plugin) {
        let feed = (|| {
            let raw = installer::fetch(
                &installer::client()?,
                reqwest::Url::parse(FAB_INTEGRATION_FEED).unwrap(),
                1024 * 1024,
            )?;
            serde_json::from_slice::<serde_json::Value>(&raw)
                .map_err(|_| "Invalid Epic integration feed".to_owned())
        })();
        return check_fab_integration(&plugin, engine, feed.as_ref().map_err(Clone::clone));
    }
    if let Ok(items) = &fab_items {
        if receipt::read(&plugin).is_some_and(|r| {
            items.iter().any(|i| {
                i.plugin()
                    && i.project_versions
                        .iter()
                        .any(|v| v.artifact_id == r.artifact)
            })
        }) {
            return check_owned_plugin(&plugin, engine, items, |item, version, fallback| {
                let (manifest, bases) = crate::fab::package(session, item, version)?;
                if wait_for_library_builds {
                    version.require_listed_windows_build(&manifest.build)?;
                }
                candidate_package(manifest, bases, &plugin, engine, fallback, None)
            });
        }
    }
    if let Some(id) = link {
        match &fab_items {
            Ok(items) => {
                check_fab_plugin(&plugin, engine, &id, items, |item, version, fallback| {
                    let (manifest, bases) = crate::fab::package(session, item, version)?;
                    if wait_for_library_builds {
                        version.require_listed_windows_build(&manifest.build)?;
                    }
                    candidate_package(manifest, bases, &plugin, engine, fallback, Some(&id))
                })
            }
            Err(error) => Checked {
                plugin,
                update: None,
                available_version: None,
                status: error.clone(),
            },
        }
    } else {
        match &fab_items {
            Ok(items) => check_owned_plugin(&plugin, engine, items, |item, version, fallback| {
                let (manifest, bases) = crate::fab::package(session, item, version)?;
                if wait_for_library_builds {
                    version.require_listed_windows_build(&manifest.build)?;
                }
                candidate_package(manifest, bases, &plugin, engine, fallback, None)
            }),
            Err(error) => Checked {
                plugin,
                update: None,
                available_version: None,
                status: error.clone(),
            },
        }
    }
}

fn check_plugin(
    plugin: &Plugin,
    engine: &Installation,
    assets: &[serde_json::Value],
    fetch: impl FnMut(&serde_json::Value, bool) -> Result<Update, String>,
) -> Checked {
    check_plugin_candidates(plugin, engine, assets, fetch, false)
}
fn check_plugin_candidates(
    plugin: &Plugin,
    engine: &Installation,
    assets: &[serde_json::Value],
    mut fetch: impl FnMut(&serde_json::Value, bool) -> Result<Update, String>,
    owned_candidates: bool,
) -> Checked {
    let mut item = Checked {
        plugin: plugin.clone(),
        update: None,
        available_version: None,
        status: "Managed by its publisher".into(),
    };
    // An existing catalog identity takes precedence, even if it has no build
    // for this engine. Fallback is only for IDs absent from the owned catalog.
    let legacy = plugin
        .catalog_id
        .as_ref()
        .is_none_or(|id| !assets.iter().any(|asset| same_catalog(asset, id)));
    item.status = if plugin.catalog_id.is_some() {
        "No entitled compatible build"
    } else {
        "No verified Fab match"
    }
    .into();
    let mut verified_catalogs = HashSet::new();
    let mut best: Option<Update> = None;
    let mut ambiguous_versions = false;
    for asset in assets.iter().filter(|a| {
        compatible(a["buildVersion"].as_str().unwrap_or(""), &engine.version)
            && if owned_candidates {
                true
            } else if plugin.catalog_id.is_none() {
                unlinked_app_hint(a, plugin)
            } else if legacy {
                legacy_app_hint(a, plugin)
            } else {
                same_catalog(a, plugin.catalog_id.as_ref().unwrap())
            }
    }) {
        match fetch(asset, legacy) {
            Ok(update) => {
                verified_catalogs.insert((
                    asset["namespace"].as_str().unwrap_or("").to_owned(),
                    asset["catalogItemId"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase(),
                ));
                if let Some(previous) = &best {
                    if update.number > previous.number {
                        ambiguous_versions = false;
                    }
                    match compare_versions(
                        update.number,
                        &update.version,
                        previous.number,
                        &previous.version,
                    ) {
                        Some(std::cmp::Ordering::Greater) => best = Some(update),
                        None => ambiguous_versions = true,
                        _ => {}
                    }
                } else {
                    best = Some(update);
                }
            }
            Err(error) => item.status = error,
        }
    }
    if verified_catalogs.len() > 1 {
        item.status =
            "Multiple catalog items match this plugin; automatic update unavailable".into();
        return item;
    }
    if let Some(update) = best {
        item.available_version = Some(update.version.clone());
        item.status = if ambiguous_versions {
            "Version labels differ; cannot determine which is newer"
        } else {
            match compare_versions(
                update.number,
                &update.version,
                plugin.number,
                &plugin.version,
            ) {
                Some(std::cmp::Ordering::Greater) => {
                    item.update = Some(update);
                    "Update available"
                }
                Some(std::cmp::Ordering::Equal)
                    if receipt::changed_build(plugin, &update.manifest) =>
                {
                    item.update = Some(update);
                    "Update available"
                }
                Some(std::cmp::Ordering::Equal)
                    if !receipt::read(plugin)
                        .is_some_and(|r| r.artifact == update.manifest.app) =>
                {
                    "Publisher version matches; original package build is not recorded"
                }
                Some(std::cmp::Ordering::Equal) => "Up to date",
                Some(std::cmp::Ordering::Less) => "Installed version is newer",
                None => "Version labels differ; cannot determine which is newer",
            }
        }
        .into();
    }
    item
}
fn plugin_root(engine: &Installation, plugin: &Plugin) -> Result<PathBuf, String> {
    installer::no_links(Path::new(&engine.path))?;
    installer::no_links(&plugin.descriptor)?;
    let engine = fs::canonicalize(&engine.path).map_err(|e| e.to_string())?;
    let plugins = engine.join("Engine/Plugins");
    let root = fs::canonicalize(plugin.descriptor.parent().ok_or("Missing plugin folder")?)
        .map_err(|e| e.to_string())?;
    if root == plugins || root == plugins.join("Marketplace") || !root.starts_with(&plugins) {
        return Err("Refusing to modify a shared or external plugin folder".into());
    }
    let actual = installed(&engine)
        .into_iter()
        .find(|p| fs::canonicalize(&p.descriptor).ok() == fs::canonicalize(&plugin.descriptor).ok())
        .ok_or("Not an installed third-party plugin")?;
    if actual.number != plugin.number || actual.version != plugin.version {
        return Err("Plugin changed; refresh before modifying it".into());
    }
    // Do not move a parent containing another plugin or a linked external tree.
    fn walk(dir: &Path, descriptor: &Path) -> Result<(), String> {
        for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let p = e.map_err(|e| e.to_string())?.path();
            installer::no_links(&p)?;
            if p.is_dir() {
                walk(&p, descriptor)?;
            } else if p.extension().is_some_and(|s| s == "uplugin")
                && fs::canonicalize(&p).ok() != fs::canonicalize(descriptor).ok()
            {
                return Err(
                    "Folder contains multiple plugins; automatic removal/update is unsafe".into(),
                );
            }
        }
        Ok(())
    }
    walk(&root, &plugin.descriptor)?;
    Ok(root)
}
fn backup_path(engine: &Installation, root: &Path) -> Result<PathBuf, String> {
    let base = Path::new(&engine.path).join(".engine-workspace-plugin-backups");
    installer::no_links(&base)?;
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(base.join(format!(
        "{}-{stamp}",
        root.file_name().unwrap().to_string_lossy()
    )))
}
// Every deletion is bounded by a canonical engine-owned directory, and rejects links.
fn delete_scoped_tree(path: &Path, base: &Path) -> Result<(), String> {
    installer::no_links(path)?;
    let base = fs::canonicalize(base).map_err(|e| e.to_string())?;
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if path == base || !path.starts_with(&base) {
        return Err("Refusing deletion outside the plugin directory".into());
    }
    for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let child = entry.path();
        installer::no_links(&child)?;
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            delete_scoped_tree(&child, &base)?;
        } else {
            fs::remove_file(&child)
                .map_err(|e| format!("Could not delete {}: {e}", child.display()))?;
        }
    }
    fs::remove_dir(&path).map_err(|e| e.to_string())
}
pub fn remove_plugin(engine: &Installation, plugin: &Plugin) -> Result<String, String> {
    let eng = fs::canonicalize(&engine.path).map_err(|e| e.to_string())?;
    let _lock = crate::maintenance::lock(&eng)?;
    crate::maintenance::ensure_closed(&eng)?;
    let root = plugin_root(engine, plugin)?;
    let _change = crate::vault_health::changing(&root);
    delete_scoped_tree(&root,&eng.join("Engine/Plugins"))
        .map_err(|e|format!("Plugin removal failed: {e}. Some files may already have been removed; reinstall the plugin if needed."))?;
    Ok(format!("Removed {} from this engine.", plugin.name))
}
/// Progress describes file preparation separately from the final folder replacement.
#[derive(Clone, Debug, Default)]
pub struct UpdateProgress {
    pub phase: &'static str,
    pub file: String,
    pub files_done: usize,
    pub files_total: usize,
    pub prepared: u64,
    pub total: u64,
    pub downloaded: u64,
    pub read: u64,
    pub written: u64,
    /// Compressed size of unique package chunks, including chunks already cached.
    pub package_bytes: Option<u64>,
}
impl UpdateProgress {
    pub fn fraction(&self) -> Option<f32> {
        (self.phase == "Preparing and verifying files" && self.total > 0)
            .then(|| (self.prepared as f32 / self.total as f32).clamp(0., 1.))
    }
}
pub fn update_plugin(
    engine: &Installation,
    plugin: &Plugin,
    update: &Update,
) -> Result<String, String> {
    update_plugin_with_progress(engine, plugin, update, |_| {})
}
pub fn update_plugin_with_progress(
    engine: &Installation,
    plugin: &Plugin,
    update: &Update,
    mut report: impl FnMut(UpdateProgress),
) -> Result<String, String> {
    report(UpdateProgress {
        phase: "Checking installed plugin",
        ..Default::default()
    });
    crate::download_policy::check_install_allowed()?;
    let root = plugin_root(engine, plugin)?;
    let eng = fs::canonicalize(&engine.path).map_err(|e| e.to_string())?;
    let _lock = crate::maintenance::lock(&eng)?;
    crate::maintenance::ensure_closed(&eng)?;
    let version_order = compare_versions(
        update.number,
        &update.version,
        plugin.number,
        &plugin.version,
    );
    if version_order != Some(std::cmp::Ordering::Greater)
        && !(version_order == Some(std::cmp::Ordering::Equal)
            && receipt::changed_build(plugin, &update.manifest))
    {
        return Err("This is not a newer plugin version".into());
    }
    // Retention only removes recognized, completed plugin backups for this engine.
    backups::expire_locked(engine, crate::epic::now())?;
    let mut names = std::collections::HashSet::new();
    let mut files = vec![];
    for f in &update.manifest.files {
        let name = installer::relative(&f.name)?;
        if let Ok(relative) = name.strip_prefix(&update.prefix) {
            if relative.as_os_str().is_empty() {
                continue;
            }
            if !package_files::windows_file(relative) {
                continue;
            }
            if !names.insert(relative.to_string_lossy().to_lowercase()) {
                return Err("Duplicate plugin paths".into());
            }
            files.push((f, relative.to_owned()));
        }
    }
    if files.is_empty() {
        return Err("Empty plugin package".into());
    }
    let bytes: u64 = files.iter().map(|(f, _)| f.size()).sum();
    let mut chunk_ids = HashSet::new();
    let package_bytes = files
        .iter()
        .flat_map(|(f, _)| &f.parts)
        .filter(|p| chunk_ids.insert(&p.guid))
        .try_fold(0u64, |sum, p| {
            let size = update.manifest.chunks.get(&p.guid)?.size;
            (size > 0).then_some(sum + size)
        });
    if fs2::available_space(&eng).map_err(|e| e.to_string())? < bytes + 64 * 1024 * 1024 {
        return Err("Not enough space for plugin update".into());
    }
    let stage = backup_path(engine, &root)?.with_extension("staging");
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let mut state = UpdateProgress {
        phase: "Preparing and verifying files",
        files_total: files.len(),
        total: bytes,
        package_bytes,
        ..Default::default()
    };
    prepare_update_files(
        &update.manifest,
        &update.bases,
        &files,
        &stage,
        &mut HashMap::new(),
        &mut state,
        &mut report,
    )?;
    state.phase = "Recording verified package";
    state.file.clear();
    report(state.clone());
    receipt::write(&stage, &update.manifest, &update.prefix)?;
    let _change = crate::vault_health::changing(&root);
    activate_with_progress(
        engine,
        plugin,
        &root,
        &stage,
        update.number,
        &update.version,
        &mut |backup| {
            state.phase = backup.phase;
            state.file = backup.file;
            state.prepared = backup.prepared;
            state.total = backup.total;
            state.files_done = backup.files_done;
            state.files_total = backup.files_total;
            report(state.clone());
        },
    )
}
fn prepare_update_files(
    manifest: &Manifest,
    bases: &[reqwest::Url],
    files: &[(&buildpatch::File, PathBuf)],
    stage: &Path,
    cache: &mut HashMap<String, Vec<u8>>,
    state: &mut UpdateProgress,
    report: &mut impl FnMut(UpdateProgress),
) -> Result<(), String> {
    let mut last = std::time::Instant::now();
    report(state.clone());
    for (i, (file, relative)) in files.iter().enumerate() {
        let dest = stage.join(relative);
        installer::no_links(&dest)?;
        fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        state.file = relative.to_string_lossy().into_owned();
        let prepared = state.prepared;
        let downloaded = state.downloaded;
        let read = state.read;
        let written = state.written;
        payload_to_file_io(manifest, bases, file, cache, &dest, &mut |io| {
            state.prepared = prepared + io.prepared;
            state.downloaded = downloaded + io.downloaded;
            state.read = read + io.read;
            state.written = written + io.written;
            if last.elapsed().as_millis() >= 150 {
                report(state.clone());
                last = std::time::Instant::now();
            }
        })?;
        state.files_done = i + 1;
        if last.elapsed().as_millis() >= 150 || i + 1 == files.len() {
            report(state.clone());
            last = std::time::Instant::now();
        }
    }
    Ok(())
}

#[cfg(test)]
fn activate(
    engine: &Installation,
    plugin: &Plugin,
    root: &Path,
    stage: &Path,
    number: i64,
    version: &str,
) -> Result<String, String> {
    activate_with_progress(engine, plugin, root, stage, number, version, &mut |_| {})
}
fn activate_with_progress(
    engine: &Installation,
    plugin: &Plugin,
    root: &Path,
    stage: &Path,
    number: i64,
    version: &str,
    report: &mut impl FnMut(UpdateProgress),
) -> Result<String, String> {
    report(UpdateProgress {
        phase: "Validating prepared plugin",
        ..Default::default()
    });
    let eng = fs::canonicalize(&engine.path).map_err(|e| e.to_string())?;
    let desc = stage.join(plugin.descriptor.file_name().ok_or("Missing descriptor")?);
    let v: serde_json::Value = serde_json::from_slice(&fs::read(desc).map_err(|e| e.to_string())?)
        .map_err(|_| "Invalid staged descriptor")?;
    if v["Version"].as_i64() != Some(number)
        || v["VersionName"].as_str() != Some(version)
        || !compatible(v["EngineVersion"].as_str().unwrap_or(""), &engine.version)
    {
        return Err("Staged plugin version/compatibility check failed".into());
    }
    plugin_root(engine, plugin)?;
    crate::maintenance::ensure_closed(&eng)?;
    let days = crate::storage::load_preferences().plugin_backup_days;
    let mut backup = backups::prepare(engine, plugin, root, days, report)?;
    crate::maintenance::ensure_closed(&eng)?;
    report(UpdateProgress {
        phase: "Replacing installed plugin",
        ..Default::default()
    });
    backups::replace(root, stage, &mut backup)?;
    // Remove the container only when empty; never touch backups from earlier operations.
    let _ = fs::remove_dir(eng.join(".engine-workspace-plugin-backups"));
    Ok(format!(
        "Updated {} to {}. Previous plugin version saved in Plugin backups.",
        plugin.name, version
    ))
}
#[cfg(test)]
mod operation_tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Installation, Plugin) {
        let d = tempfile::tempdir().unwrap();
        let engine = Installation {
            path: d.path().join("UE_5.7").to_string_lossy().into_owned(),
            version: "5.7.4".into(),
            association: "5.7".into(),
        };
        let root = Path::new(&engine.path).join("Engine/Plugins/Marketplace/Test");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Test.uplugin"),
            br#"{"Version":1,"VersionName":"1.0","FriendlyName":"Test","EngineVersion":"5.7.0"}"#,
        )
        .unwrap();
        fs::write(root.join("custom.txt"), b"preserve old files").unwrap();
        let p = installed(Path::new(&engine.path)).remove(0);
        (d, engine, p)
    }
    #[test]
    fn removal_deletes_only_selected_engine_plugin_without_backup() {
        let (d, e, p) = fixture();
        let sibling = d.path().join("outside.uproject");
        fs::write(&sibling, b"keep").unwrap();
        let other = d
            .path()
            .join("UE_5.8/Engine/Plugins/Marketplace/Test/Test.uplugin");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, b"keep").unwrap();
        remove_plugin(&e, &p).unwrap();
        assert_eq!(fs::read(other).unwrap(), b"keep");
        assert!(installed(Path::new(&e.path)).is_empty());
        assert!(sibling.exists());
        assert!(!p.descriptor.parent().unwrap().exists());
        assert!(
            !Path::new(&e.path)
                .join(".engine-workspace-plugin-backups")
                .exists()
        );
    }
    #[test]
    fn update_activation_retains_previous_plugin_and_rejects_other_engine() {
        let (_d, e, p) = fixture();
        let root = p.descriptor.parent().unwrap();
        let stage = Path::new(&e.path).join("stage");
        fs::create_dir(&stage).unwrap();
        fs::write(
            stage.join("Test.uplugin"),
            br#"{"Version":2,"VersionName":"2.0","EngineVersion":"5.8.0"}"#,
        )
        .unwrap();
        assert!(activate(&e, &p, root, &stage, 2, "2.0").is_err());
        assert!(root.join("custom.txt").exists());
        fs::write(
            stage.join("Test.uplugin"),
            br#"{"Version":2,"VersionName":"2.0","EngineVersion":"5.7.0"}"#,
        )
        .unwrap();
        activate(&e, &p, root, &stage, 2, "2.0").unwrap();
        let saved = backups::list(&e).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].version, "1.0");
        assert_eq!(
            fs::read(
                Path::new(&e.path)
                    .join(".dnw-plugin-backups")
                    .join(&saved[0].id)
                    .join("files/custom.txt")
            )
            .unwrap(),
            b"preserve old files"
        );
        assert_eq!(installed(Path::new(&e.path))[0].number, 2);
        assert!(!root.join("custom.txt").exists());
        assert!(
            !Path::new(&e.path)
                .join(".engine-workspace-plugin-backups")
                .exists()
        );
    }
    #[test]
    fn windows_update_retains_required_build_files_and_skips_foreign_artifacts() {
        let (_dir, engine, plugin) = fixture();
        let cache = tempfile::tempdir().unwrap();
        let manifest = package_files::cached_fixture(cache.path(), "5.7.0");
        let update = Update {
            version: "2.0".into(),
            number: 2,
            manifest,
            bases: vec![],
            prefix: PathBuf::from("Test"),
        };
        let mut total = 0;
        crate::download_policy::with_cache_root(Some(cache.path().to_owned()), || {
            update_plugin_with_progress(&engine, &plugin, &update, |p| {
                if p.phase == "Preparing and verifying files" {
                    total = p.total;
                }
            })
        })
        .unwrap();
        let root = plugin.descriptor.parent().unwrap();
        assert!(
            total < 1024,
            "Progress must exclude the foreign package files"
        );
        assert!(root.join("Binaries/Win64/Test.dll").is_file());
        assert!(
            root.join("Intermediate/Build/Win64/x64/UnrealGame/Test.cpp.obj")
                .is_file()
        );
        assert!(
            root.join("Intermediate/Build/Win64/UnrealGame/Test.precompiled")
                .is_file()
        );
        assert!(root.join("Source/Mac/Module.cpp").is_file());
        for path in [
            "Binaries/Mac",
            "Intermediate/Build/Mac",
            "Intermediate/Build/IOS",
            "Intermediate/Build/Android",
        ] {
            assert!(!root.join(path).exists(), "Copied {path}");
        }
        assert_eq!(
            backups::list(&engine).unwrap().len(),
            1,
            "Previous plugin must remain backed up"
        );
    }
    #[test]
    fn reused_counter_activation_verifies_label_and_detects_changed_installs() {
        let (_d, e, p) = fixture();
        let root = p.descriptor.parent().unwrap();
        let stage = Path::new(&e.path).join("stage");
        fs::create_dir(&stage).unwrap();
        fs::write(
            stage.join("Test.uplugin"),
            br#"{"Version":1,"VersionName":"1.1","EngineVersion":"5.7.0"}"#,
        )
        .unwrap();
        assert!(activate(&e, &p, root, &stage, 1, "1.2").is_err());
        assert!(root.join("custom.txt").exists());
        activate(&e, &p, root, &stage, 1, "1.1").unwrap();
        let installed = installed(Path::new(&e.path));
        assert_eq!(installed[0].number, 1);
        assert_eq!(installed[0].version, "1.1");
        assert!(plugin_root(&e, &p).unwrap_err().contains("Plugin changed"));
    }

    #[test]
    fn nested_plugins_and_external_paths_are_rejected() {
        let (d, e, p) = fixture();
        let root = p.descriptor.parent().unwrap();
        fs::write(root.join("Other.uplugin"), b"{}").unwrap();
        assert!(remove_plugin(&e, &p).is_err());
        assert!(p.descriptor.exists());
        let external = Plugin {
            descriptor: d.path().join("outside.uplugin"),
            ..p
        };
        fs::write(&external.descriptor, b"{}").unwrap();
        assert!(remove_plugin(&e, &external).is_err());
    }
    #[test]
    fn corrupt_payload_is_rejected() {
        let f = buildpatch::File {
            name: "Test.uplugin".into(),
            sha: [1; 20],
            ..Default::default()
        };
        assert!(payload(&Manifest::default(), &[], &f, &mut HashMap::new()).is_err());
        assert!(!compatible("5.8.0", "5.7.4"));
        assert!(compatible("5.7.0", "5.7.4"));
    }
}
#[cfg(test)]
mod live_update_test {
    use super::*;
    #[test]
    #[ignore = "Requires signed-in Epic and downloads an owned plugin into a temporary engine"]
    fn owned_plugin_updates_in_isolated_engine() {
        let session = crate::storage::load_session().expect("Sign in first");
        let assets = Epic::new().unwrap().owned_assets(&session).unwrap();
        let mut selected = None;
        for a in assets
            .iter()
            .filter(|a| {
                a["appName"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("PivotTool_")
            })
            .take(25)
        {
            let Ok((m, b)) = load_package(&session, a) else {
                continue;
            };
            let descriptors: Vec<_> = m
                .files
                .iter()
                .filter(|f| f.name.ends_with(".uplugin"))
                .collect();
            if descriptors.len() != 1 {
                continue;
            }
            let engine_version = EngineVersion::parse(a["buildVersion"].as_str().unwrap())
                .unwrap()
                .to_string();
            let desc = descriptors[0];
            let prefix = installer::relative(&desc.name)
                .unwrap()
                .parent()
                .unwrap()
                .to_owned();
            let bytes: u64 = m
                .files
                .iter()
                .filter(|f| Path::new(&f.name).starts_with(&prefix))
                .map(|f| f.size())
                .sum();
            if bytes > 100 * 1024 * 1024 {
                continue;
            }
            let Ok(raw) = payload(&m, &b, desc, &mut HashMap::new()) else {
                continue;
            };
            let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            let Some(number) = value["Version"].as_i64() else {
                continue;
            };
            if number < 1
                || !compatible(
                    value["EngineVersion"].as_str().unwrap_or(""),
                    &engine_version,
                )
            {
                continue;
            }
            let name = Path::new(&desc.name).file_name().unwrap().to_owned();
            let version = value["VersionName"]
                .as_str()
                .unwrap_or("Unknown")
                .to_owned();
            value["Version"] = serde_json::json!(number - 1);
            value["VersionName"] = serde_json::json!("fixture old version");
            selected = Some((
                Update {
                    manifest: m,
                    bases: b,
                    prefix,
                    number,
                    version,
                },
                value,
                name,
                engine_version,
            ));
            break;
        }
        let (update, value, name, engine_version) =
            selected.expect("No small compatible owned plugin fixture found");
        let d = tempfile::tempdir().unwrap();
        let engine = Installation {
            path: d.path().join("UE_5.7").to_string_lossy().into_owned(),
            version: engine_version,
            association: "5.7".into(),
        };
        let root = Path::new(&engine.path).join("Engine/Plugins/Marketplace/Fixture");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(name), serde_json::to_vec(&value).unwrap()).unwrap();
        let plugin = installed(Path::new(&engine.path)).remove(0);
        let legacy_id = plugin
            .catalog_id
            .as_ref()
            .expect("Fixture must retain its Marketplace ID");
        assert!(
            !assets.iter().any(|asset| same_catalog(asset, legacy_id)),
            "Fixture must exercise a stale catalog ID"
        );
        let descriptor_before = fs::read(&plugin.descriptor).unwrap();
        let mut checked = check(&engine, &session).expect("Live update detection failed");
        assert_eq!(checked.len(), 1);
        let detected = checked.remove(0);
        assert_eq!(detected.status, "Update available");
        let detected = detected
            .update
            .expect("Old Marketplace ID must resolve without editing the descriptor");
        assert_eq!(fs::read(&plugin.descriptor).unwrap(), descriptor_before);
        assert_eq!(detected.number, update.number);
        let update = detected;
        let result = update_plugin(&engine, &plugin, &update).unwrap();
        assert_eq!(installed(Path::new(&engine.path))[0].number, update.number);
        for f in &update.manifest.files {
            if let Ok(relative) = Path::new(&f.name).strip_prefix(&update.prefix) {
                let data = fs::read(root.join(relative)).unwrap();
                assert_eq!(buildpatch::digest(&data), f.sha);
            }
        }
        println!(
            "Live plugin update passed: {} -> {}, legacy ID detected, all downloaded file hashes verified",
            plugin.name, update.version
        );
        assert!(result.starts_with("Updated"));
        let after = check(&engine, &session).unwrap();
        assert_eq!(after[0].status, "Up to date");
        assert!(after[0].update.is_none());
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use sha1::{Digest, Sha1};
    fn fixture(count: usize) -> (Manifest, buildpatch::File, HashMap<String, Vec<u8>>) {
        let data = vec![0x5au8; 1024 * 1024];
        let guid = "stream-test".to_owned();
        let mut hash = Sha1::new();
        let parts = (0..count)
            .map(|_| {
                hash.update(&data);
                buildpatch::Part {
                    guid: guid.clone(),
                    offset: 0,
                    size: data.len() as u32,
                }
            })
            .collect();
        let file = buildpatch::File {
            name: "large.pak".into(),
            sha: hash.finalize().into(),
            parts,
            ..Default::default()
        };
        let mut m = Manifest::default();
        m.chunks.insert(guid.clone(), buildpatch::Chunk::default());
        (m, file, HashMap::from([(guid, data)]))
    }
    #[test]
    fn stopping_large_file_interrupts_stream_and_removes_partial_output() {
        let (manifest, file, mut cache) = fixture(4);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.bin");
        let control = Control::default();
        let mut prepared = 0;
        let result = payload_to_file_controlled(
            &manifest,
            &[],
            &file,
            &mut cache,
            &path,
            &control,
            &mut |written, _| {
                prepared = written;
                if written > 0 {
                    control.stop();
                }
            },
        );
        assert!(result.unwrap_err().starts_with("Stopped."));
        assert_eq!(
            prepared,
            1024 * 1024,
            "stop between chunks, not after the entire file"
        );
        assert!(!path.exists(), "incomplete output must be removed");
        let result = payload_to_file_controlled(
            &manifest,
            &[],
            &file,
            &mut cache,
            &path,
            &control,
            &mut |_, _| panic!("cancelled transfer must not write"),
        );
        assert!(result.is_err());
        assert!(!path.exists());
    }
    #[test]
    fn update_progress_counts_cached_bytes_across_files_and_keeps_install_phase_separate() {
        let (manifest, file, mut cache) = fixture(3);
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            (&file, PathBuf::from("one.bin")),
            (&file, PathBuf::from("two.bin")),
        ];
        let mut state = UpdateProgress {
            phase: "Preparing and verifying files",
            files_total: 2,
            total: 2 * file.size(),
            ..Default::default()
        };
        let mut events = vec![];
        prepare_update_files(
            &manifest,
            &[],
            &files,
            dir.path(),
            &mut cache,
            &mut state,
            &mut |p| events.push(p),
        )
        .unwrap();
        assert_eq!(state.prepared, 6 * 1024 * 1024);
        assert_eq!(state.downloaded, 0);
        assert_eq!(state.read, 0); // This fixture supplies chunks already in memory.
        assert_eq!(state.written, 6 * 1024 * 1024);
        assert_eq!(state.files_done, 2);
        assert_eq!(state.fraction(), Some(1.));
        assert_eq!(events.first().unwrap().prepared, 0);
        assert_eq!(events.last().unwrap().files_done, 2);
        assert!(events.windows(2).all(|p| p[0].prepared <= p[1].prepared));
        state.phase = "Replacing installed plugin";
        assert_eq!(state.fraction(), None);
        assert_eq!(
            fs::metadata(dir.path().join("two.bin")).unwrap().len(),
            file.size()
        );
    }
    #[test]
    fn update_progress_does_not_count_corrupt_files_as_verified() {
        let (manifest, mut file, mut cache) = fixture(1);
        file.sha = [0; 20];
        let dir = tempfile::tempdir().unwrap();
        let files = vec![(&file, PathBuf::from("bad.bin"))];
        let mut state = UpdateProgress {
            phase: "Preparing and verifying files",
            files_total: 1,
            total: file.size(),
            ..Default::default()
        };
        assert!(
            prepare_update_files(
                &manifest,
                &[],
                &files,
                dir.path(),
                &mut cache,
                &mut state,
                &mut |_| {}
            )
            .is_err()
        );
        assert_eq!(state.files_done, 0);
        assert!(!dir.path().join("bad.bin").exists());
    }
    #[test]
    fn writes_and_verifies_file_larger_than_one_gib() {
        let (m, f, mut cache) = fixture(1025);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.pak");
        payload_to_file(&m, &[], &f, &mut cache, &path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), 1025 * 1024 * 1024);
        assert_eq!(cache.values().map(Vec::len).sum::<usize>(), 1024 * 1024);
        use std::io::Read;
        let mut input = fs::File::open(path).unwrap();
        let mut buffer = [0u8; 64 * 1024];
        let mut hash = Sha1::new();
        loop {
            let n = input.read(&mut buffer).unwrap();
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        let actual: [u8; 20] = hash.finalize().into();
        assert_eq!(actual, f.sha);
    }
    #[test]
    fn streaming_reports_incremental_bytes_without_counting_cached_data_as_downloaded() {
        let (m, f, mut cache) = fixture(3);
        let mut events = Vec::new();
        stream_payload_progress(
            &m,
            &[],
            &f,
            &mut cache,
            &mut std::io::sink(),
            &mut |written, downloaded| events.push((written, downloaded)),
        )
        .unwrap();
        assert_eq!(
            events,
            vec![(1024 * 1024, 0), (2 * 1024 * 1024, 0), (3 * 1024 * 1024, 0)]
        );
    }
    #[test]
    fn rejects_corruption_and_preserves_existing_files() {
        let (m, mut f, mut cache) = fixture(2);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data");
        f.sha = [0; 20];
        assert!(payload_to_file(&m, &[], &f, &mut cache, &path).is_err());
        assert!(!path.exists());
        fs::write(&path, b"keep").unwrap();
        assert!(payload_to_file(&m, &[], &f, &mut cache, &path).is_err());
        assert_eq!(fs::read(path).unwrap(), b"keep");
    }
    #[test]
    fn rejects_bad_extents_and_propagates_write_failure() {
        let (m, mut f, mut cache) = fixture(1);
        struct Fails;
        impl std::io::Write for Fails {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(
            stream_payload(&m, &[], &f, &mut cache, &mut Fails)
                .unwrap_err()
                .contains("disk full")
        );
        f.parts[0].offset = 1;
        assert!(
            stream_payload(&m, &[], &f, &mut cache, &mut std::io::sink())
                .unwrap_err()
                .contains("extent")
        );
    }
}

#[cfg(test)]
mod catalog_identity_tests {
    use super::*;
    use serde_json::{Value, json};
    const OLD: &str = "a220ab040cb844beb355b0f159eabefa";
    const CURRENT: &str = "1bc9b0eb0d5e4043b4ced5fbc2623859";
    fn fixture() -> (Plugin, Installation) {
        (
            Plugin {
                name: "Custom display name".into(),
                version: "old".into(),
                descriptor: PathBuf::from("Engine/Plugins/Marketplace/PivotTool/PivotTool.uplugin"),
                catalog_id: Some(OLD.into()),
                number: 1,
            },
            Installation {
                path: "unused".into(),
                version: "5.4.4".into(),
                association: "5.4".into(),
            },
        )
    }
    fn asset(id: &str, app: &str, engine: &str) -> Value {
        json!({"catalogItemId":id,"appName":app,"buildVersion":engine})
    }
    fn descriptor(id: &str) -> Value {
        json!({"MarketplaceURL":format!("com.epicgames.launcher://ue/marketplace/content/{id}")})
    }
    fn update(number: i64) -> Update {
        Update {
            version: format!("v{number}"),
            number,
            manifest: Manifest::default(),
            bases: vec![],
            prefix: PathBuf::new(),
        }
    }
    #[test]
    fn legacy_id_resolves_only_after_package_confirms_identity() {
        let (p, e) = fixture();
        let a = [asset(CURRENT, "PivotTool_build", "5.4.0")];
        let checked = check_plugin(&p, &e, &a, |_, legacy| {
            assert!(legacy);
            verify_legacy_identity(&p, &descriptor(OLD))?;
            Ok(update(2))
        });
        assert_eq!(checked.status, "Update available");
        assert_eq!(checked.update.unwrap().number, 2);
        assert_eq!(checked.plugin.catalog_id.as_deref(), Some(OLD));
        for wrong in [descriptor(CURRENT), json!({})] {
            let checked = check_plugin(&p, &e, &a, |_, _| {
                verify_legacy_identity(&p, &wrong)?;
                Ok(update(2))
            });
            assert!(checked.update.is_none());
            assert!(checked.status.contains("does not confirm"));
        }
    }
    #[test]
    fn exact_owned_id_takes_precedence_over_name_hint() {
        let (p, e) = fixture();
        let a = [
            asset(OLD, "RenamedPackage_build", "5.4.0"),
            asset(CURRENT, "PivotTool_build", "5.4.0"),
        ];
        let checked = check_plugin(&p, &e, &a, |a, legacy| {
            assert!(!legacy);
            assert!(same_catalog(a, OLD));
            Ok(update(2))
        });
        assert!(checked.update.is_some());
        // An incompatible exact ID must not be replaced by a different product.
        let a = [
            asset(OLD, "RenamedPackage_build", "5.3.0"),
            asset(CURRENT, "PivotTool_build", "5.4.0"),
        ];
        assert!(
            check_plugin(&p, &e, &a, |_, _| panic!("must not fetch"))
                .update
                .is_none()
        );
    }
    #[test]
    fn fallback_excludes_other_engines_and_similarly_named_products() {
        let (p, e) = fixture();
        let a = [
            asset(CURRENT, "PivotTool_build", "5.3.0"),
            asset(CURRENT, "PivotToolPro_build", "5.4.0"),
            asset(CURRENT, "Other_build", "5.4.0"),
        ];
        assert!(
            check_plugin(&p, &e, &a, |_, _| panic!("must not fetch"))
                .update
                .is_none()
        );
        let mut p = p;
        p.catalog_id = None;
        assert!(
            check_plugin(
                &p,
                &e,
                &[asset(CURRENT, "PivotTool_build", "5.4.0")],
                |_, _| {
                    verify_unlinked_identity(&p, &serde_json::json!({}))?;
                    Ok(update(2))
                }
            )
            .update
            .is_none()
        );
    }
    #[test]
    fn multiple_verified_catalog_ids_are_not_automatically_updated() {
        let (p, e) = fixture();
        let a = [
            asset(CURRENT, "PivotTool_one", "5.4.0"),
            asset("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "PivotTool_two", "5.4.0"),
        ];
        let checked = check_plugin(&p, &e, &a, |_, _| Ok(update(2)));
        assert!(checked.update.is_none());
        assert!(checked.status.contains("Multiple catalog items"));
    }
    #[test]
    fn unlisted_package_waits_without_exposing_an_update_then_becomes_available() {
        let (p, e) = fixture();
        let assets = [asset(OLD, "PivotTool_build", "5.4.0")];
        let mut version: crate::fab::Version = serde_json::from_value(serde_json::json!({
            "artifactId": "PivotTool_build",
            "buildVersions": [{"platform":"Windows", "buildVersion":"older"}]
        }))
        .unwrap();
        let waiting = check_plugin(&p, &e, &assets, |_, _| {
            version.require_listed_windows_build("newer")?;
            Ok(update(2))
        });
        assert_eq!(waiting.status_label(), "Waiting for Fab library");
        assert!(!waiting.has_update());
        assert!(waiting.available_version.is_none());
        // The existing path still offers this package when the optional gate is off.
        assert!(check_plugin(&p, &e, &assets, |_, _| Ok(update(2))).has_update());
        version.build_versions[0].build_version = "newer".into();
        let listed = check_plugin(&p, &e, &assets, |_, _| {
            version.require_listed_windows_build("newer")?;
            Ok(update(2))
        });
        assert!(listed.has_update());
    }

    #[test]
    fn repeated_builds_choose_highest_version_without_downgrading() {
        let (mut p, e) = fixture();
        let a = [
            asset(CURRENT, "PivotTool_one", "5.4.0"),
            asset(CURRENT, "PivotTool_two", "5.4.0"),
        ];
        for (installed, status) in [
            (1, "Update available"),
            (
                3,
                "Publisher version matches; original package build is not recorded",
            ),
            (4, "Installed version is newer"),
        ] {
            p.number = installed;
            p.version = format!("v{installed}");
            let checked = check_plugin(&p, &e, &a, |a, _| {
                Ok(update(if a["appName"] == "PivotTool_one" {
                    3
                } else {
                    2
                }))
            });
            assert_eq!(checked.status, status);
            assert_eq!(checked.update.is_some(), installed == 1);
        }
    }
    #[test]
    fn pcgex_reused_counter_detects_newer_label_and_selects_latest_compatible_build() {
        let (mut p, mut e) = fixture();
        p.version = "0.75.6".into();
        p.number = 1;
        e.version = "5.6.1".into();
        let assets = [
            asset(OLD, "PCGEx_older", "5.6.0"),
            asset(OLD, "PCGEx_newer", "5.6.0"),
            asset(OLD, "PCGEx_other_engine", "5.8.0"),
        ];
        let checked = check_plugin(&p, &e, &assets, |a, _| {
            assert_ne!(a["appName"], "PCGEx_other_engine");
            let mut u = update(1);
            u.version = if a["appName"] == "PCGEx_newer" {
                "0.76.1"
            } else {
                "0.75.6"
            }
            .into();
            Ok(u)
        });
        assert_eq!(checked.status, "Update available");
        assert_eq!(checked.update.unwrap().version, "0.76.1");
    }

    #[test]
    fn reused_counter_never_downgrades_a_newer_label() {
        let (mut p, e) = fixture();
        p.version = "0.79.1".into();
        let assets = [asset(OLD, "PCGEx", "5.4.0")];
        let checked = check_plugin(&p, &e, &assets, |_, _| {
            let mut u = update(1);
            u.version = "0.76.1".into();
            Ok(u)
        });
        assert_eq!(checked.status, "Installed version is newer");
        assert!(checked.update.is_none());
    }

    #[test]
    fn version_labels_are_numeric_and_unknown_labels_are_not_declared_current() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions(1, "0.76.1", 1, "0.75.6"), Some(Greater));
        assert_eq!(compare_versions(1, "1.10", 1, "1.9"), Some(Greater));
        assert_eq!(compare_versions(1, "v1.0", 1, "1.0.0"), Some(Equal));
        assert_eq!(compare_versions(1, "1.0-preview", 1, "1.0"), None);
        assert_eq!(compare_versions(1, "new build", 1, "old build"), None);
        assert_eq!(
            compare_versions(2, "custom", 1, "newer name"),
            Some(Greater)
        );
        let (mut p, e) = fixture();
        p.version = "old build".into();
        let checked = check_plugin(&p, &e, &[asset(OLD, "PCGEx", "5.4.0")], |_, _| {
            let mut u = update(1);
            u.version = "new build".into();
            Ok(u)
        });
        assert_eq!(checked.status_label(), "Version unclear");
        assert!(checked.update.is_none());
    }

    #[test]
    fn unknown_catalog_matches_are_not_reported_as_no_updates() {
        let (mut p, e) = fixture();
        let missing = check_plugin(&p, &e, &[], |_, _| panic!("must not fetch"));
        assert_eq!(missing.status_label(), "No compatible build");
        p.catalog_id = None;
        let unlinked = check_plugin(&p, &e, &[], |_, _| panic!("must not fetch"));
        assert_eq!(unlinked.status_label(), "No verified Fab match");
    }

    #[test]
    fn conflicting_unordered_labels_do_not_pick_an_arbitrary_package() {
        let (p, e) = fixture();
        let assets = [
            asset(OLD, "release", "5.4.0"),
            asset(OLD, "preview", "5.4.0"),
        ];
        for reverse in [false, true] {
            let mut assets = assets.clone();
            if reverse {
                assets.reverse();
            }
            let checked = check_plugin(&p, &e, &assets, |a, _| {
                let mut u = update(1);
                u.version = if a["appName"] == "release" {
                    "0.76.1"
                } else {
                    "0.76.1-preview"
                }
                .into();
                Ok(u)
            });
            assert_eq!(checked.status_label(), "Version unclear");
            assert!(checked.update.is_none());
        }
    }

    #[test]
    fn marketplace_ids_are_case_insensitive_but_not_arbitrary_strings() {
        let (p, _) = fixture();
        assert!(verify_legacy_identity(&p, &descriptor(&OLD.to_uppercase())).is_ok());
        assert!(marketplace_id(&descriptor("PivotTool")).is_none());
        assert!(same_catalog(&asset(&OLD.to_uppercase(), "x", "5.4.0"), OLD));
    }
}

#[cfg(test)]
mod unlinked_pcgex_tests {
    use super::*;
    use serde_json::{Value, json};
    // Reduced metadata from the reporter's Fab-installed descriptor and the
    // publisher's 5.6 branch (2026-10-15). The remote fixture is NOT a Fab claim.
    const LOCAL: &[u8] = include_bytes!("../tests/fixtures/pcgex-fab-0.75.6-ue5.6.json");
    const REMOTE: &[u8] = include_bytes!("../tests/fixtures/pcgex-source-0.76.1-ue5.6.json");
    fn fixture() -> (tempfile::TempDir, Installation, Plugin, Value) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Engine/Plugins/Marketplace/PCGEx");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("PCGExtendedToolkit.uplugin"), LOCAL).unwrap();
        let engine = Installation {
            path: dir.path().to_string_lossy().into_owned(),
            version: "5.6.1".into(),
            association: "5.6".into(),
        };
        let plugin = installed(dir.path()).remove(0);
        (dir, engine, plugin, serde_json::from_slice(REMOTE).unwrap())
    }
    fn asset(id: &str, app: &str, engine: &str) -> Value {
        json!({"catalogItemId":id,"appName":app,"buildVersion":engine})
    }
    fn confirmed(plugin: &Plugin, remote: &Value) -> Result<Update, String> {
        verify_unlinked_identity(plugin, remote)?;
        Ok(Update {
            version: remote["VersionName"].as_str().unwrap().into(),
            number: remote["Version"].as_i64().unwrap(),
            manifest: Manifest::default(),
            bases: vec![],
            prefix: PathBuf::new(),
        })
    }
    #[test]
    fn attached_fab_descriptor_without_catalog_url_can_detect_compatible_update() {
        let (_dir, engine, plugin, remote) = fixture();
        assert!(plugin.catalog_id.is_none());
        assert_eq!(plugin.number, 1);
        assert_eq!(plugin.version, "0.75.6");
        let before = fs::read(&plugin.descriptor).unwrap();
        for app in [
            "PCGExtendedToolkit_build",
            "PCGExtende123456V1",
            "PCGEx_build",
        ] {
            let assets = [asset("fixture-owned-catalog", app, "5.6.0")];
            let checked = check_plugin(&plugin, &engine, &assets, |_, fallback| {
                assert!(fallback);
                assert!(compatible(
                    remote["EngineVersion"].as_str().unwrap(),
                    &engine.version
                ));
                confirmed(&plugin, &remote)
            });
            assert_eq!(checked.status, "Update available");
            assert_eq!(checked.update.unwrap().version, "0.76.1");
        }
        assert_eq!(fs::read(&plugin.descriptor).unwrap(), before);
    }
    #[test]
    fn modern_fab_library_resolves_short_artifact_names_without_legacy_catalog() {
        let (_dir, engine, plugin, remote) = fixture();
        let item: crate::fab::Item = serde_json::from_value(json!({
            "assetId":"modern-id", "assetNamespace":"publisher-namespace", "title":"PCG Extended Toolkit",
            "distributionMethod":"CODE_PLUGIN", "projectVersions":[
                {"artifactId":"PCGExten123456789abcV4", "engineVersions":["UE_5.6"], "targetPlatforms":["Windows"]},
                {"artifactId":"PCGExten123456789abcV5", "engineVersions":["UE_5.7"], "targetPlatforms":["Windows"]}
            ]
        })).unwrap();
        let before = fs::read(&plugin.descriptor).unwrap();
        let checked = check_owned_plugin(&plugin, &engine, &[item.clone()], |i, v, fallback| {
            assert_eq!(i.asset_namespace, "publisher-namespace");
            assert!(v.artifact_id.ends_with("V4"));
            assert!(fallback);
            confirmed(&plugin, &remote)
        });
        assert_eq!(checked.update.unwrap().version, "0.76.1");
        let mut newer = plugin.clone();
        newer.version = "0.79.1".into();
        let checked = check_owned_plugin(&newer, &engine, &[item.clone()], |_, _, _| {
            confirmed(&newer, &remote)
        });
        assert_eq!(checked.status, "Installed version is newer");
        let mut other = item.clone();
        other.asset_id = "another-product".into();
        let checked = check_owned_plugin(&plugin, &engine, &[item.clone(), other], |_, _, _| {
            confirmed(&plugin, &remote)
        });
        assert!(checked.update.is_none());
        assert!(checked.status.starts_with("Multiple catalog"));
        let checked = check_owned_plugin(&plugin, &engine, &[item], |_, _, _| {
            Err("Unverified publisher".into())
        });
        assert_eq!(checked.status, "Unverified publisher");
        assert!(checked.update.is_none());
        assert_eq!(fs::read(&plugin.descriptor).unwrap(), before);
    }
    #[test]
    fn no_url_fallback_rejects_other_publishers_missing_metadata_and_module_mismatch() {
        let (_dir, _engine, plugin, remote) = fixture();
        assert!(verify_unlinked_identity(&plugin, &remote).is_ok());
        for (key, bad) in [
            ("CreatedByURL", json!("https://example.com/other-publisher")),
            ("CreatedBy", json!("Someone else")),
            ("FriendlyName", json!("PCGEx Pro")),
            ("Modules", json!([])),
        ] {
            let mut changed = remote.clone();
            changed[key] = bad;
            assert!(
                verify_unlinked_identity(&plugin, &changed).is_err(),
                "{key}"
            );
        }
        assert!(verify_unlinked_identity(&plugin, &json!({})).is_err());
    }
    #[test]
    fn identical_content_only_descriptor_confirms_current_but_cannot_authorize_updates() {
        let (_dir, _engine, plugin, _) = fixture();
        let local = json!({"FriendlyName":"Content Tool", "CreatedBy":"Publisher", "Version":1, "VersionName":"1.0", "CanContainContent":true});
        fs::write(&plugin.descriptor, serde_json::to_vec(&local).unwrap()).unwrap();
        assert!(verify_unlinked_identity(&plugin, &local).is_ok());
        let mut newer = local;
        newer["VersionName"] = "2.0".into();
        assert!(verify_unlinked_identity(&plugin, &newer).is_err());
    }
    #[test]
    fn no_url_fallback_is_engine_scoped_and_ambiguous_products_are_rejected() {
        let (_dir, engine, plugin, remote) = fixture();
        let wrong = [asset("fixture", "PCGEx_build", "5.8.0")];
        let unavailable = check_plugin(&plugin, &engine, &wrong, |_, _| {
            panic!("must not fetch a different engine branch")
        });
        assert!(unavailable.update.is_none());
        assert_eq!(unavailable.status_label(), "No verified Fab match");
        let ambiguous = [
            asset("fixture-one", "PCGEx_build", "5.6.0"),
            asset("fixture-two", "PCGEx_other", "5.6.0"),
        ];
        let checked = check_plugin(&plugin, &engine, &ambiguous, |_, _| {
            confirmed(&plugin, &remote)
        });
        assert!(checked.update.is_none());
        assert!(checked.status.contains("Multiple catalog items"));
    }
}

#[cfg(test)]
mod fab_listing_update_tests {
    use super::*;
    const ID: &str = "cc258205-8fcf-41e5-9b48-6ec44e46d7eb";
    #[test]
    fn pcgex_faburl_launcher_links_resolve_without_marketplace_url() {
        // Public PCGEx v0.79.1-preview.1 descriptor, metadata fields only.
        let pcgex_id = "a5061972-6875-4052-a7fd-7c33f6531ec3";
        let descriptor = serde_json::json!({"Version":1,"VersionName":"0.79.1","FriendlyName":"PCGEx","FabURL":format!("com.epicgames.launcher://ue/Fab/product/{pcgex_id}"),"EngineVersion":"5.8.0"});
        assert_eq!(fab_descriptor_id(&descriptor).as_deref(), Some(pcgex_id));
        assert!(verify_fab_identity(&descriptor, pcgex_id).is_ok());
        for bad in [
            format!("https://example.com/listings/{pcgex_id}"),
            format!("com.epicgames.launcher://other/Fab/product/{pcgex_id}"),
            format!("com.epicgames.launcher://ue/Fab/product/{pcgex_id}/extra"),
        ] {
            assert!(fab_descriptor_id(&serde_json::json!({"FabURL":bad})).is_none());
        }
        let mut conflicting = descriptor;
        conflicting["MarketplaceURL"] = format!("https://www.fab.com/listings/{ID}").into();
        assert!(fab_descriptor_id(&conflicting).is_none());
        assert!(verify_fab_identity(&conflicting, pcgex_id).is_err());
    }
    fn fixture() -> (Plugin, Installation, crate::fab::Item) {
        let plugin = Plugin {
            name: "Ascent Combat Framework Ultimate".into(),
            version: "4.4".into(),
            number: 44,
            catalog_id: None,
            descriptor: "AscentCombatFramework.uplugin".into(),
        };
        let engine = Installation {
            path: "temporary-test-only".into(),
            version: "5.8.3".into(),
            association: "5.8".into(),
        };
        let item=serde_json::from_value(serde_json::json!({
            "assetId":"22610fb032ac499aa0f9fefe3d95b5e7","assetNamespace":"test","title":"ACF U 4.5",
            "distributionMethod":"CODE_PLUGIN","url":format!("https://www.fab.com/listings/{ID}"),
            "projectVersions":[
                {"artifactId":"older-engine","engineVersions":["UE_5.7"],"targetPlatforms":["Windows"]},
                {"artifactId":"ACFUAsce5ab7c1439afbV5","engineVersions":["UE_5.8"],"targetPlatforms":["Windows"]}
            ]
        })).unwrap();
        (plugin, engine, item)
    }
    fn update() -> Update {
        Update {
            version: "4.5".into(),
            number: 45,
            manifest: Manifest::default(),
            bases: vec![],
            prefix: "ACF".into(),
        }
    }
    #[test]
    fn owned_plugin_with_missing_public_link_still_checks_verified_package() {
        let (plugin, engine, mut item) = fixture();
        item.title = plugin.name.clone();
        item.url = None;
        let remote = serde_json::json!({"MarketplaceURL":format!("com.epicgames.launcher://ue/Fab/product/{ID}")});
        let row = check_fab_plugin(&plugin, &engine, ID, &[item], |_, version, fallback| {
            assert_eq!(version.artifact_id, "ACFUAsce5ab7c1439afbV5");
            assert!(fallback);
            verify_fab_candidate_identity(&plugin, &remote, ID, fallback)?;
            Ok(update())
        });
        assert_eq!(row.status_label(), "Update available");
        assert_eq!(row.available_version.as_deref(), Some("4.5"));
    }
    #[test]
    fn stale_fab_link_fallback_verifies_metadata_and_rejects_ambiguous_products() {
        let dir = tempfile::tempdir().unwrap();
        let (mut plugin, engine, mut item) = fixture();
        plugin.descriptor = dir.path().join("AscentCombatFramework.uplugin");
        let local = serde_json::json!({
            "FriendlyName":plugin.name, "CreatedBy":"Fixture publisher",
            "CreatedByURL":"https://example.com/publisher", "Version":44,"VersionName":"4.4",
            "MarketplaceURL":format!("https://www.fab.com/listings/{ID}"),
            "Modules":[{"Name":"AscentCombatFramework"}]
        });
        fs::write(&plugin.descriptor, serde_json::to_vec(&local).unwrap()).unwrap();
        let original = fs::read(&plugin.descriptor).unwrap();
        let mut remote = local.clone();
        remote["MarketplaceURL"] =
            "https://www.fab.com/listings/a5061972-6875-4052-a7fd-7c33f6531ec3".into();
        remote["Version"] = 45.into();
        remote["VersionName"] = "4.5".into();
        item.title = plugin.name.clone();
        item.url = remote["MarketplaceURL"].as_str().map(str::to_owned);
        let check = |items: &[crate::fab::Item], descriptor: &serde_json::Value| {
            check_fab_plugin(&plugin, &engine, ID, items, |_, _, fallback| {
                assert!(fallback);
                verify_fab_candidate_identity(&plugin, descriptor, ID, fallback)?;
                Ok(update())
            })
        };
        assert!(check(&[item.clone()], &remote).has_update());
        for (field, bad) in [
            ("CreatedBy", serde_json::json!("Other publisher")),
            ("CreatedByURL", serde_json::json!("")),
            ("FriendlyName", serde_json::json!("Other plugin")),
            ("Modules", serde_json::json!([{"Name":"DifferentModule"}])),
            (
                "FabURL",
                serde_json::json!(format!("https://www.fab.com/listings/{ID}")),
            ),
        ] {
            let mut wrong = remote.clone();
            wrong[field] = bad;
            let row = check(&[item.clone()], &wrong);
            assert!(!row.has_update(), "{field}");
            assert!(row.available_version.is_none(), "{field}");
        }
        let mut other = item.clone();
        other.asset_namespace = "another-publisher-namespace".into();
        let ambiguous = check(&[item.clone(), other], &remote);
        assert!(!ambiguous.has_update());
        assert!(ambiguous.status.starts_with("Multiple catalog"));
        let mut wrong_engine = item.clone();
        wrong_engine
            .project_versions
            .iter_mut()
            .for_each(|v| v.engine_versions = vec!["UE_5.6".into()]);
        let mut mac = item.clone();
        mac.project_versions
            .iter_mut()
            .for_each(|v| v.target_platforms = vec!["Mac".into()]);
        for items in [vec![], vec![wrong_engine], vec![mac]] {
            let row = check_fab_plugin(&plugin, &engine, ID, &items, |_, _, _| {
                panic!("Must not fetch unavailable builds")
            });
            assert!(!row.has_update());
            assert!(row.available_version.is_none());
        }
        // An exact listing is authoritative: do not mask a bad package or an
        // incompatible exact listing by switching to another owned product.
        let mut exact = item.clone();
        exact.url = Some(format!("https://www.fab.com/listings/{ID}"));
        let row = check_fab_plugin(
            &plugin,
            &engine,
            ID,
            &[exact.clone(), item.clone()],
            |_, _, fallback| {
                assert!(!fallback);
                verify_fab_candidate_identity(&plugin, &remote, ID, fallback)?;
                Ok(update())
            },
        );
        assert!(!row.has_update());
        exact.project_versions.clear();
        let row = check_fab_plugin(&plugin, &engine, ID, &[exact, item], |_, _, _| {
            panic!("Exact identity must take precedence")
        });
        assert!(!row.has_update());
        assert_eq!(fs::read(&plugin.descriptor).unwrap(), original);
    }
    #[test]
    fn missing_owned_fab_listing_is_unresolved_not_a_request_failure() {
        let (plugin, engine, mut other) = fixture();
        other.url =
            Some("https://www.fab.com/listings/a5061972-6875-4052-a7fd-7c33f6531ec3".into());
        for items in [vec![], vec![other]] {
            let row = check_fab_plugin(&plugin, &engine, ID, &items, |_, _, _| {
                panic!("An unmatched listing must not fetch a package")
            });
            assert_eq!(row.status, "No owned Fab listing matches this plugin");
            assert_eq!(row.status_label(), "No verified Fab match");
            assert!(row.available_version.is_none());
            assert!(!row.has_update());
            assert!(row.status_detail().contains("listing ID"));
        }
    }
    #[test]
    fn fab_only_acf_detects_45_for_58_without_legacy_catalog() {
        let (plugin, engine, item) = fixture();
        let row = check_fab_plugin(&plugin, &engine, ID, &[item], |_, version, fallback| {
            assert!(!fallback);
            assert_eq!(version.artifact_id, "ACFUAsce5ab7c1439afbV5");
            Ok(update())
        });
        assert_eq!(row.status, "Update available");
        assert_eq!(row.update.unwrap().version, "4.5");
        assert_eq!(row.plugin.version, "4.4");
        assert!(row.plugin.catalog_id.is_none());
    }
    #[test]
    fn fab_updates_never_downgrade_and_current_version_is_up_to_date() {
        for (number, name, status) in [
            (
                45,
                "4.5",
                "Publisher version matches; original package build is not recorded",
            ),
            (46, "4.6", "Installed version is newer"),
        ] {
            let (mut plugin, engine, item) = fixture();
            plugin.number = number;
            plugin.version = name.into();
            let row = check_fab_plugin(&plugin, &engine, ID, &[item], |_, _, _| Ok(update()));
            assert_eq!(row.status, status);
            assert!(row.update.is_none());
        }
    }
    #[test]
    fn fab_updates_require_owned_unique_listing_and_exact_windows_engine() {
        let (plugin, mut engine, item) = fixture();
        for items in [vec![], vec![item.clone(), item.clone()]] {
            let row = check_fab_plugin(&plugin, &engine, ID, &items, |_, _, _| {
                panic!("Must not fetch unowned or ambiguous items")
            });
            assert!(row.update.is_none());
        }
        engine.version = "5.6.1".into();
        let row = check_fab_plugin(&plugin, &engine, ID, &[item.clone()], |_, _, _| {
            panic!("Must not fetch a different engine branch")
        });
        assert!(row.update.is_none());
        engine.version = "5.8.3".into();
        let mut mac = item;
        mac.project_versions
            .iter_mut()
            .for_each(|v| v.target_platforms = vec!["Mac".into()]);
        let row = check_fab_plugin(&plugin, &engine, ID, &[mac], |_, _, _| {
            panic!("Must not fetch Mac binaries")
        });
        assert!(row.update.is_none());
    }
    #[test]
    fn downloaded_fab_descriptor_must_confirm_exact_listing_identity() {
        let value =
            serde_json::json!({"MarketplaceURL":format!("https://www.fab.com/listings/{ID}")});
        assert!(verify_fab_identity(&value, ID).is_ok());
        for url in [
            "https://example.com/listings/cc258205-8fcf-41e5-9b48-6ec44e46d7eb",
            "https://www.fab.com/listings/98ff449d-79db-4f54-9303-75486c4fb9d9",
            "",
        ] {
            assert!(verify_fab_identity(&serde_json::json!({"MarketplaceURL":url}), ID).is_err());
        }
    }
}
