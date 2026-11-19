//! Opt-in record of slow frames, for finding what makes the interface stutter.
//! Start the launcher with `--frame-log`; slow frames are appended to
//! `frame-stalls.log` in the launcher's data folder. Nothing runs without the flag.
use std::{
    io::Write,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// Duration of the last complete redraw: the launcher's frame plus drawing it and
/// handing it to the display.
static LAST_REDRAW_MICROS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn redraw_took(duration: Duration) {
    LAST_REDRAW_MICROS.store(duration.as_micros() as u64, Ordering::Relaxed);
}

// What the window's event loop did between two frames.
static LOOP_CYCLES: AtomicU64 = AtomicU64::new(0);
static MAX_OS_WAIT_MICROS: AtomicU64 = AtomicU64::new(0);
static MAX_HANDLER_MICROS: AtomicU64 = AtomicU64::new(0);
static MAX_HANDLER: Mutex<&'static str> = Mutex::new("");

pub(crate) fn enabled() -> bool {
    state().is_some()
}

/// One turn of the event loop began, after waiting this long inside the system.
pub(crate) fn loop_turn(os_wait: Duration) {
    LOOP_CYCLES.fetch_add(1, Ordering::Relaxed);
    MAX_OS_WAIT_MICROS.fetch_max(os_wait.as_micros() as u64, Ordering::Relaxed);
}

/// An event handler other than a redraw took this long.
pub(crate) fn handled(name: &'static str, duration: Duration) {
    let micros = duration.as_micros() as u64;
    if MAX_HANDLER_MICROS.fetch_max(micros, Ordering::Relaxed) < micros {
        *MAX_HANDLER.lock().unwrap() = name;
    }
}

/// A frame's own work above this is a stall inside the launcher's code.
const SLOW_FRAME: Duration = Duration::from_millis(40);
/// A wait above this between two frames, while the page animates, is a stall
/// outside the launcher's code: drawing, presenting, or the system itself.
const LONG_GAP: Duration = Duration::from_millis(150);
const MAX_LINES: usize = 5_000;

struct State {
    file: std::fs::File,
    started: Instant,
    previous_end: Option<Instant>,
    previous_frame: Duration,
    lines: usize,
}

fn state() -> Option<&'static Mutex<State>> {
    static STATE: OnceLock<Option<Mutex<State>>> = OnceLock::new();
    STATE
        .get_or_init(|| {
            if !std::env::args().any(|arg| arg == "--frame-log") {
                return None;
            }
            let path = engine_workspace::storage::data_dir().join("frame-stalls.log");
            let mut file = std::fs::File::create(path).ok()?;
            let _ = writeln!(
                file,
                "seconds since start | kind | frame ms | wait before frame ms | of which drawing and presenting the previous frame ms | busy | sections ms | loop turns during wait | longest system wait ms | longest other handler ms"
            );
            Some(Mutex::new(State {
                file,
                started: Instant::now(),
                previous_end: None,
                previous_frame: Duration::ZERO,
                lines: 0,
            }))
        })
        .as_ref()
}

pub(super) struct Frame {
    start: Instant,
    last: Instant,
    sections: Vec<(&'static str, Duration)>,
}

/// Begin timing a frame. Returns nothing unless the log was requested.
pub(super) fn begin() -> Option<Frame> {
    state()?;
    let now = Instant::now();
    Some(Frame {
        start: now,
        last: now,
        sections: Vec::with_capacity(8),
    })
}

/// Close the section of the frame that just ran.
pub(super) fn mark(frame: &mut Option<Frame>, section: &'static str) {
    if let Some(frame) = frame {
        let now = Instant::now();
        frame.sections.push((section, now - frame.last));
        frame.last = now;
    }
}

/// Finish the frame and record it when it, or the wait before it, was slow.
/// `animating` says the page asked for continuous frames, so a long wait is a stall.
pub(super) fn end(frame: Option<Frame>, busy: bool, animating: bool) {
    let (Some(frame), Some(state)) = (frame, state()) else {
        return;
    };
    let now = Instant::now();
    let mut state = state.lock().unwrap();
    let total = now - frame.start;
    let gap = state
        .previous_end
        .map(|end| frame.start.saturating_duration_since(end))
        .unwrap_or_default();
    state.previous_end = Some(now);
    // The previous redraw includes the previous frame's own work; the rest of it
    // is drawing and presenting, and falls inside the wait measured here.
    let drawing = Duration::from_micros(LAST_REDRAW_MICROS.load(Ordering::Relaxed))
        .saturating_sub(state.previous_frame);
    state.previous_frame = total;
    let turns = LOOP_CYCLES.swap(0, Ordering::Relaxed);
    let os_wait = MAX_OS_WAIT_MICROS.swap(0, Ordering::Relaxed) / 1000;
    let handler = MAX_HANDLER_MICROS.swap(0, Ordering::Relaxed) / 1000;
    let handler_name = std::mem::take(&mut *MAX_HANDLER.lock().unwrap());
    let kind = if total >= SLOW_FRAME {
        "slow frame"
    } else if animating && gap >= LONG_GAP {
        "long wait"
    } else {
        return;
    };
    if state.lines >= MAX_LINES {
        return;
    }
    state.lines += 1;
    let sections: Vec<String> = frame
        .sections
        .iter()
        .filter(|(_, time)| time.as_millis() > 0)
        .map(|(name, time)| format!("{name}={}", time.as_millis()))
        .collect();
    let at = frame.start.duration_since(state.started).as_secs_f32();
    let _ = writeln!(
        state.file,
        "{at:.2} | {kind} | {} | {} | {} | {} | {} | {turns} | {os_wait} | {handler} {handler_name}",
        total.as_millis(),
        gap.as_millis(),
        drawing.as_millis(),
        if busy { "busy" } else { "idle" },
        sections.join(" ")
    );
}
