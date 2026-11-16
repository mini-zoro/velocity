//! Paired, device-free layout-cache eviction microbenchmark.
//! The baseline cache below is the pre-phase-2 implementation, retained as a model.
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    hint::black_box,
    sync::Arc,
    time::Instant,
};
use vel_render::{
    cache::{LayoutStyle, LineLayoutCache},
    font::ShapedGlyph,
};

type BeforeEntry = (String, Arc<[ShapedGlyph]>, u64);
struct Before {
    entries: HashMap<(u64, LayoutStyle), BeforeEntry>,
    tick: u64,
    maximum: usize,
}
impl Before {
    fn key(text: &str, style: LayoutStyle) -> (u64, LayoutStyle) {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hash);
        (hash.finish(), style)
    }
    fn get(&mut self, text: &str, style: LayoutStyle) -> Option<Arc<[ShapedGlyph]>> {
        self.tick += 1;
        let entry = self.entries.get_mut(&Self::key(text, style))?;
        if entry.0 != text {
            return None;
        }
        entry.2 = self.tick;
        Some(entry.1.clone())
    }
    fn insert(&mut self, text: &str, style: LayoutStyle, glyphs: Arc<[ShapedGlyph]>) {
        if self.entries.len() >= self.maximum {
            let key = *self.entries.iter().min_by_key(|(_, e)| e.2).unwrap().0;
            self.entries.remove(&key);
        }
        self.tick += 1;
        self.entries
            .insert(Self::key(text, style), (text.into(), glyphs, self.tick));
    }
}
fn cache(before: bool, lines: &[String], capacity: usize) -> f64 {
    let style = LayoutStyle {
        font_style: Default::default(),
        px64: 832,
        font_epoch: 0,
    };
    let glyphs: Arc<[ShapedGlyph]> = Arc::from([]);
    let mut old = Before {
        entries: HashMap::new(),
        tick: 0,
        maximum: capacity,
    };
    let mut new = LineLayoutCache::new(capacity);
    for text in &lines[..capacity] {
        if before {
            old.insert(text, style, glyphs.clone());
        } else {
            new.insert(text, style, glyphs.clone());
        }
    }
    let iterations = 30_000;
    let at = Instant::now();
    for i in 0..iterations {
        let text = black_box(&lines[(capacity + i) % lines.len()]);
        if before {
            if old.get(text, style).is_none() {
                old.insert(text, style, glyphs.clone());
            }
        } else if new.get(text, style).is_none() {
            new.insert(text, style, glyphs.clone());
        }
    }
    black_box((old.entries.len(), new.len()));
    at.elapsed().as_nanos() as f64 / iterations as f64
}
fn main() {
    let file = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/data/rust-10mb.rs")
        });
    let source = std::fs::read_to_string(file).expect("10 MB Rust fixture");
    let mut seen = HashSet::new();
    let lines: Vec<_> = source
        .lines()
        .filter(|line| line.len() <= 16_384 && seen.insert(*line))
        .take(8192)
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines.len(),
        8192,
        "fixture needs 8192 distinct bounded lines"
    );
    let mut results = serde_json::Map::new();
    results.insert("fixture".into(), serde_json::json!({"bytes": source.len(), "lines":source.lines().count(),"distinct_lines_in_churn":lines.len()}));
    for capacity in [128, 2048] {
        let mut before = Vec::new();
        let mut after = Vec::new();
        for pair in 0..9 {
            for old in if pair % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let ns = cache(old, &lines, capacity);
                if old {
                    before.push(ns);
                } else {
                    after.push(ns);
                }
            }
        }
        results.insert(
            format!("cache_churn_{capacity}_ns"),
            serde_json::json!({"before":before,"after":after}),
        );
    }
    println!("{}", serde_json::Value::Object(results));
}
