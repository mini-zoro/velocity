use std::{fmt::Write, path::Path, sync::Arc, time::Duration};
use vel_buffer::ChangeId;
use vel_color_scheme::ColorScheme;
use vel_highlight::DocumentHighlighter;
use vel_syntax::SyntaxSet;
use vel_text::Text;

#[test]
fn full_highlight_retained_byte_budget_for_seven_mb_of_c() {
    let packages = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/sublime-packages");
    let set = Arc::new(SyntaxSet::load_directories(&[&packages.join("C++")]).unwrap());
    let mut source = String::with_capacity(7_000_000);
    let name = "source_identifier";
    for n in 0..47_000 {
        let f = &name[..1 + n % name.len()];
        let p = &name[..1 + (n / name.len()) % (name.len() - 1)];
        writeln!(
            source,
            "static int {f}_{n} (int p) {{\n    int {p}_{n} = (p + {n}) * 79; /* test */\n    return {p}_{n} < 1000 ? {p}_{n} * 3 + 1 : p - 2;\n}}"
        ).unwrap();
    }
    assert!(
        (6_800_000..=7_800_000).contains(&source.len()),
        "{}",
        source.len()
    );
    let text = Text::from(source.as_str());
    drop(source);
    let mut engine = DocumentHighlighter::new(
        text,
        ChangeId(0),
        set.clone(),
        set.find_syntax_by_name("C").unwrap(),
        Arc::new(ColorScheme::velocity_dark()),
    );
    engine.finish().unwrap();
    let end = engine.text().len_lines();
    assert_eq!(engine.parsed_through(), end);
    let batch = engine
        .request_lines(ChangeId(0), end - 8..end, Duration::ZERO)
        .unwrap();
    assert!(batch.complete && batch.scopes.complete());
    assert!(batch.highlighted.iter().any(|line| line.spans.len() >= 5));
    let retained = engine.memory_usage();
    let total = retained.records
        + retained.parser_states
        + retained.parser_memo
        + retained.operations
        + retained.interned_scopes
        + retained.scope_runs
        + retained.style_spans
        + retained.render_cache;
    eprintln!(
        "{} source bytes: {total} retained bytes",
        engine.text().len_bytes()
    );
    assert!(total <= 35_000_000, "{total} bytes: {retained:?}");
}
