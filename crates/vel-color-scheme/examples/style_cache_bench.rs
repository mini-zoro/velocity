//! Five-run warm cache timing and allocation check. No platform introspection is required.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};
use vel_color_scheme::{ColorScheme, Highlighter};
struct Counted;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
// SAFETY: Every allocation/deallocation delegates the unchanged pointer and layout to System.
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: The caller supplied a valid allocation layout; System owns the result.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: The pointer and original allocation layout are forwarded unchanged.
        unsafe {
            System.dealloc(pointer, layout);
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: System allocated this pointer; its original layout and new size are unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counted = Counted;
fn main() {
    let mut resolver = Highlighter::new(Arc::new(ColorScheme::velocity_dark()));
    let stack = "source.rust meta.function.rust keyword.declaration.function.rust"
        .parse()
        .unwrap();
    let id = resolver.intern(&stack);
    let n = 5_000_000;
    for run in 0..5 {
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let start = Instant::now();
        for _ in 0..n {
            black_box(resolver.style_for(black_box(&stack)));
        }
        let elapsed = start.elapsed().as_secs_f64();
        let allocated = ALLOCATIONS.load(Ordering::Relaxed) - allocations;
        assert_eq!(allocated, 0);
        println!(
            "{{\"run\":{run},\"lookup\":\"scope_stack\",\"lookups_per_second\":{},\"allocations\":{allocated}}}",
            n as f64 / elapsed
        );
        let start = Instant::now();
        for _ in 0..n {
            black_box(resolver.style_for_id(black_box(id)));
        }
        println!(
            "{{\"run\":{run},\"lookup\":\"interned_id\",\"lookups_per_second\":{}}}",
            n as f64 / start.elapsed().as_secs_f64()
        );
    }
}
