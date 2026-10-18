use crate::{
    epic::Session,
    model::{Project, Snapshot},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("EngineWorkspace")
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DownloadSettings {
    pub notify_complete: bool,
    pub allow_editors: bool,
    pub limit_speed: bool,
    pub megabytes_per_second: f64,
}
impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            notify_complete: false,
            allow_editors: true,
            limit_speed: false,
            megabytes_per_second: 10.,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Activity {
    pub item: String,
    pub engine: String,
    pub completed: u64,
    pub success: bool,
    pub message: String,
    pub seen: bool,
}

impl Activity {
    pub fn item_label(&self) -> String {
        if self.item == "Unreal Engine" {
            return "Engine installation".into();
        }
        if let Some(name) = self.item.strip_prefix("Removing ") {
            return format!("Plugin removal: {}", name.trim_end_matches(['…', '.']));
        }
        if let Some(name) = self.item.strip_prefix("Updating ") {
            return format!("Plugin update: {}", name.trim_end_matches(['…', '.']));
        }
        self.item.clone()
    }

    pub fn stopped(&self) -> bool {
        !self.success
            && (self.message.starts_with("Stopped.")
                || self
                    .message
                    .starts_with("Paused. Verified files and chunks are retained;")
                || self.message.starts_with("Cancelled."))
    }
    pub fn status_label(&self) -> &'static str {
        if self.success {
            if self.item == "Engine verification"
                || self.message.starts_with("Verified ")
                || self.message.starts_with("Repaired ")
            {
                "Verify successful"
            } else if matches!(self.item.as_str(), "Unreal Engine" | "Engine installation")
                || self.item.starts_with("Installation: ")
            {
                "Installed"
            } else if self.item == "Engine removal" || self.item.starts_with("Removing ") {
                "Removed"
            } else if self.item == "Engine component changes" || self.item.starts_with("Updating ")
            {
                "Updated"
            } else if self.item.starts_with("Project creation: ") {
                "Project created"
            } else {
                "Completed"
            }
        } else if self.stopped() {
            if self.message.starts_with("Cancelled.") {
                "Cancelled"
            } else {
                "Stopped"
            }
        } else {
            "Failed"
        }
    }
}
#[cfg(test)]
mod activity_tests {
    use super::Activity;
    #[test]
    fn operation_outcomes_are_distinct() {
        let mut a = Activity {
            item: "Engine verification".into(),
            engine: "5.7.4".into(),
            completed: 0,
            success: true,
            message: "Verified 123 files. No repair needed.".into(),
            seen: false,
        };
        assert_eq!(a.status_label(), "Verify successful");
        a.success = false;
        a.message =
            "Paused. Verified files and chunks are retained; resume with the same destination."
                .into();
        assert_eq!(a.status_label(), "Stopped");
        a.message = "Cancelled.".into();
        assert_eq!(a.status_label(), "Cancelled");
        a.message = "Removal stopped at a file: access denied".into();
        assert_eq!(a.status_label(), "Failed");
        a.message = "Hash mismatch".into();
        assert_eq!(a.status_label(), "Failed");
        a.success = true;
        assert_eq!(a.status_label(), "Verify successful");
        a.message = "Installed F:/Epic/UE_5.5".into();
        for (item, expected) in [
            ("Unreal Engine", "Installed"),
            ("Engine installation", "Installed"),
            ("Engine removal", "Removed"),
            ("Removing Bridge…", "Removed"),
            ("Updating 2 plugin(s)…", "Updated"),
            ("Engine component changes", "Updated"),
            ("Project creation: Sample", "Project created"),
        ] {
            a.item = item.into();
            assert_eq!(a.status_label(), expected);
        }
        a.item = "Unreal Engine".into();
        assert_eq!(a.item_label(), "Engine installation");
        a.item = "Removing Bridge…".into();
        assert_eq!(a.item_label(), "Plugin removal: Bridge");
        a.success = false;
        a.message = "Access denied".into();
        assert_eq!(a.status_label(), "Failed");
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub library_organization:
        std::collections::HashMap<String, crate::library_organization::Organization>,
    pub hidden_projects: std::collections::HashSet<String>,
    pub project_categories: Vec<String>,
    pub project_category_assignments: std::collections::HashMap<String, String>,
    pub window_size: Option<[f32; 2]>,
    pub window_maximized: bool,
    pub downloads: DownloadSettings,
    pub run_at_startup: bool,
    pub theme: String,
    pub default_engine: String,
    pub launch_engine: Option<String>,
    pub projects: Vec<Project>,
    pub engine_paths: Vec<String>,
    pub pending_engines: Vec<String>,
    pub check_minutes: u64,
    pub wait_for_library_plugin_builds: bool,
    pub display_name: String,
    pub install_root: String,
    pub fab_cache: String,
    pub project_roots: Vec<String>,
    pub manual_projects: Vec<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            library_organization: Default::default(),
            hidden_projects: Default::default(),
            project_categories: vec![],
            project_category_assignments: Default::default(),
            window_size: None,
            window_maximized: false,
            downloads: DownloadSettings::default(),
            run_at_startup: false,
            theme: "dark".into(),
            default_engine: String::new(),
            launch_engine: None,
            projects: vec![],
            engine_paths: vec![],
            pending_engines: vec![],
            check_minutes: 15,
            wait_for_library_plugin_builds: true,
            display_name: "Developer".into(),
            install_root: "C:/Program Files/Epic Games".into(),
            fab_cache: "C:/ProgramData/Epic/EpicGamesLauncher/VaultCache".into(),
            project_roots: vec![],
            manual_projects: vec![],
        }
    }
}
pub fn load_preferences() -> Preferences {
    read_json(&data_dir().join("preferences.json")).unwrap_or_default()
}
pub fn save_preferences(p: &Preferences) -> Result<(), String> {
    write_json(&data_dir().join("preferences.json"), p)
}
pub fn load_snapshot(account: &str) -> Snapshot {
    read_json(&data_dir().join(format!("releases-{}.json", safe_id(account)))).unwrap_or_default()
}
pub fn save_snapshot(account: &str, s: &Snapshot) -> Result<(), String> {
    write_json(
        &data_dir().join(format!("releases-{}.json", safe_id(account))),
        s,
    )
}
fn safe_id(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(64)
        .collect()
}
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
pub fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<(), String> {
    write_atomic(
        path,
        &serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?,
    )
}
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("Missing parent directory")?)
        .map_err(|e| e.to_string())?;
    let temp = path.with_extension("tmp");
    use std::io::Write;
    let mut f = fs::File::create(&temp).map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    drop(f);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    #[cfg(not(windows))]
    fs::rename(temp, path).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn save_session(s: &Session) -> Result<(), String> {
    let plain = serde_json::to_vec(s).map_err(|e| e.to_string())?;
    let encrypted = protect(&plain, true)?;
    write_atomic(&data_dir().join("session.dpapi"), &encrypted)
}
pub fn load_session() -> Option<Session> {
    let bytes = fs::read(data_dir().join("session.dpapi")).ok()?;
    serde_json::from_slice(&protect(&bytes, false).ok()?).ok()
}
pub fn sign_out() -> Result<(), String> {
    let _lock = crate::epic::session_lock()?;
    let p = data_dir().join("session.dpapi");
    if p.exists() {
        fs::remove_file(p).map_err(|e| e.to_string())?
    }
    Ok(())
}
#[cfg(windows)]
fn protect(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err("Windows could not protect or unlock the Epic session.".into());
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as _);
        Ok(result)
    }
}
#[cfg(not(windows))]
fn protect(_bytes: &[u8], _encrypt: bool) -> Result<Vec<u8>, String> {
    Err("Secure session storage is currently implemented for Windows only.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_overwrite_is_valid() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state.json");
        write_json(&p, &vec![1]).unwrap();
        write_json(&p, &vec![2, 3]).unwrap();
        assert_eq!(read_json::<Vec<i32>>(&p).unwrap(), vec![2, 3]);
    }
    #[cfg(windows)]
    #[test]
    fn credentials_round_trip_encrypted() {
        let bytes = b"test-only-session";
        let blob = protect(bytes, true).unwrap();
        assert_ne!(blob, bytes);
        assert_eq!(protect(&blob, false).unwrap(), bytes);
    }
}

#[cfg(test)]
mod hidden_project_tests {
    use super::*;
    #[test]
    fn hidden_projects_survive_restart_and_old_preferences_default_to_visible() {
        let legacy: Preferences = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert!(legacy.hidden_projects.is_empty());
        let mut prefs = legacy;
        prefs
            .hidden_projects
            .insert("C:/Projects/Private/Private.uproject".into());
        let saved = serde_json::to_vec(&prefs).unwrap();
        let mut restarted: Preferences = serde_json::from_slice(&saved).unwrap();
        assert!(
            restarted
                .hidden_projects
                .contains("C:/Projects/Private/Private.uproject")
        );
        restarted.hidden_projects.clear();
        let restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&restarted).unwrap()).unwrap();
        assert!(restored.hidden_projects.is_empty());
    }
}

#[cfg(test)]
mod plugin_update_preferences_tests {
    use super::*;
    #[test]
    fn new_and_legacy_preferences_wait_for_library_builds() {
        assert!(Preferences::default().wait_for_library_plugin_builds);
        let old: Preferences =
            serde_json::from_str(r#"{"theme":"dark","check_minutes":30}"#).unwrap();
        assert!(old.wait_for_library_plugin_builds);
        assert_eq!(old.check_minutes, 30);
    }

    #[test]
    fn saved_plugin_update_choice_survives_restart() {
        for wait in [false, true] {
            let saved = format!(
                r#"{{"theme":"dark","check_minutes":30,"wait_for_library_plugin_builds":{wait}}}"#
            );
            let prefs: Preferences = serde_json::from_str(&saved).unwrap();
            assert_eq!(prefs.wait_for_library_plugin_builds, wait);
            let restored: Preferences =
                serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
            assert_eq!(restored.wait_for_library_plugin_builds, wait);
            assert_eq!(restored.check_minutes, 30);
        }
    }
}
