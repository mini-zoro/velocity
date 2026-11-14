//! Full-parser backend differential and five-run throughput evidence, without sysctl.
use serde_json::json;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use vel_regex::{MultiDfa, MultiDfaConfig, OnigBackend, PatternSet, SearchCache};
use vel_syntax::{LineOps, ParseState, SyntaxSet};
#[path = "regex-bench/profile.rs"]
mod profile;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn scan_context(matcher: &PatternSet, text: &str) -> (f64, usize) {
    let mut cache = SearchCache::default();
    let mut queries = 0;
    let time = Instant::now();
    for line in text.split_inclusive('\n') {
        cache.reset_line();
        let mut at = 0;
        loop {
            queries += 1;
            let Some(hit) = matcher.find_earliest(line, at, &mut cache) else {
                break;
            };
            if hit.end > at {
                at = hit.end;
            } else if let Some(ch) = line[at..].chars().next() {
                at += ch.len_utf8();
            } else {
                break;
            }
            std::hint::black_box(hit);
        }
    }
    (time.elapsed().as_secs_f64(), queries)
}
fn compare_context(a: &PatternSet, b: &PatternSet, text: &str) {
    let mut left = SearchCache::default();
    let mut right = SearchCache::default();
    for line in text.split_inclusive('\n') {
        left.reset_line();
        right.reset_line();
        let mut at = 0;
        loop {
            let expected = a.find_earliest(line, at, &mut left);
            assert_eq!(
                b.find_earliest(line, at, &mut right),
                expected,
                "static context at {at}: {line:?}"
            );
            let Some(hit) = expected else {
                break;
            };
            if hit.end > at {
                at = hit.end;
            } else if let Some(ch) = line[at..].chars().next() {
                at += ch.len_utf8();
            } else {
                break;
            }
        }
    }
}
fn compare(
    a: Arc<SyntaxSet>,
    b: Arc<SyntaxSet>,
    syntax: &str,
    text: &str,
    file: &str,
) -> Result<serde_json::Value> {
    let mut left = ParseState::new(
        a.clone(),
        a.find_syntax_by_path(syntax)
            .ok_or("reference syntax missing")?,
    );
    let mut right = ParseState::new(
        b.clone(),
        b.find_syntax_by_path(syntax)
            .ok_or("candidate syntax missing")?,
    );
    left = left.with_line_cache_capacity(0);
    right = right.with_line_cache_capacity(0);
    let mut rewinds = 0;
    let mut operations = 0;
    let mut lines = 0;
    for (row, line) in text.split_inclusive('\n').enumerate() {
        let expected = left.parse_line(line)?;
        let actual = right.parse_line(line)?;
        if expected != actual {
            return Err(format!(
                "{file}:{}: backend divergence\nline={line:?}\nonig={expected:?}\nmulti={actual:?}",
                row + 1
            )
            .into());
        }
        lines += 1;
        operations += actual.ops.len();
        rewinds += usize::from(actual.rewind.is_some());
    }
    Ok(
        json!({"file":file,"bytes":text.len(),"lines":lines,"operations":operations,"rewinds":rewinds,"divergences":0}),
    )
}
fn parse(set: &Arc<SyntaxSet>, syntax: &str, text: &str) -> Result<f64> {
    let mut state = ParseState::new(
        set.clone(),
        set.find_syntax_by_name(syntax)
            .ok_or("benchmark syntax missing")?,
    )
    .with_line_cache_capacity(0);
    let mut output = LineOps::default();
    let start = Instant::now();
    for line in text.split_inclusive('\n') {
        state.parse_line_into(line, &mut output)?;
        std::hint::black_box(&output);
    }
    std::hint::black_box(output);
    Ok(start.elapsed().as_secs_f64())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let cold = args.iter().any(|a| a == "--cold-only");
    let micro = args.iter().any(|a| a == "--contexts-only");
    let diff = !cold && !micro && !args.iter().any(|a| a == "--bench-only");
    let bench = !cold && !micro && !args.iter().any(|a| a == "--diff-only");
    let quick = args.iter().any(|a| a == "--quick");
    let file_filter = args
        .iter()
        .position(|a| a == "--file")
        .and_then(|i| args.get(i + 1));
    let filter = args
        .iter()
        .position(|a| a == "--language")
        .and_then(|i| args.get(i + 1));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let packages = root.join("third_party/sublime-packages");
    let onig_profile = Arc::new(profile::Backend::new(Arc::new(OnigBackend::default())));
    let profile = args.iter().any(|a| a == "--profile");
    let onig = Arc::new(
        SyntaxSet::load_directories(&[&packages])?.with_matcher_backend(if profile {
            onig_profile.clone()
        } else {
            Arc::new(OnigBackend::default())
        }),
    );
    let backend = Arc::new(MultiDfa::new(MultiDfaConfig {
        adaptive: args.iter().any(|a| a == "--adaptive"),
        verify_matches: args.iter().any(|a| a == "--verify-matches"),
        ..MultiDfaConfig::default()
    }));
    let multi_profile = Arc::new(profile::Backend::new(backend.clone()));
    let multi = Arc::new((*onig).clone().with_matcher_backend(if profile {
        multi_profile.clone()
    } else {
        backend.clone()
    }));
    let mut official = Vec::new();
    if diff && filter.is_none() && !args.iter().any(|arg| arg == "--large-only") {
        let mut files: Vec<_> = walkdir::WalkDir::new(&packages)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_type().is_file()
                    && e.file_name().to_string_lossy().starts_with("syntax_test_")
            })
            .map(|e| e.into_path())
            .collect();
        files.sort();
        assert_eq!(files.len(), 242);
        for file in files {
            if file_filter.is_some_and(|f| !file.to_string_lossy().contains(f)) {
                continue;
            }
            let text = std::fs::read_to_string(&file)?.replace('\r', "");
            let syntax = text
                .lines()
                .next()
                .and_then(|s| s.split('"').nth(1))
                .ok_or("syntax header missing")?;
            let name = file.strip_prefix(&packages)?.to_string_lossy();
            official.push(compare(onig.clone(), multi.clone(), syntax, &text, &name)?);
            eprintln!("diff {name}: {} lines", official.last().unwrap()["lines"]);
        }
    }
    let fixtures = [
        ("rust", "Rust", "spikes/data/rust-10mb.rs"),
        ("javascript", "JavaScript", "spikes/data/javascript-10mb.js"),
        ("python", "Python", "prototypes/sregex/data/python-5mb.py"),
        ("cpp", "C++", "prototypes/sregex/data/cpp-5mb.cpp"),
        ("json", "JSON", "spikes/data/json-20mb.json"),
        ("markdown", "Markdown", "spikes/data/markdown-5mb.md"),
        ("yaml", "YAML", "prototypes/sregex/data/yaml-5mb.yaml"),
        ("html", "HTML", "prototypes/sregex/data/html-5mb.html"),
    ];
    let mut large_diff = Vec::new();
    let mut timings = Vec::new();
    for (language, syntax, path) in fixtures {
        if micro && !matches!(language, "rust" | "cpp") {
            continue;
        }
        if filter.is_some_and(|f| f != language) {
            continue;
        }
        let mut text = if cold {
            let mut bytes = Vec::new();
            std::fs::File::open(root.join(path))?
                .take(66_048)
                .read_to_end(&mut bytes)?;
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            std::fs::read_to_string(root.join(path))?
        };
        let limit = if cold { 65_536 } else { 200_000 };
        if (quick || cold || micro) && text.len() > limit {
            let mut boundary = limit;
            while !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            let end = text[..boundary]
                .rfind('\n')
                .ok_or("no newline in fixture")?
                + 1;
            text.truncate(end);
        }
        if micro {
            let name = if language == "rust" {
                "statements"
            } else {
                "global"
            };
            let a = onig
                .context_matcher(
                    onig.find_syntax_by_name(syntax).ok_or("reference syntax")?,
                    name,
                )?
                .ok_or("reference context")?;
            let b = multi
                .context_matcher(
                    multi
                        .find_syntax_by_name(syntax)
                        .ok_or("candidate syntax")?,
                    name,
                )?
                .ok_or("candidate context")?;
            compare_context(&a, &b, &text);
            scan_context(&a, &text);
            scan_context(&b, &text);
            let mut left = Vec::new();
            let mut right = Vec::new();
            let mut queries = Vec::new();
            for run in 0..5 {
                let (x, y) = if run % 2 == 0 {
                    (scan_context(&a, &text), scan_context(&b, &text))
                } else {
                    let y = scan_context(&b, &text);
                    (scan_context(&a, &text), y)
                };
                assert_eq!(x.1, y.1);
                left.push(x.0);
                right.push(y.0);
                queries.push(x.1);
            }
            timings.push(json!({"language":language,"context":name,"bytes":text.len(),"queries":queries,"onig_seconds":left,"multi_seconds":right}));
        }
        if cold {
            let before = backend.stats().compile_ms.len();
            parse(&multi, syntax, &text)?;
            let stats = backend.stats();
            timings.push(json!({"language":language,"bytes":text.len(),"compile_ms":&stats.compile_ms[before..]}));
        }
        if diff {
            let syntax_path = &onig
                .find_syntax_by_name(syntax)
                .ok_or("fixture syntax missing")?
                .path;
            large_diff.push(compare(
                onig.clone(),
                multi.clone(),
                syntax_path,
                &text,
                path,
            )?);
            eprintln!("diff {language}: {} bytes", text.len());
        }
        if bench {
            parse(&onig, syntax, &text)?;
            parse(&multi, syntax, &text)?;
            let mut reference = Vec::new();
            let mut candidate = Vec::new();
            for run in 0..5 {
                onig_profile
                    .run
                    .store(run, std::sync::atomic::Ordering::Relaxed);
                multi_profile
                    .run
                    .store(run, std::sync::atomic::Ordering::Relaxed);
                if run % 2 == 0 {
                    reference.push(parse(&onig, syntax, &text)?);
                    candidate.push(parse(&multi, syntax, &text)?);
                } else {
                    candidate.push(parse(&multi, syntax, &text)?);
                    reference.push(parse(&onig, syntax, &text)?);
                }
                eprintln!(
                    "{language} run {run}: onig {:.4}s multi {:.4}s",
                    reference[run], candidate[run]
                );
            }
            timings.push(json!({"language":language,"bytes":text.len(),"lines":text.lines().count(),"onig_seconds":reference,"multi_seconds":candidate}));
            onig_profile
                .run
                .store(usize::MAX, std::sync::atomic::Ordering::Relaxed);
            multi_profile
                .run
                .store(usize::MAX, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let result = json!({"quick":quick,"micro_contexts":micro,"instrumented":profile,"syntaxes":onig.syntaxes().len(),"official":official,"large_diff":large_diff,"timings":timings,"multi":backend.stats(),"regex":vel_regex::metrics(),"contexts": if profile {json!({"onig":onig_profile.results(),"multi":multi_profile.results()})} else {serde_json::Value::Null}});
    let output = args
        .iter()
        .position(|a| a == "--output")
        .and_then(|i| args.get(i + 1))
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/regex-perf.json"),
            PathBuf::from,
        );
    std::fs::write(Path::new(&output), serde_json::to_vec_pretty(&result)?)?;
    println!("{}", output.display());
    Ok(())
}
