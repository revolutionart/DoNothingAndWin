//! Engine operation state shared by dialogs, Downloads and update protection.
use crate::{
    installer::{Control, Plan, Progress},
    model::Installation,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
};

pub enum Event {
    Prepared(Result<Plan, String>),
    Maintained(Installation, bool, Result<String, String>),
    Finished(Result<PathBuf, String>),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Install,
    Verify,
    Remove,
    Components,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Preparing,
    Ready,
    Running,
    Completed,
    Failed,
}
pub struct Worker {
    pub events: Sender<Event>,
    pub control: Control,
    pub progress: Arc<Mutex<Progress>>,
}
#[derive(Default)]
pub struct Operation {
    phase: Phase,
    kind: Kind,
    receiver: Option<Receiver<Event>>,
    parent: PathBuf,
    plan: Option<Plan>,
    installed: Option<PathBuf>,
    engine: Option<Installation>,
    control: Control,
    progress: Arc<Mutex<Progress>>,
}
impl Operation {
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Preparing | Phase::Running)
    }
    pub fn preparing(&self) -> bool {
        self.phase == Phase::Preparing
    }
    pub fn transferring(&self) -> bool {
        self.phase == Phase::Running
    }
    pub fn kind(&self) -> Kind {
        self.kind
    }
    pub fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }
    pub fn plan_mut(&mut self) -> Option<&mut Plan> {
        if self.busy() {
            None
        } else {
            self.plan.as_mut()
        }
    }
    pub fn installed(&self) -> Option<&PathBuf> {
        self.installed.as_ref()
    }
    pub fn progress(&self) -> Progress {
        self.progress.lock().unwrap().clone()
    }
    pub fn can_stop(&self) -> bool {
        self.transferring() && self.kind != Kind::Remove
    }
    pub fn stop(&self) {
        if self.can_stop() {
            self.control.stop();
        }
    }
    pub fn can_resume(&self) -> bool {
        !self.busy()
            && self.kind == Kind::Install
            && self.plan.is_some()
            && self.installed.is_none()
    }
    pub fn dismiss(&mut self) -> bool {
        if self.busy() || self.kind != Kind::Install || self.installed.is_some() {
            return false;
        }
        *self = Self::default();
        true
    }
    pub fn prepare(&mut self, parent: PathBuf) -> Option<Worker> {
        if self.busy() {
            return None;
        }
        *self = Self {
            parent,
            phase: Phase::Preparing,
            ..Self::default()
        };
        Some(self.worker())
    }
    pub fn install(&mut self) -> Option<Worker> {
        if !self.can_resume() {
            return None;
        }
        self.phase = Phase::Running;
        Some(self.worker())
    }
    pub fn maintain(&mut self, kind: Kind, engine: Installation) -> Option<Worker> {
        if self.busy() || kind == Kind::Install {
            return None;
        }
        *self = Self {
            kind,
            phase: Phase::Running,
            engine: Some(engine),
            ..Self::default()
        };
        Some(self.worker())
    }
    fn worker(&mut self) -> Worker {
        let (events, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.control = Control::default();
        self.progress = Arc::default();
        Worker {
            events,
            control: self.control.clone(),
            progress: self.progress.clone(),
        }
    }
    pub fn poll(&mut self) -> Option<Event> {
        let event = match self.receiver.as_ref()?.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                let error = "Engine operation interrupted. You can retry manually.".to_string();
                if self.preparing() {
                    Event::Prepared(Err(error))
                } else if let Some(engine) = &self.engine {
                    Event::Maintained(engine.clone(), self.kind == Kind::Remove, Err(error))
                } else {
                    Event::Finished(Err(error))
                }
            }
        };
        self.receiver = None;
        match &event {
            Event::Prepared(Ok(plan)) => {
                let mut plan = plan.clone();
                plan.target = self.parent.join(&plan.manifest.app);
                self.plan = Some(plan);
                self.phase = Phase::Ready;
            }
            Event::Finished(Ok(path)) => {
                self.installed = Some(path.clone());
                self.phase = Phase::Completed;
            }
            Event::Maintained(_, _, Ok(_)) => self.phase = Phase::Completed,
            _ => self.phase = Phase::Failed,
        }
        Some(event)
    }
    // Explicit render-only fixture; never schedules a worker or touches disk.
    pub fn preview(&mut self, progress: Progress) {
        *self = Self {
            phase: Phase::Running,
            progress: Arc::new(Mutex::new(progress)),
            ..Self::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prepared(op: &mut Operation) {
        let worker = op.prepare("destination".into()).unwrap();
        worker
            .events
            .send(Event::Prepared(Ok(Plan {
                manifest: crate::buildpatch::Manifest {
                    app: "UE_5.4".into(),
                    ..Default::default()
                },
                bases: vec![],
                target: "ignored".into(),
                tags: vec![],
                selected: vec![],
            })))
            .ok()
            .unwrap();
        assert!(op.poll().is_some());
    }
    #[test]
    fn stop_keeps_resume_plan_completion_prevents_dismissal() {
        let mut op = Operation::default();
        prepared(&mut op);
        assert_eq!(
            op.plan().unwrap().target,
            PathBuf::from("destination").join("UE_5.4")
        );
        let worker = op.install().unwrap();
        assert!(op.can_stop());
        op.stop();
        assert!(
            worker
                .control
                .cancel
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        assert!(!op.dismiss());
        worker
            .events
            .send(Event::Finished(Err("Stopped".into())))
            .ok()
            .unwrap();
        op.poll();
        assert!(op.can_resume());
        let worker = op.install().unwrap();
        worker
            .events
            .send(Event::Finished(Ok("destination/UE_5.4".into())))
            .ok()
            .unwrap();
        op.poll();
        assert!(!op.can_resume());
        assert!(!op.dismiss());
        assert!(op.installed().is_some());
    }
    #[test]
    fn disconnected_workers_do_not_stay_busy_and_preparation_can_retry() {
        let mut op = Operation::default();
        drop(op.prepare("destination".into()).unwrap());
        assert!(matches!(op.poll(), Some(Event::Prepared(Err(_)))));
        assert!(!op.busy());
        assert!(!op.can_resume());
        prepared(&mut op);
        drop(op.install().unwrap());
        assert!(matches!(op.poll(), Some(Event::Finished(Err(_)))));
        assert!(op.can_resume());
        assert!(op.dismiss());
        assert!(op.plan().is_none());
    }
    #[test]
    fn maintenance_has_its_own_cancellation_and_completion_rules() {
        for kind in [Kind::Remove, Kind::Verify, Kind::Components] {
            let mut op = Operation::default();
            let worker = op
                .maintain(
                    kind,
                    Installation {
                        path: "engine".into(),
                        version: "5.4.4".into(),
                        association: "5.4".into(),
                    },
                )
                .unwrap();
            assert_eq!(op.can_stop(), kind != Kind::Remove);
            assert!(!op.dismiss());
            drop(worker);
            assert!(matches!(op.poll(), Some(Event::Maintained(_, _, Err(_)))));
            assert!(!op.busy());
            assert!(!op.can_resume());
            assert!(!op.dismiss());
        }
    }
}
