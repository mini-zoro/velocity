//! Five-run, parse-only and incremental editor benchmarks without sysctl.
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Instant};
use vel_syntax::{LineOps, ParseState, SyntaxSet};
fn median(xs: &[f64]) -> f64 {
    let mut sorted = xs.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let packages = root.join("third_party/sublime-packages");
    let cache = std::env::temp_dir().join(format!("velocity-syntax-bench-{}", std::process::id()));
    let source = std::fs::read_to_string(root.join("spikes/data/rust-10mb.rs"))
        .unwrap_or_else(|_| "pub fn example(x: usize) -> usize { x + 1 }\n".repeat(250_000));
    let mut yaml_ms = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        std::hint::black_box(SyntaxSet::load_directories(&[&packages])?);
        yaml_ms.push(t.elapsed().as_secs_f64() * 1000.);
    }
    SyntaxSet::load_cached(&packages, &cache)?;
    let mut cache_ms = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let (set, status) = SyntaxSet::load_cached(&packages, &cache)?;
        assert_eq!(status, vel_syntax::CacheStatus::Hit);
        std::hint::black_box(set);
        cache_ms.push(t.elapsed().as_secs_f64() * 1000.);
    }
    let set = Arc::new(SyntaxSet::load_cached(&packages, &cache)?.0);
    let syntax = set
        .find_syntax_by_name("Rust")
        .ok_or("Rust syntax missing")?;
    let lines: Vec<_> = source.split_inclusive('\n').collect();
    let mut warm = ParseState::new(set.clone(), syntax);
    let mut output = LineOps::default();
    for line in &lines {
        warm.parse_line_into(line, &mut output)?;
        std::hint::black_box(&output);
    }
    let mut parse_seconds = Vec::new();
    for _ in 0..5 {
        let mut state = ParseState::new(set.clone(), syntax);
        let mut output = LineOps::default();
        let t = Instant::now();
        for line in &lines {
            state.parse_line_into(line, &mut output)?;
            std::hint::black_box(&output);
        }
        parse_seconds.push(t.elapsed().as_secs_f64());
    }
    let sample: Vec<_> = lines.iter().copied().cycle().take(200).collect();
    let mut state = ParseState::new(set.clone(), syntax);
    let mut checkpoints = vec![state.clone()];
    for line in &sample {
        state.parse_line(line)?;
        checkpoints.push(state.clone());
    }
    let at = sample
        .iter()
        .enumerate()
        .skip(40)
        .find(|(_, l)| l.chars().any(char::is_alphabetic))
        .map_or(50, |(i, _)| i);
    let mut edited = sample[at].to_string();
    if let Some((i, c)) = edited.char_indices().find(|(_, c)| c.is_ascii_alphabetic()) {
        edited.replace_range(i..i + c.len_utf8(), if c == 'x' { "y" } else { "x" });
    }
    let mut window_ms = Vec::new();
    let mut convergence_ms = Vec::new();
    let mut reparsed_lines = Vec::new();
    for _ in 0..5 {
        let mut state = checkpoints[at].clone();
        let t = Instant::now();
        state.parse_line(&edited)?;
        for line in &sample[at + 1..at + 100] {
            std::hint::black_box(state.parse_line(line)?);
        }
        window_ms.push(t.elapsed().as_secs_f64() * 1000.);
        let mut state = checkpoints[at].clone();
        let t = Instant::now();
        state.parse_line(&edited)?;
        let mut n = 1;
        while n < 100 && state != checkpoints[at + n] {
            state.parse_line(sample[at + n])?;
            n += 1;
        }
        convergence_ms.push(t.elapsed().as_secs_f64() * 1000.);
        reparsed_lines.push(n);
    }
    let seconds = median(&parse_seconds);
    let result = json!({"runs":5,"bytes":source.len(),"lines":lines.len(),"yaml_ms":yaml_ms,"yaml_median_ms":median(&yaml_ms),"cache_ms_including_content_hashes":cache_ms,"cache_median_ms":median(&cache_ms),"parse_seconds":parse_seconds,"parse_mb_per_second":source.len() as f64/1_000_000./seconds,"parse_lines_per_second":lines.len() as f64/seconds,"window_100_lines_ms":window_ms,"window_median_ms":median(&window_ms),"incremental_convergence_ms":convergence_ms,"convergence_median_ms":median(&convergence_ms),"reparsed_lines":reparsed_lines,"regex_metrics":vel_regex::metrics()});
    println!("{}", serde_json::to_string_pretty(&result)?);
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/timings.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    std::fs::remove_dir_all(cache)?;
    Ok(())
}
