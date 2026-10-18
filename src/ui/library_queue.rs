//! Session-scoped plugin installation queue. Downloaded packages are resolved only at dispatch.
use engine_workspace::{fab::Item, model::Installation};
use std::collections::VecDeque;

#[derive(Clone)]
pub(super) struct Request {
    pub item: Item,
    pub engines: Vec<Installation>,
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
                && job
                    .engines
                    .iter()
                    .any(|e| e.path.eq_ignore_ascii_case(path))
        })
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
                engines: accepted,
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
        assert_eq!(
            first
                .engines
                .iter()
                .map(|e| e.version.as_str())
                .collect::<Vec<_>>(),
            ["5.6.1", "5.8.3"]
        );
        assert!(q.start_next(true).is_none(), "never overlap active jobs");
        q.finished(); // Success, failure and stopped completions all release the active job.
        let second = q.start_next(true).unwrap();
        assert_eq!(second.item.title, "second");
        assert_eq!(second.engines[0].version, "5.7.4");
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
        assert_eq!(q.start_next(true).unwrap().engines[0].version, "5.6.1");
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
}
