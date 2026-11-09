//! Independent, fixed-capacity ring buffers per thread with drop-oldest overflow handling.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use super::event::{EventId, ThreadRole, TraceEvent};

pub struct ThreadRing {
    slots: Vec<TraceEvent>,
    cap: usize,
    head: usize,
    len: usize,
    overflow: u64,
}

pub static TOTAL_OVERFLOW: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn total_overflow() -> u64 {
    TOTAL_OVERFLOW.load(Ordering::Relaxed)
}

static MAIN_RING: Mutex<Option<ThreadRing>> = Mutex::new(None);
static WORKER_RING: Mutex<Option<ThreadRing>> = Mutex::new(None);
static WRITE_RING: Mutex<Option<ThreadRing>> = Mutex::new(None);

impl ThreadRing {
    pub fn with_capacity(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            slots: Vec::with_capacity(cap),
            cap,
            head: 0,
            len: 0,
            overflow: 0,
        }
    }

    #[inline]
    pub fn push(&mut self, ev: TraceEvent) {
        if self.cap == 0 {
            return;
        }

        // In-place coalescing for consecutive pointer motions
        if ev.event_id == EventId::InputMotionCoalesce && self.len > 0 {
            let last_idx = (self.head + self.len - 1) % self.cap;
            let last = &mut self.slots[last_idx];
            if last.event_id == EventId::InputMotionCoalesce {
                last.dur_us = ev.ts_us.saturating_sub(last.ts_us) as u32;
                last.arg1 = last.arg1.saturating_add(1);
                last.arg2 = ev.arg2; // latest coordinates
                return;
            }
        }

        if self.len < self.cap {
            let idx = (self.head + self.len) % self.cap;
            if idx == self.slots.len() {
                self.slots.push(ev);
            } else {
                self.slots[idx] = ev;
            }
            self.len += 1;
        } else {
            // Ring full: overwrite oldest slot (drop-oldest) and advance head
            self.slots[self.head] = ev;
            self.head = (self.head + 1) % self.cap;
            self.overflow += 1;
            TOTAL_OVERFLOW.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Drain all valid events in FIFO (oldest to newest) order.
    pub fn drain_ordered(&mut self) -> (Vec<TraceEvent>, u64) {
        let mut out = Vec::with_capacity(self.len);
        for i in 0..self.len {
            let idx = (self.head + i) % self.cap;
            out.push(self.slots[idx]);
        }
        let ovf = self.overflow;
        self.head = 0;
        self.len = 0;
        self.overflow = 0;
        (out, ovf)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn overflow(&self) -> u64 {
        self.overflow
    }
}

pub fn init_rings(capacity: usize) {
    let mut main_guard = MAIN_RING.lock().unwrap();
    *main_guard = Some(ThreadRing::with_capacity(capacity));

    let mut worker_guard = WORKER_RING.lock().unwrap();
    *worker_guard = Some(ThreadRing::with_capacity(capacity));

    let mut write_guard = WRITE_RING.lock().unwrap();
    *write_guard = Some(ThreadRing::with_capacity(capacity));
}

pub fn reset_rings() {
    let mut main_guard = MAIN_RING.lock().unwrap();
    *main_guard = None;

    let mut worker_guard = WORKER_RING.lock().unwrap();
    *worker_guard = None;

    let mut write_guard = WRITE_RING.lock().unwrap();
    *write_guard = None;

    TOTAL_OVERFLOW.store(0, Ordering::Relaxed);
}

#[inline]
pub fn record(thread: ThreadRole, ev: TraceEvent) {
    match thread {
        ThreadRole::Main => {
            if let Ok(mut guard) = MAIN_RING.lock() {
                if let Some(ref mut ring) = *guard {
                    ring.push(ev);
                }
            }
        }
        ThreadRole::AsyncRender => {
            if let Ok(mut guard) = WORKER_RING.lock() {
                if let Some(ref mut ring) = *guard {
                    ring.push(ev);
                }
            }
        }
        ThreadRole::AsyncWrite => {
            if let Ok(mut guard) = WRITE_RING.lock() {
                if let Some(ref mut ring) = *guard {
                    ring.push(ev);
                }
            }
        }
    }
}

/// Drains Main, AsyncRender, and AsyncWrite rings in FIFO order and returns total overflow count.
pub fn drain_all() -> (Vec<TraceEvent>, u64) {
    let mut all_events = Vec::new();
    let mut total_ovf = 0;

    if let Ok(mut guard) = MAIN_RING.lock() {
        if let Some(ref mut ring) = *guard {
            let (mut events, ovf) = ring.drain_ordered();
            all_events.append(&mut events);
            total_ovf += ovf;
        }
    }

    if let Ok(mut guard) = WORKER_RING.lock() {
        if let Some(ref mut ring) = *guard {
            let (mut events, ovf) = ring.drain_ordered();
            all_events.append(&mut events);
            total_ovf += ovf;
        }
    }

    if let Ok(mut guard) = WRITE_RING.lock() {
        if let Some(ref mut ring) = *guard {
            let (mut events, ovf) = ring.drain_ordered();
            all_events.append(&mut events);
            total_ovf += ovf;
        }
    }

    // Sort by timestamp
    all_events.sort_by_key(|e| e.ts_us);
    (all_events, total_ovf)
}
