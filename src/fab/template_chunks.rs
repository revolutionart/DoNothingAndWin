use crate::{
    buildpatch::{Chunk, Manifest},
    download_policy, installer,
};
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

// Keep requests and decoded data bounded, even when a template has thousands of chunks.
const WINDOW: usize = 4;
type Reply = mpsc::Receiver<Result<Vec<u8>, String>>;
type Task<'a> = (&'a Chunk, mpsc::Sender<Result<Vec<u8>, String>>);

pub(super) struct Prefetch<'a> {
    remaining: std::vec::IntoIter<&'a Chunk>,
    pending: HashMap<&'a str, Reply>,
    tasks: mpsc::Sender<Task<'a>>,
    control: &'a installer::Control,
}

impl<'a> Prefetch<'a> {
    fn request(&self, chunk: &'a Chunk) -> Result<Reply, String> {
        self.control.check()?;
        let (tx, rx) = mpsc::channel();
        self.tasks
            .send((chunk, tx))
            .map_err(|_| "Project download worker stopped")?;
        Ok(rx)
    }

    fn schedule(&mut self) -> Result<(), String> {
        if let Some(chunk) = self.remaining.next() {
            let rx = self.request(chunk)?;
            self.pending.insert(&chunk.guid, rx);
        }
        Ok(())
    }

    pub(super) fn take(&mut self, chunk: &'a Chunk) -> Result<Vec<u8>, String> {
        let (reply, planned) = match self.pending.remove(chunk.guid.as_str()) {
            Some(reply) => (reply, true),
            // A reused chunk may have been evicted from the file writer's memory cache.
            None => (self.request(chunk)?, false),
        };
        let bytes = loop {
            self.control.check()?;
            match reply.recv_timeout(Duration::from_millis(100)) {
                Ok(result) => break result?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Project download worker stopped".into());
                }
            }
        };
        if planned {
            self.schedule()?;
        }
        Ok(bytes)
    }
}

pub(super) fn with_project_chunks<'a, T>(
    manifest: &'a Manifest,
    bases: &[reqwest::Url],
    chunks: Vec<&'a Chunk>,
    control: &'a installer::Control,
    work: impl FnOnce(&mut Prefetch<'a>, &AtomicU64) -> Result<T, String>,
) -> Result<T, String> {
    let client = installer::client()?;
    let downloaded = AtomicU64::new(0);
    let root = download_policy::active_cache_root();
    with_prefetch(
        chunks,
        control,
        |chunk| {
            download_policy::with_cache_root(Some(root.clone()), || {
                if let Some(bytes) = download_policy::cached_chunk(chunk) {
                    return Ok(bytes);
                }
                let bytes = installer::chunk_data_progress(
                    &client,
                    chunk,
                    bases,
                    manifest.feature,
                    control,
                    &mut |n| {
                        downloaded.fetch_add(n, Ordering::Relaxed);
                    },
                )?;
                download_policy::cache_chunk(chunk, &bytes);
                Ok(bytes)
            })
        },
        |prefetch| work(prefetch, &downloaded),
    )
}

fn with_prefetch<'a, T>(
    chunks: Vec<&'a Chunk>,
    control: &'a installer::Control,
    load: impl Fn(&Chunk) -> Result<Vec<u8>, String> + Sync,
    work: impl FnOnce(&mut Prefetch<'a>) -> Result<T, String>,
) -> Result<T, String> {
    let workers = WINDOW.min(chunks.len());
    let (tx, rx) = mpsc::channel::<Task<'a>>();
    let tasks = Mutex::new(rx);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tasks = &tasks;
            let load = &load;
            scope.spawn(move || {
                loop {
                    let task = tasks.lock().unwrap().recv();
                    let Ok((chunk, reply)) = task else { break };
                    if control.check().is_err() {
                        break;
                    }
                    let _ = reply.send(load(chunk));
                }
            });
        }
        let mut prefetch = Prefetch {
            remaining: chunks.into_iter(),
            pending: HashMap::new(),
            tasks: tx,
            control,
        };
        for _ in 0..workers {
            prefetch.schedule()?;
        }
        work(&mut prefetch)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_prefetch_starts_multiple_requests_without_waiting_for_the_first() {
        let chunks: Vec<_> = (0..6)
            .map(|n| Chunk {
                guid: n.to_string(),
                ..Default::default()
            })
            .collect();
        let (started, requests) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let gate = Mutex::new(gate);
        with_prefetch(
            chunks.iter().collect(),
            &installer::Control::default(),
            |chunk| {
                started.send(chunk.guid.clone()).unwrap();
                if chunk.guid == "0" {
                    gate.lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                }
                Ok(chunk.guid.as_bytes().to_vec())
            },
            |prefetch| {
                let first = requests.recv_timeout(Duration::from_secs(1)).unwrap();
                let second = requests.recv_timeout(Duration::from_millis(350));
                release.send(()).unwrap();
                assert_ne!(
                    first,
                    second.expect("second request should start before the first completes")
                );
                for chunk in &chunks {
                    assert_eq!(prefetch.take(chunk)?, chunk.guid.as_bytes());
                }
                assert_eq!(prefetch.take(&chunks[1])?, b"1");
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn stopping_project_prefetch_releases_the_waiting_writer() {
        let chunk = Chunk {
            guid: "waiting".into(),
            ..Default::default()
        };
        let control = installer::Control::default();
        let (started, ready) = mpsc::channel();
        let result = with_prefetch(
            vec![&chunk],
            &control,
            |_| {
                started.send(()).unwrap();
                loop {
                    control.check()?;
                    std::thread::sleep(Duration::from_millis(1));
                }
            },
            |prefetch| {
                ready.recv_timeout(Duration::from_secs(1)).unwrap();
                control.stop();
                prefetch.take(&chunk)
            },
        );
        assert!(result.unwrap_err().starts_with("Stopped."));
    }
}
