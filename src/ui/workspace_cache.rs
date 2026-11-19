use super::*;

#[derive(Default)]
pub(super) struct WorkspaceCache {
    pub engines: HashMap<String, bool>,
    pub engine_builds: HashMap<String, String>,
    pub projects: HashMap<String, bool>,
    revision: u64,
    applied: Option<(u64, Paths)>,
    next_refresh: Option<Instant>,
    pending: Option<Receiver<Event>>,
}

pub(super) fn installed_engine_builds(engines: &[Installation]) -> HashMap<String, String> {
    engines
        .iter()
        .filter_map(|engine| {
            let build =
                engine_workspace::maintenance::promoted_engine_build(Path::new(&engine.path))?;
            Some((engine.path.clone(), build))
        })
        .collect()
}

enum Event {
    Engines {
        revision: u64,
        paths: Paths,
        ready: HashMap<String, bool>,
        builds: HashMap<String, String>,
    },
    Projects {
        revision: u64,
        paths: Paths,
        existing: HashMap<String, bool>,
        associations: HashMap<String, String>,
    },
    Complete(Snapshot),
}

#[derive(Clone, PartialEq, Eq)]
struct Paths {
    engines: Vec<String>,
    projects: Vec<String>,
}
struct Snapshot {
    revision: u64,
    paths: Paths,
    engines: HashMap<String, bool>,
    counts: HashMap<String, usize>,
    last_opened: HashMap<String, u64>,
}

fn refresh_project_associations(
    projects: &mut [Project],
    associations: &HashMap<String, String>,
) -> bool {
    let mut changed = false;
    for project in projects {
        if let Some(association) = associations.get(&project.path)
            && project.engine != *association
        {
            project.engine.clone_from(association);
            changed = true;
        }
    }
    changed
}
impl WorkspaceCache {
    pub fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
    fn accepts(&self, snapshot: &Snapshot, paths: &Paths) -> bool {
        self.accepts_revision(snapshot.revision, &snapshot.paths, paths)
    }
    fn accepts_revision(&self, revision: u64, scanned: &Paths, current: &Paths) -> bool {
        revision == self.revision && scanned == current
    }
}

impl Workspace {
    pub(super) fn poll_workspace_cache(&mut self, ctx: &Context) {
        if self.demo {
            return;
        }
        let paths = Paths {
            engines: self.engines.iter().map(|e| e.path.clone()).collect(),
            projects: self.projects.iter().map(|p| p.path.clone()).collect(),
        };
        if let Some(rx) = &self.workspace_cache.pending {
            match rx.try_recv() {
                Ok(Event::Engines {
                    revision,
                    paths: scanned,
                    ready,
                    builds,
                }) => {
                    if self
                        .workspace_cache
                        .accepts_revision(revision, &scanned, &paths)
                    {
                        self.workspace_cache.engines = ready;
                        self.workspace_cache.engine_builds = builds;
                    }
                }
                Ok(Event::Projects {
                    revision,
                    paths: scanned,
                    existing,
                    associations,
                }) => {
                    if self
                        .workspace_cache
                        .accepts_revision(revision, &scanned, &paths)
                    {
                        self.workspace_cache.projects = existing;
                        if refresh_project_associations(&mut self.projects, &associations) {
                            self.persist();
                        }
                    }
                }
                Ok(Event::Complete(snapshot)) => {
                    self.workspace_cache.pending = None;
                    // An install, removal or rescan may have finished while this worker ran.
                    // Never let that older result overwrite the newly invalidated state.
                    if self.workspace_cache.accepts(&snapshot, &paths) {
                        if engine_workspace::project_history::merge(
                            &mut self.projects,
                            &snapshot.last_opened,
                        ) {
                            self.persist();
                        }
                        self.workspace_cache.engines = snapshot.engines;
                        self.plugin_counts = snapshot.counts;
                        self.workspace_cache.applied = Some((snapshot.revision, snapshot.paths));
                        self.workspace_cache.next_refresh =
                            Some(Instant::now() + Duration::from_secs(30));
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.workspace_cache.pending = None;
                    // Retry later without spawning a worker every frame if it failed.
                    self.workspace_cache.applied =
                        Some((self.workspace_cache.revision, paths.clone()));
                    self.workspace_cache.next_refresh =
                        Some(Instant::now() + Duration::from_secs(30));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let current = (self.workspace_cache.revision, paths.clone());
        let due = self.workspace_cache.applied.as_ref() != Some(&current)
            || self
                .workspace_cache
                .next_refresh
                .is_none_or(|next| Instant::now() >= next);
        if self.workspace_cache.pending.is_none() && due {
            let revision = self.workspace_cache.revision;
            let engines = self.engines.clone();
            let (tx, rx) = mpsc::channel();
            self.workspace_cache.pending = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                scan_workspace(revision, paths, engines, |event| {
                    let _ = tx.send(event);
                    ctx.request_repaint();
                });
            });
        }
    }
}

fn scan_workspace(
    revision: u64,
    paths: Paths,
    engines: Vec<Installation>,
    mut emit: impl FnMut(Event),
) {
    let ready: HashMap<_, _> = engines
        .iter()
        .map(|e| (e.path.clone(), platform::engine_ready(e)))
        .collect();
    let engine_builds = installed_engine_builds(&engines);
    // Publish engine information before plugin receipt parsing and project history.
    emit(Event::Engines {
        revision,
        paths: paths.clone(),
        ready: ready.clone(),
        builds: engine_builds,
    });
    let mut project_associations = HashMap::new();
    let projects = paths
        .projects
        .iter()
        .map(|p| {
            let path = Path::new(p);
            let exists = path.is_file();
            if exists && let Ok(association) = platform::project_engine_association(path) {
                project_associations.insert(p.clone(), association);
            }
            (p.clone(), exists)
        })
        .collect();
    emit(Event::Projects {
        revision,
        paths: paths.clone(),
        existing: projects,
        associations: project_associations,
    });
    let counts = engines
        .iter()
        .map(|e| {
            (
                e.path.clone(),
                engine_workspace::plugins::installed(Path::new(&e.path)).len(),
            )
        })
        .collect();
    emit(Event::Complete(Snapshot {
        last_opened: engine_workspace::project_history::collect(&paths.projects),
        revision,
        paths,
        engines: ready,
        counts,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_readiness_is_published_before_inventory_finishes() {
        let temp = tempfile::tempdir().unwrap();
        let binaries = temp.path().join("Engine/Binaries/Win64");
        std::fs::create_dir_all(&binaries).unwrap();
        std::fs::write(binaries.join("UnrealEditor.exe"), b"fixture").unwrap();
        let engine = Installation {
            version: "5.8.3".into(),
            path: temp.path().to_string_lossy().into_owned(),
            association: "5.8".into(),
        };
        let paths = Paths {
            engines: vec![engine.path.clone()],
            projects: vec![],
        };
        let mut readiness_published = false;
        let mut completed = false;
        scan_workspace(
            0,
            paths.clone(),
            vec![engine.clone()],
            |event| match event {
                Event::Engines {
                    revision,
                    paths: scanned,
                    ready,
                    ..
                } => {
                    assert!(!completed);
                    assert!(WorkspaceCache::default().accepts_revision(revision, &scanned, &paths));
                    assert_eq!(ready.get(&engine.path), Some(&true));
                    readiness_published = true;
                }
                Event::Projects { .. } => {}
                Event::Complete(snapshot) => {
                    assert!(
                        readiness_published,
                        "Engine cards must not wait for the full inventory"
                    );
                    assert_eq!(snapshot.counts.get(&engine.path), Some(&0));
                    completed = true;
                }
            },
        );
        assert!(readiness_published && completed);
    }

    #[test]
    #[ignore = "Read-only timing check against locally installed engines"]
    fn local_engine_status_is_not_delayed_by_plugin_inventory() {
        let engines = platform::display_engines();
        assert!(!engines.is_empty());
        let paths = Paths {
            engines: engines.iter().map(|e| e.path.clone()).collect(),
            projects: vec![],
        };
        let start = Instant::now();
        let mut ready = false;
        scan_workspace(0, paths, engines, |event| {
            if let Event::Engines { .. } = event {
                eprintln!("Engine readiness published in {:?}", start.elapsed());
                assert!(start.elapsed() < Duration::from_secs(2));
                ready = true;
            }
        });
        assert!(ready);
    }
    fn paths(project: &str) -> Paths {
        Paths {
            engines: vec!["engine".into()],
            projects: vec![project.into()],
        }
    }
    #[test]
    fn cache_rejects_results_from_before_mutation_or_changed_inventory() {
        let mut cache = WorkspaceCache::default();
        let snapshot = Snapshot {
            revision: 0,
            paths: paths("project"),
            engines: HashMap::new(),
            counts: HashMap::new(),
            last_opened: HashMap::new(),
        };
        assert!(cache.accepts(&snapshot, &paths("project")));
        assert!(!cache.accepts(&snapshot, &paths("new project")));
        cache.invalidate();
        assert!(!cache.accepts(&snapshot, &paths("project")));
        assert!(!cache.accepts_revision(0, &paths("project"), &paths("project")));
        assert!(!cache.accepts_revision(1, &paths("old"), &paths("new")));
    }

    #[test]
    fn workspace_refresh_reads_changed_project_association() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Example.uproject");
        std::fs::write(&path, br#"{"FileVersion":3,"EngineAssociation":"5.3"}"#).unwrap();
        let mut project = platform::import_project(&path).unwrap();
        project.pinned = true;
        project.last_opened = 123;
        let paths = paths(&project.path);

        for (contents, expected, changed) in [
            (
                r#"{"FileVersion":3,"EngineAssociation":"5.7"}"#,
                "5.7",
                true,
            ),
            ("{", "5.7", false),
            (
                r#"{"FileVersion":3,"EngineAssociation":"{CUSTOM}"}"#,
                "{CUSTOM}",
                true,
            ),
            (r#"{"FileVersion":3}"#, "", true),
        ] {
            std::fs::write(&path, contents).unwrap();
            scan_workspace(0, paths.clone(), vec![], |event| {
                if let Event::Projects { associations, .. } = event {
                    assert_eq!(
                        refresh_project_associations(
                            std::slice::from_mut(&mut project),
                            &associations,
                        ),
                        changed
                    );
                    assert_eq!(project.engine, expected);
                    assert!(project.pinned);
                    assert_eq!(project.last_opened, 123);
                }
            });
        }
    }
}
