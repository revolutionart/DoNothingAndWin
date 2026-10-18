//! Editor-maintained opening times, independent of which launcher started Unreal.
use std::{collections::HashMap, fs, io::Read, path::Path};

use crate::{discovery::path_key, model::Project};

/// Called by the workspace worker, never from project-card rendering.
pub fn collect(projects: &[String]) -> HashMap<String, u64> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return HashMap::new();
    };
    collect_from(&Path::new(&local).join("UnrealEngine"), projects)
}

fn collect_from(root: &Path, projects: &[String]) -> HashMap<String, u64> {
    let mut history = HashMap::new();
    if let Ok(versions) = fs::read_dir(root) {
        for version in versions.flatten() {
            for platform in ["WindowsEditor", "Windows"] {
                let file = version
                    .path()
                    .join("Saved/Config")
                    .join(platform)
                    .join("EditorSettings.ini");
                if let Some(text) = read_settings(&file) {
                    parse_settings(&text, &mut history);
                }
            }
        }
    }
    projects
        .iter()
        .filter_map(|path| {
            history
                .get(&path_key(Path::new(path)))
                .map(|time| (path.clone(), *time))
        })
        .collect()
}

pub(crate) fn read_settings(path: &Path) -> Option<String> {
    const LIMIT: u64 = 2 * 1024 * 1024;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > LIMIT {
        return None;
    }
    if bytes.starts_with(&[0xff, 0xfe]) {
        if bytes.len() % 2 != 0 {
            return None;
        }
        let units: Vec<_> = bytes[2..]
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        String::from_utf16(&units).ok()
    } else {
        String::from_utf8(bytes).ok()
    }
}

fn parse_settings(text: &str, history: &mut HashMap<String, u64>) {
    let mut editor_settings = false;
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.starts_with('[') {
            editor_settings = line == "[/Script/UnrealEd.EditorSettings]";
        }
        if !editor_settings {
            continue;
        }
        let Some(record) = line
            .trim_start_matches('+')
            .strip_prefix("RecentlyOpenedProjectFiles=")
        else {
            continue;
        };
        let Some(rest) = record.strip_prefix("(ProjectName=\"") else {
            continue;
        };
        let Some((path, date)) = rest.split_once("\",LastOpenTime=") else {
            continue;
        };
        let Some(date) = date.strip_suffix(')') else {
            continue;
        };
        let Some(time) = parse_time(date) else {
            continue;
        };
        if !Path::new(path).is_absolute() || !path.to_ascii_lowercase().ends_with(".uproject") {
            continue;
        }
        // UEditorEngine writes FDateTime::UtcNow(), not the INI file's modification time.
        let entry = history.entry(path_key(Path::new(path))).or_default();
        *entry = (*entry).max(time);
    }
}

fn parse_time(value: &str) -> Option<u64> {
    let format = time::format_description::parse_borrowed::<2>(
        "[year].[month].[day]-[hour].[minute].[second]",
    )
    .ok()?;
    let timestamp = time::PrimitiveDateTime::parse(value, &format)
        .ok()?
        .assume_utc()
        .unix_timestamp();
    let timestamp = u64::try_from(timestamp).ok()?;
    (timestamp > 0 && timestamp <= crate::epic::now()).then_some(timestamp)
}

/// Keep our existing record after Unreal prunes its ten-entry recent-project list.
pub fn merge(projects: &mut [Project], history: &HashMap<String, u64>) -> bool {
    let mut changed = false;
    for project in projects {
        if let Some(&time) = history.get(&project.path) {
            if time > project.last_opened {
                project.last_opened = time;
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_times_are_utc_and_reject_invalid_or_future_values() {
        assert_eq!(parse_time("2026.10.15-10.55.30"), Some(1792061730));
        assert_eq!(parse_time("2026.02.30-10.55.30"), None);
        assert_eq!(parse_time("0001.01.01-00.00.00"), None);
        assert_eq!(parse_time("9999.01.01-00.00.00"), None);
    }

    #[test]
    fn merges_engine_versions_and_legacy_configs_without_using_file_mtime() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("Example.uproject");
        fs::write(&project, "{}").unwrap();
        let path = project.to_string_lossy().into_owned();
        for (version, platform, date) in [
            ("5.7", "WindowsEditor", "2026.09.10-20.17.01"),
            ("4.27", "Windows", "2026.10.15-10.55.30"),
        ] {
            let dir = temp
                .path()
                .join(version)
                .join("Saved/Config")
                .join(platform);
            fs::create_dir_all(&dir).unwrap();
            let text = format!(
                "[/Script/UnrealEd.EditorSettings]\nRecentlyOpenedProjectFiles=(ProjectName=\"{path}\",LastOpenTime={date})\n[Other]\nRecentlyOpenedProjectFiles=(ProjectName=\"{path}\",LastOpenTime=2026.10.15-11.55.30)\n"
            );
            // Unreal may save Unicode INIs as UTF-16.
            let bytes: Vec<_> = [0xfeffu16]
                .into_iter()
                .chain(text.encode_utf16())
                .flat_map(u16::to_le_bytes)
                .collect();
            fs::write(dir.join("EditorSettings.ini"), bytes).unwrap();
        }
        let missing = temp
            .path()
            .join("Unknown.uproject")
            .to_string_lossy()
            .into_owned();
        let history = collect_from(temp.path(), &[path.clone(), missing.clone()]);
        assert_eq!(history.get(&path), Some(&1792061730));
        assert!(!history.contains_key(&missing));
        let mut projects = vec![Project {
            path,
            last_opened: 0,
            name: "Example".into(),
            description: String::new(),
            engine: "5.7".into(),
            pinned: false,
            artwork: None,
        }];
        assert!(merge(&mut projects, &history));
        assert!(!merge(&mut projects, &history));
        projects[0].last_opened += 60;
        assert!(!merge(&mut projects, &history));
        assert!(!merge(&mut projects, &HashMap::new()));
        assert_eq!(projects[0].last_opened, 1792061790);
    }
}
