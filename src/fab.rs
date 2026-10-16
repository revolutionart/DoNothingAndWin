use crate::{
    buildpatch,
    epic::{Epic, Session},
    installer,
    model::EngineVersion,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub artifact_id: String,
    #[serde(default)]
    pub engine_versions: Vec<String>,
    #[serde(default)]
    pub target_platforms: Vec<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SearchMetadata {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub tags: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub asset_id: String,
    pub asset_namespace: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub seller: Option<String>,
    #[serde(default)]
    pub listing_search: Option<SearchMetadata>,
    #[serde(default)]
    pub distribution_method: String,
    #[serde(default)]
    pub project_versions: Vec<Version>,
    #[serde(default)]
    pub images: Option<Vec<Value>>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub listing_updated_at: Option<i64>,
}
impl Item {
    pub fn key(&self) -> String {
        format!("{}:{}", self.asset_namespace, self.asset_id)
    }
    pub fn plugin(&self) -> bool {
        self.distribution_method == "CODE_PLUGIN"
    }
    pub fn compatible(&self, engine: &str) -> Option<Version> {
        let branch = EngineVersion::parse(engine)?.branch();
        self.project_versions
            .iter()
            .rev()
            .find(|v| {
                (v.target_platforms.is_empty() || v.target_platforms.iter().any(|p| p == "Windows"))
                    && v.engine_versions
                        .iter()
                        .any(|e| EngineVersion::parse(e).is_some_and(|e| e.branch() == branch))
            })
            .cloned()
    }
    /// Explicitly selected package branch for asset imports into other/custom projects.
    /// Plugin binaries must continue to match their installed engine.
    pub fn import_version(
        &self,
        project_engine: &str,
        alternative: &str,
    ) -> Option<(Version, String)> {
        if let Some(version) = self.compatible(project_engine) {
            return Some((version, project_engine.to_owned()));
        }
        if self.plugin() {
            return None;
        }
        if let Some(target) = EngineVersion::parse(project_engine) {
            let source = EngineVersion::parse(alternative)?;
            if (source.major, source.minor) > (target.major, target.minor) {
                return None;
            }
        }
        self.compatible(alternative)
            .map(|v| (v, alternative.to_owned()))
    }

    /// Older asset packages may be tried in newer projects, never the reverse.
    /// Custom engines without a numeric version still require an explicit choice.
    pub fn import_branches(&self, project_engine: &str) -> Vec<String> {
        self.supported_branches()
            .into_iter()
            .filter(|branch| {
                match (
                    EngineVersion::parse(project_engine),
                    EngineVersion::parse(branch),
                ) {
                    (Some(target), Some(source)) => {
                        (source.major, source.minor) <= (target.major, target.minor)
                    }
                    _ => true,
                }
            })
            .collect()
    }

    pub fn supported_branches(&self) -> Vec<String> {
        let mut branches = std::collections::BTreeSet::new();
        for version in &self.project_versions {
            for raw in &version.engine_versions {
                if let Some(v) = EngineVersion::parse(raw) {
                    if self.compatible(raw).is_some() {
                        branches.insert((v.major, v.minor));
                    }
                }
            }
        }
        branches
            .into_iter()
            .map(|(major, minor)| format!("{major}.{minor}"))
            .collect()
    }

    pub fn thumbnail(&self) -> Option<String> {
        self.images
            .as_ref()?
            .iter()
            .filter_map(|i| {
                let url = i["url"].as_str()?;
                let u = reqwest::Url::parse(url).ok()?;
                if u.scheme() != "https" {
                    return None;
                }
                let width = i["width"]
                    .as_u64()
                    .or_else(|| i["width"].as_str()?.parse().ok())
                    .unwrap_or(0);
                Some((
                    if width >= 240 {
                        width
                    } else {
                        u64::MAX - width
                    },
                    url.to_owned(),
                ))
            })
            .min_by_key(|x| x.0)
            .map(|x| x.1)
    }
}
type RecentLibrary = (String, std::time::Instant, Vec<Item>);
static RECENT_LIBRARY: std::sync::OnceLock<std::sync::Mutex<Option<RecentLibrary>>> =
    std::sync::OnceLock::new();
static LIBRARY_FETCH: std::sync::Mutex<()> = std::sync::Mutex::new(());
const LIBRARY_TTL: std::time::Duration = std::time::Duration::from_secs(300);
fn recent_library(account: &str) -> Option<Vec<Item>> {
    RECENT_LIBRARY
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(id, at, _)| id == account && at.elapsed() < LIBRARY_TTL)
        .map(|(_, _, items)| items.clone())
}
pub fn library_cached(session: &Session, force: bool) -> Result<Vec<Item>, String> {
    if !force {
        if let Some(items) = recent_library(&session.account_id) {
            return Ok(items);
        }
    }
    let _fetch = LIBRARY_FETCH.lock().unwrap();
    if !force {
        if let Some(items) = recent_library(&session.account_id) {
            return Ok(items);
        }
    }
    library(session, |_| {})
}
pub fn library(session: &Session, progress: impl Fn(usize)) -> Result<Vec<Item>, String> {
    library_pages(session, |items| progress(items.len()))
}
/// Publish each page so a first-time library can be browsed before pagination finishes.
pub fn library_pages(session: &Session, progress: impl Fn(&[Item])) -> Result<Vec<Item>, String> {
    let epic = Epic::new()?;
    let mut cursor = String::new();
    let mut seen = HashSet::new();
    let mut items: BTreeMap<String, Item> = BTreeMap::new();
    loop {
        let mut url = reqwest::Url::parse(&format!(
            "https://www.fab.com/e/accounts/{}/ue/library",
            session.account_id
        ))
        .map_err(|e| e.to_string())?;
        url.query_pairs_mut().append_pair("count", "1000");
        if !cursor.is_empty() {
            url.query_pairs_mut().append_pair("cursor", &cursor);
        }
        let page = epic.fab_request(
            session,
            &format!(
                "{}?{}",
                url.path().trim_start_matches("/e/"),
                url.query().unwrap_or("")
            ),
            None,
        )?;
        for value in page["results"]
            .as_array()
            .ok_or("Invalid Fab library response")?
        {
            let item: Item =
                serde_json::from_value(value.clone()).map_err(|_| "Invalid Fab item")?;
            if item.distribution_method == "ENGINE" {
                continue;
            }
            if let Some(old) = items.get_mut(&item.key()) {
                if old.url.is_none() {
                    old.url = item.url;
                }
                if old.description.is_none() {
                    old.description = item.description;
                }
                if old.seller.is_none() {
                    old.seller = item.seller;
                }
                for v in item.project_versions {
                    if let Some(existing) = old
                        .project_versions
                        .iter_mut()
                        .find(|x| x.artifact_id == v.artifact_id)
                    {
                        for e in v.engine_versions {
                            if !existing.engine_versions.contains(&e) {
                                existing.engine_versions.push(e);
                            }
                        }
                    } else {
                        old.project_versions.push(v);
                    }
                }
            } else {
                items.insert(item.key(), item);
            }
        }
        progress(&items.values().cloned().collect::<Vec<_>>());
        cursor = page["cursors"]["next"].as_str().unwrap_or("").to_owned();
        if cursor.is_empty() {
            break;
        }
        if !seen.insert(cursor.clone()) {
            return Err("Fab repeated a library page".into());
        }
    }
    let mut result: Vec<_> = items.into_values().collect();
    result.sort_by_key(|x| x.title.to_lowercase());
    *RECENT_LIBRARY.get_or_init(Default::default).lock().unwrap() = Some((
        session.account_id.clone(),
        std::time::Instant::now(),
        result.clone(),
    ));
    Ok(result)
}
fn validate_artifact_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || matches!(id, "." | "..")
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
    {
        return Err("Invalid artifact ID".into());
    }
    Ok(())
}
#[cfg(test)]
mod artifact_id_tests {
    use super::*;
    #[test]
    fn accepts_fab_versioned_ids_but_rejects_url_path_injection() {
        for id in [
            "QuixelBridge_5.8",
            "Prefabricator_4.21",
            "AIClaudu022329952b3cV3",
        ] {
            assert!(validate_artifact_id(id).is_ok());
        }
        for id in [
            "", ".", "..", "../other", "a/b", "a\\b", "%2e%2e", "a?b", "a#b",
        ] {
            assert!(validate_artifact_id(id).is_err());
        }
    }
}
pub fn package(
    session: &Session,
    item: &Item,
    version: &Version,
) -> Result<(buildpatch::Manifest, Vec<reqwest::Url>), String> {
    validate_artifact_id(&version.artifact_id)?;
    let info = Epic::new()?.fab_request(session, &format!("artifacts/{}/manifest",version.artifact_id), Some(&serde_json::json!({"item_id":item.asset_id,"namespace":item.asset_namespace,"platform":"Windows"})))?;
    let info = info["downloadInfo"]
        .as_array()
        .and_then(|a| a.first())
        .ok_or("Fab supplied no download")?;
    let expected = info["manifestHash"]
        .as_str()
        .ok_or("Missing manifest hash")?;
    let mut bases = Vec::new();
    let mut manifest = None;
    let client = installer::client()?;
    for point in info["distributionPoints"]
        .as_array()
        .ok_or("Missing download locations")?
    {
        let url = reqwest::Url::parse(
            point["manifestUrl"]
                .as_str()
                .ok_or("Invalid manifest URL")?,
        )
        .map_err(|_| "Invalid URL")?;
        if url.scheme() != "https" {
            continue;
        }
        let mut base = url.join(".").map_err(|_| "Invalid CDN directory")?;
        base.set_query(url.query());
        bases.push(base);
        if manifest.is_none() {
            if let Ok(raw) = installer::fetch(&client, url, 128 * 1024 * 1024) {
                let hash = buildpatch::digest(&raw)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                if hash.eq_ignore_ascii_case(expected) {
                    manifest = Some(buildpatch::parse(&raw)?);
                }
            }
        }
    }
    Ok((
        manifest.ok_or("Could not download a verified Fab manifest")?,
        bases,
    ))
}

// Only content is imported into an existing project; never its configuration or source.
fn content_relative(name: &str) -> Result<Option<PathBuf>, String> {
    let path = installer::relative(name)?;
    let parts: Vec<_> = path.iter().collect();
    Ok(parts
        .iter()
        .position(|p| p.to_string_lossy().eq_ignore_ascii_case("Content"))
        .filter(|i| i + 1 < parts.len())
        .map(|i| parts[i..].iter().collect()))
}
pub fn install(
    session: &Session,
    item: &Item,
    version: &Version,
    target: &Path,
    engine_version: &str,
    progress: impl Fn(String),
) -> Result<String, String> {
    crate::download_policy::check_install_allowed()?;
    if item.compatible(engine_version).is_none()
        || !version.engine_versions.iter().any(|e| {
            EngineVersion::parse(e)
                .zip(EngineVersion::parse(engine_version))
                .is_some_and(|(a, b)| a.branch() == b.branch())
        })
    {
        return Err("No compatible build for this engine".into());
    }
    installer::no_links(target)?;
    let target = fs::canonicalize(target).map_err(|e| e.to_string())?;
    let _lock = crate::maintenance::lock(&target)?;
    if item.plugin() {
        crate::maintenance::ensure_closed(&target)?;
    }
    progress("Reading verified download manifest...".into());
    let (manifest, bases) = package(session, item, version)?;
    let prefix = if item.plugin() {
        let descriptors: Vec<_> = manifest
            .files
            .iter()
            .filter(|f| f.name.ends_with(".uplugin"))
            .collect();
        if descriptors.len() != 1 {
            return Err("Package must contain exactly one plugin".into());
        }
        let raw = crate::plugins::payload(&manifest, &bases, descriptors[0], &mut HashMap::new())?;
        let descriptor: Value =
            serde_json::from_slice(&raw).map_err(|_| "Invalid plugin descriptor")?;
        if let Some(required) = descriptor["EngineVersion"].as_str() {
            if !EngineVersion::parse(required)
                .zip(EngineVersion::parse(engine_version))
                .is_some_and(|(a, b)| a.branch() == b.branch())
            {
                return Err("Plugin descriptor does not match the selected engine".into());
            }
        }
        installer::relative(&descriptors[0].name)?
            .parent()
            .ok_or("Invalid plugin root")?
            .to_owned()
    } else {
        PathBuf::new()
    };
    let destination = if item.plugin() {
        let name = prefix
            .file_name()
            .ok_or("Plugin package has no root folder")?;
        target.join("Engine/Plugins/Marketplace").join(name)
    } else {
        target.clone()
    };
    if item.plugin() && destination.exists() {
        return Err("Plugin already exists. Use Manage plugins to update it.".into());
    }
    let mut files = Vec::new();
    let mut names = HashSet::new();
    for file in &manifest.files {
        let relative = if item.plugin() {
            installer::relative(&file.name)?
                .strip_prefix(&prefix)
                .ok()
                .map(Path::to_owned)
        } else {
            content_relative(&file.name)?
        };
        if let Some(relative) = relative {
            if relative.as_os_str().is_empty() {
                continue;
            }
            if !names.insert(relative.to_string_lossy().to_lowercase()) {
                return Err("Duplicate package path".into());
            }
            let dest = destination.join(&relative);
            installer::no_links(&dest)?;
            if dest.exists() {
                return Err(format!(
                    "Existing file would be overwritten: {}. Choose another project.",
                    dest.display()
                ));
            }
            files.push((file, relative));
        }
    }
    if files.is_empty() {
        return Err("Package contains no importable content".into());
    }
    let bytes: u64 = files.iter().map(|(f, _)| f.size()).sum();
    if fs2::available_space(&target).map_err(|e| e.to_string())? < bytes + 64 * 1024 * 1024 {
        return Err("Not enough disk space".into());
    }
    let stage = target.join(format!(
        ".engine-workspace-fab-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut cache = HashMap::new();
        let mut prepared = 0u64;
        let mut downloaded = 0u64;
        for (i, (file, relative)) in files.iter().enumerate() {
            progress(format!("Downloading {} / {} files", i + 1, files.len()));
            let path = stage.join(relative);
            fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut file_downloaded = 0;
            let mut last_report = std::time::Instant::now();
            crate::plugins::payload_to_file_progress(
                &manifest,
                &bases,
                file,
                &mut cache,
                &path,
                &mut |written, received| {
                    file_downloaded = received;
                    if last_report.elapsed().as_millis() >= 150 || written == file.size() {
                        progress(transfer_status(
                            i + 1,
                            files.len(),
                            prepared + written,
                            bytes,
                            downloaded + received,
                        ));
                        last_report = std::time::Instant::now();
                    }
                },
            )?;
            prepared += file.size();
            downloaded += file_downloaded;
        }
        progress("Installing verified files...".into());
        if item.plugin() {
            crate::maintenance::ensure_closed(&target)?;
            crate::plugins::receipt::write(&stage, &manifest, &prefix)?;
            installer::no_links(&destination)?;
            if destination.exists() {
                return Err("Plugin destination appeared during download".into());
            }
            fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::rename(&stage, &destination).map_err(|e| e.to_string())?;
        } else {
            let mut created = Vec::new();
            let commit = (|| {
                for (_, relative) in &files {
                    let dest = destination.join(relative);
                    installer::no_links(&dest)?;
                    fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
                    let mut out = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&dest)
                        .map_err(|e| e.to_string())?;
                    created.push(dest);
                    let mut source =
                        fs::File::open(stage.join(relative)).map_err(|e| e.to_string())?;
                    std::io::copy(&mut source, &mut out).map_err(|e| e.to_string())?;
                }
                Ok::<(), String>(())
            })();
            if let Err(e) = commit {
                for p in created {
                    let _ = fs::remove_file(p);
                }
                return Err(e);
            }
        }
        Ok(format!("Installed {} ({} files)", item.title, files.len()))
    })();
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_content_only_and_rejects_traversal() {
        assert_eq!(
            content_relative("Pack/Content/Meshes/Tree.uasset").unwrap(),
            Some(PathBuf::from("Content/Meshes/Tree.uasset"))
        );
        assert!(
            content_relative("Pack/Config/DefaultEngine.ini")
                .unwrap()
                .is_none()
        );
        assert!(content_relative("../Content/escape.uasset").is_err());
    }
    #[test]
    fn asset_import_requires_explicit_supported_alternative() {
        let mut item: Item = serde_json::from_value(serde_json::json!({"assetId":"test","assetNamespace":"fab","title":"Test","projectVersions":[{"artifactId":"test57","engineVersions":["UE_5.7"],"targetPlatforms":["Windows"]},{"artifactId":"mac58","engineVersions":["UE_5.8"],"targetPlatforms":["Mac"]}]})).unwrap();
        assert!(item.import_version("Custom", "").is_none());
        assert!(item.import_version("Custom", "5.8").is_none());
        let (version, branch) = item.import_version("Custom", "5.7").unwrap();
        assert_eq!(version.artifact_id, "test57");
        assert_eq!(branch, "5.7");
        assert_eq!(item.supported_branches(), vec!["5.7"]);
        assert!(item.import_version("5.8", "5.7").is_some());
        item.distribution_method = "CODE_PLUGIN".into();
        assert!(item.import_version("5.8", "5.7").is_none());
    }

    #[test]
    fn asset_import_rejects_newer_packages_and_filters_options() {
        let item: Item = serde_json::from_value(serde_json::json!({"assetId":"test","assetNamespace":"fab","title":"Test","projectVersions":[{"artifactId":"test51","engineVersions":["UE_5.1"],"targetPlatforms":["Windows"]},{"artifactId":"test53","engineVersions":["UE_5.3"],"targetPlatforms":["Windows"]}]})).unwrap();
        assert!(item.import_branches("4.26").is_empty());
        assert!(item.import_version("4.26", "5.1").is_none());
        assert!(item.import_version("5.0", "5.1").is_none());
        assert!(item.import_version("5.2", "5.3").is_none());
        assert_eq!(item.import_branches("5.2"), vec!["5.1"]);
        assert!(item.import_version("5.2", "5.1").is_some());
        assert!(item.import_version("5.1.4", "").is_some());
        assert_eq!(item.import_branches("5.10"), vec!["5.1", "5.3"]);
    }

    #[test]
    fn compatibility_requires_exact_engine_branch() {
        let item: Item = serde_json::from_value(serde_json::json!({"assetId":"test","assetNamespace":"fab","title":"Test","projectVersions":[{"artifactId":"test57","engineVersions":["UE_5.7"],"targetPlatforms":["Windows"]}]})).unwrap();
        assert!(item.compatible("5.7.4").is_some());
        assert!(item.compatible("5.8.0").is_none());
    }
}

#[derive(Serialize, Deserialize)]
struct TemplateReceipt {
    item: String,
    branch: String,
    build: String,
}
fn template_receipt_path(item: &Item, branch: &str) -> PathBuf {
    crate::storage::data_dir()
        .join("fab-templates")
        .join(format!(
            "{}.json",
            buildpatch::digest(format!("{}:{branch}", item.key()).as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ))
}
/// Compare the same package branch with the last template successfully downloaded.
/// Working project files are never used as a cache or updated in place.
pub fn project_updates(session: &Session, items: &[Item]) -> Vec<(String, String)> {
    let mut updates = Vec::new();
    for item in items
        .iter()
        .filter(|i| i.distribution_method == "COMPLETE_PROJECT")
    {
        for branch in item.supported_branches() {
            let Some(receipt) =
                crate::storage::read_json::<TemplateReceipt>(&template_receipt_path(item, &branch))
            else {
                continue;
            };
            if receipt.item != item.key() || receipt.branch != branch {
                continue;
            }
            let Some(version) = item.compatible(&branch) else {
                continue;
            };
            if let Ok((manifest, _)) = package(session, item, &version) {
                if manifest.build != receipt.build {
                    updates.push((item.key(), format!("Updated template available for UE {branch}. Create a new project to use it; existing projects stay unchanged.")));
                }
            }
        }
    }
    updates
}
fn template_files(
    manifest: &buildpatch::Manifest,
) -> Result<Vec<(&buildpatch::File, PathBuf)>, String> {
    let descriptors: Vec<_> = manifest
        .files
        .iter()
        .filter(|f| f.name.to_ascii_lowercase().ends_with(".uproject"))
        .collect();
    if descriptors.len() != 1 {
        return Err("Template must contain exactly one project descriptor".into());
    }
    let descriptor = installer::relative(&descriptors[0].name)?;
    let root = descriptor.parent().ok_or("Invalid project root")?;
    let mut result = Vec::new();
    let mut names = HashSet::new();
    for file in &manifest.files {
        let path = installer::relative(&file.name)?;
        if let Ok(relative) = path.strip_prefix(root) {
            if relative.as_os_str().is_empty() {
                continue;
            }
            if !names.insert(relative.to_string_lossy().to_lowercase()) {
                return Err("Duplicate template path".into());
            }
            result.push((file, relative.to_owned()));
        }
    }
    Ok(result)
}
pub fn create_project(
    session: &Session,
    item: &Item,
    version: &Version,
    parent: &Path,
    name: &str,
    branch: &str,
    progress: impl Fn(String),
) -> Result<String, String> {
    crate::download_policy::check_install_allowed()?;
    if item.distribution_method != "COMPLETE_PROJECT" {
        return Err("Not a project template".into());
    }
    if name.is_empty()
        || name.len() > 64
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("Use 1-64 letters, digits or underscores for the project folder.".into());
    }
    if item
        .compatible(branch)
        .is_none_or(|v| v.artifact_id != version.artifact_id)
    {
        return Err("Choose a supported template version".into());
    }
    installer::no_links(parent)?;
    let parent = fs::canonicalize(parent).map_err(|e| e.to_string())?;
    let destination = parent.join(name);
    if destination.exists() {
        return Err("Destination already exists. Choose a new folder name.".into());
    }
    progress("Reading project template manifest...".into());
    let (manifest, bases) = package(session, item, version)?;
    let files = template_files(&manifest)?;
    let bytes: u64 = files.iter().map(|(f, _)| f.size()).sum();
    if fs2::available_space(&parent).map_err(|e| e.to_string())?
        < bytes.saturating_add(64 * 1024 * 1024)
    {
        return Err("Not enough disk space".into());
    }
    // Reserve the destination exclusively. Other folders, including existing projects, are untouched.
    fs::create_dir(&destination).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut cache = HashMap::new();
        let mut prepared = 0u64;
        let mut downloaded = 0u64;
        let mut project = None;
        for (i, (file, relative)) in files.iter().enumerate() {
            progress(format!(
                "Downloading project file {} / {}",
                i + 1,
                files.len()
            ));
            let path = destination.join(relative);
            installer::no_links(&path)?;
            fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut file_downloaded = 0;
            let mut last_report = std::time::Instant::now();
            crate::plugins::payload_to_file_progress(
                &manifest,
                &bases,
                file,
                &mut cache,
                &path,
                &mut |written, received| {
                    file_downloaded = received;
                    if last_report.elapsed().as_millis() >= 150 || written == file.size() {
                        progress(transfer_status(
                            i + 1,
                            files.len(),
                            prepared + written,
                            bytes,
                            downloaded + received,
                        ));
                        last_report = std::time::Instant::now();
                    }
                },
            )?;
            prepared += file.size();
            downloaded += file_downloaded;
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("uproject"))
            {
                project = Some(path);
            }
        }
        let project = project.ok_or("No project descriptor installed")?;
        // Keep the publisher's descriptor/module names, including C++ module references.
        crate::platform::import_project(&project)?;
        let receipt = TemplateReceipt {
            item: item.key(),
            branch: branch.into(),
            build: manifest.build.clone(),
        };
        let _ = crate::storage::write_json(&template_receipt_path(item, branch), &receipt);
        Ok(format!("Created project: {}", project.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&destination);
    }
    result
}

#[cfg(test)]
mod template_tests {
    use super::*;
    #[test]
    fn template_keeps_config_source_and_descriptor() {
        let mut m = buildpatch::Manifest::default();
        m.files = [
            "Pack/Example.uproject",
            "Pack/Config/DefaultEngine.ini",
            "Pack/Source/Example.cpp",
            "Pack/Content/Map.umap",
        ]
        .iter()
        .map(|name| buildpatch::File {
            name: (*name).into(),
            ..Default::default()
        })
        .collect();
        let paths: Vec<_> = template_files(&m)
            .unwrap()
            .into_iter()
            .map(|(_, p)| p)
            .collect();
        assert!(paths.contains(&PathBuf::from("Example.uproject")));
        assert!(paths.contains(&PathBuf::from("Source/Example.cpp")));
        assert!(paths.contains(&PathBuf::from("Config/DefaultEngine.ini")));
        m.files.push(buildpatch::File {
            name: "Pack/../escape".into(),
            ..Default::default()
        });
        assert!(template_files(&m).is_err());
    }
    #[test]
    fn ambiguous_templates_are_rejected() {
        let mut m = buildpatch::Manifest::default();
        assert!(template_files(&m).is_err());
        m.files = ["A.uproject", "B.uproject"]
            .iter()
            .map(|n| buildpatch::File {
                name: (*n).into(),
                ..Default::default()
            })
            .collect();
        assert!(template_files(&m).is_err());
    }
}

fn cache_key(value: &str) -> String {
    buildpatch::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn load_library_cache(account: &str) -> Vec<Item> {
    if account.is_empty() {
        return Vec::new();
    }
    crate::storage::read_json(
        &crate::storage::data_dir()
            .join("library")
            .join(format!("{}.json", cache_key(account))),
    )
    .unwrap_or_default()
}
pub fn save_library_cache(account: &str, items: &[Item]) -> Result<(), String> {
    if account.is_empty() {
        return Err("Missing account identity".into());
    }
    crate::storage::write_json(
        &crate::storage::data_dir()
            .join("library")
            .join(format!("{}.json", cache_key(account))),
        &items,
    )
}
pub fn thumbnail_is_cached(url: &str) -> bool {
    crate::storage::data_dir()
        .join("library/thumbnails")
        .join(cache_key(url))
        .is_file()
}
pub fn thumbnail_image(url: &str) -> Result<image::DynamicImage, String> {
    let path = crate::storage::data_dir()
        .join("library/thumbnails")
        .join(cache_key(url));
    if let Ok(metadata) = fs::metadata(&path) {
        if metadata.len() <= 8 * 1024 * 1024 {
            if let Ok(raw) = fs::read(&path) {
                if let Ok(image) = image::load_from_memory(&raw) {
                    return Ok(image);
                }
            }
        }
    }
    let url = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    if url.scheme() != "https" {
        return Err("Thumbnail must use HTTPS".into());
    }
    static CLIENT: std::sync::OnceLock<Result<reqwest::blocking::Client, String>> =
        std::sync::OnceLock::new();
    let client = CLIENT
        .get_or_init(installer::client)
        .as_ref()
        .map_err(Clone::clone)?;
    let raw = installer::fetch(client, url, 8 * 1024 * 1024)?;
    let image = image::load_from_memory(&raw).map_err(|e| e.to_string())?;
    let _ = crate::storage::write_atomic(&path, &raw);
    Ok(image)
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    #[ignore = "Read-only live Fab timing probe; requires a connected Epic account"]
    fn library_latency_probe() {
        let session = crate::storage::load_session().expect("Connect Epic first");
        let start = std::time::Instant::now();
        let saved = load_library_cache(&session.account_id);
        println!(
            "Saved library: {} items in {:?}",
            saved.len(),
            start.elapsed()
        );
        let start = std::time::Instant::now();
        let items = library_pages(&session, |page| {
            println!(
                "Library page: {} items at {:?}",
                page.len(),
                start.elapsed()
            )
        })
        .unwrap();
        println!("Catalog complete in {:?}", start.elapsed());
        let start = std::time::Instant::now();
        let mut failures = 0;
        for url in items.iter().take(8).filter_map(Item::thumbnail) {
            if thumbnail_image(&url).is_err() {
                failures += 1;
            }
        }
        println!(
            "First 8 thumbnail reads/decodes (sequential): {:?}; {} failures",
            start.elapsed(),
            failures
        );
    }
    #[test]
    fn library_cache_is_account_scoped_and_round_trips() {
        let d = tempfile::tempdir().unwrap();
        let account = format!("cache-test:{}", d.path().display());
        let item:Item=serde_json::from_value(serde_json::json!({"assetId":"a","assetNamespace":"n","title":"Cached project","distributionMethod":"COMPLETE_PROJECT"})).unwrap();
        save_library_cache(&account, &[item]).unwrap();
        assert_eq!(load_library_cache(&account)[0].title, "Cached project");
        assert!(load_library_cache(&(account.clone() + "other")).is_empty());
        assert!(load_library_cache("").is_empty());
        std::fs::remove_file(
            crate::storage::data_dir()
                .join("library")
                .join(format!("{}.json", cache_key(&account))),
        )
        .unwrap();
    }
}

fn transfer_status(
    file: usize,
    files: usize,
    prepared: u64,
    total: u64,
    downloaded: u64,
) -> String {
    let mib = 1024. * 1024.;
    let percent = if total == 0 {
        100.
    } else {
        prepared as f64 / total as f64 * 100.
    };
    format!(
        "Downloading file {file} / {files} - {percent:.1}%\n{:.1} / {:.1} MiB prepared • {:.1} MiB downloaded",
        prepared as f64 / mib,
        total as f64 / mib,
        downloaded as f64 / mib
    )
}
