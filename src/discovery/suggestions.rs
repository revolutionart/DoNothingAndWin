//! Folder recommendations from discovered projects. This never saves preferences.
use super::{inside, path_key};
use crate::model::Project;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

pub struct Suggestion {
    pub path: PathBuf,
    pub projects: Vec<Project>,
}

pub fn folders(projects: Vec<Project>, configured: &[String]) -> Vec<Suggestion> {
    let mut seen = HashSet::new();
    let mut groups = BTreeMap::<String, (PathBuf, Vec<Project>)>::new();
    for project in projects {
        let path = Path::new(&project.path);
        if configured.iter().any(|root| inside(path, Path::new(root)))
            || !seen.insert(path_key(path))
        {
            continue;
        }
        let Some(directory) = path.parent() else {
            continue;
        };
        // Group sibling project folders, but never recommend an entire drive.
        let parent = directory
            .parent()
            .filter(|p| p.file_name().is_some())
            .unwrap_or(directory);
        groups
            .entry(path_key(parent))
            .or_insert_with(|| (parent.to_owned(), vec![]))
            .1
            .push(project);
    }
    let mut candidates: Vec<_> = groups
        .into_values()
        .map(|(parent, projects)| {
            let distinct: HashSet<_> = projects
                .iter()
                .filter_map(|p| Path::new(&p.path).parent())
                .map(path_key)
                .collect();
            let path = if distinct.len() > 1 {
                parent
            } else {
                Path::new(&projects[0].path).parent().unwrap().to_owned()
            };
            Suggestion { path, projects }
        })
        .collect();
    candidates.sort_by_key(|s| (s.path.components().count(), path_key(&s.path)));
    let mut result: Vec<Suggestion> = vec![];
    for suggestion in candidates {
        if let Some(parent) = result
            .iter_mut()
            .find(|r| inside(&suggestion.path, &r.path))
        {
            parent.projects.extend(suggestion.projects);
        } else {
            result.push(suggestion);
        }
    }
    result.sort_by_key(|s| path_key(&s.path));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scan_suggests_custom_roots_without_duplicates_or_known_projects() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "Studio/One",
            "Studio/Two",
            "Studio/Nested/Three",
            "Solo/Game",
            "Known/Game",
            "Ignored/Intermediate/Fake",
        ] {
            let dir = root.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("Game.uproject"),
                r#"{"FileVersion":3,"EngineAssociation":"5.8"}"#,
            )
            .unwrap();
        }
        let report = super::super::scan(
            &[root.path().to_owned()],
            Default::default(),
            Default::default(),
        );
        let mut projects = report.projects;
        projects.push(projects[0].clone());
        let known = vec![root.path().join("Known").to_string_lossy().into_owned()];
        let suggestions = folders(projects, &known);
        assert_eq!(suggestions.len(), 2);
        let studio = suggestions
            .iter()
            .find(|s| path_key(&s.path) == path_key(&root.path().join("Studio")))
            .unwrap();
        assert_eq!(studio.projects.len(), 3);
        assert!(suggestions.iter().any(|s| path_key(&s.path)
            == path_key(&root.path().join("Solo/Game"))
            && s.projects.len() == 1));
        assert!(root.path().join("Studio/One/Game.uproject").is_file());
    }
    #[test]
    fn scan_skips_configured_and_engine_roots_and_can_stop_with_partial_results() {
        let root = tempfile::tempdir().unwrap();
        for name in ["EngineSource", "Known", "KnownExtra", "Custom"] {
            let dir = root.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("Game.uproject"),
                r#"{"FileVersion":3,"EngineAssociation":"5.8"}"#,
            )
            .unwrap();
        }
        let skipped = [root.path().join("EngineSource"), root.path().join("Known")];
        let report = super::super::scan_excluding(
            &[root.path().to_owned()],
            &skipped,
            Default::default(),
            Default::default(),
            |_| {},
        );
        assert_eq!(report.projects.len(), 2);
        assert!(
            report
                .projects
                .iter()
                .any(|p| p.path.contains("KnownExtra"))
        );
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let report = super::super::scan_excluding(
            &[root.path().to_owned()],
            &skipped,
            cancel.clone(),
            Default::default(),
            |_| {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            },
        );
        assert!(report.cancelled);
        assert_eq!(report.projects.len(), 1);
        assert_eq!(folders(report.projects, &[]).len(), 1);
    }
}
