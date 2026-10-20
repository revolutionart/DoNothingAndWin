use crate::{
    buildpatch::{self, Chunk, Manifest},
    epic::{Epic, Session},
    model::Release,
};
use reqwest::{Url, blocking::Client};
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
#[derive(Clone)]
pub struct Plan {
    pub manifest: Manifest,
    pub bases: Vec<Url>,
    pub target: PathBuf,
    pub tags: Vec<String>,
    pub selected: Vec<String>,
}
impl Plan {
    pub fn files(&self) -> Vec<&buildpatch::File> {
        self.manifest
            .files
            .iter()
            .filter(|f| f.tags.is_empty() || f.tags.iter().any(|t| self.selected.contains(t)))
            .collect()
    }
    pub fn sizes(&self) -> (u64, u64, u64) {
        let files = self.files();
        let ids: HashSet<_> = files
            .iter()
            .flat_map(|f| f.parts.iter().map(|p| &p.guid))
            .collect();
        (
            files.iter().map(|f| f.size()).sum(),
            ids.iter().map(|id| self.manifest.chunks[*id].size).sum(),
            ids.iter()
                .map(|id| self.manifest.chunks[*id].window as u64)
                .sum(),
        )
    }
}
#[derive(Clone, Default)]
pub struct Progress {
    pub phase: String,
    pub detail: String,
    pub completed: u64,
    pub total: u64,
    pub network: u64,
    pub read: u64,
    pub written: u64,
    pub package_bytes: u64,
    pub files_total: usize,
    pub files: usize,
}
#[derive(Clone, Default)]
pub struct Control {
    pub cancel: Arc<AtomicBool>,
}
impl Control {
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(
                "Stopped. Verified files and chunks are retained; restart the operation with the same destination."
                    .into(),
            )
        } else {
            Ok(())
        }
    }
}
pub fn client() -> Result<Client, String> {
    static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .user_agent("EngineWorkspace/0.1")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| client).clone())
}

fn valid_cdn(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|h| {
            h == "epicgames.com"
                || h == "media.fab.com"
                || h == "cdn.quixel.com"
                || h == "egdownload.fastly-edge.com"
                || h == "epicgames-download1.akamaized.net"
                || h.ends_with(".epicgames.com")
                || h.ends_with(".epicgamescdn.com")
        })
}
pub fn fetch(c: &Client, url: Url, limit: u64) -> Result<Vec<u8>, String> {
    fetch_progress(c, url, limit, &mut |_| {})
}
fn fetch_progress(
    c: &Client,
    url: Url,
    limit: u64,
    received: &mut impl FnMut(u64),
) -> Result<Vec<u8>, String> {
    if !valid_cdn(&url) {
        return Err("Unexpected CDN host or protocol".into());
    }
    let r = c.get(url).send().map_err(|_| "CDN connection failed")?;
    if !r.status().is_success() {
        return Err(format!("CDN returned HTTP {}", r.status().as_u16()));
    }
    if r.content_length().is_some_and(|n| n > limit) {
        return Err("CDN response exceeds size limit".into());
    }
    let mut data = Vec::new();
    let mut stream = r.take(limit + 1);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = stream
            .read(&mut buffer)
            .map_err(|_| "CDN transfer interrupted")?;
        if n == 0 {
            break;
        }
        received(n as u64);
        crate::download_policy::throttle(n);
        data.extend_from_slice(&buffer[..n]);
    }
    if data.len() as u64 > limit {
        return Err("Oversized CDN response".into());
    }
    Ok(data)
}
pub fn prepare(session: &Session, release: &Release, parent: &Path) -> Result<Plan, String> {
    let urls = Epic::new()?.manifest_locations(session, release)?;
    let c = client()?;
    let mut error = "No valid engine manifest".to_string();
    let mut parsed = None;
    let mut original = Vec::new();
    let mut bases = Vec::new();
    for u in &urls {
        if valid_cdn(u) {
            let mut base = u.join(".").map_err(|_| "Invalid CDN base")?;
            base.set_query(None);
            bases.push(base);
        }
    }
    for url in urls {
        match fetch(&c, url, 128 * 1024 * 1024).and_then(|b| {
            let m = buildpatch::parse(&b)?;
            original = b;
            Ok(m)
        }) {
            Ok(m) => {
                parsed = Some(m);
                break;
            }
            Err(e) => error = e,
        }
    }
    let manifest = parsed.ok_or(error)?;
    if manifest.app != release.app {
        return Err("Manifest belongs to a different application".into());
    }
    if release.build.as_ref().is_some_and(|b| *b != manifest.build) {
        return Err(
            "Engine build changed since release check. Refresh releases and try again.".into(),
        );
    }
    validate_manifest(&manifest)?;
    crate::epic_registration::cache(release, &manifest, &original)?;
    let mut tags: Vec<_> = manifest.files.iter().flat_map(|f| f.tags.clone()).collect();
    tags.sort();
    tags.dedup();
    let default = parent.join(&manifest.app);
    let target = if default.exists() {
        parent.join(format!(
            "{}-{}-Workspace",
            manifest.app,
            crate::model::EngineVersion::parse(&manifest.build).ok_or("Invalid engine version")?
        ))
    } else {
        default
    };
    Ok(Plan {
        manifest,
        bases,
        target,
        tags: tags.clone(),
        selected: tags
            .into_iter()
            .filter(|t| matches!(t.as_str(), "engine_source" | "templates"))
            .collect(),
    })
}
pub fn relative(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.starts_with(['/', '\\']) || name.contains([':', '\0']) {
        return Err(format!("Unsafe manifest path: {name}"));
    }
    let mut out = PathBuf::new();
    for p in name.split(['/', '\\']) {
        if p.is_empty()
            || p == "."
            || p == ".."
            || p.ends_with(['.', ' '])
            || p.chars().any(|c| c < ' ' || "<>\"|?*".contains(c))
        {
            return Err("Unsafe manifest path component".into());
        }
        let stem = p.split('.').next().unwrap().to_ascii_uppercase();
        if [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(&stem.as_str())
        {
            return Err("Reserved Windows path".into());
        }
        out.push(p);
    }
    Ok(out)
}
pub(crate) fn validate_manifest(m: &Manifest) -> Result<(), String> {
    let mut seen = HashSet::new();
    for f in &m.files {
        relative(&f.name)?;
        if !seen.insert(f.name.replace('\\', "/").to_lowercase()) {
            return Err("Duplicate manifest file".into());
        }
    }
    if !m
        .files
        .iter()
        .any(|f| f.name.ends_with("/UnrealEditor.exe") || f.name.ends_with("/UE4Editor.exe"))
    {
        return Err("Manifest does not contain an Unreal Editor".into());
    }
    Ok(())
}
pub(crate) fn no_links(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for c in path.components() {
        current.push(c);
        if let Ok(m) = fs::symlink_metadata(&current) {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if m.file_attributes() & 0x400 != 0 {
                    return Err("Installation paths cannot pass through links or junctions".into());
                }
            }
            if m.file_type().is_symlink() {
                return Err("Installation paths cannot pass through symbolic links".into());
            }
        }
    }
    Ok(())
}
fn hash_file(path: &Path) -> Result<[u8; 20], String> {
    hash_file_progress(path, &mut |_| Ok(()))
}
pub(crate) fn hash_file_progress(
    path: &Path,
    received: &mut impl FnMut(u64) -> Result<(), String>,
) -> Result<[u8; 20], String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha1::new();
    let mut buf = [0u8; 128 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        received(n as u64)?;
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().into())
}
pub(crate) fn valid_file(path: &Path, hash: &[u8; 20], size: u64) -> bool {
    fs::metadata(path).is_ok_and(|m| m.len() == size) && hash_file(path).is_ok_and(|h| h == *hash)
}
pub(crate) fn valid_cached_chunk(path: &Path, chunk: &Chunk) -> bool {
    fs::metadata(path).is_ok_and(|m| m.len() == chunk.window as u64)
        && fs::read(path).is_ok_and(|b| buildpatch::valid_chunk_data(&b, chunk))
}
// Count completed payload I/O without another verification pass. Memory-cache hits
// and metadata operations do not contribute to storage throughput.
fn read_payload(path: &Path, progress: &Mutex<Progress>) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        progress.lock().unwrap().read += n as u64;
        bytes.extend_from_slice(&buffer[..n]);
    }
    Ok(bytes)
}
fn write_payload(
    out: &mut impl Write,
    mut bytes: &[u8],
    progress: &Mutex<Progress>,
) -> Result<(), String> {
    while !bytes.is_empty() {
        match out.write(&bytes[..bytes.len().min(128 * 1024)]) {
            Ok(0) => return Err("Could not write package data".into()),
            Ok(n) => {
                progress.lock().unwrap().written += n as u64;
                bytes = &bytes[n..];
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
pub(crate) fn chunk_data(
    client: &Client,
    chunk: &Chunk,
    bases: &[Url],
    feature: u32,
    control: &Control,
) -> Result<Vec<u8>, String> {
    chunk_data_progress(client, chunk, bases, feature, control, &mut |_| {})
}
pub(crate) fn chunk_data_progress(
    client: &Client,
    chunk: &Chunk,
    bases: &[Url],
    feature: u32,
    control: &Control,
    received: &mut impl FnMut(u64),
) -> Result<Vec<u8>, String> {
    let mut error = "No working CDN".to_string();
    for attempt in 0..3 {
        for base in bases {
            control.check()?;
            let mut url = base
                .join(&chunk.path(feature))
                .map_err(|_| "Invalid chunk location")?;
            url.set_query(base.query());
            match fetch_progress(client, url, 64 * 1024 * 1024, received)
                .and_then(|b| buildpatch::decode_chunk(&b, chunk))
            {
                Ok(b) => return Ok(b),
                Err(e) => error = e,
            }
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(500 * (attempt + 1)));
        }
    }
    Err(error)
}
pub fn install(
    plan: &Plan,
    control: &Control,
    progress: Arc<Mutex<Progress>>,
) -> Result<PathBuf, String> {
    install_from(plan, control, progress, None)
}

/// Build a complete replacement beside the installed engine, reusing verified files.
/// Registration happens only after the caller commits it at the original location.
pub(crate) fn stage_update(
    plan: &Plan,
    source: &Path,
    control: &Control,
    progress: Arc<Mutex<Progress>>,
) -> Result<PathBuf, String> {
    install_from(plan, control, progress, Some(source))
}

fn install_from(
    plan: &Plan,
    control: &Control,
    progress: Arc<Mutex<Progress>>,
    source: Option<&Path>,
) -> Result<PathBuf, String> {
    crate::download_policy::check_install_allowed()?;
    validate_manifest(&plan.manifest)?;
    control.check()?;
    let parent = plan
        .target
        .parent()
        .ok_or("Choose an installation parent folder")?;
    if !parent.is_absolute() {
        return Err("Choose an absolute installation path".into());
    }
    no_links(parent)?;
    fs::create_dir_all(parent).map_err(|e| format!("Cannot create install folder: {e}"))?;
    no_links(&plan.target)?;
    if plan.target.exists()
        && (!plan.target.is_dir()
            || fs::read_dir(&plan.target)
                .map_err(|e| e.to_string())?
                .next()
                .is_some())
    {
        return Err("Destination must be empty. Existing engines are never overwritten.".into());
    }
    let key = buildpatch::hex(&buildpatch::digest(
        format!("{}:{}", plan.manifest.app, plan.manifest.build).as_bytes(),
    ));
    let work = parent.join(format!(".engine-workspace-{}", &key[..16]));
    no_links(&work)?;
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(work.join("install.lock"))
        .map_err(|e| e.to_string())?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| "Another installer is using this build")?;
    let stage = work.join("files");
    let cache = work.join("chunks");
    no_links(&stage)?;
    no_links(&cache)?;
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let files = plan.files();
    if !files
        .iter()
        .any(|f| f.name.ends_with("/UnrealEditor.exe") || f.name.ends_with("/UE4Editor.exe"))
    {
        return Err("Selected components exclude Unreal Editor".into());
    }
    let (install_size, package_bytes, _) = plan.sizes();
    *progress.lock().unwrap() = Progress {
        phase: "Checking staging and available disk space".into(),
        package_bytes,
        files_total: files.len(),
        ..Default::default()
    };
    let free = fs2::available_space(parent).map_err(|e| e.to_string())?;
    crate::storage::write_json(
        &work.join("plan.json"),
        &serde_json::json!({"app":plan.manifest.app,"build":plan.manifest.build,"destination":plan.target,"selected_tags":plan.selected}),
    )?;
    let mut needed = HashSet::new();
    let mut pending = Vec::new();
    let mut verified = 0u64;
    for (index, f) in files.iter().enumerate() {
        if index % 128 == 0 {
            progress.lock().unwrap().phase =
                format!("Checking staged files: {} / {}", index, files.len());
        }
        control.check()?;
        let dest = stage.join(relative(&f.name)?);
        no_links(&dest)?;
        if !dest.exists() {
            if let Some(source) = source {
                let original = source.join(relative(&f.name)?);
                no_links(&original)?;
                if valid_file(&original, &f.sha, f.size()) {
                    fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
                    // Same-volume hard links avoid copying unchanged engine data.
                    // The installer replaces invalid staging files instead of writing through links.
                    fs::hard_link(&original, &dest).map_err(|e| {
                        format!("Cannot reuse installed engine file {}: {e}", f.name)
                    })?;
                }
            }
        }
        if fs::metadata(&dest).is_ok_and(|m| m.len() == f.size())
            && hash_file_progress(&dest, &mut |n| {
                progress.lock().unwrap().read += n;
                Ok(())
            })
            .is_ok_and(|hash| hash == f.sha)
        {
            verified += f.size();
            progress.lock().unwrap().files += 1;
        } else {
            pending.push(*f);
            needed.extend(f.parts.iter().map(|p| p.guid.clone()));
        }
    }
    let chunks: Vec<_> = needed
        .iter()
        .map(|id| plan.manifest.chunks[id].clone())
        .collect();
    let mut cached_bytes = 0u64;
    for ch in &chunks {
        let path = cache.join(&ch.guid);
        no_links(&path)?;
        if fs::metadata(&path).is_ok_and(|m| m.len() == ch.window as u64) {
            cached_bytes += ch.window as u64;
        }
    }
    let remaining_cache: u64 = chunks
        .iter()
        .map(|c| c.window as u64)
        .sum::<u64>()
        .saturating_sub(cached_bytes);
    let required = install_size
        .saturating_sub(verified)
        .saturating_add(remaining_cache)
        .saturating_add(1_073_741_824);
    if free < required {
        return Err(format!(
            "Insufficient free space: {:.1} GiB needed including staging; {:.1} GiB available",
            required as f64 / 1073741824.,
            free as f64 / 1073741824.
        ));
    }
    let total = chunks.iter().map(|c| c.size).sum();
    {
        let mut p = progress.lock().unwrap();
        p.phase = "Downloading and verifying chunks".into();
        p.total = total;
    }
    let next = AtomicUsize::new(0);
    let failure = Mutex::new(None::<String>);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                let client = match client() {
                    Ok(c) => c,
                    Err(e) => {
                        *failure.lock().unwrap() = Some(e);
                        return;
                    }
                };
                loop {
                    if control.check().is_err() || failure.lock().unwrap().is_some() {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(chunk) = chunks.get(i) else { break };
                    let path = cache.join(&chunk.guid);
                    let result = (|| {
                        no_links(&path)?;
                        let cached = fs::metadata(&path)
                            .is_ok_and(|m| m.len() == chunk.window as u64)
                            && read_payload(&path, &progress)
                                .is_ok_and(|bytes| buildpatch::valid_chunk_data(&bytes, chunk));
                        if !cached {
                            let bytes = chunk_data_progress(
                                &client,
                                chunk,
                                &plan.bases,
                                plan.manifest.feature,
                                control,
                                &mut |n| progress.lock().unwrap().network += n,
                            )?;
                            let tmp = path.with_extension("partial");
                            no_links(&tmp)?;
                            let mut out = File::create(&tmp).map_err(|e| e.to_string())?;
                            write_payload(&mut out, &bytes, &progress)?;
                            drop(out);
                            if path.exists() {
                                fs::remove_file(&path).map_err(|e| e.to_string())?;
                            }
                            fs::rename(tmp, &path).map_err(|e| e.to_string())?;
                        }
                        Ok::<_, String>(())
                    })();
                    if let Err(e) = result {
                        *failure.lock().unwrap() = Some(e);
                        break;
                    }
                    let mut p = progress.lock().unwrap();
                    p.completed += chunk.size;
                }
            });
        }
    });
    control.check()?;
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(format!("{e}. Resume to retry; verified data is retained."));
    }
    {
        let mut p = progress.lock().unwrap();
        p.phase = "Reconstructing and verifying engine files".into();
        p.completed = verified;
        p.total = install_size;
    }
    let mut chunk_memory: HashMap<String, Arc<Vec<u8>>> = HashMap::new();
    let mut chunk_order: VecDeque<String> = VecDeque::new();
    for f in pending {
        control.check()?;
        let dest = stage.join(relative(&f.name)?);
        no_links(&dest)?;
        let dir = dest.parent().unwrap();
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = work.join("assembling.partial");
        no_links(&tmp)?;
        let mut out = File::create(&tmp).map_err(|e| e.to_string())?;
        let mut hash = Sha1::new();
        for part in &f.parts {
            control.check()?;
            let ch = &plan.manifest.chunks[&part.guid];
            let bytes = if let Some(bytes) = chunk_memory.get(&part.guid) {
                bytes.clone()
            } else {
                let bytes = read_payload(&cache.join(&part.guid), &progress)?;
                if !buildpatch::valid_chunk_data(&bytes, ch) {
                    return Err("Cached chunk integrity failed; resume to repair".into());
                }
                while !chunk_memory.is_empty()
                    && (chunk_memory.len() >= 64
                        || chunk_memory.values().map(|v| v.len()).sum::<usize>() + bytes.len()
                            > 64 * 1024 * 1024)
                {
                    if let Some(old) = chunk_order.pop_front() {
                        chunk_memory.remove(&old);
                    }
                }
                let bytes = Arc::new(bytes);
                chunk_memory.insert(part.guid.clone(), bytes.clone());
                chunk_order.push_back(part.guid.clone());
                bytes
            };
            let piece = bytes
                .get(part.offset as usize..part.offset as usize + part.size as usize)
                .ok_or("Invalid chunk extent")?;
            write_payload(&mut out, piece, &progress)?;
            hash.update(piece);
        }
        let actual: [u8; 20] = hash.finalize().into();
        if actual != f.sha {
            return Err(format!("File verification failed: {}", f.name));
        }
        out.sync_all().map_err(|e| e.to_string())?;
        drop(out);
        if dest.exists() {
            fs::remove_file(&dest).map_err(|e| e.to_string())?;
        }
        fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
        let mut p = progress.lock().unwrap();
        p.completed += f.size();
        p.files += 1;
    }
    control.check()?;
    if crate::platform::inspect_engine(&stage, "").is_none() {
        return Err("Engine verification failed: editor or Build.version is missing".into());
    }
    crate::storage::write_json(
        &stage.join(".engine-workspace-install.json"),
        &serde_json::json!({"app":plan.manifest.app,"build":plan.manifest.build,"installed_at":crate::epic::now(),"prerequisite":plan.manifest.prereq,"files":files.len(),"selected_tags":plan.selected}),
    )?;
    crate::maintenance::save_plan(&stage, plan)?;
    no_links(&plan.target)?;
    if plan.target.exists() {
        // remove_dir succeeds only for an empty directory; never delete contents.
        fs::remove_dir(&plan.target).map_err(|_| {
            "Destination is no longer empty. Verified staging is retained.".to_owned()
        })?;
    }
    fs::rename(&stage, &plan.target).map_err(|e| format!("Cannot commit installation: {e}"))?;
    #[cfg(not(test))]
    if source.is_none() {
        progress.lock().unwrap().phase = "Registering engine with Epic Games Launcher".into();
        let warning = crate::epic_registration::register(plan).err();
        crate::storage::write_json(
            &plan.target.join(".alpine-epic-registration.json"),
            &serde_json::json!({"registered":warning.is_none(),"warning":warning}),
        )?;
    }
    // Delete only this transaction's named cache files, never a recursive caller-selected path.
    {
        let mut p = progress.lock().unwrap();
        p.phase = "Cleaning up downloaded chunk cache".into();
        p.total = 0;
    }
    for id in needed {
        let p = cache.join(id);
        let _ = fs::remove_file(p);
    }
    progress.lock().unwrap().phase = if source.is_some() {
        "Update files prepared - final verification pending"
    } else {
        "Installation complete - files verified"
    }
    .into();
    Ok(plan.target.clone())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resumes_verified_chunks_then_commits_complete_engine() {
        use crate::buildpatch::{File as ManifestFile, Part};
        let d = tempfile::tempdir().unwrap();
        let data = [
            b"editor fixture".as_slice(),
            br#"{"MajorVersion":5,"MinorVersion":8,"PatchVersion":3}"#.as_slice(),
        ]
        .concat();
        let guid = "00000001000000020000000300000004".to_string();
        let chunk = Chunk {
            guid: guid.clone(),
            sha: buildpatch::digest(&data),
            window: data.len() as u32,
            size: data.len() as u64,
            ..Default::default()
        };
        let files = vec![
            ManifestFile {
                name: "Engine/Binaries/Win64/UnrealEditor.exe".into(),
                sha: buildpatch::digest(&data[..14]),
                parts: vec![Part {
                    guid: guid.clone(),
                    offset: 0,
                    size: 14,
                }],
                ..Default::default()
            },
            ManifestFile {
                name: "Engine/Build/Build.version".into(),
                sha: buildpatch::digest(&data[14..]),
                parts: vec![Part {
                    guid: guid.clone(),
                    offset: 14,
                    size: (data.len() - 14) as u32,
                }],
                ..Default::default()
            },
        ];
        let plan = Plan {
            manifest: Manifest {
                app: "UE_5.8".into(),
                build: "test-resume".into(),
                files,
                chunks: [(guid.clone(), chunk)].into_iter().collect(),
                ..Default::default()
            },
            target: d.path().join("Engine"),
            bases: vec![],
            tags: vec![],
            selected: vec![],
        };
        let key = buildpatch::hex(&buildpatch::digest(b"UE_5.8:test-resume"));
        let work = d.path().join(format!(".engine-workspace-{}", &key[..16]));
        let cache = work.join("chunks");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join(&guid), &data).unwrap();
        let paused = Control::default();
        paused.stop();
        assert!(install(&plan, &paused, Arc::default()).is_err());
        assert!(!plan.target.exists());
        let staged_editor = work.join("files/Engine/Binaries/Win64/UnrealEditor.exe");
        fs::create_dir_all(staged_editor.parent().unwrap()).unwrap();
        fs::write(&staged_editor, b"editor fixture").unwrap();
        let progress = Arc::new(Mutex::new(Progress::default()));
        let result = install(&plan, &Control::default(), progress.clone()).unwrap();
        let p = progress.lock().unwrap();
        assert_eq!(
            p.network, 0,
            "verified cached chunks need no network traffic"
        );
        assert_eq!(
            p.read,
            14 + 2 * data.len() as u64,
            "staged verification plus cache verification and reconstruction"
        );
        assert_eq!(
            p.written,
            data.len() as u64 - 14,
            "only the missing file is written"
        );
        assert_eq!(p.completed, data.len() as u64);
        assert_eq!(p.files, p.files_total);
        assert_eq!(p.files_total, 2);
        assert_eq!(p.package_bytes, data.len() as u64);
        assert_eq!(result, plan.target);
        assert_eq!(
            fs::read(result.join("Engine/Binaries/Win64/UnrealEditor.exe")).unwrap(),
            b"editor fixture"
        );
        assert!(result.join(".engine-workspace-install.json").is_file());
        assert!(!cache.join(guid).exists());
    }
    #[test]
    fn payload_counters_include_corrupt_reads_and_partial_writes() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("chunk");
        fs::write(&file, b"corrupt data").unwrap();
        let progress = Mutex::new(Progress::default());
        assert_eq!(read_payload(&file, &progress).unwrap(), b"corrupt data");
        assert_eq!(progress.lock().unwrap().read, 12);
        struct FailingWriter(bool);
        impl Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::other("disk full"));
                }
                self.0 = true;
                Ok(3)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(write_payload(&mut FailingWriter(false), b"payload", &progress).is_err());
        assert_eq!(progress.lock().unwrap().written, 3);
    }
    #[test]
    fn unsafe_windows_paths_rejected() {
        for p in [
            "../escape",
            "C:/escape",
            "/escape",
            "a/../../x",
            "a\\..\\x",
            "file:stream",
            "NUL.txt",
            "dir./file",
            "a//b",
        ] {
            assert!(relative(p).is_err(), "{p}");
        }
        assert!(relative("Engine/Binaries/Win64/UnrealEditor.exe").is_ok());
    }
    #[test]
    fn existing_destination_is_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("UE");
        fs::create_dir(&target).unwrap();
        let marker = target.join("mine");
        fs::write(&marker, b"keep").unwrap();
        let p = Plan {
            manifest: Manifest {
                files: vec![buildpatch::File {
                    name: "Engine/Binaries/Win64/UnrealEditor.exe".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            target,
            bases: vec![],
            tags: vec![],
            selected: vec![],
        };
        assert!(install(&p, &Control::default(), Arc::default()).is_err());
        assert_eq!(fs::read(marker).unwrap(), b"keep");
    }
}
