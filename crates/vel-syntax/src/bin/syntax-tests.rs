//! Executes official package scope assertions and emits reproducible JSON evidence.
use std::path::PathBuf;
use vel_syntax::{SyntaxSet, syntest};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let filter = args
        .iter()
        .find(|s| !s.starts_with('-'))
        .map(String::as_str);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/sublime-packages");
    let now = std::time::Instant::now();
    let set = SyntaxSet::load_directories(&[&root])?;
    eprintln!(
        "Loaded {} syntaxes in {:.1} ms ({} warnings)",
        set.syntaxes().len(),
        now.elapsed().as_secs_f64() * 1000.,
        set.warnings().len()
    );
    let result = syntest::run(&set, &root, filter);
    for (name, c) in &result.syntaxes {
        if c.passed + c.failed + c.skipped > 0 {
            println!(
                "{name}: {} passed / {} failed / {} skipped ({:.3}%)",
                c.passed,
                c.failed,
                c.skipped,
                c.rate() * 100.
            );
        }
    }
    println!(
        "TOTAL: {} syntaxes, {} files, {} passed / {} failed / {} skipped ({:.3}%)",
        result.loaded,
        result.test_files,
        result.overall.passed,
        result.overall.failed,
        result.overall.skipped,
        result.overall.rate() * 100.
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/last-run.json");
    std::fs::write(path, serde_json::to_vec_pretty(&result)?)?;
    if args.iter().any(|s| s == "--update-baseline") {
        if filter.is_some() {
            return Err("baseline updates require the complete corpus".into());
        }
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/baseline.json");
        let mut improved = true;
        if let Ok(bytes) = std::fs::read(&path) {
            let old: syntest::SuiteResult = serde_json::from_slice(&bytes)?;
            let metadata_baseline = old.scope_counts.passed + old.scope_counts.failed > 0;
            if result.overall.passed < old.overall.passed
                || result.overall.skipped > old.overall.skipped
            {
                return Err("pass-count regression or new skips; refusing baseline update".into());
            }
            for expected in &old.files {
                let actual = result
                    .files
                    .iter()
                    .find(|f| f.file == expected.file)
                    .ok_or("test file disappeared")?;
                let previous_scope = if metadata_baseline {
                    &expected.scope_counts
                } else {
                    &expected.counts
                };
                if actual.scope_counts.passed < previous_scope.passed
                    || actual.scope_counts.failed > previous_scope.failed
                {
                    return Err(format!("{}: scope assertions regressed", expected.file).into());
                }
                if metadata_baseline
                    && (actual.symbol_counts.passed < expected.symbol_counts.passed
                        || actual.symbol_counts.failed > expected.symbol_counts.failed
                        || actual.reindent_counts.passed < expected.reindent_counts.passed
                        || actual.reindent_counts.failed > expected.reindent_counts.failed)
                {
                    return Err(format!("{}: metadata assertions regressed", expected.file).into());
                }
            }
            for (name, expected) in &old.syntaxes {
                let actual = result.syntaxes.get(name).ok_or("syntax disappeared")?;
                if actual.passed < expected.passed
                    || (metadata_baseline && actual.failed > expected.failed)
                    || actual.skipped > expected.skipped
                {
                    return Err(format!(
                        "{name} regressed or changed skips; refusing baseline update"
                    )
                    .into());
                }
            }
            improved = result.overall.passed > old.overall.passed
                || result.overall.failed < old.overall.failed
                || result.overall.skipped < old.overall.skipped;
            if improved {
                std::fs::write(
                    std::env::temp_dir().join(format!(
                        "velocity-syntax-baseline-{}.previous.json",
                        std::process::id()
                    )),
                    bytes,
                )?;
            }
        }
        if improved {
            let temporary = path.with_extension("json.tmp");
            std::fs::write(&temporary, serde_json::to_vec_pretty(&result)?)?;
            std::fs::rename(temporary, path)?;
        } else {
            eprintln!("Baseline unchanged: assertion counts have not improved.");
        }
    }
    if result.files.iter().any(|f| f.error.is_some()) {
        std::process::exit(1);
    }
    Ok(())
}
