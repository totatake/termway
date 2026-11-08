//! Frame pipeline tracing helpers and atomic frame_id allocator.

use std::sync::atomic::{AtomicU32, Ordering};

/// Monotonically increasing frame ID allocated per submitted frame.
pub static NEXT_FRAME_ID: AtomicU32 = AtomicU32::new(1);

#[inline]
pub fn alloc_frame_id() -> u32 {
    NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn reset_frame_id() {
    NEXT_FRAME_ID.store(1, Ordering::Relaxed);
}
