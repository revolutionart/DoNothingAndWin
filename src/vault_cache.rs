use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub title: String,
    pub bytes: u64,
    pub identities: Vec<String>,
    pub removable: bool,
    pub incomplete: bool,
}
impl Entry {
    pub fn is_metadata(&self) -> bool {
        let name = self
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        name.ends_with(".db")
            || name.ends_with(".db-wal")
            || name.ends_with(".db-shm")
            || name.ends_with(".sqlite")
            || name.ends_with(".sqlite3")
    }
}
#[derive(Default)]
pub struct Inventory {
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
}

/// Enumerate the configured cache, keeping shared chunks separate from packages.
pub fn scan(root: &Path, mut progress: impl FnMut(&Path)) -> Result<Inventory, String> {
    if !root.is_absolute() {
        return Err("Choose an absolute cache folder in Download settings.".into());
    }
    crate::installer::no_links(root)?;
    let mut out = Inventory::default();
    if !root.exists() {
        return Ok(out);
    }
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    enumerate(&root, &root, &mut out, &mut progress)?;
    out.entries
        .sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.title.cmp(&b.title)));
    Ok(out)
}
fn enumerate(
    root: &Path,
    dir: &Path,
    out: &mut Inventory,
    progress: &mut impl FnMut(&Path),
) -> Result<(), String> {
    for child in fs::read_dir(dir).map_err(|e| format!("Cannot read {}: {e}", dir.display()))? {
        let child = match child {
            Ok(c) => c,
            Err(e) => {
                out.warnings.push(e.to_string());
                continue;
            }
        };
        let path = child.path();
        if let Err(e) = crate::installer::no_links(&path) {
            out.warnings
                .push(format!("Skipped {}: {e}", path.display()));
            continue;
        }
        let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
        let key = rel.to_string_lossy().replace('\\', "/").to_lowercase();
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                out.warnings.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        // Containers are not cards: each downloaded plugin/version is one entry.
        let container = meta.is_dir()
            && (matches!(
                key.as_str(),
                "engineworkspace" | "engineworkspace/plugins" | "fablibrary"
            ) || (key.starts_with("engineworkspace/plugins/")
                && rel.components().count() == 3));
        if container {
            if let Err(e) = enumerate(root, &path, out, progress) {
                out.warnings.push(e);
            }
            continue;
        }
        progress(&path);
        let name = child.file_name().to_string_lossy().into_owned();
        let mut entry = Entry {
            path: path.clone(),
            title: if key == "engineworkspace/chunks" {
                "Shared download chunks".into()
            } else {
                name
            },
            bytes: 0,
            identities: vec![],
            removable: meta.is_dir(),
            incomplete: false,
        };
        let before = out.warnings.len();
        measure(&path, 0, &mut entry, &mut out.warnings, progress);
        entry.incomplete = out.warnings.len() != before;
        entry.removable &= !entry.incomplete && !entry.title.starts_with('.');
        out.entries.push(entry);
    }
    Ok(())
}
fn measure(
    path: &Path,
    depth: usize,
    entry: &mut Entry,
    warnings: &mut Vec<String>,
    progress: &mut impl FnMut(&Path),
) {
    if depth > 128 {
        warnings.push(format!("Folder nesting too deep: {}", path.display()));
        return;
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            warnings.push(format!("{}: {e}", path.display()));
            return;
        }
    };
    if linked(&meta) {
        warnings.push(format!("Linked path skipped: {}", path.display()));
        return;
    }
    progress(path);
    if meta.is_file() {
        entry.bytes = entry.bytes.saturating_add(meta.len());
        if depth <= 4 && meta.len() <= 32 * 1024 * 1024 && entry.identities.len() < 100 {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.ends_with(".manifest")
                || name.ends_with(".mancpn")
                || name.ends_with(".uplugin")
                || name == ".dnw-plugin-install.json"
            {
                if let Ok(bytes) = fs::read(path) {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        for key in [
                            "AppName",
                            "CatalogItemId",
                            "CatalogNamespace",
                            "artifact",
                            "AppNameString",
                        ] {
                            if let Some(id) = v[key].as_str().filter(|s| !s.is_empty()) {
                                entry.identities.push(id.into());
                            }
                        }
                        if let Some(title) = v["FriendlyName"].as_str().filter(|s| !s.is_empty()) {
                            entry.title = title.into();
                        }
                    } else if name.ends_with(".manifest") {
                        if let Ok(m) = crate::buildpatch::parse(&bytes) {
                            entry.identities.push(m.app);
                        }
                    }
                }
            }
        }
    } else if meta.is_dir() {
        match fs::read_dir(path) {
            Ok(children) => {
                for child in children {
                    match child {
                        Ok(c) => measure(&c.path(), depth + 1, entry, warnings, progress),
                        Err(e) => warnings.push(format!("{}: {e}", path.display())),
                    }
                }
            }
            Err(e) => warnings.push(format!("{}: {e}", path.display())),
        }
    }
}

fn linked(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// Only allow a selected inventory folder inside the same configured cache.
pub fn remove(root: &Path, entry: &Entry) -> Result<(), String> {
    if !entry.removable {
        return Err("This cache entry cannot be removed here.".into());
    }
    crate::installer::no_links(root)?;
    crate::installer::no_links(&entry.path)?;
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    if root.parent().is_none() {
        return Err("A drive root cannot be used for cache removal.".into());
    }
    let target = fs::canonicalize(&entry.path).map_err(|e| e.to_string())?;
    let relative = target
        .strip_prefix(&root)
        .map_err(|_| "Cache item is outside the configured folder")?;
    let key = relative.to_string_lossy().replace('\\', "/").to_lowercase();
    if key.is_empty()
        || matches!(
            key.as_str(),
            "engineworkspace" | "engineworkspace/plugins" | "fablibrary"
        )
    {
        return Err("Cannot remove the cache root or a cache container.".into());
    }
    // Preflight the whole tree before removing anything; never traverse junctions.
    let mut checked = entry.clone();
    let mut warnings = vec![];
    measure(&target, 0, &mut checked, &mut warnings, &mut |_| {});
    if !warnings.is_empty() {
        return Err(warnings.join("\n"));
    }
    remove_tree(&target)
}
fn remove_tree(path: &Path) -> Result<(), String> {
    crate::installer::no_links(path)?;
    if fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        for child in fs::read_dir(path).map_err(|e| e.to_string())? {
            remove_tree(&child.map_err(|e| e.to_string())?.path())?;
        }
        fs::remove_dir(path).map_err(|e| format!("Cannot remove {}: {e}", path.display()))
    } else {
        fs::remove_file(path).map_err(|e| format!("Cannot remove {}: {e}", path.display()))
    }
}

pub fn match_item<'a>(
    entry: &Entry,
    items: &'a [crate::fab::Item],
) -> Option<&'a crate::fab::Item> {
    let matches: Vec<_> = items
        .iter()
        .filter(|item| {
            entry.identities.iter().any(|id| {
                id == &item.asset_id
                    || item.legacy_item_id.as_ref() == Some(id)
                    || item.project_versions.iter().any(|v| &v.artifact_id == id)
            })
        })
        .collect();
    if matches.len() == 1 {
        return Some(matches[0]);
    }
    if !matches.is_empty() {
        return None;
    }
    // Exact unique titles only; an unmatched folder is still useful to inspect.
    let matches: Vec<_> = items
        .iter()
        .filter(|i| i.title.eq_ignore_ascii_case(&entry.title))
        .collect();
    (matches.len() == 1).then(|| matches[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Reads the locally configured cache; does not change files"]
    fn configured_cache_inventory() {
        let prefs = crate::storage::load_preferences();
        let start = std::time::Instant::now();
        let inventory = scan(Path::new(&prefs.fab_cache), |_| {}).unwrap();
        println!(
            "{} entries, {} bytes, {} skipped paths in {:?}",
            inventory.entries.len(),
            inventory.entries.iter().map(|e| e.bytes).sum::<u64>(),
            inventory.warnings.len(),
            start.elapsed()
        );
        for entry in &inventory.entries {
            println!("{}: {} bytes", entry.title, entry.bytes);
        }
    }
    #[test]
    fn metadata_matches_unique_owned_artifact_and_keeps_unknown_folders() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("opaque-folder")).unwrap();
        fs::write(
            d.path().join("opaque-folder/item.mancpn"),
            r#"{"AppName":"artifact-test"}"#,
        )
        .unwrap();
        let inventory = scan(d.path(), |_| {}).unwrap();
        let item: crate::fab::Item = serde_json::from_value(serde_json::json!({"assetId":"test-id","assetNamespace":"test-ns","title":"My Asset","projectVersions":[{"artifactId":"artifact-test"}]})).unwrap();
        let items = vec![item.clone()];
        assert_eq!(
            match_item(&inventory.entries[0], &items).unwrap().title,
            "My Asset"
        );
        assert!(match_item(&inventory.entries[0], &[]).is_none());
        assert!(match_item(&inventory.entries[0], &[item.clone(), item]).is_none());
        let other = tempfile::tempdir().unwrap();
        assert!(scan(other.path(), |_| {}).unwrap().entries.is_empty());
        assert!(scan(Path::new("relative/path"), |_| {}).is_err());
    }
    #[test]
    fn grouped_inventory_and_safe_removal() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        for (path, bytes) in [
            ("EpicAsset/data/file", 8),
            ("EngineWorkspace/Chunks/hash", 12),
            ("EngineWorkspace/Plugins/Test/UE_5.8/Test/data", 17),
            ("FabLibrary/listings_v1.db", 4),
        ] {
            let p = root.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, vec![0; bytes]).unwrap();
        }
        let inv = scan(root, |_| {}).unwrap();
        assert_eq!(inv.entries.len(), 4);
        assert_eq!(inv.entries.iter().map(|e| e.bytes).sum::<u64>(), 41);
        let mut chosen = inv
            .entries
            .iter()
            .find(|e| e.title == "EpicAsset")
            .unwrap()
            .clone();
        chosen.path = root.to_owned();
        assert!(remove(root, &chosen).is_err());
        chosen.path = d.path().parent().unwrap().to_owned();
        assert!(remove(root, &chosen).is_err());
        let chosen = inv.entries.iter().find(|e| e.title == "EpicAsset").unwrap();
        remove(root, chosen).unwrap();
        assert!(!chosen.path.exists());
        assert!(root.join("EngineWorkspace/Chunks/hash").exists());
        assert!(remove(root, inv.entries.iter().find(|e| !e.removable).unwrap()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn links_are_not_measured_or_removed() {
        let d = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("asset")).unwrap();
        std::os::unix::fs::symlink(other.path(), d.path().join("asset/link")).unwrap();
        let inv = scan(d.path(), |_| {}).unwrap();
        assert_eq!(inv.entries[0].bytes, 0);
        assert!(!inv.entries[0].removable);
        assert!(!inv.warnings.is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn windows_junctions_are_skipped_and_block_removal() {
        use std::os::windows::process::CommandExt;
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), b"keep").unwrap();
        let folder = d.path().join("asset");
        fs::create_dir(&folder).unwrap();
        let link = folder.join("junction");
        let output = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(output.status.success(), "Could not create test junction");
        let inventory = scan(d.path(), |_| {}).unwrap();
        assert_eq!(inventory.entries[0].bytes, 0);
        assert!(!inventory.entries[0].removable);
        assert!(!inventory.warnings.is_empty());
        let mut entry = inventory.entries[0].clone();
        entry.removable = true;
        assert!(remove(d.path(), &entry).is_err());
        assert_eq!(fs::read(outside.path().join("keep")).unwrap(), b"keep");
        fs::remove_dir(link).unwrap();
    }
}
