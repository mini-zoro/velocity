//! Five-run production worker parse+style throughput; results go to the requested path.
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::ChangeId;
use vel_color_scheme::ColorScheme;
use vel_highlight::{DocumentHighlighter, HighlightService};
use vel_syntax::SyntaxSet;
use vel_text::Text;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let data =
        std::env::var_os("VEL_REGEX_FIXTURE_ROOT").map_or_else(|| root.clone(), PathBuf::from);
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("crates/vel-regex/round9-highlight.json"));
    let filter = std::env::var("VEL_BENCH_LANGUAGE").ok();
    let set = Arc::new(SyntaxSet::load_directories(&[
        root.join("third_party/sublime-packages")
    ])?);
    let scheme = Arc::new(ColorScheme::velocity_dark());
    let mut timings = Vec::new();
    for (language, name, path) in [
        ("rust", "Rust", "spikes/data/rust-10mb.rs"),
        ("javascript", "JavaScript", "spikes/data/javascript-10mb.js"),
        ("python", "Python", "prototypes/sregex/data/python-5mb.py"),
        ("cpp", "C++", "prototypes/sregex/data/cpp-5mb.cpp"),
        ("json", "JSON", "spikes/data/json-20mb.json"),
        ("markdown", "Markdown", "spikes/data/markdown-5mb.md"),
        ("yaml", "YAML", "prototypes/sregex/data/yaml-5mb.yaml"),
        ("html", "HTML", "prototypes/sregex/data/html-5mb.html"),
    ] {
        if filter.as_ref().is_some_and(|filter| filter != language) {
            continue;
        }
        let source = std::fs::read_to_string(data.join(path))?;
        let text = Text::from(source.as_str());
        let syntax = set.find_syntax_by_name(name).ok_or("missing syntax")?;
        // Compile and exercise every rule in the complete file before taking samples.
        let mut warm = DocumentHighlighter::new(
            text.clone(),
            ChangeId(0),
            set.clone(),
            syntax,
            scheme.clone(),
        );
        warm.finish()?;
        drop(warm);
        let mut seconds = Vec::new();
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
                if start.elapsed() > Duration::from_secs(180) {
                    return Err("highlight worker timeout".into());
                }
                std::thread::park_timeout(Duration::from_millis(1));
            }
            let elapsed = start.elapsed().as_secs_f64();
            seconds.push(elapsed);
            eprintln!("{language} worker {run}: {elapsed:.6}s");
            drop(service);
        }
        let mut sorted = seconds.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[2];
        let p95 = sorted[4];
        timings.push(serde_json::json!({"language":language,"bytes":source.len(),"file":path,"blake3":blake3::hash(source.as_bytes()).to_hex().to_string(),"seconds":seconds,"median_seconds":median,"p95_seconds":p95,"mb_per_second":source.len()as f64/1e6/median,"target_mb_per_second":2.5}));
    }
    std::fs::write(
        &output,
        serde_json::to_vec_pretty(
            &serde_json::json!({"runs":5,"case":"production_background_parse_style_checkpoints","timings":timings}),
        )?,
    )?;
    println!("{}", output.display());
    Ok(())
}
