//! Test-only global allocator that counts live heap bytes, for memory soak tests.
//!
//! Every allocation in the test binary goes through here; the cost is one relaxed
//! atomic add per call. The count is process-wide, so soak tests run alone:
//! `cargo test -p velocity --bin velocity -- --ignored --test-threads=1 memory_soak`.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
};

pub(crate) struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to `System` with the caller's layout unchanged and
// only adjusts a counter afterwards, so `System`'s guarantees carry over.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; see the impl comment.
        let pointer = unsafe { System.alloc(layout) };
        CALLS.fetch_add(1, Ordering::Relaxed);
        if !pointer.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; see the impl comment.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        CALLS.fetch_add(1, Ordering::Relaxed);
        if !pointer.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwarded unchanged; see the impl comment.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: forwarded unchanged; see the impl comment.
        let moved = unsafe { System.realloc(pointer, layout, size) };
        CALLS.fetch_add(1, Ordering::Relaxed);
        if !moved.is_null() {
            LIVE.fetch_add(size as isize - layout.size() as isize, Ordering::Relaxed);
        }
        moved
    }
}

/// Heap bytes currently allocated by the whole test process.
pub(crate) fn live_bytes() -> isize {
    LIVE.load(Ordering::Relaxed)
}

/// Allocation and reallocation calls made by the whole test process so far.
pub(crate) fn allocation_calls() -> usize {
    CALLS.load(Ordering::Relaxed)
}
