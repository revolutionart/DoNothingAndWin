use crate::{
    buildpatch::{Chunk, File, Manifest},
    download_policy, installer,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// How many chunk downloads run at once.
///
/// [`Connections::ADAPTIVE`] follows Epic's BuildPatchServices
/// (`FDownloadConnectionCountConfig`: at least 8, at most 100, scaled on measured
/// speed and download health), starting from Epic's fixed fallback of 16.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Connections {
    pub start: usize,
    pub min: usize,
    pub max: usize,
}

impl Connections {
    pub(crate) const ADAPTIVE: Self = Self {
        start: 16,
        min: 8,
        max: 100,
    };
    #[cfg(test)]
    pub(crate) const fn fixed(count: usize) -> Self {
        Self {
            start: count,
            min: count,
            max: count,
        }
    }
}

const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const SPEED_WINDOW: Duration = Duration::from_secs(1);
/// Grow while speed stays above this share of the best speed seen.
const HIGH_BANDWIDTH: f64 = 0.85;
/// Shrink once speed stays below this share of the best speed seen.
const LOW_BANDWIDTH: f64 = 0.65;
const POSITIVE_HYSTERESIS: u32 = 2;
const NEGATIVE_HYSTERESIS: u32 = 4;
const HEALTH_HYSTERESIS: u32 = 4;
const REPORT_INTERVAL: Duration = Duration::from_millis(150);

/// Chooses the number of simultaneous downloads from measured throughput.
struct Scaler {
    limits: Connections,
    target: usize,
    best: f64,
    up: u32,
    down: u32,
    unhealthy: u32,
    last: Option<(Instant, ChunkIo)>,
    speeds: VecDeque<(Instant, u64)>,
}

impl Scaler {
    fn new(limits: Connections) -> Self {
        Self {
            limits,
            target: limits.start.clamp(limits.min, limits.max),
            best: 0.,
            up: 0,
            down: 0,
            unhealthy: 0,
            last: None,
            speeds: VecDeque::new(),
        }
    }

    fn observe(&mut self, now: Instant, io: ChunkIo, in_flight: usize) -> usize {
        if self.limits.min == self.limits.max {
            return self.target;
        }
        let Some((at, previous)) = self.last else {
            self.last = Some((now, io));
            self.speeds.push_back((now, io.downloaded));
            return self.target;
        };
        if now.duration_since(at) < SAMPLE_INTERVAL {
            return self.target;
        }
        self.last = Some((now, io));
        self.speeds.push_back((now, io.downloaded));
        while self.speeds.len() > 2 && now.duration_since(self.speeds[1].0) >= SPEED_WINDOW {
            self.speeds.pop_front();
        }

        let failures = io.failures.saturating_sub(previous.failures);
        let completed = io.requests.saturating_sub(previous.requests);
        if failures > 0 && failures * 10 > failures + completed {
            self.unhealthy += 1;
        } else {
            self.unhealthy = 0;
        }
        if self.unhealthy >= HEALTH_HYSTERESIS {
            self.shrink();
            return self.target;
        }

        // Speed says something about the network only while most of the window
        // is downloading; otherwise file writing or the local cache sets the pace.
        if io.downloaded == previous.downloaded || in_flight * 4 < self.target * 3 {
            self.up = 0;
            self.down = 0;
            return self.target;
        }
        let (start, bytes) = self.speeds[0];
        let seconds = now.duration_since(start).as_secs_f64();
        if seconds <= 0. {
            return self.target;
        }
        let speed = io.downloaded.saturating_sub(bytes) as f64 / seconds;
        self.best = self.best.max(speed);
        if speed >= self.best * HIGH_BANDWIDTH {
            self.down = 0;
            self.up += 1;
            if self.up >= POSITIVE_HYSTERESIS {
                self.target = (self.target + (self.target / 2).max(4)).min(self.limits.max);
                self.up = 0;
            }
        } else if speed < self.best * LOW_BANDWIDTH {
            self.up = 0;
            self.down += 1;
            if self.down >= NEGATIVE_HYSTERESIS {
                self.shrink();
                // Let a lasting slowdown become the new reference instead of
                // shrinking all the way to the minimum.
                self.best = speed.max(self.best * 0.8);
            }
        } else {
            self.up = 0;
            self.down = 0;
        }
        self.target
    }

    fn shrink(&mut self) {
        self.target = (self.target * 3 / 4).max(self.limits.min);
        self.up = 0;
        self.down = 0;
        self.unhealthy = 0;
    }
}

type Reply = mpsc::Receiver<Result<Vec<u8>, String>>;
type Task<'a> = (&'a Chunk, mpsc::Sender<Result<Vec<u8>, String>>);

pub(crate) struct Prefetch<'a> {
    remaining: std::vec::IntoIter<&'a Chunk>,
    pending: HashMap<&'a str, Reply>,
    /// Chunks requested before their turn; scheduling skips them later.
    requested: HashSet<&'a str>,
    tasks: mpsc::Sender<Task<'a>>,
    control: &'a installer::Control,
    scaler: Scaler,
    in_flight: Arc<AtomicUsize>,
    meter: Box<dyn Fn() -> ChunkIo + Send + 'a>,
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

    /// Rescale, then keep the chosen number of chunks requested ahead of the writers.
    pub(crate) fn fill(&mut self) -> Result<(), String> {
        let target = self.scaler.observe(
            Instant::now(),
            (self.meter)(),
            self.in_flight.load(Ordering::Relaxed),
        );
        while self.pending.len() < target {
            let Some(chunk) = self.remaining.next() else {
                break;
            };
            if self.requested.contains(chunk.guid.as_str()) {
                continue;
            }
            let rx = self.request(chunk)?;
            self.pending.insert(&chunk.guid, rx);
        }
        Ok(())
    }

    fn claim(&mut self, chunk: &'a Chunk) -> Result<Reply, String> {
        let reply = match self.pending.remove(chunk.guid.as_str()) {
            Some(reply) => reply,
            // Needed before its turn by a parallel writer, or evicted from a
            // writer's memory cache (then it is read back from the disk cache).
            None => {
                self.requested.insert(&chunk.guid);
                self.request(chunk)?
            }
        };
        self.fill()?;
        Ok(reply)
    }

    #[cfg(test)]
    pub(crate) fn take(&mut self, chunk: &'a Chunk) -> Result<Vec<u8>, String> {
        self.take_with_progress(chunk, &mut || {})
    }

    pub(crate) fn take_with_progress(
        &mut self,
        chunk: &'a Chunk,
        progress: &mut dyn FnMut(),
    ) -> Result<Vec<u8>, String> {
        let reply = self.claim(chunk)?;
        let control = self.control;
        receive(&reply, control, &mut || {
            progress();
            let _ = self.fill();
        })
    }
}

fn receive(
    reply: &Reply,
    control: &installer::Control,
    waiting: &mut dyn FnMut(),
) -> Result<Vec<u8>, String> {
    loop {
        control.check()?;
        match reply.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => waiting(),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Package download worker stopped".into());
            }
        }
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct ChunkIo {
    pub downloaded: u64,
    pub read: u64,
    pub written: u64,
    /// Chunks fetched from the network.
    pub requests: u64,
    /// Failed network attempts, including ones a retry recovered from.
    pub failures: u64,
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
    requests: AtomicU64,
    failures: AtomicU64,
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
            requests: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        })
    }

    pub(crate) fn load(&self, chunk: &Chunk) -> Result<Vec<u8>, String> {
        if let Some(bytes) = self.cache.read(chunk, &mut |n| {
            self.read.fetch_add(n, Ordering::Relaxed);
        }) {
            return Ok(bytes);
        }
        let bytes = installer::chunk_data_observed(
            &self.client,
            chunk,
            self.bases,
            self.manifest.feature,
            self.control,
            &mut |n| {
                self.downloaded.fetch_add(n, Ordering::Relaxed);
            },
            &mut || {
                self.failures.fetch_add(1, Ordering::Relaxed);
            },
        )?;
        self.requests.fetch_add(1, Ordering::Relaxed);
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
            requests: self.requests.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
        }
    }
}

pub(crate) fn ordered_chunks<'a>(
    manifest: &'a Manifest,
    files: impl IntoIterator<Item = &'a File>,
) -> Result<Vec<&'a Chunk>, String> {
    let mut seen = HashSet::new();
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

/// Delete the cached chunks of `files` once they are installed. The files on
/// disk are the result; keeping the unpacked chunks as well doubles the space
/// a package takes. Epic's launcher also discards its chunks after an install.
pub(crate) fn forget_chunks<'a>(manifest: &'a Manifest, files: impl IntoIterator<Item = &'a File>) {
    if let Ok(chunks) = ordered_chunks(manifest, files) {
        download_policy::ChunkCache::new().remove(chunks);
    }
}

/// Copy the files of a saved download into place instead of fetching them again.
///
/// Each file is hashed while it is copied and kept only if it matches the
/// package. Returns, per file, whether its copy was kept; the caller downloads
/// the rest. `report` gets the bytes and files copied so far.
pub(crate) fn copy_saved_files(
    files: &[(&File, PathBuf, PathBuf)],
    control: &installer::Control,
    mut report: impl FnMut(u64, usize),
) -> Result<Vec<bool>, String> {
    use sha1::{Digest, Sha1};
    use std::io::{Read, Write};

    fn copy(
        file: &File,
        from: &Path,
        to: &Path,
        control: &installer::Control,
        bytes: &AtomicU64,
    ) -> Result<bool, String> {
        if installer::no_links(from).is_err()
            || !std::fs::metadata(from).is_ok_and(|m| m.is_file() && m.len() == file.size())
        {
            return Ok(false);
        }
        let Ok(mut input) = std::fs::File::open(from) else {
            return Ok(false);
        };
        installer::no_links(to)?;
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .map_err(|e| format!("Creating {}: {e}", to.display()))?;
        let mut hash = Sha1::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut copied = 0u64;
        let result = (|| loop {
            control.check()?;
            // A source that cannot be read is skipped, not an install failure.
            let Ok(n) = input.read(&mut buffer) else {
                return Ok(false);
            };
            if n == 0 {
                let matches = <[u8; 20]>::from(hash.finalize()) == file.sha;
                if matches {
                    output.sync_all().map_err(|e| e.to_string())?;
                }
                return Ok(matches);
            }
            hash.update(&buffer[..n]);
            output
                .write_all(&buffer[..n])
                .map_err(|e| format!("Writing {}: {e}", to.display()))?;
            copied += n as u64;
            bytes.fetch_add(n as u64, Ordering::Relaxed);
        })();
        drop(output);
        if !matches!(result, Ok(true)) {
            let _ = std::fs::remove_file(to);
            // Bytes of a rejected copy are downloaded instead; do not count them.
            bytes.fetch_sub(copied, Ordering::Relaxed);
        }
        result
    }

    let kept: Vec<_> = files.iter().map(|_| AtomicUsize::new(0)).collect();
    let (next, done, bytes) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicU64::new(0));
    let failure = Mutex::new(None);
    let child = control.child();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..writer_count(files.len()))
            .map(|_| {
                scope.spawn(|| {
                    while child.check().is_ok() {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some((file, from, to)) = files.get(index) else {
                            break;
                        };
                        match copy(file, from, to, &child, &bytes) {
                            Ok(true) => {
                                kept[index].store(1, Ordering::Relaxed);
                                done.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(false) => {}
                            Err(error) => {
                                failure.lock().unwrap().get_or_insert(error);
                                child.stop();
                                break;
                            }
                        }
                    }
                })
            })
            .collect();
        while !workers.iter().all(|worker| worker.is_finished()) {
            std::thread::sleep(Duration::from_millis(100));
            report(bytes.load(Ordering::Relaxed), done.load(Ordering::Relaxed));
        }
    });
    control.check()?;
    if let Some(error) = failure.into_inner().unwrap() {
        return Err(error);
    }
    report(bytes.load(Ordering::Relaxed), done.load(Ordering::Relaxed));
    Ok(kept
        .iter()
        .map(|kept| kept.load(Ordering::Relaxed) == 1)
        .collect())
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WriteProgress {
    /// Index of the most recently started file.
    pub file: usize,
    pub files_done: usize,
    pub prepared: u64,
    pub io: ChunkIo,
}

impl WriteProgress {
    /// 1-based file number for "Preparing file N / total".
    pub(crate) fn file_number(&self, total: usize) -> usize {
        (self.files_done + 1).min(total.max(1))
    }
}

enum WriterEvent {
    Started(usize),
    Wrote(usize, u64),
    Done(usize),
    Failed(String),
}

fn writer_count(files: usize) -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(2, 8)
        .min(files)
        .max(1)
}

/// Write `files` with several writers while chunks download ahead of them, with
/// the number of simultaneous downloads scaled to the connection.
/// `report` runs on the calling thread at each file start and end, and at most
/// every 100–150 ms in between, including while waiting on the network.
/// Chunks already in `cache` are used as-is and not requested.
pub(crate) fn write_files<'a>(
    manifest: &'a Manifest,
    bases: &'a [reqwest::Url],
    files: &[(&'a File, PathBuf)],
    cache: &mut HashMap<String, Vec<u8>>,
    transfer: &installer::Control,
    mut report: impl FnMut(WriteProgress),
) -> Result<(), String> {
    let mut chunks = ordered_chunks(manifest, files.iter().map(|(file, _)| *file))?;
    chunks.retain(|chunk| !cache.contains_key(&chunk.guid));
    // Writers and downloads stop with this child, so a failed file does not
    // leave work running.
    let control = transfer.child();
    let source = ChunkSource::new(manifest, bases, &control)?;
    let seed = &*cache;
    let writers = writer_count(files.len());
    let result = with_prefetch(
        chunks,
        Connections::ADAPTIVE,
        &control,
        |chunk| source.load(chunk),
        || source.io(),
        |prefetch| {
            let prefetch = Mutex::new(prefetch);
            let next = AtomicUsize::new(0);
            let (events, received) = mpsc::channel();
            std::thread::scope(|scope| {
                for _ in 0..writers {
                    let events = events.clone();
                    let (prefetch, next, control) = (&prefetch, &next, &control);
                    scope.spawn(move || {
                        let mut local = seed.clone();
                        while control.check().is_ok() {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some((file, path)) = files.get(index) else {
                                break;
                            };
                            let _ = events.send(WriterEvent::Started(index));
                            let result = write_file(
                                manifest,
                                bases,
                                file,
                                path,
                                &mut local,
                                control,
                                prefetch,
                                &mut |written| {
                                    let _ = events.send(WriterEvent::Wrote(index, written));
                                },
                            );
                            match result {
                                Ok(()) => {
                                    let _ = events.send(WriterEvent::Done(index));
                                }
                                Err(error) => {
                                    // Send first: stop-induced errors from other
                                    // writers then arrive after the real cause.
                                    let _ = events.send(WriterEvent::Failed(error));
                                    control.stop();
                                    break;
                                }
                            }
                        }
                    });
                }
                drop(events);

                let mut in_progress = HashMap::new();
                let mut state = WriteProgress::default();
                let mut done_bytes = 0u64;
                let mut failure = None;
                loop {
                    match received.recv_timeout(Duration::from_millis(100)) {
                        Ok(WriterEvent::Started(i)) => {
                            state.file = state.file.max(i);
                            in_progress.insert(i, 0);
                        }
                        Ok(WriterEvent::Wrote(i, n)) => {
                            in_progress.insert(i, n);
                        }
                        Ok(WriterEvent::Done(i)) => {
                            in_progress.remove(&i);
                            done_bytes += files[i].0.size();
                            state.files_done += 1;
                        }
                        Ok(WriterEvent::Failed(error)) => {
                            failure.get_or_insert(error);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            // Keep scaling while every writer waits on the network.
                            let _ = prefetch.lock().unwrap().fill();
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    if failure.is_none() {
                        state.prepared = done_bytes + in_progress.values().sum::<u64>();
                        state.io = source.io();
                        report(state);
                    }
                }
                match failure {
                    Some(error) => Err(error),
                    None => Ok(()),
                }
            })
        },
    );
    // Report the user's stop rather than the child's.
    transfer.check()?;
    result
}

#[allow(clippy::too_many_arguments)]
fn write_file<'p>(
    manifest: &'p Manifest,
    bases: &[reqwest::Url],
    file: &File,
    path: &Path,
    cache: &mut HashMap<String, Vec<u8>>,
    control: &installer::Control,
    prefetch: &Mutex<&mut Prefetch<'p>>,
    wrote: &mut dyn FnMut(u64),
) -> Result<(), String> {
    installer::no_links(path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut last = Instant::now();
    crate::plugins::payload_to_file_with_prefetch(
        manifest,
        bases,
        file,
        cache,
        path,
        control,
        &mut |written, _| {
            if last.elapsed() >= REPORT_INTERVAL {
                wrote(written);
                last = Instant::now();
            }
        },
        |chunk, heartbeat| {
            let reply = prefetch.lock().unwrap().claim(chunk)?;
            receive(&reply, control, heartbeat).map(Some)
        },
    )
}

pub(crate) fn with_prefetch<'a, T>(
    chunks: Vec<&'a Chunk>,
    connections: Connections,
    control: &'a installer::Control,
    load: impl Fn(&Chunk) -> Result<Vec<u8>, String> + Sync,
    meter: impl Fn() -> ChunkIo + Send + 'a,
    work: impl FnOnce(&mut Prefetch<'a>) -> Result<T, String>,
) -> Result<T, String> {
    // Requests and decoded data stay bounded by the connection limit,
    // regardless of the total package size.
    let workers = connections.max.min(chunks.len());
    let in_flight = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel::<Task<'a>>();
    let tasks = Mutex::new(rx);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tasks = &tasks;
            let load = &load;
            let in_flight = &*in_flight;
            scope.spawn(move || {
                loop {
                    let task = tasks.lock().unwrap().recv();
                    let Ok((chunk, reply)) = task else { break };
                    if control.check().is_err() {
                        break;
                    }
                    in_flight.fetch_add(1, Ordering::Relaxed);
                    let result = load(chunk);
                    in_flight.fetch_sub(1, Ordering::Relaxed);
                    let _ = reply.send(result);
                }
            });
        }
        let mut prefetch = Prefetch {
            remaining: chunks.into_iter(),
            pending: HashMap::new(),
            requested: HashSet::new(),
            tasks: tx,
            control,
            scaler: Scaler::new(connections),
            in_flight: in_flight.clone(),
            meter: Box::new(meter),
        };
        prefetch.fill()?;
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
            Connections::fixed(window),
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
            ChunkIo::default,
            |prefetch| {
                let started: HashSet<_> = (0..window)
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
            Connections::fixed(4),
            &control,
            |_| {
                started.send(()).unwrap();
                loop {
                    control.check()?;
                    std::thread::sleep(Duration::from_millis(1));
                }
            },
            ChunkIo::default,
            |prefetch| {
                ready.recv_timeout(Duration::from_secs(1)).unwrap();
                control.stop();
                prefetch.take(&chunk)
            },
        );
        assert!(result.unwrap_err().starts_with("Stopped."));
    }

    /// Feed the scaler `samples` readings 250 ms apart at `speed` bytes/s.
    fn run(
        scaler: &mut Scaler,
        now: &mut Instant,
        io: &mut ChunkIo,
        samples: usize,
        speed: u64,
        failures: u64,
        busy: bool,
    ) -> usize {
        let mut target = scaler.target;
        for _ in 0..samples {
            *now += SAMPLE_INTERVAL;
            io.downloaded += speed / 4;
            io.requests += 10;
            io.failures += failures;
            let in_flight = if busy { target } else { 1 };
            target = scaler.observe(*now, *io, in_flight);
        }
        target
    }

    #[test]
    fn connections_grow_to_epics_limit_while_speed_holds() {
        let mut scaler = Scaler::new(Connections::ADAPTIVE);
        let (mut now, mut io) = (Instant::now(), ChunkIo::default());
        scaler.observe(now, io, 16);
        assert_eq!(
            run(&mut scaler, &mut now, &mut io, 40, 100 << 20, 0, true),
            100
        );
    }

    #[test]
    fn connections_do_not_change_while_the_writer_sets_the_pace() {
        let mut scaler = Scaler::new(Connections::ADAPTIVE);
        let (mut now, mut io) = (Instant::now(), ChunkIo::default());
        scaler.observe(now, io, 16);
        assert_eq!(
            run(&mut scaler, &mut now, &mut io, 40, 100 << 20, 0, false),
            16
        );
    }

    #[test]
    fn connections_back_off_when_downloads_fail_or_speed_collapses() {
        let mut scaler = Scaler::new(Connections::ADAPTIVE);
        let (mut now, mut io) = (Instant::now(), ChunkIo::default());
        scaler.observe(now, io, 16);
        let grown = run(&mut scaler, &mut now, &mut io, 20, 100 << 20, 0, true);
        assert!(grown > 16);
        let failing = run(&mut scaler, &mut now, &mut io, 8, 100 << 20, 5, true);
        assert!(failing < grown, "{failing} < {grown}");

        let mut scaler = Scaler::new(Connections::ADAPTIVE);
        let (mut now, mut io) = (Instant::now(), ChunkIo::default());
        scaler.observe(now, io, 16);
        let grown = run(&mut scaler, &mut now, &mut io, 20, 100 << 20, 0, true);
        let slow = run(&mut scaler, &mut now, &mut io, 12, 10 << 20, 0, true);
        assert!(slow < grown, "{slow} < {grown}");
        assert!(slow >= Connections::ADAPTIVE.min);
    }

    #[test]
    fn fixed_connections_never_scale() {
        let mut scaler = Scaler::new(Connections::fixed(4));
        let (mut now, mut io) = (Instant::now(), ChunkIo::default());
        scaler.observe(now, io, 4);
        assert_eq!(
            run(&mut scaler, &mut now, &mut io, 40, 100 << 20, 0, true),
            4
        );
        assert_eq!(
            run(&mut scaler, &mut now, &mut io, 40, 100 << 20, 9, true),
            4
        );
    }
}
