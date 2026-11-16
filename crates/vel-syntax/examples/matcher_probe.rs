//! Diagnostic comparison of bounded DFA settings over the same document.
use std::{path::PathBuf, sync::Arc, time::Instant};
use vel_regex::{MultiDfa, MultiDfaConfig};
use vel_syntax::{ParseState, SyntaxSet};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let source = std::fs::read_to_string(&args[0])?;
    let packages =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/sublime-packages");
    let cache = std::env::temp_dir().join("velocity-matcher-probe-syntax-cache");
    for kib in [128, 256, 512, 1024] {
        let backend = Arc::new(MultiDfa::new(MultiDfaConfig {
            context_cache_limit: kib * 1024,
            ..MultiDfaConfig::default()
        }));
        let (set, _) = SyntaxSet::load_cached(&packages, &cache)?;
        let set = Arc::new(set.with_matcher_backend(backend.clone()));
        let syntax = set.find_syntax_by_name(&args[1]).ok_or("syntax missing")?;
        let mut state = ParseState::new(set.clone(), syntax).with_line_cache_capacity(0);
        let start = Instant::now();
        for line in source.split_inclusive('\n') {
            std::hint::black_box(state.parse_line(line)?);
        }
        println!(
            "{}",
            serde_json::json!({"kib":kib,"seconds":start.elapsed().as_secs_f64(),"stats":backend.stats()})
        );
    }
    Ok(())
}
