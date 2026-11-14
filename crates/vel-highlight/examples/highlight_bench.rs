//! End-to-end style spans, Text snapshots and ChangeSets, at least five independent runs.
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::{Change, ChangeId, ChangeSet};
use vel_color_scheme::ColorScheme;
use vel_highlight::{DocumentHighlighter, HighlightService};
use vel_syntax::SyntaxSet;
use vel_text::Text;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = std::fs::read_to_string(root.join("spikes/data/rust-10mb.rs"))?;
    let set = Arc::new(SyntaxSet::load_directories(&[
        root.join("third_party/sublime-packages")
    ])?);
    let syntax = set
        .find_syntax_by_name("Rust")
        .ok_or("missing Rust syntax")?;
    let scheme = Arc::new(ColorScheme::velocity_dark());
    if std::env::args().any(|a| a == "--background-only") {
        let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("timings.json");
        let mut timings: Vec<serde_json::Value> = std::fs::read(&output)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        timings.retain(|r| r["case"] != "full_10mb_rust_background");
        let text = Text::from(source.as_str());
        for run in 0..5 {
            let engine = DocumentHighlighter::new(
                text.clone(),
                ChangeId(0),
                set.clone(),
                syntax,
                scheme.clone(),
            );
            let start = Instant::now();
            let service = HighlightService::new(engine)?;
            loop {
                if service
                    .progress()
                    .is_some_and(|p| p.parsed_lines == p.total_lines)
                {
                    break;
                }
                if start.elapsed() > Duration::from_secs(120) {
                    return Err("background highlight did not finish".into());
                }
                std::thread::park_timeout(Duration::from_millis(1));
            }
            let seconds = start.elapsed().as_secs_f64();
            let record = serde_json::json!({"run":run,"case":"full_10mb_rust_background","bytes":source.len(),"lines":text.len_lines(),"seconds":seconds,"mb_per_second":source.len()as f64/1_000_000./seconds});
            println!("{record}");
            timings.push(record);
        }
        std::fs::write(output, serde_json::to_vec_pretty(&timings)?)?;
        return Ok(());
    }
    let sample: String = source.split_inclusive('\n').take(10_000).collect();
    let text = Text::from(sample.as_str());
    let edit = text.line_to_byte(5000)
        + text
            .line(5000)
            .to_string()
            .char_indices()
            .find(|(_, c)| c.is_ascii_alphabetic())
            .map_or(0, |(i, _)| i);
    if std::env::args().any(|a| a == "--worker-edits-only") {
        let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("timings.json");
        let mut timings: Vec<serde_json::Value> = std::fs::read(&output)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        timings.retain(|r| r["case"] != "worker_edit_10k_visible_100");
        for run in 0..5 {
            let mut engine = DocumentHighlighter::new(
                text.clone(),
                ChangeId(0),
                set.clone(),
                syntax,
                scheme.clone(),
            );
            engine.finish()?;
            let mut service = HighlightService::new(engine)?;
            let mut current = text.clone();
            let mut samples = Vec::new();
            for version in 1..=100 {
                current.replace(edit..edit + 1, if version % 2 == 0 { "x" } else { "y" });
                let changes = ChangeSet::new(vec![Change {
                    old_range: edit..edit + 1,
                    new_len: 1,
                }]);
                let start = Instant::now();
                service.apply_change(current.clone(), ChangeId(version), changes)?;
                let batch = service.request_lines(ChangeId(version), 4950..5050)?;
                if !batch.complete {
                    loop {
                        if let Some(result) = service.try_recv()
                            && result?.complete
                        {
                            break;
                        }
                        if start.elapsed() > Duration::from_secs(3) {
                            return Err("worker edit did not complete".into());
                        }
                        std::thread::yield_now();
                    }
                }
                samples.push(start.elapsed().as_secs_f64() * 1000.);
            }
            let mut sorted = samples.clone();
            sorted.sort_by(f64::total_cmp);
            let record = serde_json::json!({"run":run,"case":"worker_edit_10k_visible_100","samples":samples,"p95_ms":sorted[94],"max_ms":sorted[99]});
            println!("{record}");
            timings.push(record);
        }
        std::fs::write(output, serde_json::to_vec_pretty(&timings)?)?;
        return Ok(());
    }
    let mut timings = Vec::new();
    for run in 0..5 {
        let mut engine = DocumentHighlighter::new(
            text.clone(),
            ChangeId(0),
            set.clone(),
            syntax,
            scheme.clone(),
        );
        engine.finish()?;
        let mut edits = Vec::new();
        let mut parsed = Vec::new();
        for version in 1..=100 {
            let mut next = engine.text().clone();
            next.replace(edit..edit + 1, if version % 2 == 0 { "x" } else { "y" });
            let changes = ChangeSet::new(vec![Change {
                old_range: edit..edit + 1,
                new_len: 1,
            }]);
            let start = Instant::now();
            engine.apply_change(next, ChangeId(version), &changes)?;
            let result =
                engine.request_lines(ChangeId(version), 4950..5050, Duration::from_millis(2))?;
            edits.push(start.elapsed().as_secs_f64() * 1000.);
            assert!(
                result.complete,
                "stable edit did not finish within viewport budget"
            );
            parsed.push(engine.stats().parsed_lines);
        }
        let mut sorted = edits.clone();
        sorted.sort_by(f64::total_cmp);
        let record = serde_json::json!({"run":run,"case":"edit_10k_visible_100","samples":edits,"p95_ms":sorted[94],"max_ms":sorted[99],"parsed_lines":parsed});
        println!("{record}");
        timings.push(record);
    }
    // Warm all Rust regex paths before full-file measurements.
    let text = Text::from(source.as_str());
    {
        let mut warm = DocumentHighlighter::new(
            text.clone(),
            ChangeId(0),
            set.clone(),
            syntax,
            scheme.clone(),
        );
        warm.finish()?;
    }
    for run in 0..5 {
        let mut engine = DocumentHighlighter::new(
            text.clone(),
            ChangeId(0),
            set.clone(),
            syntax,
            scheme.clone(),
        );
        let start = Instant::now();
        engine.finish()?;
        let seconds = start.elapsed().as_secs_f64();
        let record = serde_json::json!({"run":run,"case":"full_10mb_rust","bytes":source.len(),"lines":text.len_lines(),"seconds":seconds,"mb_per_second":source.len()as f64/1_000_000./seconds});
        println!("{record}");
        timings.push(record);
    }
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("timings.json"),
        serde_json::to_vec_pretty(&timings)?,
    )?;
    Ok(())
}
