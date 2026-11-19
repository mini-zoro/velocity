use super::*;
use crate::{panels::Panels, workspace::Workspace};
use std::{
    path::Path,
    time::{Duration, Instant},
};

fn workspace(text: &str, syntax: &str, profile: Option<&Path>) -> Workspace {
    crate::test_login::install();
    let mut w = Workspace::new(
        Text::from(text),
        profile.map(Path::to_path_buf),
        None,
        false,
    )
    .unwrap();
    w.set_file_type(syntax).unwrap();
    w.editor_mut().set_caret(text.len(), false);
    w
}

fn set(e: &mut Editor, key: &str, value: Value) {
    e.settings.set(Layer::View, key, value);
    e.sync();
}

fn type_text(e: &mut Editor, value: &str) {
    e.text_input(TextInput::Insert {
        text: value.into(),
        replacement: None,
    });
}

fn ready(e: &mut Editor, condition: impl Fn(&Editor) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition(e) {
        e.poll_language();
        e.poll_completion();
        assert!(
            Instant::now() < deadline,
            "text: {:?}, completion: {:?}",
            e.text,
            e.completion
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

fn scopes_ready(e: &mut Editor) {
    ready(e, |e| {
        e.language
            .borrow()
            .as_ref()
            .unwrap()
            .scopes_ready_at(&e.text, e.caret)
    });
}

fn resolve(w: &mut Workspace, panels: &mut Panels, key: &str) -> Resolution {
    panels
        .resolve_key(w, &Key::parse(key).unwrap(), 0..40)
        .unwrap()
}

fn key(w: &mut Workspace, panels: &mut Panels, chord: &str) {
    let Resolution::Command(command) = resolve(w, panels, chord) else {
        panic!("unresolved {chord}");
    };
    if command.command == "run_macro_file" {
        let playback = panels
            .macro_playback(w, &command.command, &command.args, 0..40)
            .unwrap()
            .unwrap();
        for (name, args) in playback.commands.iter() {
            w.editor_mut().run(name, args).unwrap();
        }
    } else if !panels
        .action(w, &command.command, &command.args, 0..40)
        .unwrap()
    {
        w.editor_mut().run(&command.command, &command.args).unwrap();
    }
}

#[test]
fn round21_standard_selector_commands() {
    let cases = [
        (
            "moveToBeginningOfLine:",
            "move_to",
            json!({"to":"bol","extend":false}),
        ),
        (
            "moveToBeginningOfParagraph:",
            "move_to",
            json!({"to":"hardbol","extend":false}),
        ),
        (
            "moveToBeginningOfLineAndModifySelection:",
            "move_to",
            json!({"to":"bol","extend":true}),
        ),
        (
            "moveToBeginningOfParagraphAndModifySelection:",
            "move_to",
            json!({"to":"hardbol","extend":true}),
        ),
        (
            "moveToEndOfLine:",
            "move_to",
            json!({"to":"eol","extend":false}),
        ),
        (
            "moveToEndOfParagraph:",
            "move_to",
            json!({"to":"hardeol","extend":false}),
        ),
        (
            "moveToEndOfLineAndModifySelection:",
            "move_to",
            json!({"to":"eol","extend":true}),
        ),
        (
            "moveToEndOfParagraphAndModifySelection:",
            "move_to",
            json!({"to":"hardeol","extend":true}),
        ),
        (
            "insertNewlineIgnoringFieldEditor:",
            "insert",
            json!({"characters":"\n","literal":true}),
        ),
        ("deleteWordForward:", "delete_word", json!({"forward":true})),
        (
            "deleteWordBackward:",
            "delete_word",
            json!({"forward":false}),
        ),
        (
            "deleteBackwardByDecomposingPreviousCharacter:",
            "left_delete",
            json!({"decompose":true}),
        ),
        (
            "pageUpAndModifySelection:",
            "move",
            json!({"by":"pages","forward":false,"extend":true}),
        ),
        (
            "pageDownAndModifySelection:",
            "move",
            json!({"by":"pages","forward":true,"extend":true}),
        ),
        ("scrollPageUp:", "scroll_page", json!({"forward":false})),
        ("scrollPageDown:", "scroll_page", json!({"forward":true})),
        (
            "moveParagraphBackwardAndModifySelection:",
            "move",
            json!({"by":"paragraphs","forward":false,"extend":true}),
        ),
        (
            "moveParagraphForwardAndModifySelection:",
            "move",
            json!({"by":"paragraphs","forward":true,"extend":true}),
        ),
        ("selectWord:", "expand_selection", json!({"to":"word"})),
        ("selectLine:", "expand_selection", json!({"to":"line"})),
        (
            "selectParagraph:",
            "expand_selection",
            json!({"to":"paragraph"}),
        ),
    ];
    let registry = Registry::standard();
    for (selector, name, args) in cases.into_iter().chain(
        [
            ("centerSelectionInVisibleArea:", "show_at_center"),
            ("deleteToEndOfParagraph:", "delete_to_hard_eol"),
            ("deleteToBeginningOfLine:", "delete_to_bol"),
            ("insertBacktab:", "unindent"),
            ("yank:", "yank"),
            ("transpose:", "transpose"),
            ("setMark:", "set_mark"),
            ("selectToMark:", "select_to_mark"),
            ("swapWithMark:", "swap_with_mark"),
            ("deleteToMark:", "delete_to_mark"),
            ("uppercaseWord:", "upper_case"),
            ("lowercaseWord:", "lower_case"),
            ("capitalizeWord:", "title_case"),
        ]
        .into_iter()
        .map(|(a, b)| (a, b, json!({}))),
    ) {
        assert_eq!(
            selector_command(Command::from_selector(selector).unwrap()),
            (name, args),
            "{selector}"
        );
        assert!(registry.kind(name).is_some(), "{name}");
    }
    // The original AppKit open-line sequence does not copy indentation or move the caret.
    let mut e = Editor::new(Text::from("    one two\nother"));
    let send = |e: &mut Editor, selector| e.command(Command::from_selector(selector).unwrap());
    e.set_caret(7, false);
    send(&mut e, "moveToBeginningOfParagraph:");
    assert_eq!(e.caret, 0);
    send(&mut e, "moveToEndOfParagraphAndModifySelection:");
    assert_eq!(e.selection(), 0..11);
    e.set_caret(7, false);
    send(&mut e, "insertNewlineIgnoringFieldEditor:");
    send(&mut e, "moveBackward:");
    assert_eq!(e.text.to_string(), "    one\n two\nother");
    assert_eq!(e.caret, 7);
    send(&mut e, "deleteToEndOfParagraph:");
    assert_eq!(e.text.to_string(), "    one two\nother");
    send(&mut e, "yank:");
    assert_eq!(e.text.to_string(), "    one\n two\nother");
    let mut e = Editor::new(Text::from("e\u{301}"));
    e.set_caret(e.text.len_bytes(), false);
    send(&mut e, "deleteBackwardByDecomposingPreviousCharacter:");
    assert_eq!(e.text.to_string(), "e");
    let mut e = Editor::new(Text::from("\u{e9}"));
    e.set_caret(e.text.len_bytes(), false);
    send(&mut e, "deleteBackwardByDecomposingPreviousCharacter:");
    assert_eq!(e.text.to_string(), "e");
}

#[test]
fn round21_keymap_resolution_and_unicode_quotes() {
    let mut w = workspace("text", "HTML", None);
    scopes_ready(w.editor_mut());
    let mut panels = Panels::new(&w, false);
    for (chord, name, args) in [
        ("super+shift+[", "prev_view", json!({})),
        ("super+shift+]", "next_view", json!({})),
        ("super+9", "select_by_index", json!({"index":-1})),
        // Lane B round 21 rebinds these to next/prev_view_in_stack and adds switch_file.
        ("ctrl+tab", "next_view_in_stack", json!({})),
        ("ctrl+shift+tab", "prev_view_in_stack", json!({})),
        ("super+f12", "goto_definition", json!({"side_by_side":true})),
        (
            "super+shift+f12",
            "goto_reference",
            json!({"side_by_side":true}),
        ),
        ("ctrl+shift+p", "show_scope_name", json!({})),
    ] {
        let Resolution::Command(binding) = resolve(&mut w, &mut panels, chord) else {
            panic!("{chord}");
        };
        assert_eq!(
            (binding.command.as_str(), binding.args),
            (name, args),
            "{chord}"
        );
    }
    for quote in ["'", "\""] {
        let mut e = Editor::new(Text::from("caf\u{e9}"));
        e.set_caret(e.text.len_bytes(), false);
        if !e.key(&Key::parse(quote).unwrap()).unwrap() {
            type_text(&mut e, quote);
        }
        assert_eq!(e.text.to_string(), format!("caf\u{e9}{quote}"));
    }
}

#[test]
fn round21_dedent_by_language_keeps_manual_indent() {
    for (syntax, open, close) in [
        ("C", "if (yes) {", "}"),
        ("HTML", "<div>", "</div>"),
        ("Python", "if yes:", "else:"),
        ("Ruby", "if yes", "end"),
        ("Bash", "if true; then", "fi"),
    ] {
        let mut w = workspace(open, syntax, None);
        let e = w.editor_mut();
        set(e, "auto_match_enabled", json!(false));
        set(e, "auto_close_tags", json!(false));
        set(e, "translate_tabs_to_spaces", json!(true));
        e.run("insert", &json!({"characters":"\n"})).unwrap();
        assert_eq!(e.text.to_string(), format!("{open}\n    "), "{syntax}");
        // No scope polling after Enter or during the word: exercise the stale-scope path.
        for c in close.chars() {
            type_text(e, c.encode_utf8(&mut [0; 4]));
        }
        assert_eq!(e.text.to_string(), format!("{open}\n{close}"), "{syntax}");
        assert_eq!(e.caret, e.text.len_bytes());
    }
    let mut w = workspace("{", "C", None);
    let e = w.editor_mut();
    set(e, "translate_tabs_to_spaces", json!(true));
    type_text(e, "\n");
    type_text(e, " ");
    type_text(e, "}");
    assert_eq!(e.text.to_string(), "{\n     }");
    let mut w = workspace("{", "C", None);
    let e = w.editor_mut();
    set(e, "translate_tabs_to_spaces", json!(true));
    type_text(e, "\n");
    type_text(e, "}");
    e.run("undo", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "{\n    ");
    e.run("redo", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "{\n}");
}

#[test]
fn round21_dedent_recovers_keyword_prefixes_and_keeps_code_spaces() {
    for (syntax, open, typed, dedented_prefixes) in [
        ("Bash", "if true; then", "find . -name x", &["fi"][..]),
        ("Bash", "for x in y; do", "install_it", &["in"][..]),
        ("Bash", "while true; do", "docker ps", &["do"][..]),
        ("Ruby", "def f", "end_time = 1", &["end"][..]),
        ("Ruby", "def f", "rescue_from Foo", &["rescue"][..]),
        ("Lua", "function f()", "endpoint = 1", &["end"][..]),
        (
            "Lua",
            "if yes then",
            "elseif_count = 1",
            &["else", "elsei", "elseif"][..],
        ),
        ("Python", "if yes:", "elseif_like = 1", &[][..]),
        ("Python", "if yes:", "elif x:", &["elif x:"][..]),
        ("Python", "try:", "except E:", &["except E:"][..]),
        ("Python", "    case 1:", "case 2:", &["case 2:"][..]),
        ("C", "if (yes) {", "const char *p = \"}\";", &[][..]),
        ("JavaScript", "if (yes) {", "const p = `}`;", &[][..]),
        ("HTML", "<div>", "<endpoint>", &[][..]),
    ] {
        let mut w = workspace(open, syntax, None);
        let e = w.editor_mut();
        set(e, "auto_match_enabled", json!(false));
        set(e, "auto_close_tags", json!(false));
        set(e, "translate_tabs_to_spaces", json!(true));
        scopes_ready(e);
        type_text(e, "\n");
        let base = open.bytes().take_while(|&b| b == b' ').count();
        assert_eq!(
            e.text.to_string(),
            format!("{open}\n{}", " ".repeat(base + 4))
        );
        // Type each character with the same already-published scopes. No parser
        // can run synchronously on this path, even while the trace is corrected.
        let parses = e.exact_scope_parses.get();
        for (i, ch) in typed.char_indices() {
            type_text(e, ch.encode_utf8(&mut [0; 4]));
            let prefix = &typed[..i + ch.len_utf8()];
            let indent = base
                + if dedented_prefixes.contains(&prefix) {
                    0
                } else {
                    4
                };
            assert_eq!(
                e.text.to_string(),
                format!("{open}\n{}{prefix}", " ".repeat(indent)),
                "{syntax} {prefix}"
            );
        }
        assert_eq!(e.exact_scope_parses.get(), parses);
    }
    let mut w = workspace("if true; then", "Bash", None);
    let e = w.editor_mut();
    set(e, "translate_tabs_to_spaces", json!(true));
    type_text(e, "\n");
    for key in ["f", "i", "n"] {
        type_text(e, key);
    }
    assert_eq!(e.text.to_string(), "if true; then\n    fin");
    e.run("undo", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "if true; then\nfi");
    e.run("redo", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "if true; then\n    fin");
}

#[test]
fn round21_exact_enter_scope_blocks_dedent_in_stale_literals_and_heredocs() {
    for (syntax, base, opener, typed) in [
        ("Python", "    value = ", "\"\"\"", "else:"),
        ("Ruby", "    value = ", "\"", "end"),
        ("Ruby", "    value = ", "<<~EOS", "end"),
        ("Bash", "    cat ", "<<EOF", "fi"),
        ("C", "    int x; ", "/*", "}"),
        ("JavaScript", "    const value = ", "`", "}"),
    ] {
        for published in [false, true] {
            let mut w = workspace(base, syntax, None);
            let e = w.editor_mut();
            set(e, "translate_tabs_to_spaces", json!(true));
            set(e, "auto_match_enabled", json!(false));
            scopes_ready(e);
            type_text(e, opener);
            if published {
                scopes_ready(e);
            } else {
                assert!(
                    !e.language
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .scopes_ready_at(&e.text, e.caret)
                );
            }
            type_text(e, "\n");
            // Published comment and template scopes can change what Enter
            // inserts. Only subsequent typing should leave that indent alone.
            let after_enter = e.text.to_string();
            let indent = after_enter
                .strip_prefix(&format!("{base}{opener}\n"))
                .unwrap();
            assert!(indent.bytes().all(|byte| byte == b' '));
            assert!(syntax == "C" && published || !indent.is_empty());
            let parses = e.exact_scope_parses.get();
            for (i, c) in typed.char_indices() {
                type_text(e, c.encode_utf8(&mut [0; 4]));
                assert_eq!(
                    e.text.to_string(),
                    format!("{base}{opener}\n{indent}{}", &typed[..i + c.len_utf8()]),
                    "{syntax}, published={published}"
                );
            }
            assert_eq!(e.exact_scope_parses.get(), parses);
        }
    }
    let text = format!("{}    if x:", "pass\n".repeat(256));
    let mut w = workspace(&text, "Python", None);
    let e = w.editor_mut();
    set(e, "translate_tabs_to_spaces", json!(true));
    let parses = e.exact_scope_parses.get();
    type_text(e, "\n");
    type_text(e, "else:");
    assert_eq!(e.text.to_string(), format!("{text}\n        else:"));
    assert_eq!(
        e.exact_scope_parses.get(),
        parses,
        "Enter past the stale scope budget"
    );
}

#[test]
fn round21_dedent_retains_same_line_backspace_for_multiple_carets() {
    for (syntax, open, typo, correction, end) in [
        ("Ruby", "def f", "ed", "nd", "end"),
        ("HTML", "<div>", "</dx", "iv>", "</div>"),
    ] {
        // Backspace removes the last scalar typed; feed the corrected token as
        // individual input events while the original highlight is still stale.
        let text = format!("{open}\n{open}");
        let mut w = workspace(&text, syntax, None);
        let e = w.editor_mut();
        set(e, "translate_tabs_to_spaces", json!(true));
        set(e, "auto_match_enabled", json!(false));
        set(e, "auto_close_tags", json!(false));
        e.set_selection(Selection::from_regions([
            Region::caret(open.len()),
            Region::caret(text.len()),
        ]));
        let mut panels = Panels::new(&w, false);
        type_text(w.editor_mut(), "\n");
        type_text(w.editor_mut(), typo);
        key(&mut w, &mut panels, "backspace");
        for c in correction.chars() {
            type_text(w.editor_mut(), c.encode_utf8(&mut [0; 4]));
        }
        assert_eq!(
            w.editor().text.to_string(),
            format!("{open}\n{end}\n{open}\n{end}"),
            "{syntax}"
        );
    }
}

#[test]
fn round21_dedent_randomized_typing_and_backspace_model() {
    let mut rng = 0xb7391d23_u64;
    let mut random = |n: usize| {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        (rng >> 32) as usize % n
    };
    for (syntax, open, markers) in [
        ("Ruby", "def f", &["end", "else", "rescue", "ensure"][..]),
        ("Bash", "if true; then", &["fi", "in", "done", "else"][..]),
    ] {
        let mut w = workspace(open, syntax, None);
        set(w.editor_mut(), "translate_tabs_to_spaces", json!(true));
        let mut panels = Panels::new(&w, false);
        for _ in 0..160 {
            let marker = markers[random(markers.len())];
            let mut typed = String::new();
            let mut indented = true;
            let mut events = Vec::new();
            for c in marker.chars() {
                if random(3) == 0 {
                    events.extend([Some('x'), None]);
                }
                events.push(Some(c));
            }
            events.extend((0..random(7)).map(|_| match random(4) {
                0 => None,
                1 => Some('_'),
                2 => Some('x'),
                _ => Some('e'),
            }));
            type_text(w.editor_mut(), "\n");
            let parses = w.editor().exact_scope_parses.get();
            for event in events {
                if let Some(c) = event {
                    typed.push(c);
                    indented = !markers.contains(&typed.as_str())
                        && !matches!((syntax, typed.as_str()), ("Bash", "do"));
                    type_text(w.editor_mut(), c.encode_utf8(&mut [0; 4]));
                } else if typed.pop().is_some() {
                    key(&mut w, &mut panels, "backspace");
                }
                let indent = if indented { "    " } else { "" };
                assert_eq!(
                    w.editor().text.to_string(),
                    format!("{open}\n{indent}{typed}"),
                    "{syntax}, {marker}, {event:?}"
                );
            }
            assert_eq!(w.editor().exact_scope_parses.get(), parses);
            while w.editor().text.len_bytes() > open.len() {
                key(&mut w, &mut panels, "super+z");
            }
            assert_eq!(w.editor().text.to_string(), open);
        }
    }
}

#[test]
fn round21_comment_flags_variants_and_round_trip() {
    let mut w = workspace("    alpha\n    beta\n\n", "Rust", None);
    let e = w.editor_mut();
    e.run("select_all", &json!({})).unwrap();
    e.run("toggle_comment", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "    // alpha\n    // beta\n\n");
    e.run("toggle_comment", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "    alpha\n    beta\n\n");
    let mut w = workspace("rem one\nREM two\n", "Batch File", None);
    let e = w.editor_mut();
    e.run("select_all", &json!({})).unwrap();
    e.run("toggle_comment", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "one\ntwo\n");
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("Comments.tmPreferences"), r#"<?xml version="1.0"?>
      <plist version="1.0"><dict><key>scope</key><string>text.plain</string>
      <key>settings</key><dict><key>shellVariables</key><array>
      <dict><key>name</key><string>TM_COMMENT_START</string><key>value</key><string># </string></dict>
      <dict><key>name</key><string>TM_COMMENT_START_2</string><key>value</key><string>REM </string></dict>
      <dict><key>name</key><string>TM_COMMENT_DISABLE_INDENT_2</string><key>value</key><string>yes</string></dict>
      <dict><key>name</key><string>TM_COMMENT_CASE_INSENSITIVE_2</string><key>value</key><string>yes</string></dict>
      <dict><key>name</key><string>TM_COMMENT_START_3</string><key>value</key><string>/*</string></dict>
      <dict><key>name</key><string>TM_COMMENT_END_3</string><key>value</key><string>*/</string></dict>
      <dict><key>name</key><string>TM_COMMENT_START_4</string><key>value</key><string>&lt;!</string></dict>
      <dict><key>name</key><string>TM_COMMENT_END_4</string><key>value</key><string>!&gt;</string></dict>
      </array></dict></dict></plist>"#).unwrap();
    let mut w = workspace("    item", "Plain Text", Some(profile.path()));
    let e = w.editor_mut();
    e.run("toggle_comment", &json!({"variant":2})).unwrap();
    assert_eq!(e.text.to_string(), "REM     item");
    e.run("toggle_comment", &json!({"variant":1})).unwrap();
    assert_eq!(e.text.to_string(), "    item");
    e.run("toggle_comment", &json!({"variant":99})).unwrap();
    assert_eq!(e.text.to_string(), "    # item");
    e.run("toggle_comment", &json!({"variant":2})).unwrap();
    assert_eq!(e.text.to_string(), "    item");
    let mut w = workspace(
        "# first\nrem second\nREM third\n",
        "Plain Text",
        Some(profile.path()),
    );
    let e = w.editor_mut();
    e.run("select_all", &json!({})).unwrap();
    e.run("toggle_comment", &json!({"variant":4})).unwrap();
    assert_eq!(e.text.to_string(), "first\nsecond\nthird\n");
    let mut w = workspace("remaining\n", "Plain Text", Some(profile.path()));
    let e = w.editor_mut();
    e.run("select_all", &json!({})).unwrap();
    e.run("toggle_comment", &json!({"variant":2})).unwrap();
    assert_eq!(e.text.to_string(), "REM remaining\n");
    let mut w = workspace("alpha\n  beta", "Plain Text", Some(profile.path()));
    let e = w.editor_mut();
    e.run("select_all", &json!({})).unwrap();
    e.run("toggle_comment", &json!({"block":true,"variant":4}))
        .unwrap();
    assert_eq!(e.text.to_string(), "<!alpha\n  beta!>");
    e.run("toggle_comment", &json!({"block":true,"variant":3}))
        .unwrap();
    assert_eq!(e.text.to_string(), "alpha\n  beta");
    e.run("toggle_comment", &json!({"block":true,"variant":4}))
        .unwrap();
    e.run("toggle_comment", &json!({"variant":2})).unwrap();
    assert_eq!(e.text.to_string(), "alpha\n  beta");
}

#[test]
fn round21_subword_motion_with_configured_delimiters() {
    for (text, stops) in [
        ("fooBarBaz", &([3, 6, 9])[..]),
        ("HTTPServer2Go", &[13]),
        ("a-b", &[1, 2, 3]),
        ("\u{e9}cho\u{c9}lan", &[5, 10]),
    ] {
        let mut w = workspace(text, "Plain Text", None);
        let e = w.editor_mut();
        set(e, "word_separators", json!(""));
        set(e, "sub_word_separators", json!("-_"));
        e.set_caret(0, false);
        let mut panels = Panels::new(&w, false);
        for &stop in stops {
            key(&mut w, &mut panels, "ctrl+right");
            assert_eq!(w.editor().caret, stop, "{text}");
        }
        for &stop in std::iter::once(&0).chain(stops).rev().skip(1) {
            key(&mut w, &mut panels, "ctrl+shift+left");
            assert_eq!(w.editor().caret, stop, "{text}");
            assert_eq!(w.editor().anchor, text.len());
        }
    }
}

#[test]
fn round21_swap_moves_past_a_whole_fold_and_undo_redo_preserve_the_forest() {
    let text = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
    let mut w = workspace(text, "Plain Text", None);
    let e = w.editor_mut();
    ready(e, |e| {
        e.language.borrow().as_ref().unwrap().analysis.is_some()
    });
    for (start, end) in [(1, 5), (7, 9)] {
        e.set_selection(Selection::from_regions([Region::new(
            e.text.line_to_byte(start),
            e.text.line_to_byte(end),
        )]));
        e.run("fold", &json!({})).unwrap();
    }
    e.set_caret(e.text.line_to_byte(5), false);
    assert_eq!(e.folded().len(), 2);
    let mut panels = Panels::new(&w, false);
    key(&mut w, &mut panels, "ctrl+super+up");
    let e = w.editor_mut();
    assert_eq!(
        e.text.to_string(),
        "one\nsix\ntwo\nthree\nfour\nfive\nseven\neight\nnine\n"
    );
    assert_eq!(
        e.folded()
            .iter()
            .map(|r| e.text.slice(r.to_range()).to_string())
            .collect::<Vec<_>>(),
        ["two\nthree\nfour\nfive\n", "eight\nnine\n"]
    );
    e.run("undo", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), text);
    assert_eq!(e.folded().len(), 2);
    e.run("redo", &json!({})).unwrap();
    assert_eq!(e.folded().len(), 2);
}

#[test]
fn round21_fold_swaps_follow_independent_visible_unit_model() {
    #[derive(Clone, Copy)]
    struct Unit {
        rows: &'static str,
        // Offset of the fold into its visible unit and whether it owns the
        // terminating separator, even when that separator is absent at EOF.
        fold: Option<(usize, bool)>,
    }
    fn projection(units: &[Unit]) -> (String, Vec<usize>, Vec<Region>, Vec<String>) {
        let mut text = String::new();
        let mut carets = Vec::new();
        let mut forest = Vec::new();
        let mut contents = Vec::new();
        for (i, unit) in units.iter().enumerate() {
            let start = text.len();
            let separator = i + 1 < units.len();
            carets.push(start);
            text.push_str(unit.rows);
            if separator {
                text.push('\n');
            }
            if let Some((skip, owns_separator)) = unit.fold {
                let bytes = unit.rows.len() + usize::from(owns_separator && separator);
                if skip < bytes {
                    forest.push(Region::new(start + skip, start + bytes));
                    contents.push(text[start + skip..start + bytes].to_owned());
                }
            }
        }
        (text, carets, forest, contents)
    }
    let verify = |e: &Editor, model: &[Unit]| {
        let (text, _, forest, contents) = projection(model);
        assert_eq!(e.text.to_string(), text);
        let folds = e.folded();
        assert_eq!(folds, forest, "{text:?}");
        assert_eq!(
            folds
                .iter()
                .map(|r| e.text.slice(r.to_range()).to_string())
                .collect::<Vec<_>>(),
            contents
        );
    };
    let mut state = 0x165a90fd_u64;
    let mut random = |bound| {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 32) as usize % bound
    };
    for run in 0..40 {
        let mut model = vec![
            Unit {
                rows: "first",
                fold: None,
            },
            Unit {
                rows: "a\nβ\nc",
                fold: Some((0, true)),
            },
            Unit {
                rows: "d",
                fold: None,
            },
            Unit {
                rows: "e\nf",
                fold: Some((1, true)),
            },
            Unit {
                rows: "g",
                fold: None,
            },
            Unit {
                rows: "end\nhidden",
                fold: Some((3, run % 2 == 0)),
            },
        ];
        if run % 2 == 0 {
            model.push(Unit {
                rows: "",
                fold: None,
            });
        }
        let (text, _, forest, _) = projection(&model);
        let mut e = Editor::new(Text::from(text.as_str()));
        e.set_folded(forest);
        for step in 0..42 {
            let (text, carets, _, _) = projection(&model);
            let forward = random(2) == 0;
            let mut selected = vec![false; model.len()];
            selected[random(model.len())] = true;
            if step % 3 == 0 {
                selected[model.len() - 1] = true;
                selected[random(model.len() - 1)] = true;
            } else if step % 3 == 1 {
                selected[random(model.len())] = true;
            }
            e.set_selection(Selection::from_regions(
                carets
                    .into_iter()
                    .zip(&selected)
                    .filter_map(|(at, &s)| s.then_some(Region::caret(at))),
            ));
            // Independent tile model: mark selected units, then move each run
            // across its preceding unselected tile in the direction of travel.
            let previous = model.clone();
            if forward {
                model.reverse();
                selected.reverse();
            }
            let mut i = 1;
            while i < selected.len() {
                if !selected[i - 1] && selected[i] {
                    let mut end = i + 1;
                    while selected.get(end) == Some(&true) {
                        end += 1;
                    }
                    let neighbor = model.remove(i - 1);
                    model.insert(end - 1, neighbor);
                    i = end + 1;
                } else {
                    i += 1;
                }
            }
            if forward {
                model.reverse();
            }
            e.run(
                if forward {
                    "swap_line_down"
                } else {
                    "swap_line_up"
                },
                &json!({}),
            )
            .unwrap();
            verify(&e, &model);
            if e.text.to_string() != text {
                e.run("undo", &json!({})).unwrap();
                verify(&e, &previous);
                e.run("redo", &json!({})).unwrap();
                verify(&e, &model);
            }
        }
    }
}

#[test]
fn round21_select_all_indent_and_comment_keep_nested_folds() {
    let mut w = workspace("header\n    a\n        b\n    c\nend\n", "Python", None);
    let e = w.editor_mut();
    set(e, "translate_tabs_to_spaces", json!(true));
    let original = e.text.to_string();
    for (name, args) in [
        ("indent", json!({})),
        ("unindent", json!({})),
        ("toggle_comment", json!({})),
        ("toggle_comment", json!({})),
    ] {
        if e.folded().is_empty() {
            e.set_folded(vec![
                Region::new("header".len(), e.text.line_to_byte(4)),
                Region::new(e.text.line_to_byte(2) + 8, e.text.line_to_byte(3)),
            ]);
        }
        e.run("select_all", &json!({})).unwrap();
        e.run(name, &args).unwrap();
        assert_eq!(e.folded().len(), 2, "{name}");
        assert_eq!(e.text.byte_to_line(e.folded()[1].begin()), 2);
        assert_eq!(e.text.byte_to_line(e.folded()[0].end()), 4);
    }
    assert_eq!(e.text.to_string(), original);
}

#[test]
fn round21_builtin_pair_backspace_is_one_undo_step() {
    let mut w = workspace("()\n()", "Plain Text", None);
    w.editor_mut().set_selection(Selection::from_regions([
        Region::caret(1),
        Region::caret(4),
    ]));
    let mut panels = Panels::new(&w, false);
    key(&mut w, &mut panels, "backspace");
    assert_eq!(w.editor().text.to_string(), "\n");
    key(&mut w, &mut panels, "super+z");
    assert_eq!(w.editor().text.to_string(), "()\n()");
    assert_eq!(
        w.editor().regions(),
        vec![Region::caret(1), Region::caret(4)]
    );
    key(&mut w, &mut panels, "super+shift+z");
    assert_eq!(w.editor().text.to_string(), "\n");
}

#[test]
fn round21_markup_completion_scopes_and_tab() {
    for (syntax, source, token, label) in [
        ("HTML", "div|", "div", "HTML element"),
        ("HTML", "<di|", "div", "HTML element"),
        ("HTML", "<a hr|>", "href", "HTML attribute"),
        ("HTML", "<div dir=\"rt|\">", "rtl", "HTML value"),
        ("HTML", "<input type=\"em|\">", "email", "HTML value"),
        ("HTML", "<div oncl|>", "onclick", "HTML attribute"),
        ("HTML", "<div cla|>", "class", "HTML attribute"),
        ("HTML", "<div globa|>", "globals", ""),
        ("HTML", "<body onafterp|>", "onafterprint", "HTML attribute"),
        ("HTML", "<div onafterp|>", "onafterprint", ""),
        ("CSS", "a { dis| }", "display", "CSS property"),
        ("CSS", "a { display: fl| }", "flex", "CSS value"),
        ("CSS", "a { color: re| }", "red", "CSS value"),
        ("CSS", "a { color: accentc| }", "AccentColor", "CSS value"),
        ("CSS", "a { azim| }", "azimuth", ""),
        ("CSS", "a { pitc| }", "pitch", ""),
        ("CSS", "a { richn| }", "richness", ""),
        ("CSS", "dis| { }", "display", ""),
        (
            "HTML",
            "<style> a { display: fl| } </style>",
            "flex",
            "CSS value",
        ),
    ] {
        let at = source.find('|').unwrap();
        let mut w = workspace(&source.replace('|', ""), syntax, None);
        let e = w.editor_mut();
        e.set_caret(at, false);
        e.run("auto_complete", &json!({})).unwrap();
        ready(e, |e| !e.completion.busy());
        let item = e
            .completion
            .list
            .as_ref()
            .into_iter()
            .flat_map(|l| &l.items)
            .find(|i| i.trigger == token && i.annotation == label);
        assert_eq!(
            item.is_some(),
            !label.is_empty(),
            "{syntax} {source}: {:?}",
            e.completion.list
        );
    }
    for (typed, expected, caret) in [
        ("div", "<div></div>", 5),
        ("br", "<br>", 4),
        ("<div", "<div></div>", 5),
    ] {
        let mut w = workspace("", "HTML", None);
        type_text(w.editor_mut(), typed);
        let mut panels = Panels::new(&w, false);
        key(&mut w, &mut panels, "tab");
        let e = w.editor_mut();
        ready(e, |e| !e.completion.busy());
        assert_eq!(e.text.to_string(), expected, "{typed}");
        assert_eq!(e.caret, caret, "{typed}");
    }
    let mut w = workspace("<in", "HTML", None);
    let e = w.editor_mut();
    set(e, "disable_default_completions", json!(true));
    e.run("auto_complete", &json!({})).unwrap();
    ready(e, |e| !e.completion.busy());
    assert!(
        e.completion
            .list
            .as_ref()
            .is_none_or(|list| list.items.iter().all(|i| i.annotation != "HTML element"))
    );
}

#[test]
fn round21_text_tag_completion_before_close_or_open_tag() {
    for suffix in ["</p>", "<b>x</b></p>"] {
        let text = format!("<p>div{suffix}");
        let at = "<p>div".len();
        let mut w = workspace(&text, "HTML", None);
        w.editor_mut().set_caret(at, false);
        let mut panels = Panels::new(&w, false);
        key(&mut w, &mut panels, "ctrl+space");
        ready(w.editor_mut(), |e| !e.completion.busy());
        assert!(
            w.editor()
                .completion
                .list
                .as_ref()
                .unwrap()
                .items
                .iter()
                .any(|item| item.trigger == "div" && item.annotation == "HTML element")
        );

        let mut w = workspace(&text, "HTML", None);
        w.editor_mut().set_caret(at, false);
        let mut panels = Panels::new(&w, false);
        key(&mut w, &mut panels, "tab");
        ready(w.editor_mut(), |e| !e.completion.busy());
        assert_eq!(
            w.editor().text.to_string(),
            format!("<p><div></div>{suffix}")
        );
        assert_eq!(w.editor().caret, "<p><div>".len());
    }
}

#[test]
fn round21_repeated_stale_block_comment_variant_uses_exact_scope() {
    for source in ["int a;", "int a;\n    int b;\nint c;"] {
        let mut w = workspace(source, "C", None);
        let e = w.editor_mut();
        scopes_ready(e);
        e.run("select_all", &json!({})).unwrap();
        e.run("toggle_comment", &json!({"variant":2})).unwrap();
        assert_eq!(e.text.to_string(), format!("/*{source}*/"));
        e.set_caret(0, false);
        assert!(
            !e.language
                .borrow()
                .as_ref()
                .unwrap()
                .scopes_ready_at(&e.text, e.caret)
        );
        let mut panels = Panels::new(&w, false);
        key(&mut w, &mut panels, "super+/");
        assert_eq!(w.editor().text.to_string(), source);
    }
    let mut w = workspace("    int a;", "C", None);
    let e = w.editor_mut();
    scopes_ready(e);
    e.run("toggle_comment", &json!({"variant":2})).unwrap();
    assert_eq!(e.text.to_string(), "/*     int a; */");
    assert_eq!(e.caret, e.text.len_bytes());
    let mut panels = Panels::new(&w, false);
    key(&mut w, &mut panels, "super+/");
    assert_eq!(w.editor().text.to_string(), "    int a;");

    let mut w = workspace("const char *s = ", "C", None);
    type_text(w.editor_mut(), "\"/*text*/\";");
    w.editor_mut()
        .set_caret("const char *s = \"/*".len(), false);
    let mut panels = Panels::new(&w, false);
    key(&mut w, &mut panels, "super+/");
    assert_eq!(
        w.editor().text.to_string(),
        "// const char *s = \"/*text*/\";"
    );
}

#[test]
fn round21_markup_selector_excludes_markdown_prose_but_allows_embedded_html() {
    for (syntax, source, expect, override_selector) in [
        ("Markdown", "some di|", false, None),
        ("Markdown", "```html\n<di|\n```", true, None),
        ("PHP", "<di|", true, None),
        ("HTML", "<di|", true, None),
        ("HTML", "<di|", false, Some(json!("source.css"))),
        (
            "HTML",
            "<di|",
            true,
            Some(json!(["text.html", " meta.tag"])),
        ),
    ] {
        let at = source.find('|').unwrap();
        let mut w = workspace(&source.replace('|', ""), syntax, None);
        let e = w.editor_mut();
        e.set_caret(at, false);
        if let Some(value) = override_selector {
            set(e, "default_completions_selector", value);
        }
        e.run("auto_complete", &json!({})).unwrap();
        ready(e, |e| !e.completion.busy());
        let markup = e.completion.list.as_ref().is_some_and(|list| {
            list.items
                .iter()
                .any(|item| item.annotation == "HTML element" && item.trigger == "div")
        });
        assert_eq!(
            markup, expect,
            "{syntax}: {source}: {:?}",
            e.completion.list
        );
    }
    for text in ["I like the table", "see the code"] {
        let mut w = workspace(text, "Markdown", None);
        let mut panels = Panels::new(&w, false);
        key(&mut w, &mut panels, "tab");
        let e = w.editor_mut();
        ready(e, |e| !e.completion.busy());
        assert!(e.text.to_string().starts_with(text));
        assert!(!e.text.to_string().contains('<'));
    }
}

#[test]
fn round21_close_tag_with_only_selections_never_defers() {
    let mut w = workspace("<article>one <b>two", "HTML", None);
    let e = w.editor_mut();
    let selection = Selection::from_regions([Region::new(9, 12), Region::new(19, 16)]);
    e.set_selection(selection.clone());
    e.run("close_tag", &json!({})).unwrap();
    for _ in 0..50 {
        assert!(!e.poll_closing_tag());
        e.poll_language();
        assert!(e.pending_closing_tag.is_none());
    }
    assert_eq!(e.text.to_string(), "<article>one <b>two");
    assert_eq!(e.buffer.view(e.id).sel(), &selection);
}

#[test]
fn round21_close_and_wrap_tags() {
    for (syntax, original, expected) in [
        (
            "HTML",
            "<section><p><br><img src='>'/>hello",
            "<section><p><br><img src='>'/>hello</p>",
        ),
        ("XML", "<Outer><Leaf/>hello", "<Outer><Leaf/>hello</Outer>"),
    ] {
        let mut w = workspace(original, syntax, None);
        let e = w.editor_mut();
        e.run("close_tag", &json!({})).unwrap();
        ready(e, |e| e.pending_closing_tag.is_none());
        assert_eq!(e.text.to_string(), expected);
    }
    let mut w = workspace("one\ntwo", "XML", None);
    w.editor_mut().set_selection(Selection::from_regions([
        Region::new(0, 3),
        Region::new(4, 7),
    ]));
    scopes_ready(w.editor_mut());
    let mut panels = Panels::new(&w, false);
    key(&mut w, &mut panels, "ctrl+shift+w");
    type_text(w.editor_mut(), "span id=\"x\"");
    assert_eq!(
        w.editor().text.to_string(),
        "<span id=\"x\">one</span>\n<span id=\"x\">two</span>"
    );
    key(&mut w, &mut panels, "tab");
    assert_eq!(w.editor().regions().len(), 2);
}

#[test]
fn round21_scope_popup_and_copy_link() {
    let source = "fn main() { let s = \"hello\"; }";
    let mut w = workspace(source, "Rust", None);
    let e = w.editor_mut();
    e.set_caret(source.find("hello").unwrap() + 1, false);
    e.run("show_scope_name", &json!({})).unwrap();
    assert!(
        e.html
            .borrow()
            .popup
            .as_ref()
            .unwrap()
            .html
            .source
            .contains("Scopes not ready yet")
    );
    scopes_ready(e);
    e.run("show_scope_name", &json!({})).unwrap();
    let html = e.html.borrow().popup.as_ref().unwrap().html.source.clone();
    assert!(html.contains("source.rust<br>"));
    assert!(html.contains("string.quoted.double.rust"), "{html}");
    let href = html
        .split_once("href='")
        .unwrap()
        .1
        .split_once('\'')
        .unwrap()
        .0;
    let invocation = crate::html_ui::invocation(vel_minihtml::LinkTarget::parse(href).unwrap());
    e.run(&invocation.command, &invocation.args).unwrap();
    assert!(e.clipboard.starts_with("source.rust "));
    assert!(
        e.clipboard.ends_with("string.quoted.double.rust"),
        "{}",
        e.clipboard
    );
}

#[test]
fn round21_package_loader_keeps_good_resources_and_scope_override() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(user.join("__MACOSX")).unwrap();
    std::fs::write(
        user.join("__MACOSX/._Comments.tmPreferences"),
        b"\0\x05\x16\x07\0\x02",
    )
    .unwrap();
    std::fs::write(user.join("Oops.sublime-syntax"), b"name: \xe9\n").unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        r#"{"tab_size":7,"syntax":"scope:source.python","detect_indentation":false}"#,
    )
    .unwrap();
    crate::test_login::install();
    let mut w = Workspace::new(
        Text::from("print('ok')"),
        Some(profile.path().into()),
        None,
        false,
    )
    .unwrap();
    assert_eq!(w.editor().preferences.tab_size, 7);
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().root,
        "source.python".parse().unwrap()
    );
    let warnings = std::fs::read_to_string(profile.path().join("Velocity.log")).unwrap();
    assert!(warnings.contains("Oops.sublime-syntax"), "{warnings}");
    assert!(!warnings.contains("._Comments.tmPreferences"), "{warnings}");
    assert!(
        w.resource_reports
            .iter()
            .any(|r| r.contains("Oops.sublime-syntax")),
        "{:?}",
        w.resource_reports
    );
    let mut panels = Panels::new(&w, false);
    assert!(
        panels
            .action(
                &mut w,
                "set_file_type",
                &json!({"syntax":"scope:source.rust"}),
                0..40
            )
            .unwrap()
    );
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().root,
        "source.rust".parse().unwrap()
    );
}
