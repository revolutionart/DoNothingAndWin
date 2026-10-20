//! Local, per-engine plugin rollback. Only recognized, completed backups expire.
use super::*;
use serde::{Deserialize, Serialize};

mod registration;
const DIRECTORY: &str = ".dnw-plugin-backups";
const RECORD: &str = "backup.json";
const DAY: u64 = 86_400;

#[derive(Clone, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    pub name: String,
    pub version: String,
    pub created: u64,
    pub expires: Option<u64>,
    pub kept: bool,
    pub bytes: u64,
    schema: u32,
    engine: PathBuf,
    branch: String,
    relative: PathBuf,
    descriptor: String,
    number: i64,
    complete: bool,
    files: Vec<SavedFile>,
    registration: Option<registration::Saved>,
    package_receipt: Option<receipt::Receipt>,
}
#[derive(Clone, Serialize, Deserialize)]
struct SavedFile {
    name: String,
    size: u64,
    sha: [u8; 20],
}
pub(super) struct Prepared {
    record: Backup,
    pub directory: PathBuf,
}
impl Prepared {
    pub fn content(&self) -> PathBuf {
        self.directory.join("files")
    }
    pub fn finish(&mut self) -> Result<(), String> {
        self.record.complete = true;
        crate::storage::write_json(&self.directory.join(RECORD), &self.record)
    }
}
fn engine_root(engine: &Installation) -> Result<PathBuf, String> {
    installer::no_links(Path::new(&engine.path))?;
    fs::canonicalize(&engine.path).map_err(|e| e.to_string())
}
fn base(engine: &Path) -> Result<PathBuf, String> {
    let path = engine.join(DIRECTORY);
    installer::no_links(&path)?;
    Ok(path)
}
fn entry(engine: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
        return Err("Invalid plugin backup identity".into());
    }
    let path = base(engine)?.join(id);
    installer::no_links(&path)?;
    Ok(path)
}
fn load(engine: &Installation, id: &str) -> Result<(Backup, PathBuf), String> {
    let root = engine_root(engine)?;
    let directory = entry(&root, id)?;
    let path = directory.join(RECORD);
    installer::no_links(&path)?;
    if fs::metadata(&path).map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("Plugin backup record is too large".into());
    }
    let record: Backup = crate::storage::read_json(&path).ok_or("Unreadable plugin backup")?;
    let branch = EngineVersion::parse(&engine.version)
        .ok_or("Unknown engine version")?
        .branch();
    let relative = installer::relative(&record.relative.to_string_lossy())?;
    if record.schema != 1
        || record.id != id
        || record.engine != root
        || record.branch != branch
        || !relative.starts_with("Engine/Plugins")
        || relative == Path::new("Engine/Plugins")
        || relative == Path::new("Engine/Plugins/Marketplace")
        || Path::new(&record.descriptor).components().count() != 1
        || !record.descriptor.ends_with(".uplugin")
        || record.files.is_empty()
    {
        return Err("Backup does not belong to this plugin/engine installation".into());
    }
    let mut names = HashSet::new();
    for file in &record.files {
        installer::relative(&file.name)?;
        if !names.insert(file.name.to_lowercase()) {
            return Err("Duplicate backup file paths".into());
        }
    }
    if !names.contains(&record.descriptor.to_lowercase()) {
        return Err("Backup has no plugin descriptor".into());
    }
    installer::no_links(&directory.join("files"))?;
    Ok((record, directory))
}
fn inventory(root: &Path) -> Result<Vec<(String, u64)>, String> {
    let mut result = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(dir) = directories.pop() {
        installer::no_links(&dir)?;
        for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            let path = e.path();
            installer::no_links(&path)?;
            let meta = e.metadata().map_err(|e| e.to_string())?;
            if meta.is_dir() {
                directories.push(path);
            } else if meta.is_file() {
                result.push((
                    path.strip_prefix(root)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .replace('\\', "/"),
                    meta.len(),
                ));
            } else {
                return Err("Plugin backup contains an unsupported file type".into());
            }
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}
pub(super) fn prepare(
    engine: &Installation,
    plugin: &Plugin,
    root: &Path,
    days: u64,
    report: &mut impl FnMut(UpdateProgress),
) -> Result<Prepared, String> {
    let eng = engine_root(engine)?;
    installer::no_links(root)?;
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut state = UpdateProgress {
        phase: "Backing up current plugin",
        ..Default::default()
    };
    report(state.clone());
    let files = inventory(&root)?;
    state.total = files.iter().map(|(_, size)| size).sum();
    state.files_total = files.len();
    let mut saved = Vec::with_capacity(files.len());
    for (name, size) in files {
        state.file = name.clone();
        let sha =
            installer::hash_file_progress(&root.join(installer::relative(&name)?), &mut |bytes| {
                state.prepared += bytes;
                state.read += bytes;
                report(state.clone());
                Ok(())
            })?;
        state.files_done += 1;
        report(state.clone());
        saved.push(SavedFile { name, size, sha });
    }
    let created = crate::epic::now();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let id = format!("{stamp:x}-{:x}", std::process::id());
    let directory = entry(&eng, &id)?;
    fs::create_dir_all(base(&eng)?).map_err(|e| e.to_string())?;
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    let registration = registration::capture(&eng, plugin, &directory)?;
    let record = Backup {
        id,
        name: plugin.name.clone(),
        version: plugin.version.clone(),
        created,
        expires: (days > 0).then(|| created.saturating_add(days.min(3650) * DAY)),
        kept: days == 0,
        bytes: state.total,
        schema: 1,
        engine: eng.clone(),
        branch: EngineVersion::parse(&engine.version)
            .ok_or("Unknown engine version")?
            .branch(),
        relative: root.strip_prefix(&eng).map_err(|e| e.to_string())?.into(),
        descriptor: plugin
            .descriptor
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("Invalid descriptor name")?
            .into(),
        number: plugin.number,
        complete: false,
        files: saved,
        registration,
        package_receipt: receipt::read(plugin),
    };
    crate::storage::write_json(&directory.join(RECORD), &record)?;
    Ok(Prepared { record, directory })
}
pub fn list(engine: &Installation) -> Result<Vec<Backup>, String> {
    let eng = engine_root(engine)?;
    let directory = base(&eng)?;
    if !directory.exists() {
        return Ok(vec![]);
    }
    let mut backups = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let id = entry.file_name().to_string_lossy().into_owned();
        // Unknown/incomplete staging entries are never automatically deleted.
        if let Ok((record, directory)) = load(engine, &id) {
            if directory.join("files").is_dir() {
                backups.push(record);
            }
        }
    }
    backups.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    Ok(backups)
}
pub fn set_kept(engine: &Installation, id: &str, kept: bool, days: u64) -> Result<(), String> {
    let eng = engine_root(engine)?;
    let _lock = crate::maintenance::lock(&eng)?;
    let (mut record, directory) = load(engine, id)?;
    record.kept = kept;
    // Releasing a kept backup starts a fresh retention period.
    if !kept {
        let days = if days == 0 { 7 } else { days.clamp(1, 3650) };
        record.expires = Some(crate::epic::now().saturating_add(days * DAY));
    }
    crate::storage::write_json(&directory.join(RECORD), &record)
}
pub fn delete(engine: &Installation, id: &str) -> Result<(), String> {
    let eng = engine_root(engine)?;
    let _lock = crate::maintenance::lock(&eng)?;
    let (_, directory) = load(engine, id)?;
    delete_scoped_tree(&directory, &base(&eng)?)
}
/// Runs under the caller's engine maintenance lock, at update/backup-manager entry.
pub(super) fn expire_locked(engine: &Installation, now: u64) -> Result<(), String> {
    let eng = engine_root(engine)?;
    for record in list(engine)? {
        if record.complete && !record.kept && record.expires.is_some_and(|at| at <= now) {
            let (_, directory) = load(engine, &record.id)?;
            delete_scoped_tree(&directory, &base(&eng)?)?;
        }
    }
    Ok(())
}
pub fn refresh(engine: &Installation) -> Result<Vec<Backup>, String> {
    let eng = engine_root(engine)?;
    let _lock = crate::maintenance::lock(&eng)?;
    expire_locked(engine, crate::epic::now())?;
    list(engine)
}
pub fn restore(
    engine: &Installation,
    id: &str,
    days: u64,
    mut report: impl FnMut(UpdateProgress),
) -> Result<String, String> {
    let eng = engine_root(engine)?;
    let _lock = crate::maintenance::lock(&eng)?;
    crate::maintenance::ensure_closed(&eng)?;
    let (record, directory) = load(engine, id)?;
    let target = eng.join(&record.relative);
    let plugin = installed(&eng).into_iter().find(|p|p.descriptor == target.join(&record.descriptor))
        .ok_or("This plugin is no longer installed at its original location. Reinstall it before restoring this backup.")?;
    let root = plugin_root(engine, &plugin)?;
    if root != target {
        return Err("Plugin location changed".into());
    }
    let content = directory.join("files");
    // Detect added/missing files as well as modified contents.
    let actual = inventory(&content)?;
    let expected: Vec<_> = record
        .files
        .iter()
        .map(|f| (f.name.clone(), f.size))
        .collect();
    if actual != expected {
        return Err("Backup file inventory changed; the installed plugin was not modified".into());
    }
    if fs2::available_space(&eng).map_err(|e| e.to_string())?
        < record.bytes.saturating_add(64 * 1024 * 1024)
    {
        return Err("Not enough free space to restore the plugin backup".into());
    }
    let stage = backup_path(engine, &root)?.with_extension("restore");
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut state = UpdateProgress {
            phase: "Verifying plugin backup",
            total: record.bytes,
            files_total: record.files.len(),
            ..Default::default()
        };
        report(state.clone());
        for file in &record.files {
            let relative = installer::relative(&file.name)?;
            let source = content.join(&relative);
            let dest = stage.join(&relative);
            installer::no_links(&source)?;
            state.file = file.name.clone();
            let hash = installer::hash_file_progress(&source, &mut |bytes| {
                state.read += bytes;
                state.prepared += bytes;
                report(state.clone());
                Ok(())
            })?;
            if hash != file.sha {
                return Err(format!(
                    "Backup verification failed: {}. The installed plugin was not modified.",
                    file.name
                ));
            }
            fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(&source, &dest).map_err(|e| e.to_string())?;
            // Verify the copy too; a changing source cannot silently enter the live installation.
            if !installer::valid_file(&dest, &file.sha, file.size) {
                return Err("Restored file verification failed".into());
            }
            state.files_done += 1;
            report(state.clone());
        }
        let descriptor: serde_json::Value =
            crate::storage::read_json(&stage.join(&record.descriptor))
                .ok_or("Invalid backed-up plugin descriptor")?;
        if descriptor["Version"].as_i64().unwrap_or(0) != record.number
            || descriptor["VersionName"].as_str().unwrap_or("Unknown") != record.version
            || !compatible(
                descriptor["EngineVersion"].as_str().unwrap_or(""),
                &engine.version,
            )
        {
            return Err("Backup is not compatible with this engine".into());
        }
        plugin_root(engine, &plugin)?;
        if let Some(receipt) = &record.package_receipt {
            crate::storage::write_json(&stage.join(".dnw-plugin-install.json"), receipt)?;
        }
        let mut current = prepare(engine, &plugin, &root, days, &mut report)?;
        crate::maintenance::ensure_closed(&eng)?;
        report(UpdateProgress {
            phase: "Restoring plugin backup",
            ..Default::default()
        });
        replace(&root, &stage, &mut current)?;
        let registration = registration::restore(
            &eng,
            &directory,
            record.registration.as_ref(),
            &target.join(&record.descriptor),
        );
        let mut message = format!(
            "Restored {} {} for UE {}. The previous installation was saved as another backup.",
            record.name, record.version, engine.version
        );
        if let Err(warning) = registration {
            message.push_str(&format!(" Epic registration needs attention: {warning}"));
        }
        Ok(message)
    })();
    if stage.exists() {
        let _ = delete_scoped_tree(&stage, &eng.join(".engine-workspace-plugin-backups"));
    }
    result
}
pub(super) fn replace(root: &Path, stage: &Path, backup: &mut Prepared) -> Result<(), String> {
    fs::rename(root, backup.content()).map_err(|e| e.to_string())?;
    if let Err(error) = fs::rename(stage, root) {
        let restored = fs::rename(backup.content(), root);
        return Err(format!(
            "Plugin activation failed: {error}. Previous plugin restored: {}. Backup: {}",
            restored.is_ok(),
            backup.content().display()
        ));
    }
    // On write failure leave the unexpired recovery backup intact.
    backup.finish().map_err(|error| {
        format!(
            "Plugin files were replaced, but the backup needs attention: {error}. Saved at {}",
            backup.content().display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write_plugin(root: &Path, version: &str, payload: &[u8]) {
        fs::create_dir_all(root.join("Binaries")).unwrap();
        crate::storage::write_json(&root.join("Test.uplugin"),&serde_json::json!({
            "FriendlyName":"Test plugin","Version":1,"VersionName":version,"EngineVersion":"5.8.0"
        })).unwrap();
        fs::write(root.join("Binaries/test.dll"), payload).unwrap();
        fs::write(
            root.join(".dnw-plugin-install.json"),
            format!("receipt-{version}"),
        )
        .unwrap();
    }
    fn fixture() -> (tempfile::TempDir, Installation, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let eng = temp.path().join("UE_5.8");
        let root = eng.join("Engine/Plugins/Marketplace/Test");
        write_plugin(&root, "1.0", b"old binary");
        let engine = Installation {
            path: eng.to_string_lossy().into(),
            version: "5.8.3".into(),
            association: "5.8".into(),
        };
        (temp, engine, fs::canonicalize(root).unwrap())
    }
    fn update(engine: &Installation, root: &Path, version: &str, days: u64) -> String {
        let plugin = installed(Path::new(&engine.path)).remove(0);
        let mut prepared = prepare(engine, &plugin, root, days, &mut |_| {}).unwrap();
        let stage = Path::new(&engine.path).join("test-stage");
        write_plugin(&stage, version, b"new binary");
        replace(root, &stage, &mut prepared).unwrap();
        prepared.record.id
    }
    #[test]
    fn rollback_restores_plugin_and_receipt_preserves_selected_and_saves_current() {
        let (_temp, engine, root) = fixture();
        let engine_version = Path::new(&engine.path).join("Engine/Build/Build.version");
        fs::create_dir_all(engine_version.parent().unwrap()).unwrap();
        let engine_bytes = br#"{"MajorVersion":5,"MinorVersion":8,"PatchVersion":3}"#;
        fs::write(&engine_version, engine_bytes).unwrap();
        let id = update(&engine, &root, "2.0", 7);
        let before = list(&engine).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].version, "1.0");
        assert_eq!(before[0].expires, Some(before[0].created + 7 * DAY));
        let mut phases = HashSet::new();
        let message = restore(&engine, &id, 7, |p| {
            phases.insert(p.phase);
        })
        .unwrap();
        assert!(message.starts_with("Restored Test plugin 1.0"));
        assert_eq!(fs::read(&engine_version).unwrap(), engine_bytes);
        assert_eq!(installed(Path::new(&engine.path))[0].version, "1.0");
        assert_eq!(
            fs::read(root.join("Binaries/test.dll")).unwrap(),
            b"old binary"
        );
        assert_eq!(
            fs::read(root.join(".dnw-plugin-install.json")).unwrap(),
            b"receipt-1.0"
        );
        let backups = list(&engine).unwrap();
        assert_eq!(backups.len(), 2);
        assert!(backups.iter().any(|b| b.version == "2.0"));
        assert!(backups.iter().any(|b| b.id == id));
        assert!(phases.contains("Verifying plugin backup"));
        assert!(phases.contains("Backing up current plugin"));
        let redo = backups.iter().find(|b| b.version == "2.0").unwrap();
        restore(&engine, &redo.id, 7, |_| {}).unwrap();
        assert_eq!(installed(Path::new(&engine.path))[0].version, "2.0");
    }
    #[test]
    fn damaged_or_extra_backup_files_never_replace_the_installed_plugin() {
        let (_temp, engine, root) = fixture();
        let id = update(&engine, &root, "2.0", 7);
        let (_, dir) = load(&engine, &id).unwrap();
        fs::write(dir.join("files/Binaries/test.dll"), b"bad binary").unwrap();
        assert!(
            restore(&engine, &id, 7, |_| {})
                .unwrap_err()
                .contains("verification failed")
        );
        assert_eq!(
            fs::read(root.join("Binaries/test.dll")).unwrap(),
            b"new binary"
        );
        fs::write(dir.join("files/unrecorded.txt"), b"extra").unwrap();
        assert!(
            restore(&engine, &id, 7, |_| {})
                .unwrap_err()
                .contains("inventory changed")
        );
        assert_eq!(list(&engine).unwrap().len(), 1);
    }
    #[test]
    fn retention_keeps_pinned_incomplete_and_unrecognized_backups() {
        let (_temp, engine, root) = fixture();
        let id1 = update(&engine, &root, "2.0", 7);
        let id2 = update(&engine, &root, "3.0", 7);
        set_kept(&engine, &id2, true, 7).unwrap();
        let id3 = update(&engine, &root, "4.0", 7);
        let (mut incomplete, dir) = load(&engine, &id3).unwrap();
        incomplete.complete = false;
        crate::storage::write_json(&dir.join(RECORD), &incomplete).unwrap();
        let unknown = Path::new(&engine.path).join(DIRECTORY).join("unknown");
        fs::create_dir_all(&unknown).unwrap();
        fs::write(unknown.join("user-file"), b"preserve").unwrap();
        expire_locked(&engine, crate::epic::now() + 8 * DAY).unwrap();
        assert!(
            !entry(&engine_root(&engine).unwrap(), &id1)
                .unwrap()
                .exists()
        );
        assert!(
            entry(&engine_root(&engine).unwrap(), &id2)
                .unwrap()
                .exists()
        );
        assert!(dir.join("files").exists());
        assert!(unknown.join("user-file").exists());
        set_kept(&engine, &id2, false, 14).unwrap();
        let (released, _) = load(&engine, &id2).unwrap();
        assert!(!released.kept);
        assert!(released.expires.unwrap() >= crate::epic::now() + 14 * DAY - 1);
        delete(&engine, &id2).unwrap();
        assert_eq!(installed(Path::new(&engine.path))[0].version, "4.0");
    }
    #[test]
    fn rejects_wrong_engine_branch_identity_and_path_traversal() {
        let (temp, engine, root) = fixture();
        let id = update(&engine, &root, "2.0", 0);
        assert!(list(&engine).unwrap()[0].kept);
        let wrong = Installation {
            version: "5.7.4".into(),
            ..engine.clone()
        };
        assert!(restore(&wrong, &id, 7, |_| {}).is_err());
        assert!(delete(&engine, "../outside").is_err());
        let (mut record, dir) = load(&engine, &id).unwrap();
        record.files[0].name = "../../outside".into();
        crate::storage::write_json(&dir.join(RECORD), &record).unwrap();
        assert!(restore(&engine, &id, 7, |_| {}).is_err());
        record.files[0].name = "Binaries/test.dll".into();
        record.engine = temp.path().join("another-engine");
        crate::storage::write_json(&dir.join(RECORD), &record).unwrap();
        assert!(delete(&engine, &id).is_err());
        assert_eq!(installed(Path::new(&engine.path))[0].version, "2.0");
    }
    #[test]
    fn failed_activation_puts_original_back_and_never_expires_recovery_files() {
        let (_temp, engine, root) = fixture();
        let plugin = installed(Path::new(&engine.path)).remove(0);
        let mut backup = prepare(&engine, &plugin, &root, 7, &mut |_| {}).unwrap();
        assert!(replace(&root, &root.join("nonexistent-stage"), &mut backup).is_err());
        assert!(root.join("Test.uplugin").exists());
        assert!(!backup.content().exists());
        assert_eq!(installed(Path::new(&engine.path))[0].version, "1.0");
    }
}
