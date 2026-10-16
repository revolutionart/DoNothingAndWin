use std::sync::{Condvar, Mutex, mpsc};

// Shared across dialog and library scans, so simultaneous checks cannot each
// start a separate burst of requests. Installation workers are unaffected.
const LIMIT: usize = 4;
static ACTIVE: Mutex<usize> = Mutex::new(0);
static SLOT: Condvar = Condvar::new();
struct Permit;
impl Permit {
    fn acquire() -> Self {
        let mut active = ACTIVE.lock().unwrap();
        while *active >= LIMIT {
            active = SLOT.wait(active).unwrap();
        }
        *active += 1;
        Self
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        *ACTIVE.lock().unwrap() -= 1;
        SLOT.notify_one();
    }
}

pub(super) fn run<T: Send, R: Send>(
    jobs: Vec<T>,
    work: impl Fn(T) -> R + Sync,
    mut ready: impl FnMut(&R),
) -> Vec<R> {
    let workers = jobs.len().min(LIMIT);
    let queue = Mutex::new(jobs.into_iter().enumerate());
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        for _ in 0..workers {
            let (tx, queue, work) = (tx.clone(), &queue, &work);
            scope.spawn(move || {
                loop {
                    let Some((index, job)) = queue.lock().unwrap().next() else {
                        break;
                    };
                    let result = {
                        let _permit = Permit::acquire();
                        work(job)
                    };
                    if tx.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut results = Vec::new();
        for (index, result) in rx {
            ready(&result);
            results.push((index, result));
        }
        results.sort_by_key(|(index, _)| *index);
        results.into_iter().map(|(_, result)| result).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn streams_fast_results_before_slow_jobs_and_preserves_final_order() {
        let release = (Mutex::new(false), Condvar::new());
        let mut observed = Vec::new();
        let result = run(
            vec![0, 1, 2],
            |job| {
                if job == 0 {
                    let mut allowed = release.0.lock().unwrap();
                    while !*allowed {
                        allowed = release.1.wait(allowed).unwrap();
                    }
                }
                job
            },
            |result| {
                observed.push(*result);
                if *result != 0 {
                    *release.0.lock().unwrap() = true;
                    release.1.notify_all();
                }
            },
        );
        assert_ne!(observed[0], 0);
        assert_eq!(result, vec![0, 1, 2]);
    }
    #[test]
    fn concurrent_scans_share_the_worker_limit_and_process_every_job_once() {
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let work = |job| {
            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(count, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(5));
            active.fetch_sub(1, Ordering::SeqCst);
            job
        };
        std::thread::scope(|s| {
            let a = s.spawn(|| run((0..12).collect(), &work, |_| {}));
            let b = s.spawn(|| run((12..24).collect(), &work, |_| {}));
            assert_eq!(a.join().unwrap(), (0..12).collect::<Vec<_>>());
            assert_eq!(b.join().unwrap(), (12..24).collect::<Vec<_>>());
        });
        assert!(peak.load(Ordering::SeqCst) <= LIMIT);
        assert!(peak.load(Ordering::SeqCst) > 1);
    }
}
