//! Process-isolated paired dense/sparse construction and viewport probes.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::ChangeId;
use vel_color_scheme::ColorScheme;
use vel_highlight::{DocumentHighlighter, ViewportHighlighter, ViewportOptions};
use vel_syntax::{SyntaxDefinition, SyntaxSetBuilder};
use vel_text::Text;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let operation = &args[1];
    let side = &args[2];
    let mut builder = SyntaxSetBuilder::new();
    builder.add(SyntaxDefinition::load_from_str("name: Probe\nscope: source.probe\ncontexts:\n  main:\n    - match: 'x$'\n      scope: keyword.probe\n", true, None).unwrap());
    let set = Arc::new(builder.build());
    let syntax = set.syntaxes()[0].clone();
    let text = if operation == "long-viewport" {
        Text::from("a".repeat(50_000_000))
    } else {
        Text::from("a\n".repeat(10_000_000))
    };
    let lines = if operation == "long-viewport" {
        0..1
    } else {
        9_000_000..9_000_010
    };
    let scheme = Arc::new(ColorScheme::velocity_dark());
    let start = Instant::now();
    if side == "A" {
        let mut engine = DocumentHighlighter::new(text, ChangeId(0), set, &syntax, scheme);
        let init = start.elapsed().as_nanos();
        let start = Instant::now();
        let batch = engine
            .request_lines(ChangeId(0), lines, Duration::from_millis(2))
            .unwrap();
        println!(
            "{}",
            serde_json::json!({"operation":operation,"side":side,"init_ns":init,"ns":[start.elapsed().as_nanos()],"parsed_lines":engine.stats().parsed_lines,"exact":batch.complete})
        );
    } else {
        let mut engine = ViewportHighlighter::new(
            text,
            ChangeId(0),
            set,
            &syntax,
            scheme,
            ViewportOptions::default(),
        );
        let init = start.elapsed().as_nanos();
        let start = Instant::now();
        let batch = engine
            .request_lines(ChangeId(0), lines, Duration::from_millis(2))
            .unwrap();
        println!(
            "{}",
            serde_json::json!({"operation":operation,"side":side,"init_ns":init,"ns":[start.elapsed().as_nanos()],"parsed_lines":engine.stats().parsed_lines,"retained_lines":engine.retained_lines(),"exact":batch.complete})
        );
    }
}
