//! Zero-allocation timeline tracing for Termway with Perfetto-compatible Chrome Trace Event JSON output.

pub mod event;
pub mod frame;
pub mod ring;
pub mod writer;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

pub use event::{EventId, Phase, ThreadRole, TraceEvent};

pub static ENABLED: AtomicBool = AtomicBool::new(false);
pub static INPUT_ENABLED: AtomicBool = AtomicBool::new(true);
static SESSION_START: OnceLock<Instant> = OnceLock::new();

/// Initializes tracing configuration from CLI options or environment variables.
pub fn init(enabled: bool, input_enabled: bool, buffer_capacity: usize) {
    let _ = SESSION_START.get_or_init(Instant::now);
    INPUT_ENABLED.store(input_enabled, Ordering::Relaxed);
    if enabled {
        ring::init_rings(buffer_capacity);
        ENABLED.store(true, Ordering::Release);
    } else {
        ENABLED.store(false, Ordering::Release);
    }
}

/// Initializes tracing from environment variables (TERMWAY_TRACE, TERMWAY_TRACE_FILE, etc.)
pub fn init_from_env() {
    let trace_enabled = std::env::var("TERMWAY_TRACE").is_ok()
        || std::env::var("TERMWAY_TRACE_FILE").is_ok();
    let input_enabled = std::env::var("TERMWAY_TRACE_NO_INPUT").is_err();
    let buffer_capacity = std::env::var("TERMWAY_TRACE_BUFFER")
        .ok()
        .and_then(|s| parse_buffer_capacity(&s))
        .unwrap_or(65536);

    init(trace_enabled, input_enabled, buffer_capacity);
}

pub fn parse_buffer_capacity(s: &str) -> Option<usize> {
    let s = s.trim().to_lowercase();
    if let Some(num) = s.strip_suffix('m') {
        num.parse::<usize>().ok().map(|n| n * 1024 * 1024)
    } else if let Some(num) = s.strip_suffix('k') {
        num.parse::<usize>().ok().map(|n| n * 1024)
    } else {
        s.parse::<usize>().ok()
    }
}

#[inline]
pub fn session_start() -> Instant {
    *SESSION_START.get_or_init(Instant::now)
}

/// Monotonic microseconds elapsed from session start.
#[inline]
pub fn now_us() -> u64 {
    session_start().elapsed().as_micros() as u64
}

/// Returns true if timeline tracing is active.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Returns true if input tracing is active.
#[inline]
pub fn input_enabled() -> bool {
    enabled() && INPUT_ENABLED.load(Ordering::Relaxed)
}

/// Records a Complete ("X") duration slice.
#[inline]
pub fn slice(
    thread: ThreadRole,
    event_id: EventId,
    ts_us: u64,
    dur_us: u32,
    id: u32,
    arg1: u32,
    arg2: u32,
    arg3: u16,
) {
    if !enabled() {
        return;
    }
    ring::record(
        thread,
        TraceEvent {
            ts_us,
            dur_us,
            id,
            arg1,
            arg2,
            arg3,
            lane: 0,
            phase: Phase::Complete,
            thread_id: thread,
            event_id,
        },
    );
}

/// Records an Instant ("i") marker.
#[inline]
pub fn instant(
    thread: ThreadRole,
    event_id: EventId,
    ts_us: u64,
    id: u32,
    arg1: u32,
    arg2: u32,
    arg3: u16,
) {
    if !enabled() {
        return;
    }
    if event_id.category() == "input" && !input_enabled() {
        return;
    }
    ring::record(
        thread,
        TraceEvent {
            ts_us,
            dur_us: 0,
            id,
            arg1,
            arg2,
            arg3,
            lane: 0,
            phase: Phase::Instant,
            thread_id: thread,
            event_id,
        },
    );
}

/// Records a FlowStart ("s") non-blocking arrow begin.
#[inline]
pub fn flow_start(thread: ThreadRole, event_id: EventId, ts_us: u64, id: u32, arg1: u32, arg3: u16) {
    if !enabled() {
        return;
    }
    ring::record(
        thread,
        TraceEvent {
            ts_us,
            dur_us: 0,
            id,
            arg1,
            arg2: 0,
            arg3,
            lane: 0,
            phase: Phase::FlowStart,
            thread_id: thread,
            event_id,
        },
    );
}

/// Records a FlowEnd ("f") non-blocking arrow end.
#[inline]
pub fn flow_end(thread: ThreadRole, event_id: EventId, ts_us: u64, id: u32) {
    if !enabled() {
        return;
    }
    ring::record(
        thread,
        TraceEvent {
            ts_us,
            dur_us: 0,
            id,
            arg1: 0,
            arg2: 0,
            arg3: 0,
            lane: 0,
            phase: Phase::FlowEnd,
            thread_id: thread,
            event_id,
        },
    );
}

/// Records a Counter ("C") numeric track value.
#[inline]
pub fn counter(thread: ThreadRole, event_id: EventId, ts_us: u64, id: u32, value: u32) {
    if !enabled() {
        return;
    }
    ring::record(
        thread,
        TraceEvent {
            ts_us,
            dur_us: 0,
            id,
            arg1: value,
            arg2: 0,
            arg3: 0,
            lane: 0,
            phase: Phase::Counter,
            thread_id: thread,
            event_id,
        },
    );
}
