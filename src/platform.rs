use crate::{
    epic::now,
    model::{EngineVersion, Installation, Project},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
pub fn inspect_engine(path: &Path, association: &str) -> Option<Installation> {
    let v: Value =
        serde_json::from_slice(&fs::read(path.join("Engine/Build/Build.version")).ok()?).ok()?;
    if !path.join("Engine/Binaries/Win64/UnrealEditor.exe").exists()
        && !path.join("Engine/Binaries/Win64/UE4Editor.exe").exists()
    {
        return None;
    }
    let ver = EngineVersion {
        major: v["MajorVersion"].as_u64()? as u32,
        minor: v["MinorVersion"].as_u64()? as u32,
        patch: v["PatchVersion"].as_u64()? as u32,
    };
    Some(Installation {
        version: ver.to_string(),
        path: path.to_string_lossy().into_owned(),
        association: if association.is_empty() {
            ver.branch()
        } else {
            association.trim_start_matches("UE_").into()
        },
    })
}
pub fn engine_ready(engine: &Installation) -> bool {
    let root = Path::new(&engine.path);
    root.join("Engine/Binaries/Win64/UnrealEditor.exe")
        .is_file()
        || root.join("Engine/Binaries/Win64/UE4Editor.exe").is_file()
}
fn inspect_damaged_engine(root: &Path, association: &str) -> Option<Installation> {
    if !root.join("Engine").is_dir()
        || !(root.join(".engine-workspace-install.json").is_file()
            || root.join(".egstore").is_dir())
    {
        return None;
    }
    let receipt: Option<Value> =
        crate::storage::read_json(&root.join(".engine-workspace-install.json"));
    let version = receipt
        .as_ref()
        .and_then(|v| v["build"].as_str())
        .and_then(EngineVersion::parse)
        .or_else(|| {
            let v: Value = crate::storage::read_json(&root.join("Engine/Build/Build.version"))?;
            Some(EngineVersion {
                major: v["MajorVersion"].as_u64()? as u32,
                minor: v["MinorVersion"].as_u64()? as u32,
                patch: v["PatchVersion"].as_u64()? as u32,
            })
        })
        .or_else(|| {
            crate::maintenance::epic_build(root)
                .as_deref()
                .and_then(EngineVersion::parse)
        })?;
    Some(Installation {
        version: version.to_string(),
        path: root.to_string_lossy().into_owned(),
        association: if association.is_empty() {
            version.branch()
        } else {
            association.trim_start_matches("UE_").into()
        },
    })
}
pub fn discover_engines() -> Vec<Installation> {
    let mut roots: BTreeMap<PathBuf, String> = BTreeMap::new();
    for path in crate::storage::load_preferences().engine_paths {
        roots.insert(path.into(), String::new());
    }
    let program_data = std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/ProgramData"));
    let manifest = program_data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat");
    if let Ok(bytes) = fs::read(manifest) {
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
            if let Some(items) = v["InstallationList"].as_array() {
                for item in items {
                    if item["AppName"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("UE_"))
                    {
                        if let Some(path) = item["InstallLocation"].as_str() {
                            roots
                                .insert(path.into(), item["AppName"].as_str().unwrap_or("").into());
                        }
                    }
                }
            }
        }
    }
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::*};
        if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Epic Games\\Unreal Engine\\Builds")
        {
            for (id, value) in key.enum_values().flatten() {
                if let Ok(path) = key.get_value::<String, _>(&id) {
                    roots.insert(path.into(), id);
                }
                let _ = value;
            }
        }
        if let Ok(key) =
            RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey("Software\\EpicGames\\Unreal Engine")
        {
            for version in key.enum_keys().flatten() {
                if let Ok(sub) = key.open_subkey(&version) {
                    if let Ok(path) = sub.get_value::<String, _>("InstalledDirectory") {
                        roots.insert(path.into(), version);
                    }
                }
            }
        }
    }
    for drive in crate::discovery::roots() {
        let folder = drive.join("Program Files/Epic Games");
        if let Ok(entries) = fs::read_dir(folder) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with("UE_") {
                    roots.entry(entry.path()).or_default();
                }
            }
        }
    }
    let mut result: Vec<_> = roots
        .iter()
        .filter_map(|(p, a)| inspect_engine(p, a).or_else(|| inspect_damaged_engine(p, a)))
        .collect();
    result.sort_by(|a, b| EngineVersion::parse(&b.version).cmp(&EngineVersion::parse(&a.version)));
    result
}
/// Consolidate launcher cards only; maintenance/discovery retain every physical install.
pub fn display_engines() -> Vec<Installation> {
    let preferred = crate::storage::load_preferences().default_engine;
    consolidate_engines(discover_engines(), &preferred)
}
fn consolidate_engines(mut engines: Vec<Installation>, preferred: &str) -> Vec<Installation> {
    fn path_key(p: &str) -> String {
        fs::canonicalize(p)
            .unwrap_or_else(|_| PathBuf::from(p))
            .to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    }
    let preferred = path_key(preferred);
    engines.sort_by_key(|e| {
        let standard = EngineVersion::parse(&e.version).is_some_and(|v| {
            Path::new(&e.path).file_name().is_some_and(|s| {
                s.to_string_lossy()
                    .eq_ignore_ascii_case(&format!("UE_{}", v.branch()))
            })
        });
        (
            !engine_ready(e),
            path_key(&e.path) != preferred,
            !standard,
            e.path.to_lowercase(),
        )
    });
    let mut paths = std::collections::HashSet::new();
    let mut versions = std::collections::HashSet::new();
    engines.retain(|e| {
        if !paths.insert(path_key(&e.path)) {
            return false;
        }
        // GUID/custom associations identify distinct source builds and must be retained.
        let managed: Option<Value> =
            crate::storage::read_json(&Path::new(&e.path).join(".engine-workspace-install.json"));
        let managed_binary = managed
            .as_ref()
            .and_then(|v| v["build"].as_str())
            .and_then(EngineVersion::parse)
            == EngineVersion::parse(&e.version)
            && managed.is_some();
        if EngineVersion::parse(&e.association).is_some() || managed_binary {
            versions.insert(e.version.clone())
        } else {
            true
        }
    });
    engines.sort_by(|a, b| EngineVersion::parse(&b.version).cmp(&EngineVersion::parse(&a.version)));
    engines
}
/// Root PNGs take priority over the editor's saved screenshot.
pub fn project_thumbnail(project: &Path) -> Option<String> {
    let root = project.parent()?;
    let mut pngs: Vec<PathBuf> = fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
        .collect();
    // If there are multiple PNGs, prefer the project name, then a stable alphabetical order.
    pngs.sort_by_key(|p| {
        (
            !p.file_stem()
                .zip(project.file_stem())
                .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)),
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
        )
    });
    pngs.into_iter()
        .next()
        .or_else(|| {
            let p = root.join("Saved/AutoScreenshot.png");
            p.is_file().then_some(p)
        })
        .map(|p| p.to_string_lossy().into_owned())
}

pub fn import_project(path: &Path) -> Result<Project, String> {
    if !path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("uproject"))
    {
        return Err("Choose an Unreal .uproject file.".into());
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let v: Value =
        serde_json::from_slice(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes))
            .map_err(|_| "This project file is not valid JSON.")?;
    if v["FileVersion"].as_u64().is_none() {
        return Err("This file does not contain an Unreal project descriptor.".into());
    }
    let art = project_thumbnail(path);
    Ok(Project {
        name: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path: fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_owned())
            .to_string_lossy()
            .into_owned(),
        description: v["Description"]
            .as_str()
            .unwrap_or("Unreal Engine project")
            .into(),
        engine: v["EngineAssociation"].as_str().unwrap_or("").into(),
        pinned: false,
        last_opened: 0,
        artwork: art,
    })
}
pub fn discover_projects() -> Vec<Project> {
    let root = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Documents/Unreal Projects");
    let mut result = vec![];
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten().take(500) {
            let p = entry.path();
            if p.is_dir() {
                if let Ok(files) = fs::read_dir(p) {
                    for f in files.flatten().take(100) {
                        if let Ok(project) = import_project(&f.path()) {
                            result.push(project)
                        }
                    }
                }
            }
        }
    }
    result
}
// Unreal's command-line parser does not understand Win32 verbatim prefixes.
fn editor_project_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
    }
}
#[cfg(test)]
mod editor_path_tests {
    use super::*;
    #[test]
    fn editor_receives_normal_absolute_paths_as_one_argument() {
        for (input, expected) in [
            (
                r"\\?\C:\Unreal Projects\Dreamcore.uproject",
                r"C:\Unreal Projects\Dreamcore.uproject",
            ),
            (
                r"\\?\UNC\server\share\My Project.uproject",
                r"\\server\share\My Project.uproject",
            ),
            (
                r"C:\Unreal Projects\Dreamcore.uproject",
                r"C:\Unreal Projects\Dreamcore.uproject",
            ),
        ] {
            let arg = editor_project_path(input);
            assert_eq!(arg, expected);
            let mut cmd = Command::new("UnrealEditor.exe");
            cmd.arg(&arg);
            assert_eq!(
                cmd.get_args().collect::<Vec<_>>(),
                vec![std::ffi::OsStr::new(expected)]
            );
        }
    }
}
pub fn launch(engine: &Installation, project: Option<&Project>) -> Result<(), String> {
    let root = Path::new(&engine.path);
    let modern = root.join("Engine/Binaries/Win64/UnrealEditor.exe");
    let exe = if modern.exists() {
        modern
    } else {
        root.join("Engine/Binaries/Win64/UE4Editor.exe")
    };
    if !exe.is_file() {
        return Err("The engine executable is missing. Relocate the engine in Settings.".into());
    }
    let mut cmd = Command::new(exe);
    cmd.current_dir(root);
    if let Some(p) = project {
        if !Path::new(&p.path).is_file() {
            return Err("The project file has moved. Import it again.".into());
        }
        cmd.arg(editor_project_path(&p.path));
    }
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn create_engine_shortcut(engine: &Installation) -> Result<PathBuf, String> {
    let engine = engine.clone();
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            use windows::Win32::{
                System::Com::CoTaskMemFree,
                UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
            };
            let desktop = unsafe {
                let raw = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None)
                    .map_err(|e| e.to_string())?;
                let text = raw.to_string();
                CoTaskMemFree(Some(raw.0.cast()));
                PathBuf::from(text.map_err(|e| e.to_string())?)
            };
            write_engine_shortcut(&engine, &desktop)
        }
        #[cfg(not(windows))]
        {
            let _ = engine;
            Err("Desktop shortcuts currently require Windows.".into())
        }
    })
    .join()
    .map_err(|_| "Shortcut creation failed".to_owned())?
}

pub fn create_project_shortcut(project: &Project) -> Result<PathBuf, String> {
    let project = project.clone();
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            use windows::Win32::{
                System::Com::CoTaskMemFree,
                UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
            };
            let desktop = unsafe {
                let raw = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None)
                    .map_err(|e| e.to_string())?;
                let text = raw.to_string();
                CoTaskMemFree(Some(raw.0.cast()));
                PathBuf::from(text.map_err(|e| e.to_string())?)
            };
            write_project_shortcut(&project, &desktop)
        }
        #[cfg(not(windows))]
        {
            let _ = project;
            Err("Desktop shortcuts currently require Windows.".into())
        }
    })
    .join()
    .map_err(|_| "Shortcut creation failed".to_owned())?
}

#[cfg(windows)]
fn write_engine_shortcut(engine: &Installation, desktop: &Path) -> Result<PathBuf, String> {
    let root = Path::new(&engine.path);
    let modern = root.join("Engine/Binaries/Win64/UnrealEditor.exe");
    let exe = if modern.is_file() {
        modern
    } else {
        root.join("Engine/Binaries/Win64/UE4Editor.exe")
    };
    if !exe.is_file() {
        return Err("The engine executable is missing; no shortcut was created.".into());
    }
    let version = EngineVersion::parse(&engine.version).ok_or("Invalid engine version")?;
    write_native_shortcut(&exe, root, &format!("Unreal Engine {version}"), desktop)
}

#[cfg(windows)]
fn write_project_shortcut(project: &Project, desktop: &Path) -> Result<PathBuf, String> {
    // Shell links do not accept Rust's canonical Windows device-path prefix.
    let shell_path = if let Some(unc) = project.path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", unc)
    } else {
        project
            .path
            .strip_prefix(r"\\?\")
            .unwrap_or(&project.path)
            .to_owned()
    };
    let target = Path::new(&shell_path);
    if !target.is_file() {
        return Err("The project file is missing; no shortcut was created.".into());
    }
    let root = target.parent().ok_or("Missing project folder")?;
    let name = target
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Invalid project filename")?;
    write_native_shortcut(target, root, name, desktop)
}

#[cfg(windows)]
fn write_native_shortcut(
    exe: &Path,
    root: &Path,
    name: &str,
    desktop: &Path,
) -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, IPersistFile,
            },
            UI::Shell::{IShellLinkW, ShellLink},
        },
        core::{Interface, PCWSTR},
    };
    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }
    // Reserve a new name; never overwrite an existing desktop shortcut.
    let mut reserved = None;
    for number in 1..=1000 {
        let suffix = if number == 1 {
            String::new()
        } else {
            format!(" ({number})")
        };
        let path = desktop.join(format!("{name}{suffix}.lnk"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                drop(file);
                reserved = Some(path);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    let path = reserved.ok_or("No available desktop shortcut name")?;
    let result = (|| -> Result<(), String> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|e| e.to_string())?;
        }
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _apartment = Apartment;
        let executable = wide(&exe);
        let directory = wide(root);
        let destination = wide(&path);
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.to_string())?;
            link.SetPath(PCWSTR(executable.as_ptr()))
                .map_err(|e| e.to_string())?;
            link.SetWorkingDirectory(PCWSTR(directory.as_ptr()))
                .map_err(|e| e.to_string())?;
            link.SetIconLocation(PCWSTR(executable.as_ptr()), 0)
                .map_err(|e| e.to_string())?;
            let file: IPersistFile = link.cast().map_err(|e| e.to_string())?;
            file.Save(PCWSTR(destination.as_ptr()), true)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}
pub fn create_project(parent: &Path, name: &str, engine: &Installation) -> Result<Project, String> {
    if name.is_empty()
        || name.len() > 64
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("Use 1–64 letters, digits or underscores for the project name.".into());
    }
    let root = parent.join(name);
    fs::create_dir(&root)
        .map_err(|_| "This project folder already exists or cannot be created.")?;
    for sub in ["Content", "Config"] {
        fs::create_dir(root.join(sub)).map_err(|e| e.to_string())?;
    }
    let file = root.join(format!("{name}.uproject"));
    crate::storage::write_json(
        &file,
        &serde_json::json!({"FileVersion":3,"EngineAssociation":engine.association,"Category":"","Description":""}),
    )?;
    let mut p = import_project(&file)?;
    p.last_opened = now();
    Ok(p)
}
pub fn open_external(target: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let text: Vec<u16> = std::ffi::OsStr::new(target)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                text.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result as isize <= 32 {
            return Err("Windows could not open this location.".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        Err("This build targets Windows.".into())
    }
}
pub fn free_space() -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let path = crate::storage::data_dir();
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0u64;
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok != 0 { Some(free) } else { None }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn native_shortcut_targets_engine_and_preserves_existing_shortcuts() {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            Win32::{
                System::Com::{
                    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance,
                    CoInitializeEx, CoUninitialize, IPersistFile, STGM_READ,
                },
                UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink},
            },
            core::{Interface, PCWSTR},
        };
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Engine with spaces");
        let exe = root.join("Engine/Binaries/Win64/UnrealEditor.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, []).unwrap();
        let engine = Installation {
            version: "5.8.3".into(),
            path: root.to_string_lossy().into_owned(),
            association: "5.8".into(),
        };
        let first = write_engine_shortcut(&engine, temp.path()).unwrap();
        let original = fs::read(&first).unwrap();
        let second = write_engine_shortcut(&engine, temp.path()).unwrap();
        assert_ne!(first, second);
        assert_eq!(original, fs::read(&first).unwrap());
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            {
                let link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
                let persist: IPersistFile = link.cast().unwrap();
                let file: Vec<u16> = first.as_os_str().encode_wide().chain(Some(0)).collect();
                persist.Load(PCWSTR(file.as_ptr()), STGM_READ).unwrap();
                let mut path = [0u16; 32768];
                link.GetPath(&mut path, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
                    .unwrap();
                let end = path.iter().position(|c| *c == 0).unwrap();
                assert_eq!(
                    PathBuf::from(String::from_utf16(&path[..end]).unwrap()),
                    exe
                );
            }
            CoUninitialize();
        }
        let missing = Installation {
            path: temp.path().join("missing").to_string_lossy().into_owned(),
            ..engine
        };
        assert!(write_engine_shortcut(&missing, temp.path()).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn native_shortcut_targets_project_and_preserves_existing_shortcuts() {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            Win32::{
                System::Com::{
                    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance,
                    CoInitializeEx, CoUninitialize, IPersistFile, STGM_READ,
                },
                UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink},
            },
            core::{Interface, PCWSTR},
        };
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Project with spaces");
        fs::create_dir(&root).unwrap();
        let exe = root.join("My Project.uproject");
        fs::write(&exe, br#"{"FileVersion":3,"EngineAssociation":"{CUSTOM}"}"#).unwrap();
        let project = import_project(&exe).unwrap();
        let first = write_project_shortcut(&project, temp.path()).unwrap();
        let original = fs::read(&first).unwrap();
        let second = write_project_shortcut(&project, temp.path()).unwrap();
        assert_ne!(first, second);
        assert_eq!(original, fs::read(&first).unwrap());
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            {
                let link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
                let persist: IPersistFile = link.cast().unwrap();
                let file: Vec<u16> = first.as_os_str().encode_wide().chain(Some(0)).collect();
                persist.Load(PCWSTR(file.as_ptr()), STGM_READ).unwrap();
                let mut path = [0u16; 32768];
                link.GetPath(&mut path, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
                    .unwrap();
                let end = path.iter().position(|c| *c == 0).unwrap();
                assert_eq!(
                    fs::canonicalize(PathBuf::from(String::from_utf16(&path[..end]).unwrap()))
                        .unwrap(),
                    fs::canonicalize(&exe).unwrap()
                );
            }
            CoUninitialize();
        }
        assert_eq!(
            fs::read(&exe).unwrap(),
            br#"{"FileVersion":3,"EngineAssociation":"{CUSTOM}"}"#
        );
        let missing = Project {
            path: temp
                .path()
                .join("missing.uproject")
                .to_string_lossy()
                .into_owned(),
            ..project
        };
        assert!(write_project_shortcut(&missing, temp.path()).is_err());
    }
    #[test]
    fn blank_project_preserves_engine_and_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Installation {
            version: "5.7.4".into(),
            path: "unused".into(),
            association: "5.7".into(),
        };
        let project = create_project(dir.path(), "Test_Project", &engine).unwrap();
        assert_eq!(project.engine, "5.7");
        assert!(Path::new(&project.path).is_file());
        let before = fs::read(&project.path).unwrap();
        assert!(create_project(dir.path(), "Test_Project", &engine).is_err());
        assert_eq!(fs::read(&project.path).unwrap(), before);
        assert!(create_project(dir.path(), "../escape", &engine).is_err());
    }
    #[test]
    fn import_rejects_invalid_project() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Bad.uproject");
        fs::write(&p, "not json").unwrap();
        assert!(import_project(&p).is_err());
        fs::write(&p, "{}").unwrap();
        assert!(import_project(&p).is_err());
    }
}
#[cfg(test)]
mod display_tests {
    use super::*;
    fn entry(path: &str, version: &str, association: &str) -> Installation {
        Installation {
            path: path.into(),
            version: version.into(),
            association: association.into(),
        }
    }
    #[test]
    fn duplicate_binary_versions_consolidate_without_hiding_custom_builds() {
        let entries = vec![
            entry("F:/Engines/UE_5.8-5.8.3-Workspace", "5.8.3", "5.8"),
            entry("F:/Engines/UE_5.8", "5.8.3", "5.8"),
            entry("F:/Engines/Custom", "5.8.3", "{CUSTOM-GUID}"),
            entry("F:/Engines/UE_5.7", "5.7.4", "5.7"),
        ];
        let display = consolidate_engines(entries.clone(), "");
        assert_eq!(display.len(), 3);
        assert!(display.iter().any(|e| e.path == "F:/Engines/UE_5.8"));
        assert!(!display.iter().any(|e| e.path.ends_with("Workspace")));
        let display = consolidate_engines(entries, "F:/Engines/UE_5.8-5.8.3-Workspace");
        assert!(display.iter().any(|e| e.path.ends_with("Workspace")));
        assert!(!display.iter().any(|e| e.path == "F:/Engines/UE_5.8"));
    }
    #[test]
    fn case_and_separator_aliases_do_not_make_duplicate_cards() {
        let e = vec![
            entry("F:/Engines/UE_5.8", "5.8.3", "5.8"),
            entry("f:\\engines\\ue_5.8\\", "5.8.3", "{CUSTOM}"),
        ];
        assert_eq!(consolidate_engines(e, "").len(), 1);
    }
}
#[cfg(test)]
mod managed_display_test {
    use super::*;
    #[test]
    fn launcher_installed_binary_with_auto_registered_guid_is_not_a_custom_build() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("UE_5.8-test");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join(".engine-workspace-install.json"),
            br#"{"build":"5.8.3-123+++UE5+Release-5.8-Windows"}"#,
        )
        .unwrap();
        let managed = Installation {
            path: root.to_string_lossy().into_owned(),
            version: "5.8.3".into(),
            association: "{AUTO-GUID}".into(),
        };
        let official = Installation {
            path: d.path().join("UE_5.8").to_string_lossy().into_owned(),
            version: "5.8.3".into(),
            association: "5.8".into(),
        };
        let custom = Installation {
            path: d.path().join("Source").to_string_lossy().into_owned(),
            version: "5.8.3".into(),
            association: "{SOURCE-GUID}".into(),
        };
        let result = consolidate_engines(vec![managed.clone(), official, custom], &managed.path);
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|e| e.association == "{SOURCE-GUID}"));
        assert!(result.iter().any(|e| e.path == managed.path));
    }
}
