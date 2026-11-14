//! Counts Rust allocations, excluding Oniguruma's native C allocator.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use vel_syntax::{LineOps, ParseState, SyntaxDefinition, SyntaxSetBuilder};
struct Allocator;
static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
// SAFETY: every operation forwards the exact pointer/layout contract to System;
// counting uses atomics only and never allocates or inspects the allocation.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ENABLED.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: GlobalAlloc's caller supplies a valid allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr/layout are passed through unchanged from GlobalAlloc's caller.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ENABLED.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: GlobalAlloc's reallocation preconditions are forwarded unchanged.
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[test]
fn steady_stream_reuses_search_capture_and_operation_storage() {
    let syntax = SyntaxDefinition::load_from_str("name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: '(\\w+)'\n      captures:\n        1: variable.other.fixture\n", true, None).unwrap();
    let mut builder = SyntaxSetBuilder::new();
    builder.add(syntax);
    let set = Arc::new(builder.build());
    let syntax = set.find_syntax_by_name("Fixture").unwrap();
    for capacity in [0, 128] {
        let mut state = ParseState::new(set.clone(), syntax).with_line_cache_capacity(capacity);
        let mut output = LineOps::default();
        for _ in 0..128 {
            state.parse_line_into("foo bar\n", &mut output).unwrap();
        }
        ALLOCATIONS.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
        for _ in 0..10_000 {
            state.parse_line_into("foo bar\n", &mut output).unwrap();
            std::hint::black_box(&output);
        }
        ENABLED.store(false, Ordering::Relaxed);
        assert_eq!(
            ALLOCATIONS.load(Ordering::Relaxed),
            0,
            "capacity {capacity}"
        );
    }
}
