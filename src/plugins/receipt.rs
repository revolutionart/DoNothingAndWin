use super::*;
use serde::{Deserialize, Serialize};

const FILE: &str = ".dnw-plugin-install.json";
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Receipt {
    schema: u32,
    pub artifact: String,
    pub build: String,
    descriptor: String,
    descriptor_sha: String,
    #[serde(default)]
    package_hash: String,
}

fn package_hash(manifest: &Manifest) -> String {
    let mut files: Vec<_> = manifest.files.iter().collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    let mut data = Vec::new();
    for file in files {
        data.extend_from_slice(file.name.as_bytes());
        data.push(0);
        data.extend_from_slice(&file.sha);
        data.extend_from_slice(&file.size().to_le_bytes());
    }
    buildpatch::hex(&buildpatch::digest(&data))
}

pub(crate) fn write(stage: &Path, manifest: &Manifest, prefix: &Path) -> Result<(), String> {
    let descriptors: Vec<_> = manifest
        .files
        .iter()
        .filter(|f| Path::new(&f.name).parent() == Some(prefix) && f.name.ends_with(".uplugin"))
        .collect();
    if descriptors.len() != 1 {
        return Err("Cannot record an ambiguous plugin installation".into());
    }
    let file = descriptors[0];
    let descriptor = Path::new(&file.name)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Invalid plugin descriptor name")?;
    if buildpatch::digest(&fs::read(stage.join(descriptor)).map_err(|e| e.to_string())?) != file.sha
    {
        return Err("Installed plugin descriptor does not match its package".into());
    }
    crate::storage::write_json(
        &stage.join(FILE),
        &Receipt {
            schema: 1,
            artifact: manifest.app.clone(),
            build: manifest.build.clone(),
            descriptor: descriptor.into(),
            descriptor_sha: buildpatch::hex(&file.sha),
            package_hash: package_hash(manifest),
        },
    )
}

pub(super) fn read(plugin: &Plugin) -> Option<Receipt> {
    read_own(plugin).or_else(|| {
        let data = std::env::var_os("PROGRAMDATA").map(PathBuf::from)?;
        read_epic(plugin, &data)
    })
}
fn read_own(plugin: &Plugin) -> Option<Receipt> {
    let path = plugin.descriptor.parent()?.join(FILE);
    if fs::metadata(&path).ok()?.len() > 64 * 1024
        || fs::metadata(&plugin.descriptor).ok()?.len() > 2 * 1024 * 1024
    {
        return None;
    }
    let receipt: Receipt = crate::storage::read_json(&path)?;
    if receipt.schema != 1
        || receipt.artifact.is_empty()
        || receipt.build.is_empty()
        || plugin.descriptor.file_name()?.to_str()? != receipt.descriptor
        || buildpatch::hex(&buildpatch::digest(&fs::read(&plugin.descriptor).ok()?))
            != receipt.descriptor_sha
    {
        return None;
    }
    Some(receipt)
}

fn read_epic(plugin: &Plugin, data: &Path) -> Option<Receipt> {
    let engine = plugin
        .descriptor
        .ancestors()
        .find(|root| plugin.descriptor.starts_with(root.join("Engine/Plugins")))?;
    let relative = plugin
        .descriptor
        .strip_prefix(engine)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    if fs::metadata(&plugin.descriptor).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let sha = buildpatch::digest(&fs::read(&plugin.descriptor).ok()?);
    let engine = fs::canonicalize(engine).ok()?;
    let store = fs::canonicalize(engine.join(".egstore")).ok()?;
    let entries = fs::read_dir(data.join("Epic/EpicGamesLauncher/Data/Manifests")).ok()?;
    let mut found: Option<Receipt> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("item")
            || entry.metadata().map_or(true, |m| m.len() > 1024 * 1024)
        {
            continue;
        }
        let Some(item) = crate::storage::read_json::<serde_json::Value>(&path) else {
            continue;
        };
        let Some(app) = item["AppName"]
            .as_str()
            .filter(|s| !s.starts_with("UE_") && !s.is_empty())
        else {
            continue;
        };
        if item["InstallLocation"]
            .as_str()
            .and_then(|p| fs::canonicalize(p).ok())
            .as_ref()
            != Some(&engine)
            || item["ManifestLocation"]
                .as_str()
                .and_then(|p| fs::canonicalize(p).ok())
                .as_ref()
                != Some(&store)
        {
            continue;
        }
        let manifest_path = store.join(path.file_stem()?).with_extension("manifest");
        if fs::metadata(&manifest_path).map_or(true, |m| m.len() > 128 * 1024 * 1024) {
            continue;
        }
        let Ok(raw) = fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = buildpatch::parse(&raw) else {
            continue;
        };
        if manifest.app != app || item["AppVersionString"].as_str() != Some(manifest.build.as_str())
        {
            continue;
        }
        if !manifest
            .files
            .iter()
            .any(|f| f.name.replace('\\', "/").eq_ignore_ascii_case(&relative) && f.sha == sha)
        {
            continue;
        }
        if found
            .as_ref()
            .is_some_and(|r| r.artifact != manifest.app || r.build != manifest.build)
        {
            return None;
        }
        found = Some(Receipt {
            schema: 1,
            package_hash: package_hash(&manifest),
            artifact: manifest.app,
            build: manifest.build,
            descriptor: plugin.descriptor.file_name()?.to_str()?.into(),
            descriptor_sha: buildpatch::hex(&sha),
        });
    }
    found
}

pub(super) fn changed_build(plugin: &Plugin, manifest: &Manifest) -> bool {
    read(plugin).is_some_and(|r| {
        r.artifact == manifest.app
            && !manifest.build.is_empty()
            && (r.build != manifest.build
                || (!r.package_hash.is_empty() && r.package_hash != package_hash(manifest)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipts_bind_build_identity_to_the_installed_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = br#"{"FriendlyName":"Test","Version":1,"VersionName":"1.0"}"#;
        let descriptor = dir.path().join("Test.uplugin");
        fs::write(&descriptor, bytes).unwrap();
        let plugin = Plugin {
            name: "Test".into(),
            version: "1.0".into(),
            number: 1,
            catalog_id: None,
            descriptor,
        };
        let mut manifest = Manifest {
            app: "TestArtifact".into(),
            build: "build-a".into(),
            files: vec![buildpatch::File {
                name: "Test/Test.uplugin".into(),
                sha: buildpatch::digest(bytes),
                ..Default::default()
            }],
            ..Default::default()
        };
        write(dir.path(), &manifest, Path::new("Test")).unwrap();
        assert!(!changed_build(&plugin, &manifest));
        manifest.build = "build-b".into();
        assert!(changed_build(&plugin, &manifest));
        let item: crate::fab::Item = serde_json::from_value(serde_json::json!({
            "assetId":"owned-item", "assetNamespace":"publisher", "title":"Different marketing title", "distributionMethod":"CODE_PLUGIN",
            "projectVersions":[{"artifactId":"TestArtifact","engineVersions":["5.7"],"targetPlatforms":["Windows"]}]
        })).unwrap();
        let engine = Installation {
            path: dir.path().to_string_lossy().into_owned(),
            version: "5.7.4".into(),
            association: String::new(),
        };
        let updated = check_owned_plugin(&plugin, &engine, &[item.clone()], |_, _, _| {
            Ok(Update {
                version: "1.0".into(),
                number: 1,
                manifest: manifest.clone(),
                bases: vec![],
                prefix: PathBuf::from("Test"),
            })
        });
        assert!(
            updated.update.is_some(),
            "A changed build with unchanged publisher version must be detected"
        );
        manifest.build = "build-a".into();
        let current = check_owned_plugin(&plugin, &engine, &[item], |_, _, _| {
            Ok(Update {
                version: "1.0".into(),
                number: 1,
                manifest: manifest.clone(),
                bases: vec![],
                prefix: PathBuf::from("Test"),
            })
        });
        assert_eq!(current.status, "Up to date");
        let original_sha = manifest.files[0].sha;
        manifest.files[0].sha = [7; 20];
        assert!(
            changed_build(&plugin, &manifest),
            "Changed package contents must not be hidden by a reused build ID"
        );
        manifest.files[0].sha = original_sha;
        manifest.build = "build-b".into();
        manifest.app = "OtherArtifact".into();
        assert!(!changed_build(&plugin, &manifest));
        manifest.app = "TestArtifact".into();
        fs::write(&plugin.descriptor, b"{}").unwrap();
        assert!(read(&plugin).is_none());
        assert!(!changed_build(&plugin, &manifest));
    }

    #[test]
    fn epic_registration_must_match_engine_artifact_build_and_descriptor_hash() {
        let dir = tempfile::tempdir().unwrap();
        let engine = dir.path().join("UE_5.7");
        let desc = engine.join("Engine/Plugins/Marketplace/Test/Test.uplugin");
        fs::create_dir_all(desc.parent().unwrap()).unwrap();
        fs::write(&desc, b"{}").unwrap();
        let store = engine.join(".egstore");
        fs::create_dir(&store).unwrap();
        let entries = dir.path().join("Epic/EpicGamesLauncher/Data/Manifests");
        fs::create_dir_all(&entries).unwrap();
        let item = serde_json::json!({"AppName":"TestArtifact","AppVersionString":"build-a","InstallLocation":engine,"ManifestLocation":store});
        crate::storage::write_json(&entries.join("test.item"), &item).unwrap();
        let sha = buildpatch::digest(b"{}")
            .iter()
            .map(|b| format!("{b:03}"))
            .collect::<String>();
        let mut manifest = serde_json::json!({"bIsFileData":false,"AppNameString":"TestArtifact","BuildVersionString":"build-a","ManifestFileVersion":"013000000000","ChunkHashList":{},"FileManifestList":[{"Filename":"Engine/Plugins/Marketplace/Test/Test.uplugin","FileHash":sha,"FileChunkParts":[]}]});
        crate::storage::write_json(&store.join("test.manifest"), &manifest).unwrap();
        let plugin = Plugin {
            name: "Test".into(),
            version: "1.0".into(),
            number: 1,
            catalog_id: None,
            descriptor: desc,
        };
        assert_eq!(read_epic(&plugin, dir.path()).unwrap().build, "build-a");
        manifest["BuildVersionString"] = "wrong-build".into();
        crate::storage::write_json(&store.join("test.manifest"), &manifest).unwrap();
        assert!(read_epic(&plugin, dir.path()).is_none());
        manifest["BuildVersionString"] = "build-a".into();
        crate::storage::write_json(&store.join("test.manifest"), &manifest).unwrap();
        fs::write(&plugin.descriptor, b"{\"Version\":2}").unwrap();
        assert!(read_epic(&plugin, dir.path()).is_none());
    }
}
