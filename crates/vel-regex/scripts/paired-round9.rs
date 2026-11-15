//! Diagnostic: alternate original/current parser chunks while preserving each state.
use std::{path::PathBuf, sync::Arc, time::Instant};
fn original(state: &mut before::ParseState, lines: &[&str], out: &mut before::LineOps) -> f64 {
    let start = Instant::now();
    for line in lines {
        state.parse_line_into(line, out).unwrap();
        std::hint::black_box(&out);
    }
    start.elapsed().as_secs_f64()
}
fn current(state: &mut after::ParseState, lines: &[&str], out: &mut after::LineOps) -> f64 {
    let start = Instant::now();
    for line in lines {
        state.parse_line_into(line, out).unwrap();
        std::hint::black_box(&out);
    }
    start.elapsed().as_secs_f64()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let definitions = PathBuf::from(&args[0]);
    let data = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    let a = Arc::new(before::SyntaxSet::load_directories(&[&definitions])?);
    let b = Arc::new(after::SyntaxSet::load_directories(&[&definitions])?);
    let mut timings = Vec::new();
    let filter = std::env::var("VEL_BENCH_LANGUAGE").ok();
    for (language, syntax, file) in [
        ("javascript", "JavaScript", "spikes/data/javascript-10mb.js"),
        ("cpp", "C++", "prototypes/sregex/data/cpp-5mb.cpp"),
        ("html", "HTML", "prototypes/sregex/data/html-5mb.html"),
        ("python", "Python", "prototypes/sregex/data/python-5mb.py"),
    ] {
        if filter.as_ref().is_some_and(|filter| filter != language) {
            continue;
        }
        let source = std::fs::read_to_string(data.join(file))?;
        let lines = source.split_inclusive('\n').collect::<Vec<_>>();
        let mut left = Vec::new();
        let mut right = Vec::new();
        // The first whole-file pair warms every context and is not a sample.
        for run in 0..6 {
            let mut old =
                before::ParseState::new(a.clone(), a.find_syntax_by_name(syntax).unwrap())
                    .with_line_cache_capacity(0);
            let mut new = after::ParseState::new(b.clone(), b.find_syntax_by_name(syntax).unwrap())
                .with_line_cache_capacity(0);
            let mut old_out = before::LineOps::default();
            let mut new_out = after::LineOps::default();
            let (mut old_time, mut new_time) = (0.0, 0.0);
            for (chunk, lines) in lines.chunks(1024).enumerate() {
                if (chunk + run) % 2 == 0 {
                    old_time += original(&mut old, lines, &mut old_out);
                    new_time += current(&mut new, lines, &mut new_out);
                } else {
                    new_time += current(&mut new, lines, &mut new_out);
                    old_time += original(&mut old, lines, &mut old_out);
                }
            }
            if run > 0 {
                left.push(old_time);
                right.push(new_time);
                eprintln!(
                    "{language} paired {run}: original {old_time:.6}s current {new_time:.6}s"
                );
            }
        }
        timings.push(serde_json::json!({"language":language,"file":file,"bytes":source.len(),"before_seconds":left,"after_seconds":right}));
    }
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&serde_json::json!({
            "runs":5,"chunk_lines":1024,"case":"interleaved_original_current_uncached_parser","timings":timings
        }))?,
    )?;
    Ok(())
}
