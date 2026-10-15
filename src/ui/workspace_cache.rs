use super::*;

#[derive(Default)]
pub(super) struct WorkspaceCache {
    pub engines: HashMap<String, bool>,
    pub projects: HashMap<String, bool>,
    revision: u64,
    applied: Option<(u64, Paths)>,
    next_refresh: Option<Instant>,
    pending: Option<Receiver<Snapshot>>,
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
        snapshot.revision == self.revision && snapshot.paths == *paths
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
                Ok(snapshot) => {
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
                let ready = engines
                    .iter()
                    .map(|e| (e.path.clone(), platform::engine_ready(e)))
                    .collect();
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
                let _ = tx.send(Snapshot {
                    last_opened: engine_workspace::project_history::collect(&paths.projects),
                    revision,
                    paths,
                    engines: ready,
                    projects,
                    counts,
                });
                ctx.request_repaint();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
