use super::*;
use crate::language::Assets;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use vel_color_scheme::ColorScheme;
use vel_config::{json, keymap::Key, settings::Layer};
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
    let end = Instant::now() + crate::test_timing::HANG_GUARD;
    loop {
        e.poll_language();
        if e.language.borrow().as_ref().is_some_and(|l| l.ready()) {
            break;
        }
        assert!(Instant::now() < end, "highlight timeout");
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

#[test]
fn brackets_probe_syntax_scopes_and_progressive_expansion() {
    for (syntax, source) in [
        ("Rust", "fn f() { call([value]); }"),
        ("Python", "call([value])"),
        ("JavaScript", "function f() { call([value]); }"),
    ] {
        let mut e = editor(source, syntax);
        let at = source.find("value").unwrap();
        e.set_caret(at + 2, false);
        e.run("expand_selection", &json!({"to":"brackets"}))
            .unwrap();
        assert_eq!(e.text.slice(e.selection()).to_string(), "value");
        e.run("expand_selection", &json!({"to":"brackets"}))
            .unwrap();
        assert_eq!(e.text.slice(e.selection()).to_string(), "[value]");
        e.run("expand_selection", &json!({"to":"brackets"}))
            .unwrap();
        assert_eq!(e.text.slice(e.selection()).to_string(), "([value])");
        let open = source.find('[').unwrap();
        let close = source.find(']').unwrap();
        for (from, to) in [
            (open, close),
            (open + 1, close),
            (close, open),
            (at + 1, close),
        ] {
            e.set_caret(from, false);
            e.run("move_to", &json!({"to":"brackets"})).unwrap();
            assert_eq!(e.caret, to, "{syntax}: from {from}");
        }
    }
    let source = "fn f() { let s = \"[value]\"; /* {local} */ }";
    let mut e = editor(source, "Rust");
    for (word, open, close) in [("value", '[', ']'), ("local", '{', '}')] {
        let at = source.find(word).unwrap();
        e.set_caret(at, false);
        e.run("move_to", &json!({"to":"brackets"})).unwrap();
        assert_eq!(e.text.char_at(e.caret), close);
        e.run("move_to", &json!({"to":"brackets"})).unwrap();
        assert_eq!(e.text.char_at(e.caret), open);
    }
    e.set_caret(source.find("local").unwrap(), false);
    e.run("expand_selection", &json!({"to":"scope"})).unwrap();
    assert!(e.text.slice(e.selection()).to_string().contains("local"));
    assert!(e.selection().len() < source.len());
    let source = "fn f() { let s = \"[ unmatched\"; }";
    let mut e = editor(source, "Rust");
    let at = source.find("unmatched").unwrap();
    e.set_caret(at, false);
    e.run("move_to", &json!({"to":"brackets"})).unwrap();
    assert_eq!(e.caret, at);

    let source = "fn many() { call([value]); }\n".repeat(1_000);
    let e = editor(&source, "Rust");
    let at = source.rfind("value").unwrap();
    let language = e.language.borrow();
    let l = language.as_ref().unwrap();
    assert!(l.analysis.as_ref().unwrap().pairs.len() > 2_000);
    let candidates = crate::language::command_pairs(Some(l), &e.text, at);
    assert_eq!(candidates.len(), 3); // body, call and subscript, without sibling pairs.
}

#[test]
fn scope_string_probe_table_and_empty_adjacent_brackets() {
    for (syntax, source) in [
        ("Rust", "let s = \"value\";"),
        ("Python", "s = \"value\""),
        ("JavaScript", "let s = \"value\";"),
    ] {
        let mut e = editor(source, syntax);
        e.set_caret(source.find("value").unwrap() + 2, false);
        e.run("expand_selection", &json!({"to":"scope"})).unwrap();
        assert_eq!(
            e.text.slice(e.selection()).to_string(),
            "\"value\"",
            "{syntax}"
        );
    }
    let mut scoped = editor("let s = \"(value)\";", "Rust");
    scoped.set_caret(12, false);
    scoped
        .run("expand_selection", &json!({"to":"scope"}))
        .unwrap();
    assert_eq!(
        scoped.text.slice(scoped.selection()).to_string(),
        "\"(value)\""
    );
    let mut separated = editor("x [value] y", "Rust");
    for (from, to) in [(2, 8), (3, 8), (8, 2), (9, 2)] {
        separated.set_caret(from, false);
        separated.run("move_to", &json!({"to":"brackets"})).unwrap();
        assert_eq!(separated.caret, to);
    }
    let mut e = editor("()[]", "Rust");
    for (from, to) in [(0, 1), (1, 0), (2, 3), (3, 2), (4, 2)] {
        e.set_caret(from, false);
        e.run("move_to", &json!({"to":"brackets"})).unwrap();
        assert_eq!(e.caret, to);
    }
}

#[test]
fn tags_and_scope_expansion_keep_reversed_primary_and_skip_comment_tags() {
    for syntax in ["HTML", "XML"] {
        let source = "<outer><item title=\">\">hello</item><!-- <fake> --></outer>";
        let mut e = editor(source, syntax);
        let at = source.find("hello").unwrap();
        e.set_caret(at + 2, false);
        for expected in [
            "hello",
            "<item title=\">\">hello</item>",
            "<item title=\">\">hello</item><!-- <fake> -->",
        ] {
            e.run("expand_selection", &json!({"to":"tag"})).unwrap();
            assert_eq!(
                e.text.slice(e.selection()).to_string(),
                expected,
                "{syntax}"
            );
        }
        e.set_selection(Selection::from(Region::new(at + 4, at + 1)));
        e.run("expand_selection", &json!({"to":"scope"})).unwrap();
        assert!(e.anchor > e.caret);
    }
}

#[test]
fn scope_extent_stays_in_the_carets_multiline_occurrence() {
    let source = "/* first\n   occurrence */\nlet gap = 1;\n/* second\n   occurrence */";
    let mut e = editor(source, "Rust");
    let at = source.find("second").unwrap();
    let language = e.language.borrow();
    let l = language.as_ref().unwrap();
    let comments = crate::language::command_scope_regions(l, at, "comment.block");
    assert_eq!(
        comments.last().cloned(),
        Some(source.rfind("/*").unwrap()..source.len())
    );
    drop(language);
    e.set_caret(at, false);
    e.run("expand_selection", &json!({"to":"scope"})).unwrap();
    assert_eq!(
        e.text.slice(e.selection()).to_string(),
        "/* second\n   occurrence */"
    );
}

#[test]
fn block_only_comment_probe_table_and_existing_scope_uncomment() {
    for (syntax, source, comment) in [
        ("HTML", "<p>hello</p>", "<!-- <p>hello</p> -->"),
        // Verbatim markers, as Sublime 4215 inserts them (black-box, 2026-11-22).
        ("CSS", "color: red;", "/*color: red;*/"),
    ] {
        let mut e = editor(source, syntax);
        e.set_caret(3, false);
        e.run("toggle_comment", &json!({})).unwrap();
        assert_eq!(e.text.to_string(), comment, "{syntax}");
        // Toggling before a replacement scope snapshot is published must also work.
        e.run("toggle_comment", &json!({})).unwrap();
        assert_eq!(e.text.to_string(), source);
        e.run("toggle_comment", &json!({})).unwrap();
        ready(&mut e);
        for block in [false, true] {
            e.set_caret(
                comment
                    .find("hello")
                    .or_else(|| comment.find("red"))
                    .unwrap(),
                false,
            );
            e.run("toggle_comment", &json!({"block":block})).unwrap();
            assert_eq!(e.text.to_string(), source, "{syntax}, block={block}");
            e.run("undo", &json!({})).unwrap();
            ready(&mut e);
        }
    }
    let mut e = editor("/* one\n two */\nfn x() {}", "Rust");
    e.set_caret(9, false);
    e.run("toggle_comment", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), " one\n two \nfn x() {}");
}

#[test]
fn noops_do_not_publish_layout_language_annotations_or_shared_changes() {
    let mut e = editor("UPPER", "Rust");
    e.set_selection(Selection::from(Region::new(0, 5)));
    let version = e.buffer.change_id();
    // A second consumer requires the replay journal; a sole view consumes edits directly.
    let _consumer = e.layout_edits.clone();
    let shared = e.layout_edits.borrow().len();
    e.run("upper_case", &json!({})).unwrap();
    assert_eq!(e.buffer.change_id(), version);
    assert!(!e.buffer.is_dirty());
    assert_eq!(e.layout_edits.borrow().len(), shared);
    assert_eq!(e.language.borrow().as_ref().unwrap().version, version);
    e.set_caret(0, false);
    e.run("left_delete", &json!({})).unwrap();
    assert_eq!(e.buffer.change_id(), version);
    e.run("insert", &json!({"characters":"x"})).unwrap();
    assert_eq!(e.buffer.change_id().0, version.0 + 1);
    assert_eq!(
        e.language.borrow().as_ref().unwrap().version,
        e.buffer.change_id()
    );
    assert_eq!(e.layout_edits.borrow().len(), shared + 1);
}

#[test]
fn view_select_lines_respects_wrap_folds_direction_and_up_down_goal() {
    let mut e = Editor::new(Text::from(
        "abcdefghijklmnop\n  HIDDEN\n  HIDDEN\nabcdefghijklmnop\n",
    ));
    e.buffer
        .add_regions(e.id, FOLDS, vec![Region::new(16, 35)], true);
    e.layout.borrow_mut().configure(
        &e.text,
        (e.buffer.id(), e.buffer.change_id()),
        crate::layout::Options {
            width: 8,
            tab: 4,
            indent: false,
        },
        &e.folded(),
    );
    e.set_selection(Selection::from(Region::new(3, 1)));
    e.run("select_lines", &json!({"forward":true})).unwrap();
    let regions = e.regions();
    assert!(regions.iter().any(|r| r.a == 11 && r.b == 9), "{regions:?}");
    assert!(regions.iter().all(|r| r.end() <= 16 || r.begin() >= 35));
    e.set_caret(3, false);
    e.run("move", &json!({"by":"lines","forward":true}))
        .unwrap();
    assert_eq!(e.caret, 11);
    assert_eq!(e.regions()[0].xpos, Some(3.));
    e.run("select_lines", &json!({"forward":true})).unwrap();
    assert!(e.regions().iter().all(|r| r.xpos == Some(3.)));
}

#[test]
fn wrap_setting_precedes_ruler_and_bookmark_gutter_accessor_retains_regions() {
    let mut e = editor("// one two three four", "Rust");
    e.settings.set(Layer::View, "wrap_width", json!(10));
    e.settings.set(Layer::View, "rulers", json!([80]));
    e.run("wrap_lines", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "// one two\n// three\n// four");
    e.set_selection(Selection::from(Region::new(6, 3)));
    e.run("toggle_bookmark", &json!({})).unwrap();
    assert_eq!(e.bookmarks(), [Region::new(6, 3)]);
    e.set_caret(0, false);
    e.run("insert", &json!({"characters":"x\n"})).unwrap();
    assert_eq!(e.bookmarks(), [Region::new(8, 5)]);
}

#[test]
fn round16_keybindings_resolve_without_gui() {
    for (keys, name) in [
        (vec!["ctrl+m"], "move_to"),
        (vec!["ctrl+shift+m"], "expand_selection"),
        (vec!["super+shift+space"], "expand_selection"),
        (vec!["super+shift+a"], "expand_selection"),
        (vec!["super+alt+q"], "wrap_lines"),
        (vec!["f2"], "next_bookmark"),
        (vec!["alt+f2"], "select_all_bookmarks"),
        (vec!["super+k", "super+w"], "delete_to_mark"),
        (vec!["super+k", "super+g"], "clear_bookmarks"),
        (vec!["ctrl+q"], "toggle_record_macro"),
        (vec!["ctrl+shift+q"], "run_macro"),
    ] {
        let mut e = Editor::new(Text::new());
        let mut last = vel_config::keymap::Resolution::Unbound;
        for key in keys {
            last = e.resolve_key(&Key::parse(key).unwrap());
        }
        let vel_config::keymap::Resolution::Command(command) = last else {
            panic!("binding for {name}: {last:?}")
        };
        assert_eq!(command.command, name);
    }
}

#[test]
#[ignore = "explicit headless timing; run without concurrent compiles"]
fn command_hook_timings_large_buffer_many_carets() {
    for size in [1usize, 32] {
        for carets in [1usize, 10_000] {
            let line = "UPPER abcdefghijklmnopqrstuvwxyz 0123456789 abcdefghijklmnopqrstuvwxyz\n";
            let source = line.repeat((size * 1024 * 1024).div_ceil(line.len()));
            let mut e = Editor::new(Text::from(source.as_str()));
            e.set_selection(Selection::from_regions(
                (0..carets).map(|i| Region::caret(i * line.len())),
            ));
            let time = |e: &mut Editor, name: &str, args: &Value| {
                let begin = Instant::now();
                for _ in 0..20 {
                    e.run(name, args).unwrap();
                }
                begin.elapsed().as_secs_f64() * 1000. / 20.
            };
            let no_op = time(&mut e, "insert", &json!({"characters":""}));
            e.run("toggle_bookmark", &json!({})).unwrap();
            let with_bookmarks = time(&mut e, "insert", &json!({"characters":"x"}));
            e.run("clear_bookmarks", &json!({})).unwrap();
            let without_bookmarks = time(&mut e, "insert", &json!({"characters":"x"}));
            e.run("toggle_record_macro", &json!({})).unwrap();
            let recording = time(&mut e, "insert", &json!({"characters":"x"}));
            e.run("toggle_record_macro", &json!({})).unwrap();
            println!(
                "{{\"size_mib\":{size},\"carets\":{carets},\"noop_ms\":{no_op:.4},\"bookmarks_ms\":{with_bookmarks:.4},\"plain_ms\":{without_bookmarks:.4},\"recording_ms\":{recording:.4}}}"
            );
        }
    }
}

#[test]
fn wrapping_uses_language_comments_and_keeps_markdown_and_plain_text_markers() {
    for syntax in ["Plain Text", "Markdown"] {
        for marker in ["*", "#", "%", ";"] {
            let source = format!("  {marker} alpha beta gamma delta epsilon zeta eta theta");
            let mut e = editor(&source, syntax);
            e.run("wrap_lines", &json!({"width":20})).unwrap();
            assert_eq!(
                e.text.to_string(),
                format!("  {marker} alpha beta gamma\n  delta epsilon zeta\n  eta theta"),
                "{syntax}: {marker}"
            );
        }
    }
    for (syntax, marker) in [
        ("Rust", "//"),
        ("Rust", "///"),
        ("Python", "#"),
        ("SQL", "--"),
    ] {
        let source = format!("  {marker} alpha beta gamma delta epsilon zeta eta theta");
        let mut e = editor(&source, syntax);
        e.run("wrap_lines", &json!({"width":20})).unwrap();
        assert!(
            e.text
                .to_string()
                .lines()
                .all(|line| line.starts_with(&format!("  {marker} "))),
            "{syntax}: {}",
            e.text
        );
    }
}

#[test]
fn identical_paste_and_indent_collapses_all_requested_selections() {
    let mut e = Editor::new(Text::from("same"));
    e.set_selection(Selection::from(Region::new(0, 4)));
    e.clipboard = "same".into();
    let version = e.buffer.change_id();
    e.run("paste_and_indent", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "same");
    assert_eq!(e.regions(), [Region::caret(4)]);
    assert_eq!(e.buffer.change_id(), version);
    assert!(!e.buffer.is_dirty());
    assert!(!e.buffer.undo(e.id));

    let mut e = Editor::new(Text::from("same other same"));
    e.set_selection(Selection::from_regions([
        Region::new(5, 10),
        Region::new(11, 15),
        Region::new(4, 0),
    ]));
    e.clipboard = "same".into();
    e.run("paste_and_indent", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "same same same");
    assert_eq!(
        e.regions(),
        [Region::caret(4), Region::caret(9), Region::caret(14)]
    );
    assert_eq!(e.buffer.view(e.id).sel().primary_index(), 0);
    e.run("undo", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "same other same");
}

/// Sublime Text 4215, observed as a black box (2026-11-22): ⌘/ on each line of an HTML file.
/// An embedded language uses its own marker set; JavaScript's `// ` never borrows HTML's
/// ` -->` end, and CSS's `/*` `*/` are inserted verbatim.
#[test]
fn embedded_languages_use_their_own_comment_markers() {
    let source = "<html>\n<script>\nvar a = 1;\n</script>\n<p>text</p>\n<style>\np { color: red; }\n</style>\n</html>\n";
    let mut e = editor(source, "HTML");
    ready(&mut e);
    for needle in ["p { color", "<p>text", "var a"] {
        let at = e.text.to_string().find(needle).unwrap();
        e.set_caret(at, false);
        e.run("toggle_comment", &json!({})).unwrap();
        ready(&mut e);
    }
    assert_eq!(
        e.text.to_string(),
        "<html>\n<script>\n// var a = 1;\n</script>\n<!-- <p>text</p> -->\n<style>\n/*p { color: red; }*/\n</style>\n</html>\n"
    );
}
