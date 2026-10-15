use crate::{
    buildpatch::{self, Manifest},
    epic::Session,
    installer::{self, Control, Plan, Progress},
    model::{Installation, Snapshot},
    storage,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const RECORD: &str = ".engine-workspace-manifest.json.gz";
#[derive(Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    selection_version: u32,
    manifest: Manifest,
    bases: Vec<String>,
    selected: Vec<String>,
}
pub fn save_plan(root: &Path, plan: &Plan) -> Result<(), String> {
    let dest = root.join(RECORD);
    installer::no_links(&dest)?;
    let temp = root.join(format!("{RECORD}.tmp"));
    installer::no_links(&temp)?;
    let mut writer = flate2::write::GzEncoder::new(
        File::create(&temp).map_err(|e| e.to_string())?,
        flate2::Compression::fast(),
    );
    serde_json::to_writer(
        &mut writer,
        &Record {
            selection_version: 2,
            manifest: plan.manifest.clone(),
            bases: plan.bases.iter().map(ToString::to_string).collect(),
            selected: plan.selected.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    writer
        .finish()
        .map_err(|e| e.to_string())?
        .sync_all()
        .map_err(|e| e.to_string())?;
    replace(&temp, &dest)
}
fn root(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().count() < 3 {
        return Err("Choose an engine installation folder, not a drive or parent folder.".into());
    }
    installer::no_links(path)?;
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !path.join("Engine").is_dir()
        || !(path.join("Engine/Build/Build.version").is_file()
            || path.join(".engine-workspace-install.json").is_file()
            || epic_build(&path).is_some())
    {
        return Err("This folder is not a recognized engine installation.".into());
    }
    installer::no_links(&path)?;
    Ok(path)
}
pub(crate) fn lock(root: &Path) -> Result<File, String> {
    let dir = storage::data_dir().join("maintenance");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let key = buildpatch::hex(&buildpatch::digest(
        root.to_string_lossy().to_lowercase().as_bytes(),
    ));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(format!("{key}.lock")))
        .map_err(|e| e.to_string())?;
    fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|_| "This engine already has a maintenance operation running.".to_owned())?;
    Ok(file)
}
pub(crate) fn ensure_closed(root: &Path) -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        use windows::{
            Win32::{
                Foundation::CloseHandle,
                System::{Diagnostics::ToolHelp::*, Threading::*},
            },
            core::PWSTR,
        };
        let snapshot =
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| e.to_string())?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut next = Process32FirstW(snapshot, &mut entry).is_ok();
        let mut running = false;
        while next {
            if let Ok(process) = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                entry.th32ProcessID,
            ) {
                let mut buffer = vec![0u16; 32768];
                let mut len = buffer.len() as u32;
                if QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    PWSTR(buffer.as_mut_ptr()),
                    &mut len,
                )
                .is_ok()
                {
                    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize]));
                    if fs::canonicalize(path).is_ok_and(|p| p.starts_with(root)) {
                        running = true;
                    }
                }
                let _ = CloseHandle(process);
            }
            if running {
                break;
            }
            next = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        if running {
            return Err(
                "Close Unreal Editor and other applications using this engine, then try again."
                    .into(),
            );
        }
    }
    Ok(())
}
fn validate(plan: &Plan) -> Result<(), String> {
    installer::validate_manifest(&plan.manifest)?;
    if let Some(v) = crate::model::EngineVersion::parse(&plan.manifest.build) {
        if plan.manifest.app != format!("UE_{}", v.branch()) {
            return Err("Repair manifest app does not match its engine version".into());
        }
    }
    for (key, c) in &plan.manifest.chunks {
        if key != &c.guid
            || c.guid.len() != 32
            || !c.guid.bytes().all(|c| c.is_ascii_hexdigit())
            || c.window > 64 * 1024 * 1024
        {
            return Err("Invalid repair chunk identity or size".into());
        }
    }
    for f in &plan.manifest.files {
        for p in &f.parts {
            let c = plan
                .manifest
                .chunks
                .get(&p.guid)
                .ok_or("Missing repair chunk")?;
            if p.offset as u64 + p.size as u64 > c.window as u64 {
                return Err("Invalid repair file extent".into());
            }
        }
    }
    Ok(())
}

pub fn epic_build(root: &Path) -> Option<String> {
    let root = fs::canonicalize(root).ok()?;
    let dir = PathBuf::from(std::env::var_os("PROGRAMDATA")?)
        .join("Epic/EpicGamesLauncher/Data/Manifests");
    for entry in fs::read_dir(dir).ok()?.flatten() {
        if entry.path().extension().is_none_or(|x| x != "item") {
            continue;
        }
        let Some(v): Option<serde_json::Value> = storage::read_json(&entry.path()) else {
            continue;
        };
        if !v["AppName"].as_str().is_some_and(|a| a.starts_with("UE_")) {
            continue;
        }
        if v["InstallLocation"]
            .as_str()
            .and_then(|p| fs::canonicalize(p).ok())
            .is_some_and(|p| p == root)
        {
            return v["AppVersionString"].as_str().map(str::to_owned);
        }
    }
    None
}
fn epic_tags(root: &Path) -> Option<Vec<String>> {
    let dir = PathBuf::from(std::env::var_os("PROGRAMDATA")?)
        .join("Epic/EpicGamesLauncher/Data/Manifests");
    for e in fs::read_dir(dir).ok()?.flatten() {
        if e.path().extension().is_none_or(|x| x != "item") {
            continue;
        }
        let Some(v): Option<serde_json::Value> = storage::read_json(&e.path()) else {
            continue;
        };
        if !v["AppName"].as_str().is_some_and(|a| a.starts_with("UE_")) {
            continue;
        }
        if v["InstallLocation"]
            .as_str()
            .and_then(|p| fs::canonicalize(p).ok())
            .is_some_and(|p| p == root)
        {
            return v["InstallTags"].as_array().map(|tags| {
                tags.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            });
        }
    }
    None
}
fn official_release_build(v: &serde_json::Value) -> bool {
    v["IsLicenseeVersion"].as_u64() == Some(0)
        && v["IsPromotedBuild"].as_u64() == Some(1)
        && v["Changelist"].as_u64().is_some_and(|n| n > 0)
        && v["BranchName"]
            .as_str()
            .is_some_and(|s| s.starts_with("++UE") && s.contains("+Release-"))
}
pub(crate) fn matches_build_version(m: &Manifest, v: &serde_json::Value) -> bool {
    let prefix = format!(
        "{}.{}.{}-{}+++",
        v["MajorVersion"], v["MinorVersion"], v["PatchVersion"], v["Changelist"]
    );
    let branch = v["BranchName"].as_str().unwrap_or("");
    !branch.is_empty() && m.build == format!("{prefix}{}-Windows", branch.trim_start_matches('+'))
}
/// Shared files do not prove that every component sharing them was installed.
fn inferred_tags(manifest: &Manifest, root: &Path) -> Vec<String> {
    let mut tags = std::collections::BTreeSet::new();
    for f in &manifest.files {
        if f.tags.len() == 1
            && !tags.contains(&f.tags[0])
            && installer::relative(&f.name).is_ok_and(|p| root.join(p).is_file())
        {
            tags.insert(f.tags[0].clone());
        }
    }
    tags.into_iter().collect()
}
/// Locate the exact installed build. Never substitute the newest build during repair.
pub fn prepare(
    engine: &Installation,
    session: Option<&Session>,
    snapshot: &Snapshot,
) -> Result<Plan, String> {
    let path = root(Path::new(&engine.path))?;
    let record_path = path.join(RECORD);
    installer::no_links(&record_path)?;
    if record_path.exists() {
        let decoder =
            flate2::read::GzDecoder::new(File::open(record_path).map_err(|e| e.to_string())?);
        let r: Record = serde_json::from_reader(decoder.take(512 * 1024 * 1024))
            .map_err(|e| format!("Saved repair manifest is invalid: {e}"))?;
        let receipt: Option<serde_json::Value> =
            storage::read_json(&path.join(".engine-workspace-install.json"));
        let registered = epic_build(&path);
        if registered.as_ref().is_some_and(|b| b != &r.manifest.build) {
            return Err("The saved manifest no longer matches the registered Epic build.".into());
        }
        if receipt
            .as_ref()
            .and_then(|v| v["build"].as_str())
            .is_some_and(|b| b != r.manifest.build)
            || crate::model::EngineVersion::parse(&r.manifest.build)
                != crate::model::EngineVersion::parse(&engine.version)
        {
            return Err("The saved manifest does not match this installed engine build.".into());
        }
        let migrate_selection = r.selection_version < 2
            && receipt
                .as_ref()
                .and_then(|v| v["selected_tags"].as_array())
                .is_none()
            && epic_tags(&path).is_none();
        let selected = if migrate_selection {
            inferred_tags(&r.manifest, &path)
        } else {
            r.selected
        };
        let plan = Plan {
            target: path,
            manifest: r.manifest,
            bases: r
                .bases
                .iter()
                .map(|s| reqwest::Url::parse(s).map_err(|_| "Invalid saved CDN URL".to_owned()))
                .collect::<Result<_, _>>()?,
            tags: vec![],
            selected,
        };
        validate(&plan)?;
        if migrate_selection {
            save_plan(&plan.target, &plan)?;
        }
        return Ok(plan);
    }
    let receipt: Option<serde_json::Value> =
        storage::read_json(&path.join(".engine-workspace-install.json"));
    let build: Option<serde_json::Value> =
        storage::read_json(&path.join("Engine/Build/Build.version"));
    if build.as_ref().and_then(|v| v["IsLicenseeVersion"].as_u64()) == Some(1) {
        return Err("Custom/licensee engines cannot be repaired using Epic's binary files.".into());
    }
    let expected = receipt
        .as_ref()
        .and_then(|v| v["build"].as_str())
        .map(str::to_owned)
        .or_else(|| epic_build(&path));
    let matches = |m: &Manifest| -> bool {
        if let Some(expected) = &expected {
            return &m.build == expected;
        }
        let Some(v) = &build else {
            return false;
        };
        matches_build_version(m, v)
    };
    let mut local = None;
    let egstore = path.join(".egstore");
    installer::no_links(&egstore)?;
    if let Ok(entries) = fs::read_dir(&egstore) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|s| s == "manifest") {
                installer::no_links(&p)?;
                if fs::metadata(&p).is_ok_and(|m| m.len() <= 128 * 1024 * 1024) {
                    if let Ok(m) = fs::read(&p)
                        .map_err(|e| e.to_string())
                        .and_then(|b| buildpatch::parse(&b))
                    {
                        if matches(&m) {
                            local = Some(m);
                            break;
                        }
                    }
                }
            }
        }
    }
    if local.is_none() && expected.is_none() && !build.as_ref().is_some_and(official_release_build)
    {
        return Err("No exact Epic installation manifest was found. Source/custom builds must be rebuilt from their original source; this tool will not replace them with a different build.".into());
    }
    let branch = crate::model::EngineVersion::parse(&engine.version)
        .ok_or("Unknown engine version")?
        .branch();
    let release = snapshot
        .releases
        .iter()
        .find(|r| r.app == format!("UE_{branch}"))
        .ok_or("Refresh engine releases before verifying this installation.")?;
    let session =
        session.ok_or("Connect your Epic account before downloading the repair manifest.")?;
    let mut plan = installer::prepare(session, release, path.parent().ok_or("Missing parent")?)?;
    if let Some(m) = local {
        plan.manifest = m;
    } else if !matches(&plan.manifest) {
        return Err("Epic no longer serves the exact installed build and no saved manifest is available. Repair will not upgrade or downgrade this engine.".into());
    }
    plan.target = path;
    plan.selected = receipt
        .as_ref()
        .and_then(|v| v["selected_tags"].as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .or_else(|| epic_tags(&plan.target))
        .unwrap_or_else(|| inferred_tags(&plan.manifest, &plan.target));
    validate(&plan)?;
    save_plan(&plan.target, &plan)?;
    Ok(plan)
}

pub fn repair(
    plan: &Plan,
    control: &Control,
    progress: Arc<Mutex<Progress>>,
) -> Result<String, String> {
    validate(plan)?;
    let path = root(&plan.target)?;
    let _lock = lock(&path)?;
    repair_locked(plan, control, progress)
}

fn repair_locked(
    plan: &Plan,
    control: &Control,
    progress: Arc<Mutex<Progress>>,
) -> Result<String, String> {
    let path = root(&plan.target)?;
    ensure_closed(&path)?;
    let files = plan.files();
    let total = files.iter().map(|f| f.size()).sum();
    *progress.lock().unwrap() = Progress {
        phase: "Verifying installed engine files".into(),
        total,
        ..Default::default()
    };
    let mut damaged = vec![];
    for f in &files {
        control.check()?;
        let dest = path.join(installer::relative(&f.name)?);
        installer::no_links(&dest)?;
        if !installer::valid_file(&dest, &f.sha, f.size()) {
            damaged.push(*f);
        }
        let mut p = progress.lock().unwrap();
        p.completed += f.size();
        p.files += 1;
    }
    if damaged.is_empty() {
        return Ok(format!("Verified {} files. No repair needed.", files.len()));
    }
    let cache = path.join(".engine-workspace-repair");
    installer::no_links(&cache)?;
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let required = damaged.iter().map(|f| f.size()).max().unwrap_or(0) + 64 * 1024 * 1024;
    if fs2::available_space(&path).map_err(|e| e.to_string())? < required {
        return Err("Not enough disk space to stage a repaired file.".into());
    }
    ensure_closed(&path)?;
    let client = installer::client()?;
    let count = damaged.len();
    *progress.lock().unwrap() = Progress {
        phase: format!("Repairing {count} missing or corrupted files"),
        total: damaged.iter().map(|f| f.size()).sum(),
        ..Default::default()
    };
    let mut cached: Option<(String, Vec<u8>)> = None;
    for f in damaged {
        control.check()?;
        let dest = path.join(installer::relative(&f.name)?);
        installer::no_links(&dest)?;
        fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        let temp = cache.join("file.partial");
        installer::no_links(&temp)?;
        let mut out = File::create(&temp).map_err(|e| e.to_string())?;
        for part in &f.parts {
            control.check()?;
            if cached.as_ref().is_none_or(|(id, _)| id != &part.guid) {
                let chunk = &plan.manifest.chunks[&part.guid];
                let disk = cache.join(&chunk.guid);
                installer::no_links(&disk)?;
                let data = if installer::valid_cached_chunk(&disk, chunk) {
                    fs::read(&disk).map_err(|e| e.to_string())?
                } else {
                    let bytes = installer::chunk_data(
                        &client,
                        chunk,
                        &plan.bases,
                        plan.manifest.feature,
                        control,
                    )?;
                    progress.lock().unwrap().network += chunk.size;
                    // Keep one decoded chunk on disk; bounded cache avoids filling a drive.
                    if let Some((old, _)) = &cached {
                        let _ = fs::remove_file(cache.join(old));
                    }
                    fs::write(&disk, &bytes).map_err(|e| e.to_string())?;
                    bytes
                };
                cached = Some((part.guid.clone(), data));
            }
            let bytes = &cached.as_ref().unwrap().1;
            out.write_all(
                &bytes[part.offset as usize..(part.offset as usize + part.size as usize)],
            )
            .map_err(|e| e.to_string())?;
        }
        out.sync_all().map_err(|e| e.to_string())?;
        drop(out);
        if !installer::valid_file(&temp, &f.sha, f.size()) {
            return Err(format!("Repaired data failed its hash check: {}", f.name));
        }
        control.check()?;
        installer::no_links(&dest)?;
        replace(&temp, &dest)?;
        let mut p = progress.lock().unwrap();
        p.completed += f.size();
        p.files += 1;
    }
    if let Some((id, _)) = cached {
        let _ = fs::remove_file(cache.join(id));
    }
    let _ = fs::remove_dir(cache);
    Ok(format!(
        "Verified {} files; repaired {count} missing or corrupted files.",
        files.len()
    ))
}

/// Apply a component selection against the exact same installed manifest.
pub fn change_components(
    original: &Plan,
    selected: Vec<String>,
    control: &Control,
    progress: Arc<Mutex<Progress>>,
) -> Result<String, String> {
    crate::download_policy::check_install_allowed()?;
    validate(original)?;
    let path = root(&original.target)?;
    let _lock = lock(&path)?;
    ensure_closed(&path)?;
    let allowed: std::collections::HashSet<_> = original
        .manifest
        .files
        .iter()
        .flat_map(|f| f.tags.iter())
        .collect();
    if selected.iter().any(|t| !allowed.contains(t)) {
        return Err("Unknown engine component".into());
    }
    if selected.contains(&"platform_TVOS".into()) && !selected.contains(&"platform_IOS".into()) {
        return Err("tvOS requires iOS".into());
    }
    let mut plan = original.clone();
    plan.selected = selected;
    let wanted: std::collections::HashSet<_> =
        plan.files().iter().map(|f| f.name.clone()).collect();
    if !wanted.contains("Engine/Binaries/Win64/UnrealEditor.exe")
        && !wanted.contains("Engine/Binaries/Win64/UE4Editor.exe")
    {
        return Err("Core editor components are required".into());
    }
    let removed: Vec<_> = original
        .files()
        .into_iter()
        .filter(|f| !wanted.contains(&f.name))
        .collect();
    for f in &removed {
        control.check()?;
        let dest = path.join(installer::relative(&f.name)?);
        installer::no_links(&dest)?;
        if dest.exists() && !installer::valid_file(&dest, &f.sha, f.size()) {
            return Err(format!(
                "Component file was modified; preserving it: {}",
                f.name
            ));
        }
    }
    repair_locked(&plan, control, progress.clone())?;
    control.check()?;
    ensure_closed(&path)?;
    let stage = path.join(format!(
        ".engine-workspace-options-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    installer::no_links(&stage)?;
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let mut moved = Vec::new();
    let commit = (|| -> Result<(), String> {
        for (i, f) in removed.iter().enumerate() {
            control.check()?;
            let dest = path.join(installer::relative(&f.name)?);
            installer::no_links(&dest)?;
            if !dest.exists() {
                continue;
            }
            if !installer::valid_file(&dest, &f.sha, f.size()) {
                return Err(format!("Component changed during operation: {}", f.name));
            }
            let backup = stage.join(i.to_string());
            fs::rename(&dest, &backup).map_err(|e| e.to_string())?;
            moved.push((dest, backup));
        }
        save_plan(&path, &plan)?;
        Ok(())
    })();
    if let Err(e) = commit {
        for (dest, backup) in moved.iter().rev() {
            if let Err(restore) = fs::rename(backup, dest) {
                return Err(format!(
                    "{e}; recovery files retained at {}: {restore}",
                    stage.display()
                ));
            }
        }
        let _ = fs::remove_dir(&stage);
        return Err(e);
    }
    for (_, backup) in moved {
        fs::remove_file(backup).map_err(|e| e.to_string())?;
    }
    fs::remove_dir(&stage).map_err(|e| e.to_string())?;
    progress.lock().unwrap().phase = "Engine component changes applied".into();
    Ok("Engine component changes applied. Installed build unchanged.".into())
}

fn replace(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                a.as_ptr(),
                b.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            return Err(format!(
                "Cannot replace engine file (close applications using it): {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(from, to).map_err(|e| e.to_string())
    }
}

/// Called only after the UI's explicit confirmation of this exact installation.
pub fn remove(engine: &Installation, progress: Arc<Mutex<Progress>>) -> Result<String, String> {
    let path = root(Path::new(&engine.path))?;
    let _lock = lock(&path)?;
    ensure_closed(&path)?;
    let mut dirs = vec![path.clone()];
    let mut files = vec![];
    let mut i = 0;
    // Preflight the entire tree before deleting anything. Never follow reparse points.
    while i < dirs.len() {
        for entry in fs::read_dir(&dirs[i]).map_err(|e| e.to_string())? {
            let item = entry.map_err(|e| e.to_string())?.path();
            installer::no_links(&item)?;
            if !item.starts_with(&path) {
                return Err("Removal escaped the confirmed engine folder".into());
            }
            if item.is_dir() {
                dirs.push(item);
            } else {
                files.push(item);
            }
        }
        i += 1;
    }
    ensure_closed(&path)?;
    *progress.lock().unwrap() = Progress {
        phase: "Removing confirmed engine installation".into(),
        total: files.len() as u64,
        ..Default::default()
    };
    // Keep recognition metadata until the other files have been removed, so a
    // partially blocked uninstall remains discoverable and can be retried.
    files.sort_by_key(|p| {
        p.ends_with("Engine/Build/Build.version") || p.ends_with(".engine-workspace-install.json")
    });
    for file in files {
        installer::no_links(&file)?;
        let mut permissions = fs::metadata(&file)
            .map_err(|e| e.to_string())?
            .permissions();
        if permissions.readonly() {
            permissions.set_readonly(false);
            fs::set_permissions(&file, permissions).map_err(|e| e.to_string())?;
        }
        fs::remove_file(&file).map_err(|e|format!("Removal stopped at {}: {e}. Some files may already be removed; close applications and retry.",file.display()))?;
        let mut p = progress.lock().unwrap();
        p.completed += 1;
        p.files += 1;
    }
    for dir in dirs.into_iter().rev() {
        installer::no_links(&dir)?;
        fs::remove_dir(&dir).map_err(|e| e.to_string())?;
    }
    #[cfg(not(test))]
    {
        progress.lock().unwrap().phase = "Removing Epic registration".into();
        let branch = engine
            .version
            .split('.')
            .take(2)
            .collect::<Vec<_>>()
            .join(".");
        crate::epic_registration::unregister_missing(&format!("UE_{branch}"), &path).map_err(
            |e| format!("Engine files removed, but Epic registration cleanup failed: {e}"),
        )?;
    }
    Ok(format!("Unreal Engine {} removed.", engine.version))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Plan, Vec<u8>) {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("UE_5.8");
        fs::create_dir_all(target.join("Engine/Build")).unwrap();
        fs::create_dir_all(target.join("Engine/Binaries/Win64")).unwrap();
        let version = br#"{"MajorVersion":5,"MinorVersion":8,"PatchVersion":3,"Changelist":123}"#;
        fs::write(target.join("Engine/Build/Build.version"), version).unwrap();
        let data = [b"correct editor".as_slice(), version, b"missing data"].concat();
        let guid = "00000001000000020000000300000004".to_owned();
        let files = vec![
            ("Engine/Binaries/Win64/UnrealEditor.exe", 0, 14),
            ("Engine/Build/Build.version", 14, version.len()),
            ("Engine/Content/missing.dat", 14 + version.len(), 12),
        ]
        .into_iter()
        .map(|(name, offset, size)| buildpatch::File {
            name: name.into(),
            sha: buildpatch::digest(&data[offset..offset + size]),
            parts: vec![buildpatch::Part {
                guid: guid.clone(),
                offset: offset as u32,
                size: size as u32,
            }],
            ..Default::default()
        })
        .collect();
        let chunk = buildpatch::Chunk {
            guid: guid.clone(),
            sha: buildpatch::digest(&data),
            window: data.len() as u32,
            size: data.len() as u64,
            ..Default::default()
        };
        let plan = Plan {
            target,
            manifest: Manifest {
                app: "UE_5.8".into(),
                build: "5.8.3-123+++UE5+Release-5.8-Windows".into(),
                files,
                chunks: [(guid, chunk)].into_iter().collect(),
                ..Default::default()
            },
            bases: vec![],
            tags: vec![],
            selected: vec![],
        };
        (temp, plan, data)
    }
    #[test]
    fn component_changes_add_remove_persist_and_preserve_custom_files() {
        let (_temp, mut plan, data) = fixture();
        plan.manifest.files[2].tags = vec!["editor_symbols".into()];
        let cache = plan.target.join(".engine-workspace-repair");
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("00000001000000020000000300000004"), data).unwrap();
        let custom = plan.target.join("keep.txt");
        fs::write(&custom, b"custom").unwrap();
        let before = fs::read(plan.target.join("Engine/Build/Build.version")).unwrap();
        change_components(
            &plan,
            vec!["editor_symbols".into()],
            &Control::default(),
            Arc::default(),
        )
        .unwrap();
        let optional = plan.target.join("Engine/Content/missing.dat");
        assert_eq!(fs::read(&optional).unwrap(), b"missing data");
        let engine = Installation {
            path: plan.target.to_string_lossy().into_owned(),
            version: "5.8.3".into(),
            association: "5.8".into(),
        };
        let added = prepare(&engine, None, &Snapshot::default()).unwrap();
        assert_eq!(added.selected, vec!["editor_symbols"]);
        fs::write(&optional, b"modified").unwrap();
        assert!(change_components(&added, vec![], &Control::default(), Arc::default()).is_err());
        assert_eq!(fs::read(&optional).unwrap(), b"modified");
        fs::write(&optional, b"missing data").unwrap();
        change_components(&added, vec![], &Control::default(), Arc::default()).unwrap();
        assert!(!optional.exists());
        assert!(
            prepare(&engine, None, &Snapshot::default())
                .unwrap()
                .selected
                .is_empty()
        );
        assert_eq!(fs::read(custom).unwrap(), b"custom");
        assert_eq!(
            fs::read(plan.target.join("Engine/Build/Build.version")).unwrap(),
            before
        );
        assert!(
            change_components(
                &plan,
                vec!["unknown".into()],
                &Control::default(),
                Arc::default()
            )
            .is_err()
        );
    }

    #[test]
    fn component_changes_keep_shared_files_and_enforce_tvos_dependency() {
        let (_temp, mut plan, data) = fixture();
        plan.manifest.files[2].tags = vec![
            "templates".into(),
            "platform_TVOS".into(),
            "platform_IOS".into(),
        ];
        let cache = plan.target.join(".engine-workspace-repair");
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("00000001000000020000000300000004"), data).unwrap();
        assert!(
            change_components(
                &plan,
                vec!["platform_TVOS".into()],
                &Control::default(),
                Arc::default()
            )
            .unwrap_err()
            .contains("requires iOS")
        );
        change_components(
            &plan,
            vec!["templates".into()],
            &Control::default(),
            Arc::default(),
        )
        .unwrap();
        plan.selected = vec!["templates".into()];
        change_components(
            &plan,
            vec!["platform_TVOS".into(), "platform_IOS".into()],
            &Control::default(),
            Arc::default(),
        )
        .unwrap();
        assert_eq!(
            fs::read(plan.target.join("Engine/Content/missing.dat")).unwrap(),
            b"missing data"
        );
    }

    #[test]
    fn repair_hashes_same_size_corruption_restores_missing_and_preserves_custom_files() {
        let (_temp, plan, data) = fixture();
        fs::write(
            plan.target.join("Engine/Binaries/Win64/UnrealEditor.exe"),
            b"broken editor!",
        )
        .unwrap();
        let custom = plan.target.join("custom-plugin.txt");
        fs::write(&custom, b"keep my plugin").unwrap();
        let cache = plan.target.join(".engine-workspace-repair");
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("00000001000000020000000300000004"), data).unwrap();
        let progress = Arc::new(Mutex::new(Progress::default()));
        let summary = repair(&plan, &Control::default(), progress.clone()).unwrap();
        assert!(summary.contains("repaired 2"));
        assert_eq!(progress.lock().unwrap().network, 0);
        assert_eq!(
            fs::read(plan.target.join("Engine/Binaries/Win64/UnrealEditor.exe")).unwrap(),
            b"correct editor"
        );
        assert_eq!(
            fs::read(plan.target.join("Engine/Content/missing.dat")).unwrap(),
            b"missing data"
        );
        assert_eq!(fs::read(custom).unwrap(), b"keep my plugin");
        assert!(
            repair(&plan, &Control::default(), Arc::default())
                .unwrap()
                .contains("No repair needed")
        );
        save_plan(&plan.target, &plan).unwrap();
        let engine = Installation {
            path: plan.target.to_string_lossy().into_owned(),
            version: "5.8.3".into(),
            association: "5.8".into(),
        };
        assert_eq!(
            prepare(&engine, None, &Snapshot::default())
                .unwrap()
                .manifest
                .build,
            plan.manifest.build
        );
    }
    #[test]
    fn pause_or_download_failure_never_replaces_corrupt_original() {
        let (_temp, plan, _) = fixture();
        let exe = plan.target.join("Engine/Binaries/Win64/UnrealEditor.exe");
        fs::write(&exe, b"my original").unwrap();
        let control = Control::default();
        control.stop();
        assert!(repair(&plan, &control, Arc::default()).is_err());
        assert!(repair(&plan, &Control::default(), Arc::default()).is_err());
        assert_eq!(fs::read(exe).unwrap(), b"my original");
    }
    #[test]
    fn remove_is_scoped_and_unrecognized_folders_are_rejected() {
        let (temp, plan, _) = fixture();
        let sibling = temp.path().join("my-project.uproject");
        fs::write(&sibling, b"keep").unwrap();
        let engine = Installation {
            path: plan.target.to_string_lossy().into_owned(),
            version: "5.8.3".into(),
            association: "5.8".into(),
        };
        remove(&engine, Arc::default()).unwrap();
        assert!(!plan.target.exists());
        assert!(sibling.exists());
        let bad = Installation {
            path: temp.path().to_string_lossy().into_owned(),
            ..engine
        };
        assert!(remove(&bad, Arc::default()).is_err());
        assert!(sibling.exists());
    }
    #[test]
    fn repair_rejects_escaping_manifest_before_modifying_files() {
        let (temp, mut plan, _) = fixture();
        let outside = temp.path().join("outside");
        fs::write(&outside, b"keep").unwrap();
        plan.manifest.files[0].name = "../outside".into();
        assert!(repair(&plan, &Control::default(), Arc::default()).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }
}

#[cfg(test)]
mod identity_recovery_tests {
    use super::*;
    #[test]
    fn promoted_build_recovery_requires_exact_identity() {
        let mut v = serde_json::json!({"MajorVersion":5,"MinorVersion":7,"PatchVersion":4,"Changelist":51494982,"IsLicenseeVersion":0,"IsPromotedBuild":1,"BranchName":"++UE5+Release-5.7"});
        assert!(official_release_build(&v));
        let mut m: Manifest=serde_json::from_value(serde_json::json!({"app":"UE_5.7","build":"5.7.4-51494982+++UE5+Release-5.7-Windows","files":[],"chunks":{},"feature":22,"prereq":""})).unwrap();
        assert!(matches_build_version(&m, &v));
        m.build = "5.7.4-51494983+++UE5+Release-5.7-Windows".into();
        assert!(!matches_build_version(&m, &v));
        v["IsLicenseeVersion"] = 1.into();
        assert!(!official_release_build(&v));
        v["IsLicenseeVersion"] = 0.into();
        v["IsPromotedBuild"] = 0.into();
        assert!(!official_release_build(&v));
    }
}

#[cfg(test)]
mod component_inference_tests {
    use super::*;
    #[test]
    fn shared_source_and_symbols_do_not_enable_platforms() {
        let dir = tempfile::tempdir().unwrap();
        let mut m: Manifest=serde_json::from_value(serde_json::json!({"app":"UE_5.7","build":"test","feature":22,"prereq":"","files":[],"chunks":{}})).unwrap();
        for (name, tags) in [
            ("source", vec!["engine_source"]),
            ("symbols", vec!["editor_symbols"]),
            ("shared", vec!["engine_source", "platform_Android"]),
            ("shared_debug", vec!["editor_symbols", "platform_Linux"]),
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
            m.files.push(crate::buildpatch::File {
                name: name.into(),
                sha: [0; 20],
                tags: tags.into_iter().map(str::to_owned).collect(),
                parts: vec![],
            });
        }
        assert_eq!(
            inferred_tags(&m, dir.path()),
            vec!["editor_symbols", "engine_source"]
        );
    }
}
