use std::path::{Path, PathBuf};
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
    for entry in entries.flatten() {
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
            visit(root, &path, depth + 1, bundled, out);
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
    epic::{Epic, Session},
    installer::{self, Control},
    model::{EngineVersion, Installation, Release},
};
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
    pub status: String,
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
    // Discovery hint only; candidate() verifies the downloaded descriptor ID.
    app == stem || app.starts_with(&format!("{stem}_"))
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
    let mut written = 0u64;
    let mut downloaded = 0u64;
    let client = installer::client()?;
    use sha1::{Digest, Sha1};
    let mut hash = Sha1::new();
    for p in &file.parts {
        let chunk = m.chunks.get(&p.guid).ok_or("Missing plugin chunk")?;
        if !cache.contains_key(&p.guid) {
            if cache.len() >= 32 || cache.values().map(Vec::len).sum::<usize>() >= 64 * 1024 * 1024
            {
                cache.clear();
            }
            let bytes = if let Some(bytes) = crate::download_policy::cached_chunk(chunk) {
                bytes
            } else {
                let bytes =
                    installer::chunk_data(&client, chunk, bases, m.feature, &Control::default())?;
                downloaded += if chunk.size > 0 {
                    chunk.size
                } else {
                    bytes.len() as u64
                };
                crate::download_policy::cache_chunk(chunk, &bytes);
                bytes
            };
            cache.insert(p.guid.clone(), bytes);
        }
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
        progress(written, downloaded);
    }
    let actual: [u8; 20] = hash.finalize().into();
    if actual != file.sha {
        return Err(format!("File failed hash verification: {}", file.name));
    }
    Ok(())
}
pub(crate) fn payload_to_file(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    path: &Path,
) -> Result<(), String> {
    payload_to_file_progress(m, bases, file, cache, path, &mut |_, _| {})
}
pub(crate) fn payload_to_file_progress(
    m: &Manifest,
    bases: &[reqwest::Url],
    file: &buildpatch::File,
    cache: &mut HashMap<String, Vec<u8>>,
    path: &Path,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(), String> {
    installer::no_links(path)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Creating {}: {e}", path.display()))?;
    let result = stream_payload_progress(m, bases, file, cache, &mut output, progress)
        .and_then(|_| output.sync_all().map_err(|e| e.to_string()));
    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}
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
fn candidate(
    session: &Session,
    asset: &serde_json::Value,
    plugin: &Plugin,
    engine: &Installation,
    legacy: bool,
) -> Result<Update, String> {
    let (manifest, bases) = load_package(session, asset)?;
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
    if legacy {
        verify_legacy_identity(plugin, &v)?;
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
pub fn check(engine: &Installation, session: &Session) -> Result<Vec<Checked>, String> {
    let assets = Epic::new()?.owned_assets(session)?;
    Ok(installed(Path::new(&engine.path))
        .into_iter()
        .map(|plugin| {
            check_plugin(&plugin, engine, &assets, |asset, legacy| {
                candidate(session, asset, &plugin, engine, legacy)
            })
        })
        .collect())
}
fn check_plugin(
    plugin: &Plugin,
    engine: &Installation,
    assets: &[serde_json::Value],
    mut fetch: impl FnMut(&serde_json::Value, bool) -> Result<Update, String>,
) -> Checked {
    let mut item = Checked {
        plugin: plugin.clone(),
        update: None,
        status: "Managed by its publisher".into(),
    };
    let Some(id) = &plugin.catalog_id else {
        return item;
    };
    // An existing catalog identity takes precedence, even if it has no build
    // for this engine. Fallback is only for IDs absent from the owned catalog.
    let legacy = !assets.iter().any(|asset| same_catalog(asset, id));
    item.status = "No entitled compatible build".into();
    let mut verified_catalogs = HashSet::new();
    let mut best: Option<Update> = None;
    for asset in assets.iter().filter(|a| {
        compatible(a["buildVersion"].as_str().unwrap_or(""), &engine.version)
            && if legacy {
                legacy_app_hint(a, plugin)
            } else {
                same_catalog(a, id)
            }
    }) {
        match fetch(asset, legacy) {
            Ok(update) => {
                verified_catalogs.insert(
                    asset["catalogItemId"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase(),
                );
                if best.as_ref().is_none_or(|u| update.number > u.number) {
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
        item.status = if update.number > plugin.number {
            item.update = Some(update);
            "Update available"
        } else if update.number == plugin.number {
            "Up to date"
        } else {
            "Installed version is newer"
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
    if actual.number != plugin.number {
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
    delete_scoped_tree(&root,&eng.join("Engine/Plugins"))
        .map_err(|e|format!("Plugin removal failed: {e}. Some files may already have been removed; reinstall the plugin if needed."))?;
    Ok(format!("Removed {} from this engine.", plugin.name))
}
pub fn update_plugin(
    engine: &Installation,
    plugin: &Plugin,
    update: &Update,
) -> Result<String, String> {
    crate::download_policy::check_install_allowed()?;
    let root = plugin_root(engine, plugin)?;
    let eng = fs::canonicalize(&engine.path).map_err(|e| e.to_string())?;
    let _lock = crate::maintenance::lock(&eng)?;
    crate::maintenance::ensure_closed(&eng)?;
    if update.number <= plugin.number {
        return Err("This is not a newer plugin version".into());
    }
    let mut names = std::collections::HashSet::new();
    let mut files = vec![];
    for f in &update.manifest.files {
        let name = installer::relative(&f.name)?;
        if let Ok(relative) = name.strip_prefix(&update.prefix) {
            if relative.as_os_str().is_empty() {
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
    if fs2::available_space(&eng).map_err(|e| e.to_string())? < bytes + 64 * 1024 * 1024 {
        return Err("Not enough space for plugin update".into());
    }
    let stage = backup_path(engine, &root)?.with_extension("staging");
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let mut cache = HashMap::new();
    for (f, relative) in files {
        let dest = stage.join(relative);
        installer::no_links(&dest)?;
        fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        payload_to_file(&update.manifest, &update.bases, f, &mut cache, &dest)?;
    }
    activate(
        engine,
        plugin,
        &root,
        &stage,
        update.number,
        &update.version,
    )
}
fn activate(
    engine: &Installation,
    plugin: &Plugin,
    root: &Path,
    stage: &Path,
    number: i64,
    version: &str,
) -> Result<String, String> {
    let eng = fs::canonicalize(&engine.path).map_err(|e| e.to_string())?;
    let desc = stage.join(plugin.descriptor.file_name().ok_or("Missing descriptor")?);
    let v: serde_json::Value = serde_json::from_slice(&fs::read(desc).map_err(|e| e.to_string())?)
        .map_err(|_| "Invalid staged descriptor")?;
    if v["Version"].as_i64() != Some(number)
        || !compatible(v["EngineVersion"].as_str().unwrap_or(""), &engine.version)
    {
        return Err("Staged plugin version/compatibility check failed".into());
    }
    plugin_root(engine, plugin)?;
    crate::maintenance::ensure_closed(&eng)?;
    let backup = backup_path(engine, &root)?;
    fs::rename(&root, &backup).map_err(|e| e.to_string())?;
    if let Err(e) = fs::rename(&stage, &root) {
        let rollback = fs::rename(&backup, &root);
        return Err(format!(
            "Update activation failed: {e}; restored old plugin: {}. Backup: {}",
            rollback.is_ok(),
            backup.display()
        ));
    }
    if let Err(error) = delete_scoped_tree(&backup, &eng.join(".engine-workspace-plugin-backups")) {
        return Ok(format!(
            "Updated {} to {}. Temporary rollback cleanup failed: {error}. Remaining files: {}",
            plugin.name,
            version,
            backup.display()
        ));
    }
    // Remove the container only when empty; never touch backups from earlier operations.
    let _ = fs::remove_dir(eng.join(".engine-workspace-plugin-backups"));
    Ok(format!("Updated {} to {}.", plugin.name, version))
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
    fn update_activation_cleans_rollback_and_rejects_other_engine() {
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
        assert_eq!(installed(Path::new(&e.path))[0].number, 2);
        assert!(!root.join("custom.txt").exists());
        assert!(
            !Path::new(&e.path)
                .join(".engine-workspace-plugin-backups")
                .exists()
        );
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
                |_, _| panic!("must not fetch")
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
    fn repeated_builds_choose_highest_version_without_downgrading() {
        let (mut p, e) = fixture();
        let a = [
            asset(CURRENT, "PivotTool_one", "5.4.0"),
            asset(CURRENT, "PivotTool_two", "5.4.0"),
        ];
        for (installed, status) in [
            (1, "Update available"),
            (3, "Up to date"),
            (4, "Installed version is newer"),
        ] {
            p.number = installed;
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
    fn marketplace_ids_are_case_insensitive_but_not_arbitrary_strings() {
        let (p, _) = fixture();
        assert!(verify_legacy_identity(&p, &descriptor(&OLD.to_uppercase())).is_ok());
        assert!(marketplace_id(&descriptor("PivotTool")).is_none());
        assert!(same_catalog(&asset(&OLD.to_uppercase(), "x", "5.4.0"), OLD));
    }
}
