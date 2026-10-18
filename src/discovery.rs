use crate::{model::Project, platform};
pub mod quick;
pub mod suggestions;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
#[derive(Default)]
pub struct Report {
    pub projects: Vec<Project>,
    pub directories: usize,
    pub skipped: usize,
    pub cancelled: bool,
    pub limited: bool,
}
pub fn roots() -> Vec<PathBuf> {
    let mut result = Vec::new();
    #[cfg(windows)]
    {
        for letter in b'A'..=b'Z' {
            let root = format!("{}:/", letter as char);
            let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            if unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr()) } == 3
            {
                result.push(root.into());
            }
        }
    }
    #[cfg(not(windows))]
    if let Some(home) = std::env::var_os("HOME") {
        result.push(home.into());
    }
    result
}
fn excluded(p: &Path) -> bool {
    p.file_name().is_some_and(|n| {
        matches!(
            n.to_string_lossy().to_ascii_lowercase().as_str(),
            "windows"
                | "$recycle.bin"
                | "system volume information"
                | "programdata"
                | "appdata"
                | ".git"
                | "node_modules"
                | "target"
                | "deriveddatacache"
                | "intermediate"
                | "binaries"
                | "saved"
                | "vaultcache"
                | ".engine-workspace"
        )
    })
}
pub fn scan(roots: &[PathBuf], cancel: Arc<AtomicBool>, progress: Arc<AtomicUsize>) -> Report {
    scan_stream(roots, cancel, progress, |_| {})
}
pub fn scan_stream(
    roots: &[PathBuf],
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    found: impl FnMut(Project),
) -> Report {
    scan_excluding(roots, &[], cancel, progress, found)
}
pub fn scan_excluding(
    roots: &[PathBuf],
    excluded_roots: &[PathBuf],
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    found: impl FnMut(Project),
) -> Report {
    walk(roots, excluded_roots, cancel, progress, None, found)
}

pub fn scan_quick(
    roots: &[PathBuf],
    excluded_roots: &[PathBuf],
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    found: impl FnMut(Project),
) -> Report {
    walk(
        roots,
        excluded_roots,
        cancel,
        progress,
        Some((4, 3000, std::time::Duration::from_secs(8))),
        found,
    )
}

fn walk(
    roots: &[PathBuf],
    excluded_roots: &[PathBuf],
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    limits: Option<(usize, usize, std::time::Duration)>,
    mut found: impl FnMut(Project),
) -> Report {
    let started = std::time::Instant::now();
    let excluded_roots: HashSet<_> = excluded_roots.iter().map(|p| path_key(p)).collect();
    let mut report = Report::default();
    let mut stack: std::collections::VecDeque<_> = roots.iter().cloned().map(|p| (p, 0)).collect();
    let mut seen = HashSet::new();
    let mut visited = HashSet::new();
    while let Some((dir, depth)) = if limits.is_some() {
        stack.pop_front()
    } else {
        stack.pop_back()
    } {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        if limits
            .is_some_and(|(_, max, time)| report.directories >= max || started.elapsed() >= time)
        {
            report.limited = true;
            break;
        }
        let Ok(meta) = std::fs::symlink_metadata(&dir) else {
            report.skipped += 1;
            continue;
        };
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                report.skipped += 1;
                continue;
            }
        }
        if meta.file_type().is_symlink()
            || excluded(&dir)
            || dir
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".engine-workspace-"))
        {
            continue;
        }
        // Resolve only after rejecting folder links. Hinted paths can be inside
        // a configured root, rather than equal to it.
        let key = path_key(&dir);
        if excluded_roots
            .iter()
            .any(|root| key == *root || key.starts_with(&format!("{root}/")))
            || !visited.insert(key)
        {
            continue;
        }
        if dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("UE_"))
            && dir.join("Engine/Build/Build.version").is_file()
        {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            report.skipped += 1;
            continue;
        };
        report.directories += 1;
        progress.store(report.directories, Ordering::Relaxed);
        let mut children = vec![];
        let mut project_folder = false;
        for entry in entries.flatten() {
            if cancel.load(Ordering::Relaxed) {
                report.cancelled = true;
                return report;
            }
            if limits.is_some_and(|(_, _, time)| started.elapsed() >= time) {
                report.limited = true;
                return report;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                children.push(path);
            } else if path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("uproject"))
            {
                if let Ok(p) = platform::import_project(&path) {
                    project_folder = true;
                    if seen.insert(p.path.to_ascii_lowercase()) {
                        found(p.clone());
                        report.projects.push(p);
                    }
                }
            }
        }
        for child in children {
            // Only prune these names after finding a valid descriptor. A user's
            // custom project collection can legitimately be named Content or Source.
            if project_folder
                && child.file_name().is_some_and(|n| {
                    matches!(
                        n.to_string_lossy().to_ascii_lowercase().as_str(),
                        "content" | "plugins" | "source" | "config"
                    )
                })
            {
                continue;
            }
            if limits.is_some_and(|(max_depth, _, _)| depth >= max_depth) {
                report.limited = true;
            } else {
                stack.push_back((child, depth + 1));
            }
        }
    }
    report
}
pub fn merge(existing: &mut Vec<Project>, found: Vec<Project>) -> usize {
    let mut keys: HashSet<_> = existing
        .iter()
        .map(|p| p.path.to_ascii_lowercase())
        .collect();
    let before = existing.len();
    for p in found {
        if keys.insert(p.path.to_ascii_lowercase()) {
            existing.push(p);
        }
    }
    existing.len() - before
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quick_search_prioritizes_hints_and_respects_work_limits_without_losing_full_scan() {
        let temp = tempfile::tempdir().unwrap();
        let known = temp.path().join("Deep/A/B/C/D/Game");
        let other = temp.path().join("Content/Collection/Other");
        for dir in [&known, &other] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("Game.uproject"), r#"{"FileVersion":3}"#).unwrap();
        }
        let roots = [known.clone(), temp.path().into()];
        let progress = Arc::new(AtomicUsize::new(0));
        let mut streamed = vec![];
        let quick = walk(
            &roots,
            &[],
            Arc::default(),
            progress.clone(),
            Some((1, 3, std::time::Duration::from_secs(5))),
            |p| streamed.push(p),
        );
        assert!(quick.limited);
        assert!(quick.directories <= 3);
        assert_eq!(progress.load(Ordering::Relaxed), quick.directories);
        assert_eq!(quick.projects.len(), 1);
        assert_eq!(streamed[0].path, quick.projects[0].path);
        let full = scan_excluding(&roots, &[], Arc::default(), Arc::default(), |_| {});
        assert_eq!(full.projects.len(), 2);
        assert!(!full.limited);
        // Deep hints cannot bypass a configured root exclusion.
        let excluded = scan_quick(
            &roots,
            &[temp.path().join("Deep")],
            Arc::default(),
            Arc::default(),
            |_| {},
        );
        assert_eq!(excluded.projects.len(), 1);
        assert!(excluded.projects[0].path.contains("Other"));
        let timed = walk(
            &roots,
            &[],
            Arc::default(),
            Arc::default(),
            Some((4, 3000, std::time::Duration::ZERO)),
            |_| panic!("deadline exceeded"),
        );
        assert!(timed.limited);
        assert_eq!(timed.directories, 0);
    }
    #[test]
    fn discovery_does_not_walk_assets_inside_a_recognized_project() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("Studio/Game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("Game.uproject"), r#"{"FileVersion":3}"#).unwrap();
        for n in 0..200 {
            std::fs::create_dir_all(game.join(format!("Content/Assets/{n}/Textures"))).unwrap();
        }
        let started = std::time::Instant::now();
        let report = scan(&[temp.path().into()], Arc::default(), Arc::default());
        eprintln!(
            "{} directories in {:?}",
            report.directories,
            started.elapsed()
        );
        assert_eq!(report.projects.len(), 1);
        assert!(
            report.directories <= 3,
            "walked {} folders inside project assets",
            report.directories
        );
    }
    #[test]
    fn finds_nested_projects_preserves_pins_and_skips_generated() {
        let d = tempfile::tempdir().unwrap();
        for root in ["Studio/Projects/Deep/Test", "Other/Intermediate/Fake"] {
            let p = d.path().join(root);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(
                p.join("Test.uproject"),
                r#"{"FileVersion":3,"EngineAssociation":"5.7"}"#,
            )
            .unwrap();
        }
        let r = scan(&[d.path().into()], Arc::default(), Arc::default());
        assert_eq!(r.projects.len(), 1);
        let mut existing = r.projects.clone();
        existing[0].pinned = true;
        assert_eq!(merge(&mut existing, r.projects), 0);
        assert!(existing[0].pinned);
    }
    #[test]
    fn cancellation_returns_partial_state() {
        let r = scan(&roots(), Arc::new(AtomicBool::new(true)), Arc::default());
        assert!(r.cancelled);
        assert_eq!(r.directories, 0);
    }
}

/// Canonical, case-insensitive Windows identity; strips the display-only long-path prefix.
pub fn path_key(path: &Path) -> String {
    // Missing descendants still need the canonical identity of their existing
    // parent (e.g. RUNNER~1 versus runneradmin on Windows CI machines).
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let mut ancestor = absolute.as_path();
    let mut tail = Vec::new();
    let path = loop {
        if let Ok(mut resolved) = std::fs::canonicalize(ancestor) {
            for component in tail.iter().rev() {
                match component {
                    std::path::Component::ParentDir => {
                        resolved.pop();
                    }
                    std::path::Component::CurDir => {}
                    _ => resolved.push(component.as_os_str()),
                }
            }
            break resolved;
        }
        let Some(parent) = ancestor.parent() else {
            break absolute.clone();
        };
        if let Some(component) = ancestor.components().next_back() {
            tail.push(component);
        }
        ancestor = parent;
    };
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}
pub fn inside(path: &Path, root: &Path) -> bool {
    let path = path_key(path);
    let root = path_key(root);
    path == root || path.starts_with(&(root + "/"))
}
pub fn installed_bytes(roots: &[PathBuf]) -> (u64, bool) {
    let mut seen = HashSet::new();
    let mut bytes = 0u64;
    let mut complete = true;
    let mut stack = roots.to_vec();
    while let Some(path) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            complete = false;
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                continue;
            }
        }
        if meta.is_dir() {
            if !seen.insert(path_key(&path)) {
                continue;
            }
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(e) => stack.push(e.path()),
                            Err(_) => complete = false,
                        }
                    }
                }
                Err(_) => complete = false,
            }
        } else if meta.is_file() {
            bytes = bytes.saturating_add(meta.len());
        }
    }
    (bytes, complete)
}
#[cfg(test)]
mod folder_tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn missing_descendant_matches_short_and_long_windows_parent_paths() {
        use std::os::windows::ffi::OsStrExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Long Project Folder Name");
        std::fs::create_dir(&root).unwrap();
        let wide: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut buffer = vec![0u16; 32768];
        let length = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
                wide.as_ptr(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            )
        };
        assert!(length > 0 && (length as usize) < buffer.len());
        let alias = PathBuf::from(String::from_utf16(&buffer[..length as usize]).unwrap());
        assert!(inside(&alias.join("NotCreated/Game.uproject"), &root));
        assert_eq!(
            path_key(&alias.join("NotCreated/Game.uproject")),
            path_key(&root.join("NotCreated/Game.uproject"))
        );
        assert!(!inside(
            &alias.join("NotCreated/../../Outside.uproject"),
            &root
        ));
    }

    #[test]
    fn roots_match_boundaries_and_deduplicate_sizes() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Projects");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("sample.bin"), [0u8; 37]).unwrap();
        assert!(inside(&root.join("Example/Game.uproject"), &root));
        assert!(!inside(
            &d.path().join("ProjectsBackup/Game.uproject"),
            &root
        ));
        assert_eq!(installed_bytes(&[root.clone(), root]), (37, true));
    }
}

pub fn retain_project_roots(projects: &mut Vec<Project>, roots: &[String], manual: &[String]) {
    let manual: HashSet<_> = manual.iter().map(|p| path_key(Path::new(p))).collect();
    projects.retain(|p| {
        manual.contains(&path_key(Path::new(&p.path)))
            || roots
                .iter()
                .any(|r| inside(Path::new(&p.path), Path::new(r)))
    });
}
#[cfg(test)]
mod managed_folder_tests {
    use super::*;
    #[test]
    fn removing_root_keeps_other_roots_and_manual_imports_without_touching_files() {
        let d = tempfile::tempdir().unwrap();
        let mut projects = vec![];
        for name in [
            "Selected/A",
            "Selected/B",
            "Backups/Tutorial",
            "Manual/Game",
        ] {
            let folder = d.path().join(name);
            std::fs::create_dir_all(&folder).unwrap();
            let file = folder.join("Game.uproject");
            std::fs::write(&file, r#"{"FileVersion":3,"EngineAssociation":"5.8"}"#).unwrap();
            projects.push(platform::import_project(&file).unwrap());
        }
        let files: Vec<_> = projects.iter().map(|p| p.path.clone()).collect();
        let manual = vec![projects[3].path.clone()];
        retain_project_roots(
            &mut projects,
            &[d.path().join("Selected").display().to_string()],
            &manual,
        );
        assert_eq!(projects.len(), 3);
        retain_project_roots(&mut projects, &[], &manual);
        assert_eq!(projects.len(), 1);
        assert!(files.iter().all(|p| Path::new(p).is_file()));
    }
}

/// Epic's standard project location, honoring redirected Windows Documents.
pub fn default_project_root() -> PathBuf {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{FOLDERID_Documents, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
        };
        if let Ok(raw) = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) {
            let text = raw.to_string().ok();
            CoTaskMemFree(Some(raw.0.cast()));
            if let Some(text) = text {
                return PathBuf::from(text).join("Unreal Projects");
            }
        }
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default())
        .join("Documents/Unreal Projects")
}
pub fn project_roots(extra: &[String]) -> Vec<String> {
    let mut roots = vec![default_project_root().to_string_lossy().into_owned()];
    for path in extra {
        if !roots
            .iter()
            .any(|p| path_key(Path::new(p)) == path_key(Path::new(path)))
        {
            roots.push(path.clone());
        }
    }
    roots
}

/// Prune only confirmed missing files; transient permission/I/O errors retain entries.
pub fn prune_missing_projects(projects: &mut Vec<Project>) -> usize {
    let before = projects.len();
    projects.retain(
        |p| !matches!(std::fs::metadata(&p.path),Err(e) if e.kind()==std::io::ErrorKind::NotFound),
    );
    before - projects.len()
}
#[cfg(test)]
mod refresh_tests {
    use super::*;
    #[test]
    fn refresh_prunes_deleted_projects_without_touching_existing_files() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("Keep.uproject");
        std::fs::write(&path, r#"{"FileVersion":3}"#).unwrap();
        let keep = crate::platform::import_project(&path).unwrap();
        let mut gone = keep.clone();
        gone.path = d
            .path()
            .join("Gone.uproject")
            .to_string_lossy()
            .into_owned();
        let mut projects = vec![keep, gone];
        assert_eq!(prune_missing_projects(&mut projects), 1);
        assert_eq!(projects.len(), 1);
        assert!(path.exists());
    }
}
