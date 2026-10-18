//! Cheap discovery hints from Unreal and Epic's own local settings.
//! Hints never add folders to preferences or replace a full search.
use super::{path_key, roots};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub fn preferred_roots() -> Vec<PathBuf> {
    let drives = roots();
    let mut candidates = local_app_data()
        .map(|local| settings_roots(&local, &drives))
        .unwrap_or_default();
    if let Some(documents) = super::default_project_root().parent() {
        candidates.push(documents.to_owned());
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        candidates.push(PathBuf::from(home));
    }
    candidates.extend(drives.iter().cloned());
    local_unique(candidates, &drives)
}

fn local_app_data() -> Option<PathBuf> {
    // Use the actual user's Epic/Unreal settings even with an isolated launcher
    // test profile. No settings from these applications are modified.
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
        };
        if let Ok(raw) = SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None) {
            let path = raw.to_string().ok().map(PathBuf::from);
            CoTaskMemFree(Some(raw.0.cast()));
            return path;
        }
    }
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

fn settings_roots(local: &Path, drives: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = vec![local.join("EpicGamesLauncher/Saved/Config/Windows/GameUserSettings.ini")];
    if let Ok(versions) = std::fs::read_dir(local.join("UnrealEngine")) {
        for version in versions.flatten().take(100) {
            for platform in ["WindowsEditor", "Windows"] {
                files.push(
                    version
                        .path()
                        .join("Saved/Config")
                        .join(platform)
                        .join("EditorSettings.ini"),
                );
            }
        }
    }
    let candidates = files
        .into_iter()
        .filter_map(|file| crate::project_history::read_settings(&file))
        .flat_map(|text| parse_locations(&text))
        .collect();
    local_unique(candidates, drives)
}

fn local_unique(candidates: Vec<PathBuf>, drives: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|path| {
            // Check lexically before resolving a hint: never probe network shares
            // or disconnected removable drives while doing a local quick search.
            let text = path.to_string_lossy().replace('\\', "/").to_lowercase();
            let local = path.is_absolute()
                && drives.iter().any(|drive| {
                    let root = drive.to_string_lossy().replace('\\', "/").to_lowercase();
                    text == root.trim_end_matches('/')
                        || text.starts_with(&format!("{}/", root.trim_end_matches('/')))
                });
            local && seen.insert(path_key(path))
        })
        .collect()
}

fn parse_locations(text: &str) -> Vec<PathBuf> {
    let mut section = "";
    let mut result = vec![];
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let line = line.trim_start_matches('+');
        if matches!(section, "[Launcher]" | "[/Script/UnrealEd.EditorSettings]") {
            if let Some(value) = line.strip_prefix("CreatedProjectPaths=") {
                result.push(PathBuf::from(value.trim_matches('"')));
            }
        }
        if section == "[/Script/UnrealEd.EditorSettings]" {
            if let Some(value) = line.strip_prefix("RecentlyOpenedProjectFiles=") {
                let path = if let Some(record) = value.strip_prefix("(ProjectName=\"") {
                    record.split_once('"').map(|(path, _)| path)
                } else {
                    Some(value.trim_matches('"'))
                };
                if let Some(path) =
                    path.filter(|path| path.to_ascii_lowercase().ends_with(".uproject"))
                {
                    if let Some(parent) = Path::new(path).parent() {
                        result.push(parent.to_owned());
                        // Nearby sibling projects are common. Do not promote a
                        // drive root ahead of the explicitly known locations.
                        if let Some(collection) =
                            parent.parent().filter(|p| p.file_name().is_some())
                        {
                            result.push(collection.to_owned());
                        }
                    }
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Read-only benchmark of project discovery on the current PC"]
    fn benchmark_local_quick_search() {
        let started = std::time::Instant::now();
        let roots = preferred_roots();
        let hints_elapsed = started.elapsed();
        let mut excluded = vec![super::super::default_project_root()];
        excluded.extend(
            crate::platform::display_engines()
                .into_iter()
                .map(|e| PathBuf::from(e.path)),
        );
        let mut first = None;
        let report = super::super::scan_quick(
            &roots,
            &excluded,
            Default::default(),
            Default::default(),
            |_| {
                first.get_or_insert_with(|| started.elapsed());
            },
        );
        eprintln!(
            "Quick discovery: hints={hints_elapsed:?}, first={first:?}, elapsed={:?}, folders={}, projects={}, limited={}",
            started.elapsed(),
            report.directories,
            report.projects.len(),
            report.limited
        );
        assert!(report.directories <= 3000);
    }
    #[test]
    fn reads_editor_and_epic_hints_without_unrelated_ini_values_or_remote_paths() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("Local");
        let studio = temp.path().join("Studio");
        let project = studio.join("Game/Game.uproject");
        let editor = local.join("UnrealEngine/5.8/Saved/Config/WindowsEditor/EditorSettings.ini");
        std::fs::create_dir_all(editor.parent().unwrap()).unwrap();
        let text = format!(
            "[/Script/UnrealEd.EditorSettings]\nRecentlyOpenedProjectFiles=(ProjectName=\"{}\",LastOpenTime=invalid)\nCreatedProjectPaths={}\nCreatedProjectPaths=\\\\offline-server\\Share\n[Other]\nCreatedProjectPaths={}\n",
            project.display(),
            studio.display(),
            temp.path().join("Wrong").display()
        );
        let bytes: Vec<_> = [0xfeffu16]
            .into_iter()
            .chain(text.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        std::fs::write(editor, bytes).unwrap();
        let epic = local.join("EpicGamesLauncher/Saved/Config/Windows/GameUserSettings.ini");
        std::fs::create_dir_all(epic.parent().unwrap()).unwrap();
        std::fs::write(
            epic,
            format!("[Launcher]\nCreatedProjectPaths={}\n", studio.display()),
        )
        .unwrap();
        let hints = settings_roots(&local, &[temp.path().into()]);
        assert_eq!(hints.len(), 2);
        assert!(hints.contains(&studio));
        assert!(hints.contains(&project.parent().unwrap().to_owned()));
    }
}
