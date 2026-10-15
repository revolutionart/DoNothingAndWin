use crate::{
    buildpatch::{self, Manifest},
    installer::{self, Plan},
    model::Release,
    storage,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
fn key(build: &str) -> String {
    buildpatch::hex(&buildpatch::digest(build.as_bytes()))
}
fn cache_root(build: &str) -> PathBuf {
    storage::data_dir()
        .join("epic-registration")
        .join(key(build))
}
pub fn cache(release: &Release, manifest: &Manifest, bytes: &[u8]) -> Result<(), String> {
    let root = cache_root(&manifest.build);
    storage::write_atomic(&root.join("original.manifest"), bytes)?;
    storage::write_json(&root.join("release.json"), release)
}
fn material(plan: &Plan) -> Result<(Release, Vec<u8>), String> {
    let root = cache_root(&plan.manifest.build);
    if let (Some(release), Ok(bytes)) = (
        storage::read_json::<Release>(&root.join("release.json")),
        fs::read(root.join("original.manifest")),
    ) {
        if buildpatch::parse(&bytes)
            .is_ok_and(|m| m.build == plan.manifest.build && m.app == plan.manifest.app)
        {
            return Ok((release, bytes));
        }
    }
    let session =
        storage::load_session().ok_or("Connect your Epic account to restore registration")?;
    let snapshot = storage::load_snapshot(&session.account_id);
    let release = snapshot
        .releases
        .iter()
        .find(|r| r.app == plan.manifest.app && r.build.as_ref() == Some(&plan.manifest.build))
        .ok_or("Exact build absent from cached Epic releases; refresh releases first")?;
    let client = installer::client()?;
    for url in crate::epic::Epic::new()?.manifest_locations(&session, release)? {
        if let Ok(bytes) = installer::fetch(&client, url, 128 * 1024 * 1024) {
            if buildpatch::parse(&bytes)
                .is_ok_and(|m| m.build == plan.manifest.build && m.app == plan.manifest.app)
            {
                cache(release, &plan.manifest, &bytes)?;
                return Ok((release.clone(), bytes));
            }
        }
    }
    Err("Epic did not provide the exact installed manifest; engine files were not changed".into())
}
fn program_data() -> PathBuf {
    std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| "C:/ProgramData".into())
}
fn path_key(s: &str) -> String {
    s.trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}
fn read_value(path: &Path, default: Value) -> Result<Value, String> {
    match fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|e| format!("Refusing to replace unreadable Epic metadata: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(e) => Err(e.to_string()),
    }
}
fn backup(path: &Path) -> Result<(), String> {
    if path.exists() {
        let dest = path.with_extension(format!("alpine-backup-{}", crate::epic::now()));
        if !dest.exists() {
            fs::copy(path, dest).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
pub fn register(plan: &Plan) -> Result<(), String> {
    let (release, bytes) = material(plan)?;
    register_at(plan, &release, &bytes, &program_data())
}
/// Remove registration only after the confirmed engine directory is gone.
pub fn unregister_missing(app: &str, target: &Path) -> Result<(), String> {
    unregister_at(app, target, &program_data())
}
fn unregister_at(app: &str, target: &Path, data: &Path) -> Result<(), String> {
    let branch = app.strip_prefix("UE_").ok_or("Not an engine app")?;
    let parts: Vec<_> = branch.split('.').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
        || !target.is_absolute()
    {
        return Err("Expected engine app UE_major.minor and an absolute path".into());
    }
    if target.try_exists().map_err(|e| e.to_string())? {
        return Err("Engine folder still exists; registration was not removed".into());
    }
    let matches = |v: &Value, app_field: &str, path_field: &str| {
        v[app_field] == app
            && v[path_field]
                .as_str()
                .is_some_and(|p| path_key(p) == path_key(&target.to_string_lossy()))
    };
    let installed = data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat");
    let mut list = read_value(&installed, json!({"InstallationList":[]}))?;
    let entries = list["InstallationList"]
        .as_array_mut()
        .ok_or("Invalid Epic installation list")?;
    let before = entries.len();
    entries.retain(|v| !matches(v, "AppName", "InstallLocation"));
    let changed = entries.len() != before;
    let eosh = data.join("Epic/EpicOnlineServicesShared/InstallHelper/InstalledItems");
    let manifests = data.join("Epic/EpicGamesLauncher/Data/Manifests");
    let mut remove = vec![];
    let mut eosh_changed = false;
    for (directory, extension) in [
        (&manifests, "item"),
        (&manifests.join("Pending"), "item"),
        (&eosh, "egi"),
    ] {
        if !directory.try_exists().map_err(|e| e.to_string())? {
            continue;
        }
        installer::no_links(directory)?;
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if !path.extension().is_some_and(|e| e == extension) {
                continue;
            }
            installer::no_links(&path)?;
            let record = read_value(&path, Value::Null)?;
            let matched = if extension == "egi" {
                let matched = matches(&record["v4"], "artifactId", "dir");
                if matched && record["v4"]["state"] != "Installed" {
                    return Err("Epic has an operation in progress for this engine".into());
                }
                eosh_changed |= matched;
                matched
            } else {
                matches(&record, "AppName", "InstallLocation")
            };
            if matched {
                remove.push(path);
            }
        }
    }
    let revision_path = eosh.join("Revision.json");
    let mut revision = Value::Null;
    if eosh_changed {
        revision = read_value(&revision_path, Value::Null)?;
        let next = revision["number"]
            .as_u64()
            .and_then(|n| n.checked_add(1))
            .ok_or("Invalid EOSH revision")?;
        revision["number"] = json!(next);
        backup(&revision_path)?;
    }
    if changed {
        backup(&installed)?;
    }
    for path in &remove {
        backup(path)?;
    }
    // Publish the revision after deleting records, so Epic reconciles the removal.
    for path in &remove {
        fs::remove_file(path).map_err(|e| format!("Could not remove {}: {e}", path.display()))?;
    }
    if changed {
        storage::write_json(&installed, &list)?;
    }
    if eosh_changed {
        storage::write_json(&revision_path, &revision)?;
    }
    Ok(())
}
fn register_at(plan: &Plan, release: &Release, bytes: &[u8], data: &Path) -> Result<(), String> {
    let original = buildpatch::parse(bytes)?;
    if original.build != plan.manifest.build
        || original.app != plan.manifest.app
        || release.app != original.app
        || release.build.as_deref() != Some(&original.build)
    {
        return Err("Registration build mismatch".into());
    }
    let version: Value = read_value(&plan.target.join("Engine/Build/Build.version"), Value::Null)?;
    if !crate::maintenance::matches_build_version(&original, &version)
        || version["IsLicenseeVersion"] == 1
        || crate::platform::inspect_engine(&plan.target, "").is_none()
    {
        return Err("Installed engine does not match the exact Epic build".into());
    }
    installer::no_links(&plan.target)?;
    let path = plan
        .target
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\");
    let directory = data.join("Epic/EpicGamesLauncher/Data/Manifests");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let mut existing = None;
    for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension().is_some_and(|s| s == "item") {
            let v = read_value(&p, Value::Null)?;
            if v["AppName"] == original.app {
                if path_key(v["InstallLocation"].as_str().unwrap_or("")) != path_key(&path) {
                    return Err("Epic already registers this engine branch at another location; registration was not replaced".into());
                }
                if existing.is_some() {
                    return Err("Duplicate Epic engine registrations require review".into());
                }
                existing = Some((p, v));
            }
        }
    }
    let id = existing
        .as_ref()
        .and_then(|(_, v)| v["InstallationGuid"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            key(&format!("{}:{}", original.app, path_key(&path)))[..32].to_uppercase()
        });
    if id.len() != 32 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid Epic installation GUID".into());
    }
    let item_path = existing
        .as_ref()
        .map(|(p, _)| p.clone())
        .unwrap_or_else(|| directory.join(format!("{id}.item")));
    let installed = data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat");
    let mut list = read_value(&installed, json!({"InstallationList":[]}))?;
    let entries = list["InstallationList"]
        .as_array_mut()
        .ok_or("Invalid Epic InstallationList")?;
    for v in entries.iter().filter(|v| v["AppName"] == original.app) {
        if path_key(v["InstallLocation"].as_str().unwrap_or("")) != path_key(&path) {
            return Err("Epic installation list points this branch to another folder".into());
        }
    }
    entries.retain(|v| v["AppName"] != original.app);
    entries.push(json!({"InstallLocation":path,"NamespaceId":"ue","ItemId":release.catalog_id,"ArtifactId":original.app,"AppVersion":original.build,"AppName":original.app}));
    let egstore = plan.target.join(".egstore");
    installer::no_links(&egstore)?;
    let manifest = egstore.join(format!("{id}.manifest"));
    let mut item = existing.map(|(_, v)| v).unwrap_or_else(|| json!({}));
    let major = version["MajorVersion"]
        .as_u64()
        .ok_or("Missing major version")?;
    let fields = json!({"FormatVersion":0,"bIsIncompleteInstall":false,"bNeedsValidation":false,"bIsApplication":false,"bIsExecutable":true,"bIsManaged":false,"bRequiresAuth":true,"bCanRunOffline":false,"bAllowMultipleInstances":true,"LaunchCommand":"","LaunchExecutable":if major>=5{"Engine/Binaries/Win64/UnrealEditor.exe"}else{"Engine/Binaries/Win64/UE4Editor.exe"},"ManifestLocation":format!("{path}\\.egstore"),"CompleteManifestPath":format!("{path}\\.egstore\\{id}.manifest"),"PendingManifestPath":format!("{path}\\.egstore\\Pending\\{id}.manifest"),"StagingLocation":format!("{path}\\.egstore\\bps"),"ManifestHash":"","InstallationGuid":id,"InstallLocation":path,"InstallTags":plan.selected,"BaseURLs":plan.bases.iter().map(ToString::to_string).collect::<Vec<_>>(),"AppCategories":[format!("engines/ue{major}"),"engines"],"TechnicalType":format!("engines/ue{major},engines"),"DisplayName":"Unreal Engine","CatalogNamespace":"ue","CatalogItemId":release.catalog_id,"AppName":original.app,"AppVersionString":original.build,"InstallSize":plan.files().iter().map(|f|f.size()).sum::<u64>(),"MandatoryAppFolderName":original.app});
    for (k, v) in fields.as_object().unwrap() {
        item[k] = v.clone();
    }
    // Preserve unrelated metadata and keep recovery copies before updating global records.
    backup(&item_path)?;
    backup(&installed)?;
    backup(&manifest)?;
    storage::write_atomic(&manifest, bytes)?;
    register_eosh(plan, release, &id, &path, &mut item, data)?;
    storage::write_json(&item_path, &item)?;
    storage::write_json(&installed, &list)?;
    Ok(())
}
fn register_eosh(
    plan: &Plan,
    release: &Release,
    id: &str,
    path: &str,
    item: &mut Value,
    data: &Path,
) -> Result<(), String> {
    let directory = data.join("Epic/EpicOnlineServicesShared/InstallHelper/InstalledItems");
    // Older Epic installations have no EOSH database; retain legacy compatibility.
    if !directory.is_dir() {
        return Ok(());
    }
    let revision_path = directory.join("Revision.json");
    let mut revision = read_value(&revision_path, Value::Null)?;
    let timestamp = revision["timestamp"]
        .as_u64()
        .ok_or("Unsupported EOSH revision format")?;
    let number = revision["number"]
        .as_u64()
        .ok_or("Unsupported EOSH revision number")?
        .checked_add(1)
        .ok_or("EOSH revision overflow")?;
    let revision_id = format!("{timestamp:016X}{number:016X}");
    for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension().is_some_and(|s| s == "egi") {
            let record = read_value(&p, Value::Null)?;
            let v = &record["v4"];
            if v["artifactId"] == release.app
                && (v["installationId"] != id
                    || path_key(v["dir"].as_str().unwrap_or("")) != path_key(path)
                    || v["state"] != "Installed")
            {
                return Err("EOSH already manages this engine at a different location or is performing an operation".into());
            }
        }
    }
    let dest = directory.join(format!("{id}.egi"));
    let mut record = read_value(&dest, json!({"v4":{}}))?;
    if !record["v4"].is_object() {
        return Err("Unsupported EOSH installation record".into());
    }
    let mut tags = plan.selected.clone();
    if !tags.iter().any(String::is_empty) {
        tags.insert(0, String::new());
    }
    let fields = json!({"installationId":id,"state":"Installed","revision":revision_id,"dir":path,"metaDir":item["ManifestLocation"],"manifestPath":item["CompleteManifestPath"],"pendingManifestPath":item["PendingManifestPath"],"platform":"Windows","sandboxId":"ue","itemId":release.catalog_id,"artifactId":release.app,"tags":tags,"pendingTags":[],"stagedVersion":"","manifestData":{"version":plan.manifest.build,"launchCommand":"","launchExe":item["LaunchExecutable"],"prereqSHA1Hash":"","buildSize":item["InstallSize"]},"pendingManifestData":{"version":"","launchCommand":"","launchExe":"","prereqSHA1Hash":"","buildSize":0},"manifestUris":[]});
    for (k, v) in fields.as_object().unwrap() {
        record["v4"][k] = v.clone();
    }
    backup(&dest)?;
    backup(&revision_path)?;
    storage::write_json(&dest, &record)?;
    revision["number"] = json!(number);
    storage::write_json(&revision_path, &revision)?;
    item["EoshRevision"] = json!(revision_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path) -> (Plan, Release, Vec<u8>) {
        let build = "5.7.4-51494982+++UE5+Release-5.7-Windows";
        let target = root.join("UE_5.7");
        fs::create_dir_all(target.join("Engine/Binaries/Win64")).unwrap();
        fs::write(
            target.join("Engine/Binaries/Win64/UnrealEditor.exe"),
            b"fixture",
        )
        .unwrap();
        storage::write_json(&target.join("Engine/Build/Build.version"),&json!({"MajorVersion":5,"MinorVersion":7,"PatchVersion":4,"Changelist":51494982u64,"IsLicenseeVersion":0,"BranchName":"++UE5+Release-5.7"})).unwrap();
        let bytes=serde_json::to_vec(&json!({"ManifestFileVersion":"012000000000","bIsFileData":false,"AppNameString":"UE_5.7","BuildVersionString":build,"ChunkHashList":{},"FileManifestList":[]})).unwrap();
        let plan = Plan {
            manifest: buildpatch::parse(&bytes).unwrap(),
            target,
            bases: vec![],
            tags: vec![],
            selected: vec!["templates".into()],
        };
        let release = Release {
            app: "UE_5.7".into(),
            catalog_id: "fixture-id".into(),
            channel: "Live".into(),
            platform: "Windows".into(),
            build: Some(build.into()),
            version: None,
        };
        (plan, release, bytes)
    }
    #[test]
    fn removal_is_scoped_requires_missing_folder_and_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let (p, r, b) = fixture(d.path());
        let data = d.path().join("ProgramData");
        let eosh = data.join("Epic/EpicOnlineServicesShared/InstallHelper/InstalledItems");
        storage::write_json(
            &eosh.join("Revision.json"),
            &json!({"timestamp":1,"number":1}),
        )
        .unwrap();
        register_at(&p, &r, &b, &data).unwrap();
        let list_path = data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat");
        let mut list: Value = storage::read_json(&list_path).unwrap();
        list["InstallationList"]
            .as_array_mut()
            .unwrap()
            .push(json!({"AppName":"UE_5.7","InstallLocation":"C:/other"}));
        list["InstallationList"]
            .as_array_mut()
            .unwrap()
            .push(json!({"AppName":"FabPlugin_5.7","InstallLocation":p.target}));
        storage::write_json(&list_path, &list).unwrap();
        let other = json!({"v4":{"artifactId":"UE_5.7","dir":"C:/other","state":"Installed"}});
        storage::write_json(&eosh.join("other.egi"), &other).unwrap();
        let manifests = data.join("Epic/EpicGamesLauncher/Data/Manifests");
        storage::write_json(
            &manifests.join("other.item"),
            &json!({"AppName":"UE_5.7","InstallLocation":"C:/other"}),
        )
        .unwrap();
        assert!(unregister_at(&r.app, &p.target, &data).is_err());
        assert_eq!(storage::read_json::<Value>(&list_path).unwrap(), list);
        fs::remove_dir_all(&p.target).unwrap();
        unregister_at(&r.app, &p.target, &data).unwrap();
        unregister_at(&r.app, &p.target, &data).unwrap();
        let remaining: Value = storage::read_json(&list_path).unwrap();
        assert_eq!(remaining["InstallationList"].as_array().unwrap().len(), 2);
        assert_eq!(
            storage::read_json::<Value>(&eosh.join("other.egi")).unwrap(),
            other
        );
        assert!(manifests.join("other.item").exists());
        assert_eq!(
            fs::read_dir(&manifests)
                .unwrap()
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "item"))
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(&eosh)
                .unwrap()
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "egi"))
                .count(),
            1
        );
        assert_eq!(
            storage::read_json::<Value>(&eosh.join("Revision.json")).unwrap()["number"],
            3
        );
    }
    #[test]
    fn registration_preserves_other_apps_is_idempotent_and_writes_exact_manifest() {
        let d = tempfile::tempdir().unwrap();
        let (p, r, b) = fixture(d.path());
        let data = d.path().join("ProgramData");
        let list = data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat");
        storage::write_json(&list,&json!({"other":"preserved","InstallationList":[{"AppName":"UE_5.8","InstallLocation":"elsewhere"},{"AppName":"FabPlugin_5.8"}]})).unwrap();
        let eosh = data.join("Epic/EpicOnlineServicesShared/InstallHelper/InstalledItems");
        storage::write_json(
            &eosh.join("Revision.json"),
            &json!({"timestamp":639156589012404317u64,"number":92}),
        )
        .unwrap();
        storage::write_json(
            &eosh.join("unrelated.egi"),
            &json!({"v4":{"artifactId":"other"}}),
        )
        .unwrap();
        register_at(&p, &r, &b, &data).unwrap();
        register_at(&p, &r, &b, &data).unwrap();
        let v: Value = storage::read_json(&list).unwrap();
        assert_eq!(v["other"], "preserved");
        assert_eq!(v["InstallationList"].as_array().unwrap().len(), 3);
        let entries: Vec<_> = fs::read_dir(data.join("Epic/EpicGamesLauncher/Data/Manifests"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|s| s == "item"))
            .collect();
        assert_eq!(entries.len(), 1);
        let rev: Value = storage::read_json(&eosh.join("Revision.json")).unwrap();
        assert_eq!(rev["number"], 94);
        assert!(eosh.join("unrelated.egi").exists());
        let item: Value = storage::read_json(&entries[0].path()).unwrap();
        assert_eq!(item["InstallTags"], json!(["templates"]));
        assert_eq!(item["AppVersionString"], p.manifest.build);
        assert_eq!(
            fs::read(p.target.join(".egstore").join(format!(
                "{}.manifest",
                item["InstallationGuid"].as_str().unwrap()
            )))
            .unwrap(),
            b
        );
    }
    #[test]
    fn refuses_another_location_or_build_without_replacing_records() {
        let d = tempfile::tempdir().unwrap();
        let (mut p, r, b) = fixture(d.path());
        let data = d.path().join("ProgramData");
        let list = data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat");
        storage::write_json(
            &list,
            &json!({"InstallationList":[{"AppName":"UE_5.7","InstallLocation":"other"}]}),
        )
        .unwrap();
        let before = fs::read(&list).unwrap();
        assert!(register_at(&p, &r, &b, &data).is_err());
        assert_eq!(fs::read(&list).unwrap(), before);
        assert!(!p.target.join(".egstore").exists());
        p.manifest.build = "different".into();
        assert!(register_at(&p, &r, &b, &data).is_err());
        assert_eq!(fs::read(&list).unwrap(), before);
    }
}
