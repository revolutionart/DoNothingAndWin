use super::*;

#[derive(Default)]
pub(super) struct WorkspaceCache {
    pub engines: HashMap<String, bool>,
    pub projects: HashMap<String, bool>,
    revision: u64,
    applied: Option<(u64, Paths)>,
    next_refresh: Option<Instant>,
    pending: Option<Receiver<Event>>,
}

enum Event {
    Engines(u64, Paths, HashMap<String, bool>),
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
    projects: HashMap<String, bool>,
    counts: HashMap<String, usize>,
    last_opened: HashMap<String, u64>,
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
                Ok(Event::Engines(revision, scanned, engines)) => {
                    if self
                        .workspace_cache
                        .accepts_revision(revision, &scanned, &paths)
                    {
                        self.workspace_cache.engines = engines;
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
                        self.workspace_cache.projects = snapshot.projects;
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
    // Publish the cheap readiness check before plugin receipt parsing and
    // project history. Those must not leave installed engines "Checking...".
    emit(Event::Engines(revision, paths.clone(), ready.clone()));
    let projects = paths
        .projects
        .iter()
        .map(|p| (p.clone(), Path::new(p).is_file()))
        .collect();
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
        projects,
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
                Event::Engines(revision, scanned, ready) => {
                    assert!(!completed);
                    assert!(WorkspaceCache::default().accepts_revision(revision, &scanned, &paths));
                    assert_eq!(ready.get(&engine.path), Some(&true));
                    readiness_published = true;
                }
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
            if let Event::Engines(..) = event {
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
            projects: HashMap::new(),
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
}
