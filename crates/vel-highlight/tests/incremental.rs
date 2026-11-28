use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::{Change, ChangeId, ChangeSet};
use vel_color_scheme::ColorScheme;
use vel_highlight::{DocumentHighlighter, HighlightService, StyleSpan, detect_syntax};
use vel_syntax::{SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};
use vel_text::{Edit, Text};

/// Hang guard: stops a wait that would otherwise never end. It is not a latency
/// bound; the test checks the condition it waits for.
const HANG_GUARD: Duration = Duration::from_secs(120);

fn set() -> Arc<SyntaxSet> {
    let mut b = SyntaxSetBuilder::new();
    b.add_with_path(SyntaxDefinition::load_from_str("name: Fixture\nfile_extensions: [fix]\nhidden_file_extensions: [fhidden]\nfirst_line_match: '^#!fixture'\nscope: source.fixture\nversion: 2\ncontexts:\n  main:\n    - match: '/\\*'\n      push:\n        - meta_scope: comment.block.fixture\n        - match: '\\*/'\n          pop: true\n    - match: '\\b(fn|let)\\b'\n      scope: keyword.fixture\n    - match: '[0-9]+'\n      scope: constant.numeric.fixture\n",true,None).unwrap(),"Packages/Fixture/Fixture.sublime-syntax");
    Arc::new(b.build())
}
fn engine(set: Arc<SyntaxSet>, text: Text, version: u64) -> DocumentHighlighter {
    let syntax = set.syntaxes()[0].clone();
    DocumentHighlighter::new(
        text,
        ChangeId(version),
        set,
        &syntax,
        Arc::new(ColorScheme::velocity_dark()),
    )
}
fn spans(engine: &mut DocumentHighlighter) -> Vec<Vec<StyleSpan>> {
    engine.finish().unwrap();
    engine
        .request_lines(
            engine.version(),
            0..engine.text().len_lines(),
            Duration::ZERO,
        )
        .unwrap()
        .highlighted
        .iter()
        .map(|l| l.spans.to_vec())
        .collect()
}

#[test]
fn edit_and_palette_bursts_converge_to_the_latest_snapshot() {
    let set = set();
    let mut text = Text::from("fn");
    let mut worker = HighlightService::new(engine(set.clone(), text.clone(), 0)).unwrap();
    let schemes = [
        Arc::new(ColorScheme::velocity_light()),
        Arc::new(ColorScheme::velocity_dark()),
    ];
    for version in 1..=257 {
        let next = Text::from(format!("fn {}\n", "let ".repeat(version as usize)).as_str());
        let changes = ChangeSet::new(vec![Change {
            old_range: 0..text.len_bytes(),
            new_len: next.len_bytes(),
        }]);
        worker
            .apply_change(next.clone(), ChangeId(version), changes)
            .unwrap();
        worker
            .set_color_scheme(schemes[version as usize % 2].clone())
            .unwrap();
        text = next;
    }
    let wanted = ChangeId(257);
    let expected = spans(&mut engine(set, text.clone(), wanted.0));
    let until = Instant::now() + HANG_GUARD;
    loop {
        let batch = worker.request_lines(wanted, 0..text.len_lines()).unwrap();
        if batch.complete {
            assert_eq!(batch.scheme_revision, wanted.0);
            assert_eq!(batch.scopes.text, text);
            assert_eq!(
                batch
                    .highlighted
                    .iter()
                    .map(|line| line.spans.to_vec())
                    .collect::<Vec<_>>(),
                expected
            );
            break;
        }
        if let Some(Err(error)) = worker.try_recv() {
            panic!("{error}");
        }
        assert!(Instant::now() < until, "latest snapshot did not converge");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn edits_converge_and_match_fresh_including_utf8_and_newlines() {
    let set = set();
    let mut text = Text::from("fn 世界() {\n    let café = 123;\n}\n".repeat(90).as_str());
    let mut actual = engine(set.clone(), text.clone(), 0);
    actual.finish().unwrap();
    let mut seed = 91u64;
    for version in 1..=100 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let at = text.floor_char_boundary((seed as usize) % text.len_bytes());
        let end = if version % 3 == 0 {
            text.ceil_char_boundary((at + 3).min(text.len_bytes()))
        } else {
            at
        };
        let replacement = match version % 6 {
            0 => "\n",
            1 => "x",
            2 => "/*",
            3 => "*/",
            4 => "🦀",
            _ => "let",
        };
        let changes = ChangeSet::new(vec![Change {
            old_range: at..end,
            new_len: replacement.len(),
        }]);
        text.apply_edits(&[Edit {
            range: at..end,
            text: replacement,
        }]);
        actual
            .apply_change(text.clone(), ChangeId(version), &changes)
            .unwrap();
        assert_eq!(
            spans(&mut actual),
            spans(&mut engine(set.clone(), text.clone(), version)),
            "version {version}"
        );
    }
}
#[test]
fn one_character_edit_only_parses_one_line_and_queued_edits_do_not_skip_dirty_regions() {
    let set = set();
    let mut text = Text::from("let value = 123;\n".repeat(10_000).as_str());
    let mut actual = engine(set.clone(), text.clone(), 0);
    actual.finish().unwrap();
    let at = text.line_to_byte(5000) + 4;
    let changes = ChangeSet::new(vec![Change {
        old_range: at..at + 1,
        new_len: 1,
    }]);
    text.replace(at..at + 1, "x");
    actual
        .apply_change(text.clone(), ChangeId(1), &changes)
        .unwrap();
    let visible = actual
        .request_lines(ChangeId(1), 4950..5050, Duration::from_millis(2))
        .unwrap();
    assert!(visible.complete);
    assert_eq!(actual.stats().parsed_lines, 1);
    assert_eq!(actual.stats().convergences, 1);
    for (version, row, insert) in [(2, 8000, "/*"), (3, 2000, "/*"), (4, 9000, "*/")] {
        let at = text.line_to_byte(row);
        let changes = ChangeSet::new(vec![Change {
            old_range: at..at,
            new_len: insert.len(),
        }]);
        text.insert(at, insert);
        actual
            .apply_change(text.clone(), ChangeId(version), &changes)
            .unwrap();
    }
    assert_eq!(spans(&mut actual), spans(&mut engine(set, text, 4)));
}

#[test]
fn sparse_checkpoints_relocate_after_a_newline_and_repair_a_later_branch() {
    let yaml = r#"
name: Branch
scope: source.fixture
version: 2
contexts:
  main:
    - match: '(?=a)'
      branch_point: reading
      branch: [first, second]
  first:
    - meta_scope: first.fixture
    - match: z
      fail: reading
    - match: t
      pop: true
  second:
    - meta_scope: second.fixture
    - match: z
      pop: true
"#;
    let mut builder = SyntaxSetBuilder::new();
    builder.add(SyntaxDefinition::load_from_str(yaml, true, None).unwrap());
    let set = Arc::new(builder.build());
    let scheme = Arc::new(
        ColorScheme::parse(
            r##"{"rules":[
        {"scope":"first","foreground":"#f00"},{"scope":"second","foreground":["#00f","#0f0"]}
    ]}"##,
        )
        .unwrap(),
    );
    let make = |text, version| {
        DocumentHighlighter::new(
            text,
            ChangeId(version),
            set.clone(),
            &set.syntaxes()[0],
            scheme.clone(),
        )
    };
    let mut text = Text::from(
        format!(
            "{}a\n{}t\n{}",
            "x\n".repeat(16_378),
            "xx é\n".repeat(40),
            "x\n".repeat(256)
        )
        .as_str(),
    );
    let mut incremental = make(text.clone(), 0);
    incremental.finish().unwrap();

    let newline = text.line_to_byte(16);
    text.insert(newline, "\n");
    incremental
        .apply_change(
            text.clone(),
            ChangeId(1),
            &ChangeSet::new(vec![Change {
                old_range: newline..newline,
                new_len: 1,
            }]),
        )
        .unwrap();
    incremental.finish().unwrap();
    assert!(incremental.stats().parsed_lines <= 32);
    assert_eq!(incremental.stats().convergences, 1);

    let branch_end = 16_420;
    let at = text.line_to_byte(branch_end);
    assert_eq!(text.byte_at(at), b't');
    text.replace(at..at + 1, "z");
    incremental
        .apply_change(
            text.clone(),
            ChangeId(2),
            &ChangeSet::new(vec![Change {
                old_range: at..at + 1,
                new_len: 1,
            }]),
        )
        .unwrap();
    incremental.finish().unwrap();
    assert_eq!(incremental.stats().rewind_from, Some(16_379));
    let rows = 16_370..16_430;
    let mut fresh = make(text.clone(), 2);
    fresh.finish().unwrap();
    // Recolor from compact operations; hashed token spans must survive expansion.
    for (actual, expected) in [
        (
            incremental
                .request_lines(ChangeId(2), rows.clone(), Duration::ZERO)
                .unwrap(),
            fresh
                .request_lines(ChangeId(2), rows.clone(), Duration::ZERO)
                .unwrap(),
        ),
        {
            let other = Arc::new(
                ColorScheme::parse(
                    r##"{"rules":[
                {"scope":"second","foreground":["#f00","#ff0"]}
            ]}"##,
                )
                .unwrap(),
            );
            incremental.set_color_scheme(other.clone()).unwrap();
            fresh.set_color_scheme(other).unwrap();
            (
                incremental
                    .request_lines(ChangeId(2), rows.clone(), Duration::ZERO)
                    .unwrap(),
                fresh
                    .request_lines(ChangeId(2), rows.clone(), Duration::ZERO)
                    .unwrap(),
            )
        },
    ] {
        assert!(actual.complete && expected.complete);
        for (a, b) in actual.highlighted.iter().zip(&expected.highlighted) {
            assert_eq!(a.spans, b.spans);
        }
        for point in text.line_to_byte(rows.start)..text.line_to_byte(rows.end) {
            use vel_highlight::ScopeProvider;
            assert_eq!(
                actual.scopes.scopes_at(point),
                expected.scopes.scopes_at(point)
            );
        }
    }
}
#[test]
fn multi_change_transaction_and_invalid_versions() {
    let set = set();
    let mut text = Text::from("fn a\nlet b=1\nfn c\nlet d=2\n");
    let mut actual = engine(set.clone(), text.clone(), 10);
    actual.finish().unwrap();
    let edits = [
        Edit {
            range: 1..2,
            text: "n\n/*",
        },
        Edit {
            range: 18..19,
            text: "*/\n",
        },
    ];
    let changes = ChangeSet::new(
        edits
            .iter()
            .map(|e| Change {
                old_range: e.range.clone(),
                new_len: e.text.len(),
            })
            .collect(),
    );
    text.apply_edits(&edits);
    actual
        .apply_change(text.clone(), ChangeId(11), &changes)
        .unwrap();
    assert_eq!(spans(&mut actual), spans(&mut engine(set, text, 11)));
    assert!(
        actual
            .request_lines(ChangeId(10), 0..2, Duration::ZERO)
            .is_err()
    );
    assert!(
        actual
            .apply_change(Text::from("bad"), ChangeId(12), &ChangeSet::default())
            .is_err()
    );
    assert_eq!(actual.version(), ChangeId(11));
}
#[test]
fn budget_fallback_and_detection() {
    let set = set();
    assert_eq!(
        detect_syntax(&set, "some.fhidden", "", None).unwrap().name,
        "Fixture"
    );
    assert!(detect_syntax(&set, "unknown", "#!fixture", None).is_some());
    assert!(detect_syntax(&set, "unknown", "", Some("Fixture")).is_some());
    let mut actual = engine(set, Text::from("let 🦀 = 12\n"), 1);
    let result = actual
        .request_lines(ChangeId(1), 0..100, Duration::ZERO)
        .unwrap();
    assert!(!result.complete);
    assert_eq!(result.lines, 0..2);
    assert!(!result.highlighted[0].exact);
    assert_eq!(result.highlighted[0].spans[0].range, 0..14);
}
#[test]
fn cross_line_backtracking_repairs_previously_returned_styles() {
    let mut builder = SyntaxSetBuilder::new();
    builder.add_with_path(SyntaxDefinition::load_from_str("name: Branch\nscope: source.fixture\nversion: 2\ncontexts:\n  main:\n    - match: '(?=a)'\n      branch_point: reading\n      branch: [first, second]\n  first:\n    - meta_scope: first.fixture\n    - match: z\n      fail: reading\n  second:\n    - meta_scope: second.fixture\n    - match: z\n      pop: true\n",true,None).unwrap(),"Branch.sublime-syntax");
    let set = Arc::new(builder.build());
    let scheme=Arc::new(ColorScheme::parse(r#"{"globals":{"foreground":"black","background":"white"},"rules":[{"scope":"first","foreground":"red"},{"scope":"second","foreground":"blue"}]}"#).unwrap());
    let mut engine = DocumentHighlighter::new(
        Text::from("a\nz\n"),
        ChangeId(1),
        set.clone(),
        &set.syntaxes()[0],
        scheme,
    );
    engine.finish().unwrap();
    let batch = engine
        .request_lines(ChangeId(1), 0..2, Duration::ZERO)
        .unwrap();
    assert_eq!(
        batch.highlighted[0].spans[0].style.foreground,
        vel_color_scheme::Color::rgb(0, 0, 255)
    );
    assert_eq!(engine.stats().rewind_from, Some(0));
    let mut after = engine.text().clone();
    after.insert(0, "\n");
    engine
        .apply_change(
            after,
            ChangeId(2),
            &ChangeSet::new(vec![Change {
                old_range: 0..0,
                new_len: 1,
            }]),
        )
        .unwrap();
    engine.finish().unwrap();
    assert_eq!(engine.stats().rewind_from, Some(1));
}
#[test]
fn worker_delivers_only_current_snapshot_without_blocking_edit_submission() {
    let set = set();
    let mut service = HighlightService::new(engine(
        set,
        Text::from("let a=1;\n".repeat(1000).as_str()),
        0,
    ))
    .unwrap();
    let _ = service.request_lines(ChangeId(0), 500..600).unwrap();
    let text = Text::from("fn updated\n");
    let old_len = "let a=1;\n".len() * 1000;
    service
        .apply_change(
            text,
            ChangeId(1),
            ChangeSet::new(vec![Change {
                old_range: 0..old_len,
                new_len: 11,
            }]),
        )
        .unwrap();
    let _ = service.request_lines(ChangeId(1), 0..2).unwrap();
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        if let Some(result) = service.try_recv() {
            let result = result.unwrap();
            assert_eq!(result.version, ChangeId(1));
            assert!(result.complete);
            assert_eq!(result.lines, 0..2);
            break;
        }
        assert!(Instant::now() < deadline, "worker did not complete");
        std::thread::yield_now();
    }
}

#[test]
fn scheme_change_during_edit_does_not_reuse_old_palette_suffix() {
    let set = set();
    let mut text = Text::from("let value=12;\n".repeat(200).as_str());
    let mut actual = engine(set.clone(), text.clone(), 0);
    actual.finish().unwrap();
    let at = text.line_to_byte(80);
    text.insert(at, "x");
    actual
        .apply_change(
            text.clone(),
            ChangeId(1),
            &ChangeSet::new(vec![Change {
                old_range: at..at,
                new_len: 1,
            }]),
        )
        .unwrap();
    actual
        .set_color_scheme(Arc::new(ColorScheme::velocity_light()))
        .unwrap();
    let mut expected = engine(set, text, 1);
    expected
        .set_color_scheme(Arc::new(ColorScheme::velocity_light()))
        .unwrap();
    assert_eq!(spans(&mut actual), spans(&mut expected));
}

#[test]
fn worker_discards_previous_scheme_results_and_keeps_initial_generation() {
    let set = set();
    let mut initial = engine(set, Text::from("let value=12;\n"), 0);
    initial
        .set_color_scheme(Arc::new(ColorScheme::velocity_light()))
        .unwrap();
    let mut service = HighlightService::new(initial).unwrap();
    let _ = service.request_lines(ChangeId(0), 0..1).unwrap();
    service
        .set_color_scheme(Arc::new(ColorScheme::velocity_dark()))
        .unwrap();
    let _ = service.request_lines(ChangeId(0), 0..1).unwrap();
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        if let Some(batch) = service.try_recv() {
            let batch = batch.unwrap();
            assert_eq!(batch.scheme_revision, 2);
            assert_eq!(
                batch.highlighted[0].spans[0].style.background,
                ColorScheme::velocity_dark().default_style().background
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn background_rewind_republishes_an_already_completed_viewport() {
    let mut builder = SyntaxSetBuilder::new();
    builder.add_with_path(SyntaxDefinition::load_from_str("name: Branch\nscope: source.fixture\nversion: 2\ncontexts:\n  main:\n    - match: '(?=a)'\n      branch_point: reading\n      branch: [first, second]\n  first:\n    - meta_scope: first.fixture\n    - match: z\n      fail: reading\n  second:\n    - meta_scope: second.fixture\n    - match: z\n      pop: true\n",true,None).unwrap(),"Branch.sublime-syntax");
    let set = Arc::new(builder.build());
    let scheme=Arc::new(ColorScheme::parse(r#"{"globals":{"foreground":"black","background":"white"},"rules":[{"scope":"first","foreground":"red"},{"scope":"second","foreground":"blue"}]}"#).unwrap());
    let mut engine = DocumentHighlighter::new(
        Text::from(format!("a\n{}z\n", "x\n".repeat(100)).as_str()),
        ChangeId(1),
        set.clone(),
        &set.syntaxes()[0],
        scheme,
    );
    let before = engine
        .request_lines(ChangeId(1), 0..1, Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        before.highlighted[0].spans[0].style.foreground,
        vel_color_scheme::Color::rgb(255, 0, 0)
    );
    let service = HighlightService::new(engine).unwrap();
    service.request_lines(ChangeId(1), 0..1).unwrap();
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        if let Some(batch) = service.try_recv() {
            let batch = batch.unwrap();
            if batch.highlighted[0].spans[0].style.foreground
                == vel_color_scheme::Color::rgb(0, 0, 255)
            {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "a later branch correction was never published"
        );
        std::thread::yield_now();
    }
}
