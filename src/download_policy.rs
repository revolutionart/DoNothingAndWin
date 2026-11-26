use crate::{
    buildpatch::{self, Chunk},
    storage,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
static RATE: AtomicU64 = AtomicU64::new(0);
static SLOT: OnceLock<Mutex<Instant>> = OnceLock::new();
pub fn configure(s: &storage::DownloadSettings) {
    let rate = if s.limit_speed && s.megabytes_per_second.is_finite() {
        (s.megabytes_per_second.clamp(0.1, 10000.) * 1_000_000.) as u64
    } else {
        0
    };
    if RATE.swap(rate, Ordering::Relaxed) != rate {
        *SLOT
            .get_or_init(|| Mutex::new(Instant::now()))
            .lock()
            .unwrap() = Instant::now();
    }
}
pub fn throttle(bytes: usize) {
    let rate = RATE.load(Ordering::Relaxed);
    if rate == 0 {
        return;
    }
    let delay = {
        let mut slot = SLOT
            .get_or_init(|| Mutex::new(Instant::now()))
            .lock()
            .unwrap();
        let now = Instant::now();
        *slot = (*slot).max(now) + Duration::from_secs_f64(bytes as f64 / rate as f64);
        slot.saturating_duration_since(now)
    };
    std::thread::sleep(delay);
}
thread_local! {
    static CACHE_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Pin a worker's destination for the duration of a download; other workers and
/// later preferences changes cannot redirect a transfer already in progress.
pub fn with_cache_root<T>(root: Option<PathBuf>, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CACHE_ROOT.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(CACHE_ROOT.with(|slot| slot.replace(root)));
    work()
}

fn selected_cache_root() -> Option<PathBuf> {
    CACHE_ROOT.with(|slot| slot.borrow().clone())
}

fn cache_directory(root: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join("EngineWorkspace/Chunks")
}

pub(crate) struct ChunkCache {
    directories: Vec<PathBuf>,
}
/// Every cache folder in use: the one this transfer writes to, then the others.
pub(crate) fn cache_roots() -> Vec<PathBuf> {
    let prefs = storage::load_preferences();
    let mut roots = vec![selected_cache_root().unwrap_or_else(|| PathBuf::from(&prefs.fab_cache))];
    for root in prefs.vault_cache_roots().into_iter().map(PathBuf::from) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

impl ChunkCache {
    pub(crate) fn new() -> Self {
        Self {
            directories: cache_roots().into_iter().map(cache_directory).collect(),
        }
    }

    /// Delete these chunks from every cache folder. They are named by content
    /// hash, so nothing but a copy of the same data can be removed.
    pub(crate) fn remove<'a>(&self, chunks: impl IntoIterator<Item = &'a Chunk>) {
        let _changes: Vec<_> = self
            .directories
            .iter()
            .map(|directory| crate::vault_health::changing(directory))
            .collect();
        for chunk in chunks {
            if chunk.rolling_only {
                continue;
            }
            let key = buildpatch::hex(&chunk.sha);
            for directory in &self.directories {
                let path = directory.join(&key);
                if crate::installer::no_links(&path).is_ok() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    pub(crate) fn read(&self, chunk: &Chunk, read: &mut impl FnMut(u64)) -> Option<Vec<u8>> {
        // Legacy chunks with only a rolling hash have no SHA-1 cache key.
        if chunk.rolling_only {
            return None;
        }
        let key = buildpatch::hex(&chunk.sha);
        self.directories
            .iter()
            .find_map(|dir| read_cached_chunk_progress(&dir.join(&key), chunk, read))
    }

    // The decoder already verified these bytes. Keep the content-addressed cache
    // atomic without a synchronous disk flush for every rebuildable chunk.
    pub(crate) fn store_verified(&self, chunk: &Chunk, bytes: &[u8]) -> bool {
        !chunk.rolling_only && store_chunk(&self.directories[0], chunk, bytes)
    }
}

#[cfg(test)]
fn cache_path(chunk: &Chunk) -> PathBuf {
    ChunkCache::new().directories[0].join(buildpatch::hex(&chunk.sha))
}
pub fn cached_chunk(chunk: &Chunk) -> Option<Vec<u8>> {
    cached_chunk_progress(chunk, &mut |_| {})
}
pub(crate) fn cached_chunk_progress(chunk: &Chunk, read: &mut impl FnMut(u64)) -> Option<Vec<u8>> {
    ChunkCache::new().read(chunk, read)
}
#[cfg(test)]
fn read_cached_chunk(path: &std::path::Path, chunk: &Chunk) -> Option<Vec<u8>> {
    read_cached_chunk_progress(path, chunk, &mut |_| {})
}
fn read_cached_chunk_progress(
    path: &std::path::Path,
    chunk: &Chunk,
    read: &mut impl FnMut(u64),
) -> Option<Vec<u8>> {
    if crate::installer::no_links(path).is_err()
        || std::fs::metadata(path).ok()?.len() != chunk.window as u64
    {
        return None;
    }
    // Validate the exact buffer returned to the installer, rather than reading
    // once for verification and a second time for use.
    let bytes = std::fs::read(path).ok()?;
    read(bytes.len() as u64);
    (bytes.len() == chunk.window as usize && buildpatch::digest(&bytes) == chunk.sha)
        .then_some(bytes)
}

pub fn cache_chunk(chunk: &Chunk, bytes: &[u8]) {
    cache_chunk_progress(chunk, bytes, &mut |_| {});
}
pub(crate) fn cache_chunk_progress(chunk: &Chunk, bytes: &[u8], written: &mut impl FnMut(u64)) {
    if bytes.len() != chunk.window as usize || buildpatch::digest(bytes) != chunk.sha {
        return;
    }
    let root = selected_cache_root()
        .unwrap_or_else(|| PathBuf::from(storage::load_preferences().fab_cache));
    if store_chunk(&cache_directory(root), chunk, bytes) {
        written(bytes.len() as u64);
    }
}

fn store_chunk(directory: &Path, chunk: &Chunk, bytes: &[u8]) -> bool {
    use std::fs;
    let path = directory.join(buildpatch::hex(&chunk.sha));
    if crate::installer::no_links(&path).is_err() || fs::create_dir_all(directory).is_err() {
        return false;
    }
    static TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("{}-{id}.tmp", std::process::id()));
    let Ok(mut output) = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
    else {
        return false;
    };
    let _change = crate::vault_health::changing(directory);
    let result = output.write_all(bytes);
    drop(output);
    if result.and_then(|()| fs::rename(&temp, &path)).is_ok() {
        return true;
    }
    let _ = fs::remove_file(&temp);
    false
}
pub fn check_install_allowed() -> Result<(), String> {
    if storage::load_preferences().downloads.allow_editors {
        return Ok(());
    }
    if editors_running()? {
        return Err("Close Unreal Editor before installing, or enable Allow installs while editors are running in Download settings.".into());
    }
    Ok(())
}
pub fn editors_running() -> Result<bool, String> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{Foundation::CloseHandle, System::Diagnostics::ToolHelp::*};
        let snapshot =
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| e.to_string())?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut next = Process32FirstW(snapshot, &mut entry).is_ok();
        let mut found = false;
        while next {
            let end = entry
                .szExeFile
                .iter()
                .position(|x| *x == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase();
            if matches!(
                name.as_str(),
                "unrealeditor.exe" | "ue4editor.exe" | "unrealeditor-cmd.exe" | "ue4editor-cmd.exe"
            ) {
                found = true;
                break;
            }
            next = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        return Ok(found);
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}
/// Read actual Windows startup registration, including choices made by Setup.
pub fn startup_settings() -> Option<bool> {
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::*};
        let value = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .and_then(|key| key.get_value::<String, _>("EngineWorkspace"))
            .ok()?;
        if value.is_empty() {
            return None;
        }
        Some(value.split_whitespace().any(|arg| arg == "--minimized"))
    }
    #[cfg(not(windows))]
    None
}

fn startup_command(exe: &std::path::Path, in_tray: bool) -> String {
    format!(
        "\"{}\" --no-scan{}",
        exe.display(),
        if in_tray { " --minimized" } else { "" }
    )
}

pub fn set_startup(enabled: bool, in_tray: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::*};
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .map_err(|e| e.to_string())?;
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            // Point at the stable launcher copy so updates do not strand startup on an old version.
            let stable = exe.parent().unwrap().join("DoNothingAndWinLauncher.exe");
            let exe = if stable.is_file() { stable } else { exe };
            key.set_value("EngineWorkspace", &startup_command(&exe, in_tray))
                .map_err(|e| e.to_string())?;
        } else {
            match key.delete_value("EngineWorkspace") {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (enabled, in_tray);
        Err("Startup integration requires Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn chosen_cache_is_scoped_to_worker_and_restored_after_nested_use() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let chunk = Chunk::default();
        let original = cache_path(&chunk);
        with_cache_root(Some(a.path().to_owned()), || {
            assert!(cache_path(&chunk).starts_with(a.path()));
            with_cache_root(Some(b.path().to_owned()), || {
                assert!(cache_path(&chunk).starts_with(b.path()))
            });
            assert!(cache_path(&chunk).starts_with(a.path()));
            let other_thread = std::thread::spawn(move || cache_path(&Chunk::default()))
                .join()
                .unwrap();
            assert!(!other_thread.starts_with(a.path()));
        });
        assert_eq!(cache_path(&chunk), original);
    }
    #[test]
    fn verified_chunks_are_reused_from_secondary_cache_when_first_is_missing_or_corrupt() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("first");
        let b = root.path().join("second");
        let chunk = Chunk {
            sha: buildpatch::digest(b"good"),
            window: 4,
            ..Default::default()
        };
        let key = buildpatch::hex(&chunk.sha);
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join(&key), b"bad!").unwrap();
        fs::write(b.join(&key), b"good").unwrap();
        let cache = ChunkCache {
            directories: vec![a, b],
        };
        let mut read = 0;
        let bytes = cache.read(&chunk, &mut |n| read += n).unwrap();
        assert_eq!(bytes, b"good");
        assert_eq!(read, 8);
    }
    #[test]
    fn startup_defaults_to_tray_but_can_show_the_window() {
        let prefs: crate::storage::Preferences = serde_json::from_str("{}").unwrap();
        assert!(prefs.startup_in_tray);
        assert!(prefs.close_to_tray);
        let close_prefs: crate::storage::Preferences =
            serde_json::from_str(r#"{"close_to_tray":false}"#).unwrap();
        assert!(!close_prefs.close_to_tray);
        assert!(
            close_prefs.startup_in_tray,
            "Closing and Windows startup are independent"
        );
        let saved = serde_json::to_string(&close_prefs).unwrap();
        assert!(
            !serde_json::from_str::<crate::storage::Preferences>(&saved)
                .unwrap()
                .close_to_tray
        );
        let exe = std::path::Path::new("C:/Program Files/Launcher.exe");
        assert_eq!(
            startup_command(exe, true),
            "\"C:/Program Files/Launcher.exe\" --no-scan --minimized"
        );
        assert_eq!(
            startup_command(exe, false),
            "\"C:/Program Files/Launcher.exe\" --no-scan"
        );
        let prefs: crate::storage::Preferences =
            serde_json::from_str(r#"{"startup_in_tray":false}"#).unwrap();
        assert!(!prefs.startup_in_tray);
    }

    #[test]
    fn cached_chunks_reject_damage_and_accept_verified_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cached-chunk");
        let original = b"verified package bytes";
        let chunk = Chunk {
            window: original.len() as u32,
            sha: buildpatch::digest(original),
            ..Default::default()
        };
        assert!(read_cached_chunk(&path, &chunk).is_none());
        std::fs::write(&path, original).unwrap();
        let mut read_bytes = 0;
        assert_eq!(
            read_cached_chunk_progress(&path, &chunk, &mut |n| read_bytes += n).unwrap(),
            original
        );
        assert_eq!(read_bytes, original.len() as u64);
        let mut damaged = original.to_vec();
        damaged[0] ^= 1;
        std::fs::write(&path, damaged).unwrap();
        assert!(read_cached_chunk(&path, &chunk).is_none());
        std::fs::write(&path, &original[..original.len() - 1]).unwrap();
        assert!(read_cached_chunk(&path, &chunk).is_none());
        std::fs::write(&path, original).unwrap();
        assert_eq!(read_cached_chunk(&path, &chunk).unwrap(), original);
    }
    #[test]
    fn bandwidth_limit_delays_transfers_and_can_be_disabled() {
        let mut s = storage::DownloadSettings::default();
        s.limit_speed = true;
        s.megabytes_per_second = 1.;
        configure(&s);
        let start = Instant::now();
        throttle(40_000);
        assert!(start.elapsed() >= Duration::from_millis(35));
        s.limit_speed = false;
        configure(&s);
        assert_eq!(RATE.load(Ordering::Relaxed), 0);
        s.limit_speed = true;
        s.megabytes_per_second = f64::NAN;
        configure(&s);
        assert_eq!(RATE.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn legacy_preferences_keep_engine_and_manual_defaults() {
        let p: storage::Preferences =
            serde_json::from_str(r#"{"default_engine":"F:/Program Files/Epic Games/UE_5.8"}"#)
                .unwrap();
        assert!(p.default_engine.ends_with("UE_5.8"));
        assert!(!p.downloads.limit_speed && !p.run_at_startup);
        let value = serde_json::to_value(p).unwrap();
        assert!(value.get("automatic_updates").is_none());
        assert!(value.get("schedule").is_none());
    }
}
