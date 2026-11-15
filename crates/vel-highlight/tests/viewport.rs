use std::{sync::Arc, time::Duration};
use vel_buffer::{Change, ChangeId, ChangeSet};
use vel_color_scheme::ColorScheme;
use vel_highlight::{DocumentHighlighter, ViewportHighlighter, ViewportOptions};
use vel_syntax::{SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};
use vel_text::Text;
fn set() -> Arc<SyntaxSet> {
    let mut b = SyntaxSetBuilder::new();
    b.add(SyntaxDefinition::load_from_str("name: Huge\nscope: source.huge\ncontexts:\n  main:\n    - match: '/\\*'\n      push:\n        - meta_scope: comment.block.huge\n        - match: '\\*/'\n          pop: true\n    - match: '\\b(let|fn)\\b'\n      scope: keyword.huge\n", true, None).unwrap());
    Arc::new(b.build())
}
fn sparse(set: Arc<SyntaxSet>, text: Text, options: ViewportOptions) -> ViewportHighlighter {
    let syntax = set.syntaxes()[0].clone();
    ViewportHighlighter::new(
        text,
        ChangeId(0),
        set,
        &syntax,
        Arc::new(ColorScheme::velocity_dark()),
        options,
    )
}
fn fresh(set: Arc<SyntaxSet>, text: Text) -> DocumentHighlighter {
    let syntax = set.syntaxes()[0].clone();
    let mut e = DocumentHighlighter::new(
        text,
        ChangeId(0),
        set,
        &syntax,
        Arc::new(ColorScheme::velocity_dark()),
    );
    e.finish().unwrap();
    e
}
#[test]
fn distant_view_does_not_parse_prefix_or_allocate_per_document_line() {
    let set = set();
    let text = Text::from("let x\n".repeat(2_000_000));
    let mut engine = sparse(set, text, ViewportOptions::default());
    assert_eq!(engine.retained_lines(), 0);
    let batch = engine
        .request_lines(ChangeId(0), 1_900_000..1_900_010, Duration::from_secs(1))
        .unwrap();
    assert_eq!(batch.highlighted.len(), 10);
    assert!(!batch.complete);
    assert_eq!(engine.stats().parsed_lines, 10);
    assert_eq!(engine.progress().parsed_lines, 0);
    assert!(batch.scopes.scope_at(1_900_000, 0).is_none());
    assert!(engine.retained_lines() < 100);
    assert_eq!(engine.checkpoint_count(), 1);
    let fallback = engine
        .request_lines(ChangeId(0), 1000..1010, Duration::ZERO)
        .unwrap();
    assert!(fallback.highlighted.iter().all(|line| !line.exact));
}
#[test]
fn oversized_line_is_capped_and_real_state_is_restored_by_background_repair() {
    let set = set();
    let text = Text::from(format!("/*\n{}*/\nlet after\n", "x".repeat(50_000_000)));
    let mut engine = sparse(set.clone(), text.clone(), ViewportOptions::default());
    let batch = engine
        .request_lines(ChangeId(0), 0..4, Duration::from_secs(1))
        .unwrap();
    assert!(batch.highlighted[0].exact);
    assert!(!batch.highlighted[1].exact);
    assert!(!batch.highlighted[2].exact);
    assert_eq!(engine.stats().parsed_lines, 3); // Does not copy/parse the 50 MB line.
    let p = engine.advance(Duration::from_secs(1)).unwrap();
    assert_eq!(p.long_line, Some(1));
    engine.repair_long_line().unwrap();
    assert!(engine.advance(Duration::from_secs(1)).unwrap().complete());
    let repaired = engine
        .request_lines(ChangeId(0), 0..4, Duration::ZERO)
        .unwrap();
    assert!(repaired.complete);
    let expected = fresh(set, text)
        .request_lines(ChangeId(0), 0..4, Duration::ZERO)
        .unwrap();
    for (a, b) in repaired.highlighted.iter().zip(expected.highlighted) {
        assert_eq!(a.spans, b.spans);
    }
    assert!(repaired.scopes.scope_at(2, 0).is_some());
}
#[test]
fn evicted_checkpoints_edits_and_palette_changes_have_an_exact_repair_path() {
    let set = set();
    let mut text = Text::from("let x\n".repeat(1500));
    let options = ViewportOptions {
        checkpoint_interval: 20,
        max_checkpoints: 3,
        max_lookback_lines: 4,
        ..ViewportOptions::default()
    };
    let mut engine = sparse(set.clone(), text.clone(), options);
    while !engine.advance(Duration::from_millis(5)).unwrap().complete() {}
    assert!(engine.checkpoint_count() <= 3);
    assert!(engine.retained_lines() <= 768);
    assert!(
        !engine
            .request_lines(ChangeId(0), 40..50, Duration::from_secs(1))
            .unwrap()
            .complete
    );
    assert!(engine.repair_lines(ChangeId(0), 40..50).unwrap().complete);
    let at = text.line_to_byte(5);
    text.insert(at, "/*\n");
    engine
        .apply_change(
            text.clone(),
            ChangeId(1),
            &ChangeSet::new(vec![Change {
                old_range: at..at,
                new_len: 3,
            }]),
        )
        .unwrap();
    assert!(
        engine
            .request_lines(ChangeId(0), 0..1, Duration::ZERO)
            .is_err()
    );
    let actual = engine.repair_lines(ChangeId(1), 50..60).unwrap();
    let expected = fresh(set.clone(), text.clone())
        .request_lines(ChangeId(0), 50..60, Duration::ZERO)
        .unwrap();
    for (a, b) in actual.highlighted.iter().zip(expected.highlighted) {
        assert_eq!(a.spans, b.spans);
    }
    engine.set_color_scheme(Arc::new(ColorScheme::velocity_dark()));
    let batch = engine.repair_lines(ChangeId(1), 50..60).unwrap();
    assert_eq!(batch.scheme_revision, 1);
    assert!(batch.complete);
    assert!(engine.checkpoint_count() <= 3);
}
#[test]
fn sparse_branch_repairs_match_the_dense_engine() {
    let mut b = SyntaxSetBuilder::new();
    b.add(SyntaxDefinition::load_from_str("name: Branch\nscope: source.branch\nversion: 2\ncontexts:\n  main:\n    - match: '(?=a)'\n      branch_point: pick\n      branch: [first, second]\n    - match: 'z'\n      scope: keyword.z\n  first:\n    - meta_scope: meta.first\n    - match: 'a'\n    - match: 'x'\n      fail: pick\n    - match: 'b'\n      pop: true\n  second:\n    - meta_scope: meta.second\n    - match: 'a'\n    - match: 'x'\n      pop: true\n", true, None).unwrap());
    let set = Arc::new(b.build());
    let text = Text::from("a\nx\nz\n".repeat(100));
    let mut engine = sparse(set.clone(), text.clone(), ViewportOptions::default());
    let actual = engine.repair_lines(ChangeId(0), 0..300).unwrap();
    let expected = fresh(set, text)
        .request_lines(ChangeId(0), 0..300, Duration::ZERO)
        .unwrap();
    assert!(engine.stats().rewinds > 0);
    for (a, b) in actual.highlighted.iter().zip(expected.highlighted) {
        assert_eq!(a.spans, b.spans, "line {}", a.line);
    }
}
