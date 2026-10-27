//! Offline cache checks. A single reader verifies content, never repairs it.
use crate::{
    buildpatch::{self, Manifest},
    installer,
    vault_cache::Entry,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
};

pub const INDEX: &str = ".dnw-cache-files.json";
pub const DAY: u64 = 86400;
static CHANGED_PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
fn record_changed(path: &Path) {
    CHANGED_PATHS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(path.to_owned());
}
/// Drain actual filesystem mutation scopes, including operations completed between UI frames.
pub fn take_changed_paths() -> Vec<PathBuf> {
    CHANGED_PATHS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .drain()
        .collect()
}
pub(crate) struct CacheChange(PathBuf);
pub(crate) fn changing(path: &Path) -> CacheChange {
    record_changed(path);
    CacheChange(path.into())
}
impl Drop for CacheChange {
    fn drop(&mut self) {
        record_changed(&self.0);
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Health {
    Healthy,
    Redownload,
    Unknown,
    MetadataOnly,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub root: PathBuf,
    pub path: PathBuf,
    pub title: String,
    pub identities: Vec<String>,
    pub health: Health,
    pub detail: String,
    pub checked_at: u64,
    pub files: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub roots: Vec<String>,
    pub finished_all_at: u64,
    pub records: HashMap<String, Record>,
}
pub fn due(last: u64, now: u64) -> bool {
    last == 0 || now.saturating_sub(last) >= DAY || last > now
}
pub fn key(path: &Path) -> String {
    crate::discovery::path_key(path)
}
fn report_path() -> PathBuf {
    crate::storage::data_dir().join("vault-health.json")
}
pub fn load() -> Report {
    if fs::metadata(report_path()).is_ok_and(|m| m.len() <= 16 * 1024 * 1024) {
        crate::storage::read_json(&report_path()).unwrap_or_default()
    } else {
        Report::default()
    }
}
pub fn save(report: &Report) {
    let _ = crate::storage::write_json(&report_path(), report);
}

#[derive(Default)]
struct Mode {
    paused: bool,
    cancelled: bool,
}
#[derive(Clone, Default)]
pub struct Gate(Arc<(Mutex<Mode>, Condvar)>, Arc<Mutex<Progress>>);
#[derive(Clone, Default, Debug)]
pub struct Progress {
    pub entry: PathBuf,
    pub file: PathBuf,
    pub phase: String,
    pub entries_completed: u64,
    pub entries_total: u64,
    pub files_completed: u64,
    pub files_total: u64,
    pub file_bytes: u64,
    pub file_total: u64,
    pub bytes_read: u64,
}
impl Gate {
    pub fn progress(&self) -> Progress {
        self.1.lock().unwrap().clone()
    }
    fn begin_file(&self, path: &Path, total: u64) {
        let mut progress = self.1.lock().unwrap();
        progress.file = path.into();
        progress.file_bytes = 0;
        progress.file_total = total;
    }
    fn checked_file(&self) {
        let mut progress = self.1.lock().unwrap();
        progress.files_completed += 1;
        progress.file_total = 0;
        progress.file_bytes = 0;
    }
    fn checking_files(&self, count: usize) {
        let mut progress = self.1.lock().unwrap();
        progress.phase = "Checking file hashes".into();
        progress.files_total = count as u64;
    }
    pub fn cancelled(&self) -> bool {
        self.0.0.lock().unwrap().cancelled
    }
    pub fn pause(&self, paused: bool) {
        let mut mode = self.0.0.lock().unwrap();
        if mode.paused != paused {
            mode.paused = paused;
            self.0.1.notify_all();
        }
    }
    pub fn cancel(&self) {
        self.0.0.lock().unwrap().cancelled = true;
        self.0.1.notify_all();
    }
    pub fn check(&self) -> Result<(), String> {
        let mut mode = self.0.0.lock().unwrap();
        while mode.paused && !mode.cancelled {
            mode = self.0.1.wait(mode).unwrap();
        }
        if mode.cancelled {
            Err("Cache verification stopped.".into())
        } else {
            Ok(())
        }
    }
    fn read(&self, bytes: u64) -> Result<(), String> {
        self.check()?;
        let mut progress = self.1.lock().unwrap();
        progress.bytes_read += bytes;
        progress.file_bytes = (progress.file_bytes + bytes).min(progress.file_total);
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct SavedFile {
    name: String,
    sha: [u8; 20],
    size: u64,
}
#[derive(Serialize, Deserialize)]
struct FileIndex {
    schema: u32,
    artifact: String,
    build: String,
    files: Vec<SavedFile>,
}

fn download_index(
    manifest: &Manifest,
    prefix: &Path,
    output: &Path,
    windows_only: bool,
) -> Result<FileIndex, String> {
    let mut files = Vec::new();
    for file in &manifest.files {
        let name = installer::relative(&file.name)?;
        if let Ok(relative) = name.strip_prefix(prefix) {
            if !relative.as_os_str().is_empty()
                && (!windows_only || crate::plugins::package_files::windows_file(relative))
            {
                files.push(SavedFile {
                    name: output.join(relative).to_string_lossy().replace('\\', "/"),
                    sha: file.sha,
                    size: file.size(),
                });
            }
        }
    }
    Ok(FileIndex {
        schema: 1,
        artifact: manifest.app.clone(),
        build: manifest.build.clone(),
        files,
    })
}
/// Expected hashes from the downloader's verified Epic manifest, not from local payloads.
pub fn write_download_index(
    root: &Path,
    manifest: &Manifest,
    prefix: &Path,
    output: &Path,
) -> Result<(), String> {
    crate::storage::write_json(
        &root.join(INDEX),
        &download_index(manifest, prefix, output, false)?,
    )
}
pub fn write_windows_download_index(
    root: &Path,
    manifest: &Manifest,
    prefix: &Path,
    output: &Path,
) -> Result<(), String> {
    crate::storage::write_json(
        &root.join(INDEX),
        &download_index(manifest, prefix, output, true)?,
    )
}
fn legacy_plugin(path: &Path) -> Option<(String, String)> {
    let folder = path.file_name()?.to_str()?.strip_prefix("UE_")?;
    let (branch, fingerprint) = folder.rsplit_once('-')?;
    if fingerprint.len() != 16 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let branch = crate::model::EngineVersion::parse(branch)?.branch();
    Some((branch, path.parent()?.file_name()?.to_str()?.to_owned()))
}
fn original_plugin_index(path: &Path, manifest: &Manifest) -> Result<Option<FileIndex>, String> {
    let Some((_, name)) = legacy_plugin(path) else {
        return Ok(None);
    };
    let fingerprint = crate::fab::plugin_download::cache_fingerprint(manifest)?;
    let windows_fingerprint = crate::fab::plugin_download::windows_cache_fingerprint(manifest)?;
    let folder = path.file_name().unwrap_or_default().to_string_lossy();
    let windows_only = folder.ends_with(&format!("-{}", &windows_fingerprint[..16]));
    if !windows_only && !folder.ends_with(&format!("-{}", &fingerprint[..16])) {
        return Ok(None);
    }
    let descriptors: Vec<_> = manifest
        .files
        .iter()
        .filter(|f| f.name.ends_with(".uplugin"))
        .collect();
    if descriptors.len() != 1 {
        return Ok(None);
    }
    let descriptor = installer::relative(&descriptors[0].name)?;
    if descriptor.file_stem().and_then(|s| s.to_str()) != Some(name.as_str()) {
        return Ok(None);
    }
    Ok(Some(download_index(
        manifest,
        descriptor.parent().ok_or("Invalid plugin root")?,
        Path::new(&name),
        windows_only,
    )?))
}
fn expected_index_path(path: &Path) -> PathBuf {
    crate::storage::data_dir()
        .join("cache-expected-files")
        .join(format!(
            "{}.json",
            buildpatch::hex(&buildpatch::digest(key(path).as_bytes()))
        ))
}
fn legacy_plugin_identity(path: &Path, gate: &Gate) -> Result<Entry, String> {
    let (_, name) = legacy_plugin(path).ok_or("Not a recognized plugin download")?;
    let descriptor_path = path.join(&name).join(format!("{name}.uplugin"));
    let descriptor: serde_json::Value =
        serde_json::from_slice(&read_small(&descriptor_path, 2 * 1024 * 1024, gate)?)
            .map_err(|_| "Cannot read the cached plugin descriptor")?;
    Ok(Entry {
        root: PathBuf::new(),
        path: path.into(),
        title: descriptor["FriendlyName"].as_str().unwrap_or(&name).into(),
        bytes: 0,
        identities: vec![],
        removable: false,
        incomplete: false,
    })
}
fn recover_original_plugin_index(
    path: &Path,
    gate: &Gate,
    session: &crate::epic::Session,
    items: &[crate::fab::Item],
    fetched: &mut HashMap<String, Vec<Manifest>>,
) -> Result<Option<FileIndex>, String> {
    let Some((branch, _)) = legacy_plugin(path) else {
        return Ok(None);
    };
    let entry = legacy_plugin_identity(path, gate)?;
    let item = crate::vault_cache::match_item(&entry, items).filter(|i| i.plugin()).ok_or("Original file hashes are missing and this plugin could not be matched to your Epic library.")?;
    gate.1.lock().unwrap().phase = "Retrieving original file hashes from Epic".into();
    let cache_key = format!("{}:{branch}", item.key());
    if !fetched.contains_key(&cache_key) {
        let mut manifests = Vec::new();
        for version in item
            .project_versions
            .iter()
            .filter(|v| v.supports_windows_engine(&branch))
        {
            gate.check()?;
            let (manifest, _) = crate::fab::package(session, item, version)?;
            gate.check()?;
            if manifest.app != version.artifact_id {
                return Err("Epic manifest identity does not match the requested plugin.".into());
            }
            manifests.push(manifest);
        }
        fetched.insert(cache_key.clone(), manifests);
    }
    for manifest in &fetched[&cache_key] {
        if let Some(index) = original_plugin_index(path, manifest)? {
            // Save reference metadata outside the user's cached payload. Never invent
            // a trusted hash by hashing the files that are being checked.
            crate::storage::write_json(&expected_index_path(path), &index)?;
            return Ok(Some(index));
        }
    }
    Err("Epic no longer supplies the original build's file hashes. This download cannot be verified against a different build.".into())
}

#[derive(Clone)]
pub enum Selection {
    All,
    Entry(PathBuf),
    Item(crate::fab::Item),
}

fn files(dir: &Path, gate: &Gate, out: &mut Vec<PathBuf>, depth: usize) -> Result<(), String> {
    gate.check()?;
    if depth > 128 || out.len() >= 1_000_000 {
        return Err("Cache folder exceeds verification limits.".into());
    }
    installer::no_links(dir)?;
    let meta = fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    if meta.is_file() {
        out.push(dir.to_owned());
        return Ok(());
    }
    for child in fs::read_dir(dir).map_err(|e| e.to_string())? {
        files(
            &child.map_err(|e| e.to_string())?.path(),
            gate,
            out,
            depth + 1,
        )?;
    }
    Ok(())
}
fn read_small(path: &Path, limit: u64, gate: &Gate) -> Result<Vec<u8>, String> {
    use std::io::Read;
    gate.check()?;
    installer::no_links(path)?;
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > limit {
        return Err("Verification metadata exceeds size limit.".into());
    }
    gate.begin_file(path, file.metadata().map_err(|e| e.to_string())?.len());
    let mut raw = Vec::new();
    let mut buf = [0; 128 * 1024];
    loop {
        gate.check()?;
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if raw.len() as u64 + n as u64 > limit {
            return Err("Verification file exceeds size limit.".into());
        }
        gate.read(n as u64)?;
        raw.extend_from_slice(&buf[..n]);
    }
    Ok(raw)
}
fn file_ok(path: &Path, expected: &SavedFile, gate: &Gate) -> Result<bool, String> {
    gate.check()?;
    gate.begin_file(path, expected.size);
    installer::no_links(path)?;
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
    };
    if !meta.is_file() || meta.len() != expected.size {
        return Ok(false);
    }
    let hash = installer::hash_file_progress(path, &mut |n| gate.read(n))?;
    let after = fs::metadata(path).map_err(|e| e.to_string())?;
    if after.len() != meta.len() || after.modified().ok() != meta.modified().ok() {
        return Err("Files changed during verification; check again when downloads finish.".into());
    }
    Ok(hash == expected.sha)
}
fn unknown(record: &mut Record, reason: impl Into<String>) {
    record.health = Health::Unknown;
    record.detail = reason.into();
}
fn verdict(record: &mut Record, bad: u64) {
    record.health = if bad == 0 {
        Health::Healthy
    } else {
        Health::Redownload
    };
    record.detail = if bad == 0 {
        format!(
            "Verified {} cached files against their hashes.",
            record.files
        )
    } else {
        format!(
            "{bad} cached files are missing or damaged. Download this package again; installed projects are not checked or changed."
        )
    };
}

fn verify_entry(
    root: &Path,
    path: &Path,
    selection: &Selection,
    gate: &Gate,
    recover: &mut impl FnMut(&Path, &Gate) -> Result<Option<FileIndex>, String>,
) -> Result<Option<Record>, String> {
    let title = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut record = Record {
        root: root.into(),
        path: path.into(),
        title,
        identities: vec![],
        health: Health::Unknown,
        detail: "No supported verification manifest or file hashes are available.".into(),
        checked_at: crate::epic::now(),
        files: 0,
    };
    let mut paths = Vec::new();
    if let Err(error) = files(path, gate, &mut paths, 0) {
        gate.check()?;
        unknown(&mut record, format!("Not verified: {error}"));
        return Ok(Some(record));
    }
    let index_paths = paths
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == INDEX));
    let mut indexes = Vec::new();
    let mut invalid_index = false;
    let mut manifests = Vec::new();
    for index_path in index_paths {
        match read_small(index_path, 64 * 1024 * 1024, gate)
            .and_then(|raw| serde_json::from_slice::<FileIndex>(&raw).map_err(|e| e.to_string()))
        {
            Ok(value) if value.schema == 1 && !value.files.is_empty() => {
                record.identities.push(value.artifact.clone());
                indexes.push((index_path.parent().unwrap(), value));
            }
            _ => {
                invalid_index = true;
                unknown(
                    &mut record,
                    "The saved file-hash receipt cannot be read. Not verified.",
                );
            }
        }
    }
    if indexes.is_empty() && !invalid_index && legacy_plugin(path).is_some() {
        let reference = expected_index_path(path);
        let saved = if reference.is_file() {
            read_small(&reference, 64 * 1024 * 1024, gate)
                .ok()
                .and_then(|raw| serde_json::from_slice::<FileIndex>(&raw).ok())
                .filter(|i| i.schema == 1 && !i.files.is_empty())
        } else {
            None
        };
        if saved.is_none() {
            if let Selection::Item(item) = selection {
                let matches = legacy_plugin_identity(path, gate)
                    .ok()
                    .is_some_and(|entry| {
                        crate::vault_cache::match_item(&entry, std::slice::from_ref(item)).is_some()
                    });
                if !matches {
                    return Ok(None);
                }
            }
        }
        let recovered = match saved {
            Some(index) => Some(index),
            None => match recover(path, gate) {
                Ok(index) => index,
                Err(error) => {
                    gate.check()?;
                    unknown(&mut record, error);
                    return Ok(Some(record));
                }
            },
        };
        if let Some(index) = recovered {
            record.identities.push(index.artifact.clone());
            indexes.push((path, index));
        } else {
            unknown(
                &mut record,
                "Original file hashes are missing. Connect your Epic account and choose Verify cached files to retrieve them without downloading the plugin again.",
            );
        }
    }
    for path in paths
        .iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n == "manifest" || p.extension().is_some_and(|e| e == "manifest"))
        })
        .take(16)
    {
        if let Ok(raw) = read_small(path, 128 * 1024 * 1024, gate) {
            if let Ok(manifest) = buildpatch::parse(&raw) {
                record.identities.push(manifest.app.clone());
                manifests.push((path, manifest));
            }
        }
        gate.check()?;
    }
    if let Selection::Item(item) = selection {
        let entry = Entry {
            root: root.into(),
            path: path.into(),
            title: record.title.clone(),
            bytes: 0,
            identities: record.identities.clone(),
            removable: false,
            incomplete: false,
        };
        if crate::vault_cache::match_item(&entry, std::slice::from_ref(item)).is_none() {
            return Ok(None);
        }
    }
    if invalid_index {
        return Ok(Some(record));
    }
    if !indexes.is_empty() {
        gate.checking_files(indexes.iter().map(|(_, index)| index.files.len()).sum());
        let mut bad = 0;
        let mut seen = HashSet::new();
        for (base, index) in indexes {
            for file in index.files {
                let relative = installer::relative(&file.name)?;
                let target = base.join(relative);
                if !seen.insert(key(&target)) {
                    unknown(&mut record, "Ambiguous file-hash receipt. Not verified.");
                    return Ok(Some(record));
                }
                match file_ok(&target, &file, gate) {
                    Ok(ok) => {
                        bad += u64::from(!ok);
                        record.files += 1;
                        gate.checked_file();
                    }
                    Err(error) => {
                        gate.check()?;
                        unknown(&mut record, format!("Not verified: {error}"));
                        return Ok(Some(record));
                    }
                }
            }
        }
        verdict(&mut record, bad);
    } else if path.ends_with("EngineWorkspace/Chunks") {
        gate.checking_files(paths.len());
        let mut bad = 0;
        for path in &paths {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.len() != 40 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                unknown(
                    &mut record,
                    "Some shared chunks have no usable hash. Not verified.",
                );
                return Ok(Some(record));
            }
            let mut sha = [0; 20];
            for (i, b) in sha.iter_mut().enumerate() {
                *b = u8::from_str_radix(&name[2 * i..2 * i + 2], 16).unwrap();
            }
            let file = SavedFile {
                name: name.into(),
                sha,
                size: fs::metadata(path).map_err(|e| e.to_string())?.len(),
            };
            bad += u64::from(!file_ok(path, &file, gate)?);
            record.files += 1;
            gate.checked_file();
        }
        if record.files > 0 {
            verdict(&mut record, bad);
        }
        record.title = "Shared download chunks".into();
    } else if !manifests.is_empty() {
        let mut by_chunk = HashMap::new();
        for path in &paths {
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            let id = stem.rsplit('_').next().unwrap_or(&stem);
            if [32, 40].contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit()) {
                if by_chunk.insert(id.to_owned(), path).is_some() {
                    unknown(
                        &mut record,
                        "Duplicate chunk identities make this cache ambiguous. Not verified.",
                    );
                    return Ok(Some(record));
                }
            }
        }
        let mut bad = 0;
        let mut payload_found = false;
        gate.checking_files(
            manifests
                .iter()
                .map(|(_, manifest)| {
                    let required: HashSet<_> = manifest
                        .files
                        .iter()
                        .flat_map(|f| f.parts.iter().map(|p| &p.guid))
                        .collect();
                    if required
                        .iter()
                        .any(|id| by_chunk.contains_key(&id.to_ascii_lowercase()))
                    {
                        required.len()
                    } else {
                        manifest.files.len()
                    }
                })
                .sum(),
        );
        for (manifest_path, manifest) in manifests {
            let required: HashSet<_> = manifest
                .files
                .iter()
                .flat_map(|f| f.parts.iter().map(|p| &p.guid))
                .collect();
            let chunk_mode = required
                .iter()
                .any(|id| by_chunk.contains_key(&id.to_ascii_lowercase()));
            if chunk_mode {
                payload_found = true;
                for id in required {
                    gate.check()?;
                    let Some(chunk) = manifest.chunks.get(id) else {
                        unknown(&mut record, "Incomplete manifest. Not verified.");
                        return Ok(Some(record));
                    };
                    let Some(chunk_path) = by_chunk.get(&id.to_ascii_lowercase()) else {
                        bad += 1;
                        record.files += 1;
                        gate.checked_file();
                        continue;
                    };
                    if chunk.window > 64 * 1024 * 1024 {
                        unknown(&mut record, "Unsupported chunk size. Not verified.");
                        return Ok(Some(record));
                    }
                    let raw = read_small(chunk_path, 68 * 1024 * 1024, gate)?;
                    let ok = if raw.starts_with(&0xb1fe3aa2u32.to_le_bytes()) {
                        buildpatch::decode_chunk(&raw, chunk).is_ok()
                    } else {
                        buildpatch::valid_chunk_data(&raw, chunk)
                    };
                    bad += u64::from(!ok);
                    record.files += 1;
                    gate.checked_file();
                }
            } else {
                let base = manifest_path.parent().unwrap();
                let mut resolved = Vec::new();
                let mut manifest_payload_found = false;
                for file in &manifest.files {
                    let relative = installer::relative(&file.name)?;
                    let candidates = [
                        path.join(&relative),
                        base.join(&relative),
                        path.join("data").join(&relative),
                    ];
                    let target = candidates.into_iter().find(|p| p.is_file());
                    manifest_payload_found |= target.is_some();
                    resolved.push((file, target));
                }
                if manifest_payload_found {
                    payload_found = true;
                    for (file, target) in resolved {
                        let expected = SavedFile {
                            name: file.name.clone(),
                            sha: file.sha,
                            size: file.size(),
                        };
                        bad += u64::from(match target {
                            Some(p) => !file_ok(&p, &expected, gate)?,
                            None => true,
                        });
                        record.files += 1;
                        gate.checked_file();
                    }
                }
            }
        }
        if payload_found && record.files > 0 {
            verdict(&mut record, bad);
        } else {
            if paths.iter().all(|p| {
                p.file_name().is_some_and(|n| {
                    n == "manifest"
                        || p.extension()
                            .is_some_and(|e| e == "manifest" || e == "mancpn")
                })
            }) {
                record.health = Health::MetadataOnly;
                record.detail = "Download metadata only. This folder contains a package manifest, not downloaded asset or plugin files. There is no package payload here to verify.".into();
            } else {
                unknown(
                    &mut record,
                    "Manifest found, but no supported package payload is stored here. Not verified.",
                );
            }
        }
    }
    gate.check()?;
    record.checked_at = crate::epic::now();
    Ok(Some(record))
}

fn entries(root: &Path, dir: &Path, gate: &Gate, out: &mut Vec<PathBuf>) -> Result<(), String> {
    gate.check()?;
    installer::no_links(dir)?;
    if !dir.exists() {
        return Ok(());
    }
    for child in fs::read_dir(dir).map_err(|e| e.to_string())? {
        gate.check()?;
        let path = child.map_err(|e| e.to_string())?.path();
        installer::no_links(&path)?;
        if !path.is_dir() {
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
        let name = relative
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "engineworkspace" | "engineworkspace/plugins" | "fablibrary"
        ) || (name.starts_with("engineworkspace/plugins/") && relative.components().count() == 3)
        {
            entries(root, &path, gate, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

pub fn verify_roots(
    roots: &[String],
    selection: &Selection,
    gate: &Gate,
    progress: impl FnMut(&Path),
) -> Result<Vec<Record>, String> {
    verify_roots_using(roots, selection, gate, progress, &mut |_, _| Ok(None))
}
/// Manual verification can retrieve missing original manifests; no payload is downloaded.
pub fn verify_roots_with_catalog(
    roots: &[String],
    selection: &Selection,
    gate: &Gate,
    progress: impl FnMut(&Path),
    session: &crate::epic::Session,
    items: &[crate::fab::Item],
) -> Result<Vec<Record>, String> {
    let mut fetched = HashMap::new();
    verify_roots_using(roots, selection, gate, progress, &mut |path, gate| {
        recover_original_plugin_index(path, gate, session, items, &mut fetched)
    })
}
fn verify_roots_using(
    roots: &[String],
    selection: &Selection,
    gate: &Gate,
    mut progress: impl FnMut(&Path),
    recover: &mut impl FnMut(&Path, &Gate) -> Result<Option<FileIndex>, String>,
) -> Result<Vec<Record>, String> {
    let mut records = Vec::new();
    *gate.1.lock().unwrap() = Progress {
        phase: "Finding cached downloads".into(),
        ..Default::default()
    };
    let mut candidates = Vec::new();
    let mut visited = HashSet::new();
    let mut roots = roots.to_vec();
    roots.sort_by_key(|r| key(Path::new(r)).len());
    let mut checked_roots: Vec<PathBuf> = Vec::new();
    for raw in &roots {
        gate.check()?;
        let requested = Path::new(raw);
        if !requested.is_absolute() || !requested.exists() {
            continue;
        }
        installer::no_links(requested)?;
        let root = fs::canonicalize(requested).map_err(|e| e.to_string())?;
        if checked_roots
            .iter()
            .any(|r| crate::discovery::inside(&root, r))
        {
            continue;
        }
        checked_roots.push(root.clone());
        if root.parent().is_none() {
            continue;
        }
        let mut paths = Vec::new();
        entries(&root, &root, gate, &mut paths)?;
        for path in paths {
            if !visited.insert(key(&path)) {
                continue;
            }
            if let Selection::Entry(selected) = selection {
                if key(selected) != key(&path) {
                    continue;
                }
            }
            if matches!(selection, Selection::Item(_)) && path.ends_with("EngineWorkspace/Chunks") {
                continue;
            }
            candidates.push((root.clone(), path));
        }
    }
    let total = candidates.len() as u64;
    for (index, (root, path)) in candidates.into_iter().enumerate() {
        {
            let mut snapshot = gate.1.lock().unwrap();
            let bytes_read = snapshot.bytes_read;
            *snapshot = Progress {
                entry: path.clone(),
                phase: "Finding verification metadata".into(),
                entries_completed: index as u64,
                entries_total: total,
                bytes_read,
                ..Default::default()
            };
        }
        gate.check()?;
        progress(&path);
        match verify_entry(&root, &path, selection, gate, recover) {
            Ok(Some(record)) => records.push(record),
            Ok(None) => {}
            Err(error) => {
                gate.check()?;
                records.push(Record {
                    root: root.clone(),
                    path,
                    title: "Cached download".into(),
                    identities: vec![],
                    health: Health::Unknown,
                    detail: format!("Not verified: {error}"),
                    checked_at: crate::epic::now(),
                    files: 0,
                });
            }
        }
        gate.1.lock().unwrap().entries_completed = index as u64 + 1;
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn original_manifest_recovery_distinguishes_full_and_windows_only_cache_folders() {
        let cache = tempfile::tempdir().unwrap();
        let manifest = crate::plugins::package_files::cached_fixture(cache.path(), "5.7.0");
        let all = crate::fab::plugin_download::cache_fingerprint(&manifest).unwrap();
        let windows = crate::fab::plugin_download::windows_cache_fingerprint(&manifest).unwrap();
        assert_ne!(
            all, windows,
            "Windows payload must not silently reuse an old full-platform folder"
        );
        for (fingerprint, expected) in [(all, 9), (windows, 5)] {
            let entry = cache.path().join(format!(
                "EngineWorkspace/Plugins/Test/UE_5.7-{}",
                &fingerprint[..16]
            ));
            let index = original_plugin_index(&entry, &manifest).unwrap().unwrap();
            assert_eq!(index.files.len(), expected);
            assert_eq!(
                index.files.iter().any(|f| f.name.contains("Binaries/Mac/")),
                expected == 9
            );
            assert!(
                index.files.iter().any(|f| f.name.contains("Source/Mac/")),
                "Preserve source code"
            );
            let mut wrong = manifest.clone();
            wrong.build = "another-build".into();
            assert!(original_plugin_index(&entry, &wrong).unwrap().is_none());
        }
    }

    #[test]
    fn read_progress_has_no_rate_delay_and_still_honors_stop() {
        let gate = Gate::default();
        let bytes = 64 * 1024 * 1024;
        gate.begin_file(Path::new("large-file"), bytes);
        let start = Instant::now();
        gate.read(bytes).unwrap();
        // The former 16 MiB/s limiter would block here for four seconds.
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(gate.progress().bytes_read, bytes);
        assert_eq!(gate.progress().file_bytes, bytes);
        gate.cancel();
        assert!(gate.read(1).is_err());
        assert_eq!(gate.progress().bytes_read, bytes);
    }

    #[test]
    fn mutation_scopes_survive_operations_that_finish_between_ui_polls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("New asset");
        let writer = changing(&path);
        assert!(
            take_changed_paths().contains(&path),
            "The start of a write must invalidate an active check"
        );
        drop(writer);
        assert!(
            take_changed_paths().contains(&path),
            "The committed or failed write must still be visible on the next UI poll"
        );
    }
    #[test]
    #[ignore = "Checks existing cached files without changing them, retrieves original Epic metadata; set CACHE_VERIFY_EXISTING_ENTRY"]
    fn existing_local_plugin_cache_can_be_verified() {
        let prefs = crate::storage::load_preferences();
        let selected = PathBuf::from(
            std::env::var("CACHE_VERIFY_EXISTING_ENTRY").expect("Set CACHE_VERIFY_EXISTING_ENTRY"),
        );
        let session = crate::storage::load_session().expect("Connect Epic first");
        let items = crate::fab::load_library_cache(&session.account_id);
        let gate = Gate::default();
        let reader = gate.clone();
        let worker = std::thread::spawn(move || {
            verify_roots_with_catalog(
                &prefs.vault_cache_roots(),
                &Selection::Entry(selected),
                &reader,
                |_| {},
                &session,
                &items,
            )
        });
        while !worker.is_finished() {
            let p = gate.progress();
            println!(
                "{}: {} / {} files; {:.1} MB read",
                p.phase,
                p.files_completed,
                p.files_total,
                p.bytes_read as f64 / 1_000_000.
            );
            std::thread::sleep(Duration::from_secs(5));
        }
        let records = worker.join().unwrap().unwrap();
        assert_eq!(records.len(), 1);
        assert!(
            records[0].health != Health::Unknown,
            "{}",
            records[0].detail
        );
        println!(
            "Result: {:?}; {} files checked; {}",
            records[0].health, records[0].files, records[0].detail
        );
        if std::env::var("CACHE_VERIFY_SAVE_REPORT").as_deref() == Ok("1") {
            let mut report = load();
            for record in records {
                report.records.insert(key(&record.path), record);
            }
            save(&report);
        }
    }
    #[test]
    fn old_download_uses_only_the_exact_original_package_and_detects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            app: "plugin-artifact".into(),
            build: "5.8.0-original".into(),
            files: vec![buildpatch::File {
                name: "Engine/Plugins/TestPlugin/TestPlugin.uplugin".into(),
                sha: buildpatch::digest(b"healthy"),
                parts: vec![buildpatch::Part {
                    size: 7,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let fingerprint = crate::fab::plugin_download::cache_fingerprint(&manifest).unwrap();
        let entry = dir.path().join(format!(
            "EngineWorkspace/Plugins/TestPlugin/UE_5.8-{}",
            &fingerprint[..16]
        ));
        fs::create_dir_all(entry.join("TestPlugin")).unwrap();
        let payload = entry.join("TestPlugin/TestPlugin.uplugin");
        fs::write(&payload, b"healthy").unwrap();
        assert!(!entry.join(INDEX).exists());
        let mut wrong = manifest.clone();
        wrong.build = "5.8.0-another-build".into();
        assert!(original_plugin_index(&entry, &wrong).unwrap().is_none());
        let roots = vec![dir.path().to_string_lossy().into_owned()];
        let check = || {
            verify_roots_using(
                &roots,
                &Selection::Entry(entry.clone()),
                &Gate::default(),
                |_| {},
                &mut |path, _| original_plugin_index(path, &manifest),
            )
            .unwrap()
        };
        assert_eq!(check()[0].health, Health::Healthy);
        let other_item: crate::fab::Item = serde_json::from_value(serde_json::json!({ "assetId": "other", "assetNamespace": "test", "title": "Unrelated plugin", "distributionMethod": "CODE_PLUGIN" })).unwrap();
        let mut requested = false;
        let unrelated = verify_roots_using(
            &roots,
            &Selection::Item(other_item),
            &Gate::default(),
            |_| {},
            &mut |_, _| {
                requested = true;
                Ok(None)
            },
        )
        .unwrap();
        assert!(unrelated.is_empty());
        assert!(
            !requested,
            "A selected-item check must not retrieve metadata for other plugins"
        );
        fs::write(&payload, b"damaged").unwrap();
        assert_eq!(check()[0].health, Health::Redownload);
        assert_eq!(fs::read(payload).unwrap(), b"damaged");
        assert!(
            !entry.join(INDEX).exists(),
            "Verification must not modify the cached download"
        );
    }
    #[test]
    fn large_file_exposes_byte_progress_before_the_entry_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let entry = fixture(dir.path());
        let payload = vec![42u8; 4 * 1024 * 1024];
        fs::write(entry.join("Content/example.uasset"), &payload).unwrap();
        crate::storage::write_json(
            &entry.join(INDEX),
            &FileIndex {
                schema: 1,
                artifact: "test-artifact".into(),
                build: "test".into(),
                files: vec![SavedFile {
                    name: "Content/example.uasset".into(),
                    sha: buildpatch::digest(&payload),
                    size: payload.len() as u64,
                }],
            },
        )
        .unwrap();
        let gate = Gate::default();
        let worker_gate = gate.clone();
        let roots = vec![dir.path().to_string_lossy().into_owned()];
        let worker =
            std::thread::spawn(move || verify_roots(&roots, &Selection::All, &worker_gate, |_| {}));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut streamed = false;
        while Instant::now() < deadline && !worker.is_finished() {
            let p = gate.progress();
            if p.files_total == 1
                && p.files_completed == 0
                && p.file_bytes > 0
                && p.file_bytes < p.file_total
            {
                streamed = true;
                assert!(p.file.ends_with("Content/example.uasset"));
                assert_eq!(p.entries_total, 1);
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(worker.join().unwrap().unwrap()[0].health, Health::Healthy);
        assert!(streamed, "No progress was exposed during a large file read");
        let p = gate.progress();
        assert_eq!(p.entries_completed, 1);
        assert_eq!(p.files_completed, 1);
        assert_eq!(
            p.file_total, 0,
            "Completed files must not count twice in progress"
        );
        assert!(p.bytes_read >= payload.len() as u64);
    }
    fn fixture(root: &Path) -> PathBuf {
        let entry = root.join("Saved asset");
        fs::create_dir_all(entry.join("Content")).unwrap();
        fs::write(entry.join("Content/example.uasset"), b"healthy").unwrap();
        crate::storage::write_json(
            &entry.join(INDEX),
            &FileIndex {
                schema: 1,
                artifact: "test-artifact".into(),
                build: "test".into(),
                files: vec![SavedFile {
                    name: "Content/example.uasset".into(),
                    sha: buildpatch::digest(b"healthy"),
                    size: 7,
                }],
            },
        )
        .unwrap();
        entry
    }
    fn run(root: &Path) -> Vec<Record> {
        verify_roots(
            &[root.to_string_lossy().into()],
            &Selection::All,
            &Gate::default(),
            |_| {},
        )
        .unwrap()
    }
    #[test]
    fn detects_same_size_corruption_and_missing_files_without_changing_them() {
        let dir = tempfile::tempdir().unwrap();
        let entry = fixture(dir.path());
        assert_eq!(run(dir.path())[0].health, Health::Healthy);
        fs::write(entry.join("Content/example.uasset"), b"damaged").unwrap();
        assert_eq!(run(dir.path())[0].health, Health::Redownload);
        assert_eq!(
            fs::read(entry.join("Content/example.uasset")).unwrap(),
            b"damaged"
        );
        fs::remove_file(entry.join("Content/example.uasset")).unwrap();
        assert_eq!(run(dir.path())[0].health, Health::Redownload);
        assert!(!entry.join("Content/example.uasset").exists());
    }
    #[test]
    fn unverifiable_and_malicious_receipts_never_turn_green_or_read_outside_cache() {
        let dir = tempfile::tempdir().unwrap();
        let entry = fixture(dir.path());
        fs::remove_file(entry.join(INDEX)).unwrap();
        assert_eq!(run(dir.path())[0].health, Health::Unknown);
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, b"healthy").unwrap();
        crate::storage::write_json(
            &entry.join(INDEX),
            &FileIndex {
                schema: 1,
                artifact: "test".into(),
                build: "test".into(),
                files: vec![SavedFile {
                    name: "../outside.txt".into(),
                    sha: buildpatch::digest(b"healthy"),
                    size: 7,
                }],
            },
        )
        .unwrap();
        assert_eq!(run(dir.path())[0].health, Health::Unknown);
        assert_eq!(fs::read(outside).unwrap(), b"healthy");
    }
    #[test]
    fn shared_chunks_are_checked_but_never_confused_with_complete_assets() {
        let dir = tempfile::tempdir().unwrap();
        let chunks = dir.path().join("EngineWorkspace/Chunks");
        fs::create_dir_all(&chunks).unwrap();
        let path = chunks.join(buildpatch::hex(&buildpatch::digest(b"good")));
        fs::write(&path, b"good").unwrap();
        let records = run(dir.path());
        assert_eq!(records[0].health, Health::Healthy);
        assert!(records[0].identities.is_empty());
        fs::write(path, b"oops").unwrap();
        assert_eq!(run(dir.path())[0].health, Health::Redownload);
    }
    #[test]
    fn multiple_locations_and_manual_selection_stay_separate() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let first = fixture(a.path());
        let second = fixture(b.path());
        fs::write(second.join("Content/example.uasset"), b"damaged").unwrap();
        let roots = [
            a.path().to_string_lossy().into(),
            b.path().to_string_lossy().into(),
        ];
        let records =
            verify_roots(&roots, &Selection::Entry(first), &Gate::default(), |_| {}).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].health, Health::Healthy);
        let item: crate::fab::Item = serde_json::from_value(serde_json::json!({
            "assetId":"asset", "assetNamespace":"namespace", "title":"Other title", "distributionMethod":"ASSET_PACK",
            "projectVersions":[{"artifactId":"test-artifact", "engineVersions":["5.8"]}]
        })).unwrap();
        let records =
            verify_roots(&roots, &Selection::Item(item), &Gate::default(), |_| {}).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records
                .iter()
                .filter(|r| r.health == Health::Healthy)
                .count(),
            1
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| r.health == Health::Redownload)
                .count(),
            1
        );
    }
    #[test]
    fn every_saved_package_in_a_cache_entry_must_pass_before_it_can_be_green() {
        let dir = tempfile::tempdir().unwrap();
        let entry = fixture(dir.path());
        fixture(&entry);
        assert_eq!(run(dir.path())[0].health, Health::Healthy);
        fs::write(entry.join("Saved asset/Content/example.uasset"), b"damaged").unwrap();
        assert_eq!(run(dir.path())[0].health, Health::Redownload);
    }
    #[test]
    fn paused_worker_blocks_until_resumed_and_cancel_wakes_it() {
        let gate = Gate::default();
        gate.pause(true);
        let worker = gate.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            tx.send(worker.check()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
        gate.pause(false);
        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap().is_ok());
        handle.join().unwrap();
        gate.pause(true);
        let worker = gate.clone();
        let handle = std::thread::spawn(move || worker.check());
        gate.cancel();
        assert!(handle.join().unwrap().is_err());
    }
    #[test]
    fn daily_schedule_and_saved_report_round_trip() {
        assert!(due(0, 100));
        assert!(!due(100, 100 + DAY - 1));
        assert!(due(100, 100 + DAY));
        assert!(due(200, 100));
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let mut report = Report {
            roots: vec![dir.path().to_string_lossy().into()],
            finished_all_at: 100,
            ..Default::default()
        };
        for record in run(dir.path()) {
            report.records.insert(key(&record.path), record);
        }
        let saved: Report = serde_json::from_slice(&serde_json::to_vec(&report).unwrap()).unwrap();
        assert_eq!(saved.finished_all_at, 100);
        assert_eq!(
            saved.records.values().next().unwrap().health,
            Health::Healthy
        );
    }
    fn epic_manifest(data: &[u8]) -> Vec<u8> {
        fn blob(data: &[u8]) -> String {
            data.iter().map(|b| format!("{b:03}")).collect()
        }
        let guid = "00000001000000020000000300000004";
        serde_json::to_vec(&serde_json::json!({
            "ManifestFileVersion":"007000000000", "bIsFileData":false, "AppNameString":"test-artifact", "BuildVersionString":"test",
            "ChunkHashList":{guid:blob(&123u64.to_le_bytes())}, "ChunkShaList":{guid:buildpatch::hex(&buildpatch::digest(data))},
            "DataGroupList":{guid:"001"}, "FileManifestList":[{"Filename":"Content/example.uasset", "FileHash":blob(&buildpatch::digest(data)),
                "FileChunkParts":[{"Guid":guid, "Offset":"000000000000", "Size":blob(&(data.len() as u32).to_le_bytes())}]}]
        })).unwrap()
    }
    #[test]
    fn epic_manifest_checks_extracted_files_and_chunk_payloads_and_metadata_only_is_neutral() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("Epic asset");
        fs::create_dir(&entry).unwrap();
        let data = vec![7u8; 1024 * 1024];
        fs::write(entry.join("manifest"), epic_manifest(&data)).unwrap();
        assert_eq!(run(dir.path())[0].health, Health::MetadataOnly);
        fs::create_dir(entry.join("Content")).unwrap();
        fs::write(entry.join("Content/example.uasset"), &data).unwrap();
        assert_eq!(run(dir.path())[0].health, Health::Healthy);
        fs::remove_file(entry.join("Content/example.uasset")).unwrap();
        let chunk = entry.join("00000001000000020000000300000004.chunk");
        fs::write(&chunk, &data).unwrap();
        assert_eq!(run(dir.path())[0].health, Health::Healthy);
        fs::write(chunk, vec![8u8; data.len()]).unwrap();
        assert_eq!(run(dir.path())[0].health, Health::Redownload);
    }
}
