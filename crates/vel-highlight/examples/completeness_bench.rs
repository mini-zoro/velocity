//! Paired completeness-query microbenchmark using the actual 10 MB line count.
use std::{hint::black_box, path::Path, time::Instant};
use vel_buffer::ChangeId;
use vel_highlight::{ScopeSnapshot, ScopeStackId, ScopeStacks, ScopedLine};
use vel_text::Text;
fn measure(snapshot: &ScopeSnapshot, before: bool) -> f64 {
    let iterations = 1440;
    let at = Instant::now();
    for _ in 0..iterations {
        let snapshot = black_box(snapshot);
        let complete = if before {
            snapshot.lines.iter().all(ScopedLine::exact)
        } else {
            snapshot.complete()
        };
        black_box(complete);
    }
    at.elapsed().as_nanos() as f64 / iterations as f64
}
fn main() {
    let file = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/data/rust-10mb.rs")
        });
    let source = std::fs::read_to_string(file).unwrap();
    let text = Text::from(source.as_str());
    let snapshot = ScopeSnapshot {
        version: ChangeId(1),
        range: 0..text.len_lines(),
        lines: (0..text.len_lines())
            .map(|_| ScopedLine::empty(ScopeStackId(0)))
            .collect(),
        text,
        stacks: ScopeStacks::default(),
    };
    let mut before = Vec::new();
    let mut after = Vec::new();
    for pair in 0..9 {
        for old in if pair % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            let ns = measure(&snapshot, old);
            if old {
                before.push(ns);
            } else {
                after.push(ns);
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"bytes":source.len(),"lines":snapshot.lines.len(),"iterations_per_pair":1440,"query_ns":{"before":before,"after":after}})
    );
}
