//! Count allocations in the registry handoff used by both Editor::run and Input::run.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    hint::black_box,
};
use vel_commands::Registry;

#[derive(Debug)]
struct CountingAllocator;
thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
fn allocated() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    }
}
// SAFETY: Every operation forwards its unchanged pointer/layout to the System allocator.
// Counters are thread-local scalars and never allocate or affect allocation ownership.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocated();
        // SAFETY: The caller supplies a valid allocation layout, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocated();
        // SAFETY: The caller supplies a valid allocation layout, forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocated();
        // SAFETY: The System allocation and its layout/size are forwarded unchanged.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The System allocation and its original layout are forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn registry_handoffs_allocate_no_placeholder_state() {
    let mut registry = Registry::standard();
    ALLOCATIONS.set(0);
    COUNTING.set(true);
    for _ in 0..10_000 {
        let taken = black_box(&mut registry).take_for_dispatch();
        registry = black_box(taken);
    }
    COUNTING.set(false);
    assert_eq!(ALLOCATIONS.get(), 0);
    assert!(registry.kind("insert").is_some());
}
