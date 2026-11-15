//! Content hashing plus warm index latency, including an original virtual fallback.
use std::{path::PathBuf, time::Instant};
use vel_syntax::{CacheStatus, SyntaxSet};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let packages = std::env::var_os("VEL_SYNTAX_ROOT")
        .map_or_else(|| root.join("third_party/sublime-packages"), PathBuf::from);
    let cache = std::env::temp_dir().join(format!("vel-roots-probe-{}", std::process::id()));
    let extra = [(
        "Packages/Default/Plain Text.sublime-syntax",
        "name: Plain Text\nfile_extensions: [txt]\nscope: text.plain\ncontexts: { main: [] }\n",
    )];
    let start = Instant::now();
    let (set, status) = SyntaxSet::load_cached_roots(&[&packages], &extra, &cache)?;
    let cold_ms = start.elapsed().as_secs_f64() * 1000.;
    assert_eq!(status, CacheStatus::Yaml);
    let count = set.syntaxes().len();
    assert!(set.find_syntax_by_name("Rust").is_some());
    assert!(set.find_syntax_by_path(extra[0].0).is_some());
    drop(set);
    let mut samples = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        let (set, status) = SyntaxSet::load_cached_roots(&[&packages], &extra, &cache)?;
        samples.push(start.elapsed().as_secs_f64() * 1000.);
        assert_eq!(status, CacheStatus::Hit);
        assert_eq!(set.syntaxes().len(), count);
        assert!(set.syntaxes().iter().all(|s| !s.contexts_are_loaded()));
        std::hint::black_box(set);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"syntaxes":count,"virtual_definitions":extra.len(),"cold_build_ms":cold_ms,"warm_ms_including_content_hashes":samples,"cache_files":std::fs::read_dir(&cache)?.count()})
        )?
    );
    std::fs::remove_dir_all(cache)?;
    Ok(())
}
