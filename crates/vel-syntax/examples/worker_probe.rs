//! A persistent worker benchmark process, driven by paired-round10.py over stdin.
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::ChangeId;
use vel_color_scheme::ColorScheme;
use vel_highlight::{DocumentHighlighter, HighlightService};
use vel_syntax::SyntaxSet;
use vel_text::Text;

#[cfg(feature = "profile")]
#[path = "../src/bin/regex-bench/profile.rs"]
mod profile;
#[cfg(feature = "profile")]
mod allocations {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
    };
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static COUNT: AtomicU64 = AtomicU64::new(0);
    static BYTES: AtomicU64 = AtomicU64::new(0);
    pub struct Allocator;
    fn record(bytes: usize) {
        if ENABLED.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
        }
    }
    // SAFETY: All allocations and deallocations delegate unchanged to System.
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record(layout.size());
            // SAFETY: The caller supplies the GlobalAlloc layout invariant.
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record(layout.size());
            // SAFETY: The caller supplies the GlobalAlloc layout invariant.
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: The pointer and layout are forwarded to their allocator.
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record(new_size);
            // SAFETY: The caller's pointer, layout and size are forwarded unchanged.
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }
    pub fn start() {
        COUNT.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
    }
    pub fn finish() -> (u64, u64) {
        ENABLED.store(false, Ordering::Relaxed);
        (COUNT.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed))
    }
}
#[cfg(feature = "profile")]
#[global_allocator]
static ALLOCATOR: allocations::Allocator = allocations::Allocator;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let definitions = PathBuf::from(&args[0]);
    let source = std::fs::read_to_string(&args[1])?;
    let syntax_name = args[2].to_str().ok_or("invalid syntax name")?;
    let set = SyntaxSet::load_directories(&[definitions])?;
    #[cfg(feature = "profile")]
    let backend = Arc::new(profile::Backend::new(Arc::new(
        vel_regex::MultiDfa::default(),
    )));
    #[cfg(feature = "profile")]
    let set = set.with_matcher_backend(backend.clone());
    let set = Arc::new(set);
    let text = Text::from(source.as_str());
    let syntax = set
        .find_syntax_by_name(syntax_name)
        .ok_or("missing syntax")?;
    let scheme = Arc::new(ColorScheme::velocity_dark());
    let engine = || {
        DocumentHighlighter::new(
            text.clone(),
            ChangeId(0),
            set.clone(),
            syntax,
            scheme.clone(),
        )
    };
    let mut warm = engine();
    warm.finish()?;
    drop(warm);
    println!(
        "{}",
        serde_json::json!({"ready":true,"bytes":source.len(),"lines":text.len_lines(),"blake3":blake3::hash(source.as_bytes()).to_hex().to_string()})
    );
    io::stdout().flush()?;
    for command in io::stdin().lock().lines() {
        let command = command?;
        if command == "quit" {
            break;
        }
        if command != "run" {
            return Err("expected run or quit".into());
        }
        let fresh = engine();
        #[cfg(feature = "profile")]
        backend.run.store(0, std::sync::atomic::Ordering::Relaxed);
        #[cfg(feature = "profile")]
        allocations::start();
        let start = Instant::now();
        let service = HighlightService::new(fresh)?;
        loop {
            if service
                .progress()
                .is_some_and(|p| p.parsed_lines == p.total_lines)
            {
                break;
            }
            if start.elapsed() > Duration::from_secs(180) {
                return Err("worker timeout".into());
            }
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let seconds = start.elapsed().as_secs_f64();
        let mut result = serde_json::json!({"seconds":seconds,"mb_per_second":source.len() as f64 / 1e6 / seconds});
        #[cfg(feature = "profile")]
        {
            let (allocations, allocated_bytes) = allocations::finish();
            let measured = service.profile().ok_or("worker still locked")?;
            result["profile"] = serde_json::json!({"input_seconds":measured.input.as_secs_f64(),"parse_seconds":measured.parse.as_secs_f64(),"render_seconds":measured.render.as_secs_f64(),"checkpoint_seconds":measured.checkpoint.as_secs_f64(),"render_hits":measured.render_hits,"render_queries":measured.render_queries,"render_entries":measured.render_entries,"regex_contexts":backend.results(),"rust_allocations":allocations,"rust_allocated_bytes":allocated_bytes});
            backend
                .run
                .store(usize::MAX, std::sync::atomic::Ordering::Relaxed);
        }
        // Drop is asynchronous. A reader waits for this process to acknowledge
        // teardown before asking either process to start another measured worker.
        drop(service);
        std::thread::park_timeout(Duration::from_millis(200));
        // Keep a mutable binding in both feature configurations.
        result["teardown_wait_ms"] = serde_json::json!(200);
        println!("{result}");
        io::stdout().flush()?;
    }
    Ok(())
}
