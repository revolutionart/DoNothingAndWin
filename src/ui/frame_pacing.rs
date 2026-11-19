//! Keeps animation smooth while a transfer runs.
//!
//! Measured on Windows 11 with `--frame-log`: during an engine transfer the
//! window's message loop was held up for 150 ms to over a second between frames,
//! about once a second, although each frame took 1-2 ms to build and draw and the
//! processor was mostly idle. With one thread in the process waking on a short
//! timer, the same transfers ran without those waits; with it off they returned.
//! Why Windows delays the message loop without it is not established. The steady
//! short wake-ups are what was tested, so that is what this does, and only while
//! there is something to animate.
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
    time::Duration,
};

const TICK: Duration = Duration::from_millis(10);

static ACTIVE: AtomicBool = AtomicBool::new(false);
static TICKER: OnceLock<Thread> = OnceLock::new();

/// Call once per frame. `animating` is true while work is running and the window
/// is visible. The ticker sleeps without waking at all when it is false.
pub(super) fn set(animating: bool) {
    if ACTIVE.swap(animating, Ordering::Relaxed) == animating || !animating {
        return;
    }
    TICKER
        .get_or_init(|| {
            std::thread::Builder::new()
                .name("frame-pacing".into())
                .spawn(|| {
                    loop {
                        if ACTIVE.load(Ordering::Relaxed) {
                            std::thread::sleep(TICK);
                        } else {
                            std::thread::park();
                        }
                    }
                })
                .expect("start frame pacing thread")
                .thread()
                .clone()
        })
        .unpark();
}
