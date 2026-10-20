//! Preserve an exact, already-existing Epic plugin registration, never fabricate one.
use super::*;
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Saved {
    item: Value,
    id: String,
    sha: [u8; 20],
}
fn data() -> Option<PathBuf> {
    std::env::var_os("PROGRAMDATA").map(PathBuf::from)
}
fn matches_engine(item: &Value, engine: &Path) -> bool {
    let Ok(store) = fs::canonicalize(engine.join(".egstore")) else {
        return false;
    };
    item["InstallLocation"]
        .as_str()
        .and_then(|s| fs::canonicalize(s).ok())
        .as_deref()
        == Some(engine)
        && item["ManifestLocation"]
            .as_str()
            .and_then(|s| fs::canonicalize(s).ok())
            .as_deref()
            == Some(store.as_path())
        && item["AppName"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && !s.starts_with("UE_"))
}
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
pub(super) fn capture(
    engine: &Path,
    plugin: &Plugin,
    directory: &Path,
) -> Result<Option<Saved>, String> {
    let Some(data) = data() else { return Ok(None) };
    capture_at(engine, plugin, directory, &data)
}
fn capture_at(
    engine: &Path,
    plugin: &Plugin,
    directory: &Path,
    data: &Path,
) -> Result<Option<Saved>, String> {
    let Some(receipt) = receipt::read(plugin) else {
        return Ok(None);
    };
    let folder = data.join("Epic/EpicGamesLauncher/Data/Manifests");
    let Ok(entries) = fs::read_dir(&folder) else {
        return Ok(None);
    };
    let mut selected = None;
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|s| s != "item")
            || e.metadata().map_or(true, |m| m.len() > 1024 * 1024)
        {
            continue;
        }
        let Some(item) = crate::storage::read_json::<Value>(&path) else {
            continue;
        };
        if !matches_engine(&item, engine)
            || item["AppName"] != receipt.artifact
            || item["AppVersionString"] != receipt.build
            || item["bIsIncompleteInstall"] == true
        {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| valid_id(s))
        else {
            continue;
        };
        let manifest_path = engine.join(".egstore").join(format!("{id}.manifest"));
        installer::no_links(&manifest_path)?;
        if fs::metadata(&manifest_path).map_or(true, |m| m.len() > 128 * 1024 * 1024) {
            continue;
        }
        let Ok(bytes) = fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = buildpatch::parse(&bytes) else {
            continue;
        };
        if manifest.app != receipt.artifact || manifest.build != receipt.build {
            continue;
        }
        if selected.is_some() {
            return Ok(None);
        }
        selected = Some((
            Saved {
                item,
                id: id.into(),
                sha: buildpatch::digest(&bytes),
            },
            bytes,
        ));
    }
    let Some((saved, bytes)) = selected else {
        return Ok(None);
    };
    crate::storage::write_atomic(&directory.join("epic.manifest"), &bytes)?;
    Ok(Some(saved))
}
pub(super) fn restore(
    engine: &Path,
    directory: &Path,
    saved: Option<&Saved>,
    descriptor: &Path,
) -> Result<(), String> {
    let saved = saved.ok_or("No matching Epic plugin registration was available when this backup was created. The plugin files and DoNothing&Win receipt were restored.")?;
    let data = data().ok_or("Epic's installation metadata location is unavailable")?;
    restore_at(engine, directory, saved, &data, descriptor)
}
fn restore_at(
    engine: &Path,
    directory: &Path,
    saved: &Saved,
    data: &Path,
    descriptor: &Path,
) -> Result<(), String> {
    if !valid_id(&saved.id) || !matches_engine(&saved.item, engine) {
        return Err("Saved Epic registration does not match this engine".into());
    }
    let source = directory.join("epic.manifest");
    installer::no_links(&source)?;
    if fs::metadata(&source).map_err(|e| e.to_string())?.len() > 128 * 1024 * 1024 {
        return Err("Saved manifest is too large".into());
    }
    let bytes = fs::read(source).map_err(|e| e.to_string())?;
    if buildpatch::digest(&bytes) != saved.sha {
        return Err("Saved Epic manifest failed verification".into());
    }
    let manifest = buildpatch::parse(&bytes)?;
    if saved.item["AppName"] != manifest.app || saved.item["AppVersionString"] != manifest.build {
        return Err("Saved Epic build identity differs from its manifest".into());
    }
    installer::no_links(descriptor)?;
    let relative = descriptor
        .strip_prefix(engine)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let hash = buildpatch::digest(&fs::read(descriptor).map_err(|e| e.to_string())?);
    if !manifest.files.iter().any(|file| {
        file.name.replace('\\', "/").eq_ignore_ascii_case(&relative) && file.sha == hash
    }) {
        return Err("Saved Epic manifest does not describe the restored plugin".into());
    }
    let folder = data.join("Epic/EpicGamesLauncher/Data/Manifests");
    installer::no_links(&folder)?;
    let mut current = Vec::new();
    for e in fs::read_dir(&folder).map_err(|e| e.to_string())?.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|s| s != "item") {
            continue;
        }
        if let Some(item) = crate::storage::read_json::<Value>(&path) {
            if matches_engine(&item, engine) && item["AppName"] == manifest.app {
                if item["bIsIncompleteInstall"] == true {
                    return Err("Epic has an unfinished installation for this plugin".into());
                }
                current.push((path, item));
            }
        }
    }
    if current.len() > 1 {
        return Err("Multiple Epic registrations match this plugin".into());
    }
    let (item_path, mut item) = current.pop().unwrap_or_else(|| {
        (
            folder.join(format!("{}.item", saved.id)),
            saved.item.clone(),
        )
    });
    let id = item_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| valid_id(s))
        .ok_or("Invalid Epic installation ID")?;
    if item_path.exists() {
        let old: Value =
            crate::storage::read_json(&item_path).ok_or("Unreadable current Epic registration")?;
        if !matches_engine(&old, engine)
            || old["AppName"] != manifest.app
            || old["CatalogItemId"] != saved.item["CatalogItemId"]
        {
            return Err("Epic registration changed; refusing to overwrite another product".into());
        }
    }
    let target = engine.join(".egstore").join(format!("{id}.manifest"));
    installer::no_links(&item_path)?;
    installer::no_links(&target)?;
    let previous = if target.exists() {
        Some(fs::read(&target).map_err(|e| e.to_string())?)
    } else {
        None
    };
    // Keep current installation/path identity, restoring the saved build's manifest metadata.
    for field in [
        "AppVersionString",
        "ManifestHash",
        "InstallSize",
        "InstallTags",
        "BaseURLs",
    ] {
        if let Some(value) = saved.item.get(field) {
            item[field] = value.clone();
        }
    }
    item["InstallationGuid"] = id.into();
    item["ManifestHash"] = buildpatch::hex(&saved.sha).into();
    item["bIsIncompleteInstall"] = false.into();
    item["bNeedsValidation"] = false.into();
    crate::storage::write_atomic(&target, &bytes)?;
    if let Err(error) = crate::storage::write_json(&item_path, &item) {
        let rollback = if let Some(previous) = previous {
            crate::storage::write_atomic(&target, &previous)
        } else {
            fs::remove_file(&target).map_err(|e| e.to_string())
        };
        return Err(format!(
            "{error}; prior manifest restored: {}",
            rollback.is_ok()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_registration_restores_exact_build_and_rejects_foreign_or_corrupt_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let engine = temp.path().join("UE_5.8");
        let root = engine.join("Engine/Plugins/Marketplace/Test");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Test.uplugin"), b"{}").unwrap();
        fs::create_dir_all(engine.join(".egstore")).unwrap();
        let engine = fs::canonicalize(engine).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let sha = buildpatch::digest(b"{}")
            .iter()
            .map(|b| format!("{b:03}"))
            .collect::<String>();
        let json = serde_json::json!({"bIsFileData":false,"AppNameString":"TestArtifact","BuildVersionString":"build-old","ManifestFileVersion":"013000000000","ChunkHashList":{},"FileManifestList":[{"Filename":"Engine/Plugins/Marketplace/Test/Test.uplugin","FileHash":sha,"FileChunkParts":[]}]});
        let bytes = serde_json::to_vec(&json).unwrap();
        let manifest = buildpatch::parse(&bytes).unwrap();
        receipt::write(
            &root,
            &manifest,
            Path::new("Engine/Plugins/Marketplace/Test"),
        )
        .unwrap();
        let plugin = Plugin {
            name: "Test".into(),
            version: "1.0".into(),
            number: 1,
            catalog_id: None,
            descriptor: root.join("Test.uplugin"),
        };
        let data = temp.path().join("data");
        let entries = data.join("Epic/EpicGamesLauncher/Data/Manifests");
        fs::create_dir_all(&entries).unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let item_path = entries.join(format!("{id}.item"));
        let manifest_path = engine.join(".egstore").join(format!("{id}.manifest"));
        let item = serde_json::json!({"AppName":"TestArtifact","AppVersionString":"build-old","CatalogItemId":"owned-plugin","InstallationGuid":id,"InstallLocation":engine,"ManifestLocation":engine.join(".egstore"),"bIsIncompleteInstall":false});
        crate::storage::write_json(&item_path, &item).unwrap();
        fs::write(&manifest_path, &bytes).unwrap();
        let backup = temp.path().join("backup");
        fs::create_dir(&backup).unwrap();
        let saved = capture_at(&engine, &plugin, &backup, &data)
            .unwrap()
            .unwrap();
        let mut newer = item.clone();
        newer["AppVersionString"] = "build-new".into();
        crate::storage::write_json(&item_path, &newer).unwrap();
        fs::write(&manifest_path, b"new manifest").unwrap();
        assert!(
            capture_at(&engine, &plugin, &backup, &data)
                .unwrap()
                .is_none()
        );
        restore_at(&engine, &backup, &saved, &data, &plugin.descriptor).unwrap();
        let restored: Value = crate::storage::read_json(&item_path).unwrap();
        assert_eq!(restored["AppVersionString"], "build-old");
        assert_eq!(
            restored["ManifestHash"],
            buildpatch::hex(&buildpatch::digest(&bytes))
        );
        assert_eq!(fs::read(&manifest_path).unwrap(), bytes);
        newer["AppName"] = "UE_5.8".into();
        crate::storage::write_json(&item_path, &newer).unwrap();
        assert!(
            restore_at(&engine, &backup, &saved, &data, &plugin.descriptor)
                .unwrap_err()
                .contains("another product")
        );
        let unchanged = fs::read(&item_path).unwrap();
        fs::write(backup.join("epic.manifest"), b"corrupt").unwrap();
        assert!(
            restore_at(&engine, &backup, &saved, &data, &plugin.descriptor)
                .unwrap_err()
                .contains("verification")
        );
        assert_eq!(fs::read(&item_path).unwrap(), unchanged);
    }
}
