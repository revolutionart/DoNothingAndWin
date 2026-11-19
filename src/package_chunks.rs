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

type Reply = mpsc::Receiver<Result<Vec<u8>, String>>;
type Task<'a> = (&'a Chunk, mpsc::Sender<Result<Vec<u8>, String>>);

pub(crate) struct Prefetch<'a> {
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
            .map_err(|_| "Package download worker stopped")?;
        Ok(rx)
    }

    fn schedule(&mut self) -> Result<(), String> {
        if let Some(chunk) = self.remaining.next() {
            let rx = self.request(chunk)?;
            self.pending.insert(&chunk.guid, rx);
        }
        Ok(())
    }

    pub(crate) fn take(&mut self, chunk: &'a Chunk) -> Result<Vec<u8>, String> {
        self.take_with_progress(chunk, &mut || {})
    }

    pub(crate) fn take_with_progress(
        &mut self,
        chunk: &'a Chunk,
        progress: &mut dyn FnMut(),
    ) -> Result<Vec<u8>, String> {
        let (reply, planned) = match self.pending.remove(chunk.guid.as_str()) {
            Some(reply) => (reply, true),
            // A reused chunk may have been evicted from the file writer's memory cache.
            None => (self.request(chunk)?, false),
        };
        let bytes = loop {
            self.control.check()?;
            match reply.recv_timeout(Duration::from_millis(100)) {
                Ok(result) => break result?,
                Err(mpsc::RecvTimeoutError::Timeout) => progress(),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Package download worker stopped".into());
                }
            }
        };
        if planned {
            self.schedule()?;
        }
        Ok(bytes)
    }
}

#[derive(Default, Clone, Copy)]
pub(crate) struct ChunkIo {
    pub downloaded: u64,
    pub read: u64,
    pub written: u64,
}

pub(crate) struct ChunkSource<'a> {
    manifest: &'a Manifest,
    bases: &'a [reqwest::Url],
    control: &'a installer::Control,
    client: reqwest::blocking::Client,
    cache: download_policy::ChunkCache,
    downloaded: AtomicU64,
    read: AtomicU64,
    written: AtomicU64,
}

impl<'a> ChunkSource<'a> {
    pub(crate) fn new(
        manifest: &'a Manifest,
        bases: &'a [reqwest::Url],
        control: &'a installer::Control,
    ) -> Result<Self, String> {
        Ok(Self {
            manifest,
            bases,
            control,
            client: installer::client()?,
            cache: download_policy::ChunkCache::new(),
            downloaded: AtomicU64::new(0),
            read: AtomicU64::new(0),
            written: AtomicU64::new(0),
        })
    }

    pub(crate) fn load(&self, chunk: &Chunk) -> Result<Vec<u8>, String> {
        if let Some(bytes) = self.cache.read(chunk, &mut |n| {
            self.read.fetch_add(n, Ordering::Relaxed);
        }) {
            return Ok(bytes);
        }
        let bytes = installer::chunk_data_progress(
            &self.client,
            chunk,
            self.bases,
            self.manifest.feature,
            self.control,
            &mut |n| {
                self.downloaded.fetch_add(n, Ordering::Relaxed);
            },
        )?;
        if self.cache.store_verified(chunk, &bytes) {
            self.written
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        Ok(bytes)
    }

    pub(crate) fn io(&self) -> ChunkIo {
        ChunkIo {
            downloaded: self.downloaded.load(Ordering::Relaxed),
            read: self.read.load(Ordering::Relaxed),
            written: self.written.load(Ordering::Relaxed),
        }
    }
}

pub(crate) fn ordered_chunks<'a>(
    manifest: &'a Manifest,
    files: impl IntoIterator<Item = &'a crate::buildpatch::File>,
) -> Result<Vec<&'a Chunk>, String> {
    let mut seen = std::collections::HashSet::new();
    files
        .into_iter()
        .flat_map(|file| &file.parts)
        .filter(|part| seen.insert(part.guid.as_str()))
        .map(|part| {
            manifest
                .chunks
                .get(&part.guid)
                .ok_or_else(|| "Missing package chunk".to_owned())
        })
        .collect()
}

pub(crate) fn with_prefetch<'a, T>(
    chunks: Vec<&'a Chunk>,
    window: usize,
    control: &'a installer::Control,
    load: impl Fn(&Chunk) -> Result<Vec<u8>, String> + Sync,
    work: impl FnOnce(&mut Prefetch<'a>) -> Result<T, String>,
) -> Result<T, String> {
    // Keep requests and decoded data bounded regardless of the total package size.
    let workers = window.min(chunks.len());
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
    use std::sync::Condvar;

    #[test]
    fn prefetch_fills_its_window_before_waiting_for_the_first_chunk() {
        let chunks: Vec<_> = (0..10)
            .map(|n| Chunk {
                guid: n.to_string(),
                ..Default::default()
            })
            .collect();
        let (started, requests) = mpsc::channel();
        let gate = (Mutex::new(false), Condvar::new());
        let window = 8;
        with_prefetch(
            chunks.iter().collect(),
            window,
            &installer::Control::default(),
            |chunk| {
                started.send(chunk.guid.clone()).unwrap();
                let (open, _) = gate
                    .1
                    .wait_timeout_while(gate.0.lock().unwrap(), Duration::from_secs(5), |open| {
                        !*open
                    })
                    .unwrap();
                assert!(*open);
                Ok(chunk.guid.as_bytes().to_vec())
            },
            |prefetch| {
                let started: std::collections::HashSet<_> = (0..window)
                    .filter_map(|_| requests.recv_timeout(Duration::from_millis(500)).ok())
                    .collect();
                assert_eq!(started.len(), window);
                assert!(requests.try_recv().is_err());
                *gate.0.lock().unwrap() = true;
                gate.1.notify_all();
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
    fn stopping_prefetch_releases_the_waiting_writer() {
        let chunk = Chunk {
            guid: "waiting".into(),
            ..Default::default()
        };
        let control = installer::Control::default();
        let (started, ready) = mpsc::channel();
        let result = with_prefetch(
            vec![&chunk],
            4,
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
