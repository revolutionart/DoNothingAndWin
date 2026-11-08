//! Session-scoped Library transfer queue. Packages are fetched only at dispatch.
use engine_workspace::{
    fab::{Item, Version},
    model::Installation,
};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(super) struct Request {
    pub item: Item,
    pub target: Target,
}

#[derive(Clone)]
pub(super) enum Target {
    Engines(Vec<Installation>),
    Import {
        project: PathBuf,
        name: String,
        version: Version,
        branch: String,
    },
    Create {
        folder: PathBuf,
        name: String,
        version: Version,
        branch: String,
    },
}

impl Target {
    pub fn engine_label(&self) -> String {
        match self {
            Self::Engines(engines) => engines
                .iter()
                .map(|engine| engine.version.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            Self::Import { branch, .. } | Self::Create { branch, .. } => branch.clone(),
        }
    }

    pub fn description(&self) -> String {
        let engine = self.engine_label();
        match self {
            Self::Engines(_) => format!("UE {engine}"),
            Self::Import { name, .. } => format!("Add to {name} · UE {engine}"),
            Self::Create { name, .. } => format!("Create {name} · UE {engine}"),
        }
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    fn key(path: &Path) -> String {
        path.components()
            .collect::<PathBuf>()
            .to_string_lossy()
            .replace('\\', "/")
    }
    key(left).eq_ignore_ascii_case(&key(right))
}
#[derive(Default)]
pub(super) struct Queue {
    waiting: VecDeque<Request>,
    active: Option<Request>,
}
impl Queue {
    pub fn waiting(&self) -> &VecDeque<Request> {
        &self.waiting
    }
    pub fn contains(&self, item: &Item, path: &str) -> bool {
        self.active.iter().chain(self.waiting.iter()).any(|job| {
            job.item.key() == item.key()
                && matches!(&job.target, Target::Engines(engines) if engines.iter().any(|e| e.path.eq_ignore_ascii_case(path)))
        })
    }

    pub fn contains_import(&self, item: &Item, path: &Path) -> bool {
        self.active.iter().chain(self.waiting.iter()).any(|job| {
            job.item.key() == item.key()
                && matches!(&job.target, Target::Import { project, .. } if same_path(project, path))
        })
    }

    pub fn contains_creation(&self, destination: &Path) -> bool {
        self.active.iter().chain(self.waiting.iter()).any(|job| {
            matches!(&job.target, Target::Create { folder, name, .. } if same_path(&folder.join(name), destination))
        })
    }

    pub fn enqueue_project(&mut self, job: Request) -> bool {
        let available = match &job.target {
            Target::Import { project, .. } => !self.contains_import(&job.item, project),
            Target::Create { folder, name, .. } => !self.contains_creation(&folder.join(name)),
            Target::Engines(_) => false,
        };
        if available {
            self.waiting.push_back(job);
        }
        available
    }
    pub fn enqueue(&mut self, item: Item, engines: Vec<Installation>) -> usize {
        let mut accepted: Vec<Installation> = vec![];
        for engine in engines {
            if !self.contains(&item, &engine.path)
                && !accepted
                    .iter()
                    .any(|e| e.path.eq_ignore_ascii_case(&engine.path))
            {
                accepted.push(engine);
            }
        }
        let count = accepted.len();
        if count > 0 {
            self.waiting.push_back(Request {
                item,
                target: Target::Engines(accepted),
            });
        }
        count
    }
    pub fn start_next(&mut self, ready: bool) -> Option<Request> {
        if !ready || self.active.is_some() {
            return None;
        }
        let next = self.waiting.pop_front()?;
        self.active = Some(next.clone());
        Some(next)
    }
    pub fn finished(&mut self) {
        self.active = None;
    }
    pub fn remove(&mut self, index: usize) {
        self.waiting.remove(index);
    }
    pub fn clear_waiting(&mut self) {
        self.waiting.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(name: &str) -> Item {
        serde_json::from_value(serde_json::json!({"assetId":name,"assetNamespace":"test","title":name,"distributionMethod":"CODE_PLUGIN"})).unwrap()
    }
    fn engine(version: &str) -> Installation {
        Installation {
            version: version.into(),
            path: format!("C:/UE_{version}"),
            association: version.into(),
        }
    }
    #[test]
    fn queue_waits_for_idle_and_dispatches_in_order_with_original_engine_targets() {
        let mut q = Queue::default();
        q.enqueue(item("first"), vec![engine("5.6.1"), engine("5.8.3")]);
        q.enqueue(item("second"), vec![engine("5.7.4")]);
        assert!(q.start_next(false).is_none());
        let first = q.start_next(true).unwrap();
        assert_eq!(first.item.title, "first");
        assert_eq!(first.target.engine_label(), "5.6.1, 5.8.3");
        assert!(q.start_next(true).is_none(), "never overlap active jobs");
        q.finished(); // Success, failure and stopped completions all release the active job.
        let second = q.start_next(true).unwrap();
        assert_eq!(second.item.title, "second");
        assert_eq!(second.target.engine_label(), "5.7.4");
        q.finished();
        assert!(q.start_next(true).is_none());
    }
    #[test]
    fn duplicate_active_and_waiting_targets_are_ignored_but_other_engines_can_be_added() {
        let mut q = Queue::default();
        assert_eq!(
            q.enqueue(item("plugin"), vec![engine("5.8.3"), engine("5.8.3")]),
            1
        );
        q.start_next(true).unwrap();
        assert_eq!(
            q.enqueue(item("plugin"), vec![engine("5.8.3"), engine("5.6.1")]),
            1
        );
        let mut alias = engine("5.6.1");
        alias.path = alias.path.to_uppercase();
        assert_eq!(q.enqueue(item("plugin"), vec![alias]), 0);
        assert_eq!(q.waiting().len(), 1);
        q.finished();
        assert_eq!(q.start_next(true).unwrap().target.engine_label(), "5.6.1");
    }
    #[test]
    fn removing_or_clearing_waiting_jobs_does_not_interrupt_the_active_job() {
        let mut q = Queue::default();
        q.enqueue(item("active"), vec![engine("5.8.3")]);
        q.start_next(true).unwrap();
        q.enqueue(item("remove"), vec![engine("5.7.4")]);
        q.enqueue(item("keep"), vec![engine("5.6.1")]);
        q.remove(0);
        assert_eq!(q.waiting()[0].item.title, "keep");
        q.clear_waiting();
        assert!(q.contains(&item("active"), &engine("5.8.3").path));
        assert!(q.start_next(true).is_none());
        q.finished();
        assert!(q.start_next(true).is_none());
    }

    #[test]
    fn project_jobs_wait_in_order_and_keep_their_original_choices() {
        let version: Version = serde_json::from_value(serde_json::json!({
            "artifactId": "chosen-build", "engineVersions": ["5.8"]
        }))
        .unwrap();
        let mut queue = Queue::default();
        queue.enqueue(item("plugin"), vec![engine("5.8.3")]);
        queue.start_next(true).unwrap();

        let import = Request {
            item: item("asset"),
            target: Target::Import {
                project: "C:/Projects/Game/Game.uproject".into(),
                name: "Game".into(),
                version: version.clone(),
                branch: "5.8".into(),
            },
        };
        let create = Request {
            item: item("template"),
            target: Target::Create {
                folder: "C:/Projects".into(),
                name: "NewGame".into(),
                version,
                branch: "5.8".into(),
            },
        };
        assert!(queue.enqueue_project(import.clone()));
        assert!(queue.enqueue_project(create.clone()));
        assert!(!queue.enqueue_project(import.clone()));
        let mut same_folder = create.clone();
        same_folder.item = item("different-template");
        assert!(!queue.enqueue_project(same_folder));
        assert!(queue.start_next(true).is_none());
        queue.finished();

        let next = queue.start_next(true).unwrap();
        assert!(
            matches!(next.target, Target::Import { project, version, .. }
            if project == Path::new("C:/Projects/Game/Game.uproject") && version.artifact_id == "chosen-build")
        );
        assert!(!queue.enqueue_project(import));
        queue.finished();
        let next = queue.start_next(true).unwrap();
        assert!(
            matches!(next.target, Target::Create { folder, name, branch, .. }
            if folder == Path::new("C:/Projects") && name == "NewGame" && branch == "5.8")
        );
        assert!(queue.contains_creation(Path::new("c:/projects/newgame")));
        queue.finished();
        assert!(!queue.contains_creation(Path::new("C:/Projects/NewGame")));
        assert!(queue.start_next(true).is_none());
    }
}
