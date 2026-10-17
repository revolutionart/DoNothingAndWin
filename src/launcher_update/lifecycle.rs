//! Owns download, waiting and handoff ordering independently of window rendering.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Downloading,
    Waiting,
    Preparing,
    Ready,
}

pub struct Lifecycle<T: Send + 'static> {
    phase: Phase,
    download: Option<Receiver<Result<T, String>>>,
    staged: Option<T>,
    handoff: Option<Receiver<Result<(), String>>>,
    cancel: Arc<AtomicBool>,
    received: Arc<AtomicU64>,
    error: Option<String>,
}
impl<T: Send + 'static> Default for Lifecycle<T> {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            download: None,
            staged: None,
            handoff: None,
            cancel: Arc::default(),
            received: Arc::default(),
            error: None,
        }
    }
}
impl<T: Send + 'static> Drop for Lifecycle<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl<T: Send + 'static> Lifecycle<T> {
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn active(&self) -> bool {
        self.phase != Phase::Idle
    }
    pub fn blocks_work(&self) -> bool {
        matches!(self.phase, Phase::Preparing | Phase::Ready)
    }
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
    pub fn start(
        &mut self,
        job: impl FnOnce(Arc<AtomicBool>, Arc<AtomicU64>) -> Result<T, String> + Send + 'static,
    ) {
        if self.active() {
            return;
        }
        self.error = None;
        self.cancel = Arc::default();
        self.received = Arc::default();
        let cancel = self.cancel.clone();
        let received = self.received.clone();
        let (tx, rx) = mpsc::channel();
        self.download = Some(rx);
        self.phase = Phase::Downloading;
        std::thread::spawn(move || {
            let result = job(cancel.clone(), received);
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(result);
            }
        });
    }
    pub fn cancel(&mut self) {
        if self.blocks_work() {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.download = None;
        self.staged = None;
        self.phase = Phase::Idle;
    }
    pub fn poll(
        &mut self,
        busy: bool,
        prepare: impl FnOnce(T, Arc<AtomicBool>) -> Result<(), String> + Send + 'static,
    ) {
        if let Some(rx) = &self.download {
            match rx.try_recv() {
                Ok(result) => {
                    self.download = None;
                    match result {
                        Ok(staged) => {
                            self.staged = Some(staged);
                            self.phase = Phase::Waiting;
                        }
                        Err(error) => self.fail(error),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.download = None;
                    self.fail("Launcher download interrupted. Please retry.".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.phase == Phase::Waiting && !busy {
            let staged = self
                .staged
                .take()
                .expect("waiting owns the staged installer");
            let cancel = self.cancel.clone();
            let (tx, rx) = mpsc::channel();
            self.handoff = Some(rx);
            self.phase = Phase::Preparing;
            std::thread::spawn(move || {
                let _ = tx.send(prepare(staged, cancel));
            });
        }
        if let Some(rx) = &self.handoff {
            match rx.try_recv() {
                Ok(result) => {
                    self.handoff = None;
                    match result {
                        Ok(()) => self.phase = Phase::Ready,
                        Err(e) => self.fail(e),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.handoff = None;
                    self.fail("Update preparation interrupted. The launcher is still open.".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }
    fn fail(&mut self, error: String) {
        self.phase = Phase::Idle;
        self.error = Some(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn until(mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !f() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn waits_for_work_then_prepares_off_the_callers_thread() {
        let mut flow = Lifecycle::default();
        let ui_thread = std::thread::current().id();
        flow.start(|_, _| Ok(7));
        until(|| {
            flow.poll(true, |_, _| panic!("active work must finish"));
            flow.phase() == Phase::Waiting
        });
        flow.poll(false, move |value, _| {
            assert_eq!(value, 7);
            assert_ne!(std::thread::current().id(), ui_thread);
            Ok(())
        });
        assert!(flow.blocks_work());
        until(|| {
            flow.poll(false, |_, _| unreachable!());
            flow.phase() == Phase::Ready
        });
    }
    #[test]
    fn cancellation_discards_late_download_and_failure_allows_manual_retry() {
        let mut flow = Lifecycle::default();
        let (tx, rx) = mpsc::channel();
        flow.start(move |_, _| {
            rx.recv().unwrap();
            Ok(1)
        });
        flow.cancel();
        tx.send(()).unwrap();
        assert_eq!(flow.phase(), Phase::Idle);
        flow.start(|_, _| Ok(2));
        until(|| {
            flow.poll(false, |_, _| Err("setup unavailable".into()));
            flow.take_error().is_some()
        });
        assert!(!flow.active());
        flow.poll(false, |_, _| panic!("must not retry automatically"));
    }
    #[test]
    fn disconnected_worker_releases_busy_state() {
        let mut flow = Lifecycle::<()>::default();
        flow.start(|_, _| panic!("simulated worker failure"));
        until(|| {
            flow.poll(false, |_, _| Ok(()));
            flow.take_error().is_some()
        });
        assert!(!flow.blocks_work());
    }
}
