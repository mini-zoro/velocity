//! Automatic CPU evidence; never creates a window or GPU device.
use std::{
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_search::{
    fuzzy::{Candidate, Matcher, Worker},
    text::{Options, Search},
};
use vel_text::Text;

fn ms(at: Instant) -> f64 {
    at.elapsed().as_secs_f64() * 1000.
}
fn quantiles(mut values: Vec<f64>) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    serde_json::json!({"samples":values.len(),"p50_ms":values[(values.len() as f64 * 0.5).ceil() as usize - 1],"p95_ms":values[(values.len() as f64 * 0.95).ceil() as usize - 1],"max_ms":values.last()})
}
fn main() {
    let files = Arc::new(
        (0..100_000)
            .map(|i| {
                Candidate::new(format!(
                    "{}/project_{:05}/{}/{}_{:05}.{}",
                    ["src", "tests", "docs", "packages"][i % 4],
                    i / 40,
                    ["components", "services", "commands", "internal"][i % 4],
                    [
                        "main",
                        "MapReader",
                        "index",
                        "README",
                        "search",
                        "parser",
                        "test",
                        "config"
                    ][i % 8],
                    i,
                    ["rs", "ts", "md", "json"][i % 4]
                ))
            })
            .collect::<Vec<_>>(),
    );
    let sequence = [
        "m", "ma", "map", "mapr", "mapre", "map", "m", "s", "sr", "src", "read", "readme", "zzzz",
        "", "p", "pa", "par", "parser",
    ];
    let mut timings = Vec::new();
    let mut by_query = std::collections::BTreeMap::<&str, Vec<f64>>::new();
    for _ in 0..10 {
        let mut matcher = Matcher::default();
        for query in sequence {
            let at = Instant::now();
            black_box(matcher.rank(&files, query, 100));
            let elapsed = ms(at);
            timings.push(elapsed);
            by_query.entry(query).or_default().push(elapsed);
        }
    }
    println!(
        "{}",
        serde_json::json!({"kind":"fuzzy_cpu","files":files.len(),"top_k":100,"budget_ms":10,"incremental_and_reset":quantiles(timings),"per_query":by_query.into_iter().map(|(q, v)| (q, quantiles(v))).collect::<std::collections::BTreeMap<_,_>>(),"gui":false,"gpu":false})
    );
    let worker = Worker::new();
    worker.set_items(files);
    let mut end_to_end = Vec::new();
    for _ in 0..5 {
        for query in sequence {
            let at = Instant::now();
            worker.query(query, 100);
            loop {
                if worker.poll().is_some() {
                    end_to_end.push(ms(at));
                    break;
                }
                assert!(at.elapsed() < Duration::from_secs(10));
                std::thread::yield_now();
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"kind":"fuzzy_worker","queue_plus_scoring":quantiles(end_to_end),"budget_ms":10,"gui":false,"gpu":false})
    );
    let mut block = "0123456789abcdef ".repeat(15);
    block.push_str("needle\n");
    let mut source = block.repeat(100_000_000 / block.len() + 1);
    source.truncate(100_000_000);
    let text = Text::from(source.as_str());
    drop(source);
    let search = Search::new(
        "needle",
        Options {
            case_sensitive: true,
            ..Options::default()
        },
    )
    .expect("literal");
    let mut full = Vec::new();
    let mut visible = Vec::new();
    let mut count = 0;
    for _ in 0..5 {
        let at = Instant::now();
        let hits = search.find(&text, &[]);
        full.push(ms(at));
        count = hits.len();
        black_box(hits);
    }
    for i in 0..100 {
        let start = text.line_to_byte(i * 300);
        let end = text.line_to_byte(i * 300 + 60);
        let at = Instant::now();
        black_box(search.find(&text, std::slice::from_ref(&(start..end))));
        visible.push(ms(at));
    }
    println!(
        "{}",
        serde_json::json!({"kind":"rope_find","bytes":text.len_bytes(),"matches":count,"all_matches":quantiles(full),"all_budget_ms":200,"viewport_60_lines":quantiles(visible),"viewport_budget_ms":5,"full_buffer_copy":false,"gui":false,"gpu":false})
    );
}
