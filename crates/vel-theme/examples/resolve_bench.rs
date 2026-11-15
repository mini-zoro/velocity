//! Seven-run warm-cache benchmark and allocation assertion; no platform introspection required.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};
use vel_theme::*;
#[derive(Debug)]
struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
// SAFETY: every allocation/deallocation is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller supplies the GlobalAlloc layout invariant; System receives it unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarded unchanged with the same allocation contract.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's live pointer/layout/new-size are forwarded unchanged.
        unsafe { System.realloc(p, layout, size) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: the live pointer and its original layout are forwarded unchanged.
        unsafe { System.dealloc(p, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn main() {
    let mut resolver = Resolver::new(Arc::new(Theme::velocity(
        Appearance::Dark,
        &Palette::default(),
    )));
    let settings = Settings::default();
    let paths: Vec<_> = ELEMENTS
        .iter()
        .map(|e| {
            [
                Element::new("window", &[]),
                Element::new("quick_panel_row", &["selected"]),
                Element::new(e.class, &["hover"]),
            ]
        })
        .collect();
    for path in &paths {
        black_box(resolver.resolve(&ElementPath(path), &settings, Platform::Mac));
    }
    for run in 0..7 {
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        let start = Instant::now();
        for i in 0..100_000 {
            black_box(resolver.resolve(
                &ElementPath(&paths[i % paths.len()]),
                &settings,
                Platform::Mac,
            ));
        }
        let elapsed = start.elapsed();
        let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
        assert_eq!(allocations, 0, "warm resolve allocated");
        println!(
            "{{\"case\":\"theme_warm_resolve\",\"run\":{run},\"ns_per_call\":{:.2},\"allocations\":{allocations},\"classes\":{}}}",
            elapsed.as_nanos() as f64 / 100_000.,
            paths.len()
        );
    }
}
