use crate::{
    buildpatch::{self, Chunk},
    storage,
};
use std::{
    path::PathBuf,
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
fn cache_path(chunk: &Chunk) -> PathBuf {
    PathBuf::from(storage::load_preferences().fab_cache)
        .join("EngineWorkspace/Chunks")
        .join(buildpatch::hex(&chunk.sha))
}
pub fn cached_chunk(chunk: &Chunk) -> Option<Vec<u8>> {
    let path = cache_path(chunk);
    if crate::installer::no_links(&path).is_err()
        || !crate::installer::valid_file(&path, &chunk.sha, chunk.window as u64)
    {
        return None;
    }
    std::fs::read(path).ok()
}
pub fn cache_chunk(chunk: &Chunk, bytes: &[u8]) {
    let path = cache_path(chunk);
    if bytes.len() != chunk.window as usize
        || buildpatch::digest(bytes) != chunk.sha
        || crate::installer::no_links(&path).is_err()
    {
        return;
    }
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _ = storage::write_atomic(&path, bytes);
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
pub fn startup_enabled() -> bool {
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::*};
        return RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .and_then(|key| key.get_value::<String, _>("EngineWorkspace"))
            .is_ok_and(|value| !value.is_empty());
    }
    #[cfg(not(windows))]
    false
}

pub fn set_startup(enabled: bool) -> Result<(), String> {
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
            key.set_value(
                "EngineWorkspace",
                &format!("\"{}\" --no-scan --minimized", exe.display()),
            )
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
        let _ = enabled;
        Err("Startup integration requires Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
