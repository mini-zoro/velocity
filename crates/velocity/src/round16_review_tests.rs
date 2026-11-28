//! Reviewer regressions driven through the editor, layout and session entry points.
use super::*;
use crate::{language::Assets, workspace::Workspace};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::{Region, Selection};
use vel_config::{json, settings::Layer};
use vel_text::Text;
fn editor(source: &str, syntax: &str) -> Editor {
    let mut e = Editor::new(Text::from(source));
    e.attach_language(
        Assets::bundled().unwrap(),
        syntax,
        Arc::new(ColorScheme::velocity_dark()),
    )
    .unwrap();
    ready(&mut e);
    e
}
fn ready(e: &mut Editor) {
    let until = Instant::now() + crate::test_timing::HANG_GUARD;
    while !e.language.borrow().as_ref().unwrap().ready()
        || e.language.borrow().as_ref().unwrap().analysis.is_none()
    {
        e.poll_language();
        assert!(Instant::now() < until);
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
fn view() -> View {
    View::new(
        Size {
            width: 600.,
            height: 320.,
        },
        8.,
        12.,
    )
}
#[test]
fn fold_header_clicks_and_vertical_motion_cross_braces_tags_and_indentation() {
    for (syntax, source) in [
        (
            "Rust",
            "fn a() {\n    hello();\n}\nlet value = something_long_here;",
        ),
        (
            "HTML",
            "<section>\n    hello\n</section>\nlet value = something_long_here;",
        ),
        (
            "Python",
            "if condition:\n    hello\n    body\nnext_value_is_a_long_line\n",
        ),
    ] {
        for width in [0, 7, 12, 80] {
            let mut e = editor(source, syntax);
            let mut v = view();
            e.settings.set(Layer::View, "word_wrap", json!(width != 0));
            e.settings.set(Layer::View, "wrap_width", json!(width));
            e.sync();
            ready(&mut e);
            e.run("fold_by_level", &json!({"level":1})).unwrap();
            v.apply_style(&e);
            let fold = e.folded()[0];
            let visual = e.layout.borrow().visual_at(&e.text, fold.begin()).0;
            let end_visual = e.layout.borrow().visual_at(&e.text, fold.end()).0;
            let expected = if visual == end_visual {
                fold.end()
            } else {
                fold.begin()
            };
            assert_eq!(
                e.layout.borrow().offset(&e.text, visual, 10000.),
                expected,
                "{syntax} width {width}"
            );
            e.set_caret(source.len(), false);
            let mut rows = Vec::new();
            let count = e.layout.borrow().total() + 2;
            for _ in 0..count {
                let row = e.layout.borrow().visual_at(&e.text, e.caret).0;
                rows.push(row);
                e.run("move", &json!({"by":"lines","forward":false}))
                    .unwrap();
                if e.caret == 0 {
                    rows.push(e.layout.borrow().visual_at(&e.text, e.caret).0);
                    break;
                }
            }
            assert!(
                rows.contains(&visual),
                "Up misses header: {syntax} width {width}: {rows:?}"
            );
            e.set_caret(0, false);
            let mut reached = false;
            let count = e.layout.borrow().total() + 2;
            for _ in 0..count {
                e.run("move", &json!({"by":"lines","forward":true}))
                    .unwrap();
                if e.text.byte_to_line(e.caret) > e.text.byte_to_line(fold.end()) {
                    reached = true;
                    break;
                }
            }
            assert!(reached, "Down misses final line: {syntax} width {width}");
            v.scene(&e);
        }
    }
}
#[test]
fn typing_after_fold_markers_reveals_only_hidden_indentation_endpoints() {
    for (syntax, source) in [
        ("Python", "if x:\n    body\nnext\n"),
        ("Rust", "fn a() {\n    hello();\n}\nnext\n"),
        ("HTML", "<section>\n    body\n</section>\nnext\n"),
    ] {
        let mut e = editor(source, syntax);
        let mut v = view();
        e.settings.set(Layer::View, "word_wrap", json!(false));
        // Include the closing newline so Rust/HTML end at a visible line start.
        // The indentation fold still ends on a hidden line.
        e.settings
            .set(Layer::View, "fold_include_trailing_newline", json!(true));
        e.sync();
        ready(&mut e);
        e.run("fold_by_level", &json!({"level":1})).unwrap();
        let fold = e.folded()[0];
        e.set_caret(fold.begin(), false);
        e.run("move", &json!({"by":"characters","forward":true}))
            .unwrap();
        assert_eq!(e.caret, fold.end());
        v.scene(&e);
        e.text_input(vel_platform::TextInput::Insert {
            text: "Z".into(),
            replacement: None,
        });
        assert_eq!(e.folded().is_empty(), syntax == "Python", "{syntax}");
        assert_eq!(e.text.slice(fold.end()..fold.end() + 1).to_string(), "Z");
        v.scene(&e);
        assert_eq!(
            v.hit_test(
                &e,
                Point {
                    x: v.rect_at(&e, e.caret).x,
                    y: v.rect_at(&e, e.caret).y + 1.
                }
            ),
            e.caret
        );
        e.run("undo", &json!({})).unwrap();
        assert_eq!(e.text.to_string(), source);
    }
}
#[test]
fn nested_revealed_folds_round_trip_sessions_and_same_identity_restore_invalidates_cache() {
    let source = "outer:\n    inner:\n        body\n    tail\nend\n";
    let mut w = Workspace::new(Text::from(source), None, None, false).unwrap();
    let outer = Region::new(6, source.find("tail").unwrap() + 4);
    let inner = Region::new(
        source.find("inner:").unwrap() + 6,
        source.find("body").unwrap() + 4,
    );
    let e = w.editor_mut();
    e.set_folded(vec![outer, inner]);
    e.set_caret(source.find("inner:").unwrap() + 1, false);
    e.set_caret(0, false);
    assert_eq!(e.folded(), [inner]);
    w.clone_file().unwrap();
    w.editor_mut().set_folded(vec![outer, inner]);
    w.select(0);
    let (window, documents) = w.snapshot().unwrap();
    assert_eq!(window.sheets[0].view.folds.len(), 1);
    assert_eq!(window.sheets[0].view.folds[0].a, inner.a);
    assert_eq!(window.sheets[1].view.folds.len(), 2);
    let (window, documents): (vel_session::Window, Vec<vel_session::Document>) =
        serde_json::from_slice(&serde_json::to_vec(&(window, documents)).unwrap()).unwrap();
    let mut restored = Workspace::new(Text::from(""), None, None, false).unwrap();
    restored.restore_window(&window, &documents).unwrap();
    // Selecting the restored lazy sheet materializes its view/history.
    restored.prepare_sheet(0).unwrap();
    assert_eq!(restored.editor().folded(), [inner]);
    restored.select(1);
    restored.prepare_sheet(1).unwrap();
    assert_eq!(restored.editor().folded(), [outer, inner]);
    restored.select(0);
    let e = restored.editor_mut();
    let mut state = e.folded_view_state(e.id, (0., 0.));
    state.folds = vec![outer.into(), inner.into()];
    state.apply(&mut e.buffer, e.id);
    e.restore_folded_view_state(&state, e.id);
    assert_eq!(e.folded(), [outer, inner]);
    state.folds = vec![inner.into()];
    state.apply(&mut e.buffer, e.id);
    e.restore_folded_view_state(&state, e.id);
    assert_eq!(e.folded(), [inner]);
}
#[test]
fn guides_keep_the_published_blank_and_active_block_geometry_during_typing_and_row_edits() {
    let source = "top:\n    a\n\n    b\nend\n    other\n";
    let mut e = editor(source, "Python");
    let mut v = view();
    e.settings.set(
        Layer::View,
        "indent_guide_options",
        json!(["draw_normal", "draw_active"]),
    );
    e.sync();
    e.set_caret(source.find('a').unwrap() + 1, false);
    let geometry = |scene: Scene| {
        scene
            .layers
            .into_iter()
            .flat_map(|l| l.rects)
            .filter(|r| r.is_stippled_guide())
            .map(|r| (r.rect.x, r.rect.y, r.color))
            .collect::<Vec<_>>()
    };
    let before = geometry(v.scene(&e));
    e.run("insert", &json!({"characters":"x"})).unwrap();
    {
        let l = e.language.borrow();
        let l = l.as_ref().unwrap();
        assert!(l.analysis.is_none());
        assert_eq!(l.guides.columns(2), Some(4));
        assert_eq!(l.guides.active(1, 4).unwrap().rows, 1..4);
    }
    assert_eq!(geometry(v.scene(&e)), before);
    e.set_caret(e.text.line_to_byte(1), false);
    e.run("insert", &json!({"characters":"\n\n"})).unwrap();
    {
        let l = e.language.borrow();
        let l = l.as_ref().unwrap();
        assert_eq!(l.guides.columns(4), Some(4));
        assert_eq!(l.guides.active(3, 4).unwrap().rows, 1..6);
    }
    e.run("undo", &json!({})).unwrap();
    assert_eq!(
        e.language.borrow().as_ref().unwrap().guides.columns(2),
        Some(4)
    );
    e.run("redo", &json!({})).unwrap();
    assert_eq!(
        e.language.borrow().as_ref().unwrap().guides.columns(4),
        Some(4)
    );
    assert_eq!(
        e.language
            .borrow()
            .as_ref()
            .unwrap()
            .guides
            .active(3, 4)
            .unwrap()
            .rows,
        1..6
    );
    // A shared-buffer host can publish several changes before sync, too.
    e.buffer
        .set_selection(e.id, Selection::from_regions([Region::caret(0)]));
    vel_buffer::ops::insert(&mut e.buffer, e.id, "\n");
    e.buffer.break_undo_group();
    vel_buffer::ops::insert(&mut e.buffer, e.id, "\n");
    e.sync();
    assert_eq!(
        e.language.borrow().as_ref().unwrap().guides.columns(6),
        Some(4)
    );
    assert_eq!(
        e.language
            .borrow()
            .as_ref()
            .unwrap()
            .guides
            .active(5, 4)
            .unwrap()
            .rows,
        3..8
    );
}
#[test]
fn bookmark_gutter_borrows_visible_regions_and_follows_mapped_edits() {
    let mut e = Editor::new(Text::from("aa\nbb\ncc\ndd\n"));
    e.set_selection(Selection::from_regions([
        Region::new(0, 2),
        Region::new(3, 8),
    ]));
    e.run("toggle_bookmark", &json!({})).unwrap();
    let marks = visible_bookmarks(&e, 3..9);
    assert_eq!(marks.len(), 1);
    for (row, expected) in [(0, true), (1, true), (2, true), (3, false)] {
        assert_eq!(
            bookmarked_line(e.bookmarks(), row * 3..row * 3 + 3),
            expected
        );
    }
    e.set_caret(0, false);
    e.run("insert", &json!({"characters":"x\n"})).unwrap();
    assert!(!bookmarked_line(e.bookmarks(), 0..2));
    assert!(bookmarked_line(e.bookmarks(), 2..5));
    e.run("undo", &json!({})).unwrap();
    assert!(bookmarked_line(e.bookmarks(), 0..3));
    let mut v = view();
    assert_eq!(
        v.scene(&e)
            .layers
            .iter()
            .flat_map(|l| &l.rects)
            // Bookmarks are round dots at the gutter's left edge.
            .filter(|r| r.rect.x == 3. && r.rect.width == r.rect.height && r.radius > 0.)
            .count(),
        3
    );
}
#[test]
fn many_folded_regions_and_carets_delete_and_insert_without_revealing_unrelated_folds() {
    for (command, expected) in [
        ("left_delete", "aX hidden z\n"),
        ("right_delete", "abXhidden z\n"),
        ("delete_word", "X hidden z\n"),
    ] {
        let mut e = Editor::new(Text::from("ab hidden z\n".repeat(256)));
        e.set_selection(Selection::from_regions(
            (0..256).map(|n| Region::caret(n * 12 + 2)),
        ));
        e.set_folded(
            (0..256)
                .map(|n| Region::new(n * 12 + 3, n * 12 + 9))
                .collect(),
        );
        e.run(command, &json!({})).unwrap();
        e.run("insert", &json!({"characters":"X"})).unwrap();
        assert_eq!(e.text.to_string(), expected.repeat(256));
        assert_eq!(e.folded().len(), 256);
        let r = e.folded()[127];
        e.set_caret(r.begin() + 1, false);
        e.set_caret(0, false);
        assert_eq!(e.folded().len(), 255);
    }
}
