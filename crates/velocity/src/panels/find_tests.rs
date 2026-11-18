//! Current-file Find through the real App keymap, committed input and CPU scene.
use super::*;
use std::ops::Range;
use vel_platform::Size;

fn find_app(text: &str) -> App {
    let mut a = app(text, "Plain Text", None);
    a.view.size = Size {
        width: 800.,
        height: 400.,
    };
    a.workspace.editor_mut().set_caret(0, false);
    set(&mut a, "find_selected_text", json!(false));
    set(&mut a, "close_find_after_find_all", json!(false));
    a
}

fn poll_find(a: &mut App) {
    let viewport = a.viewport();
    a.panels.poll(&mut a.workspace, viewport);
    if std::mem::take(&mut a.panels.reveal) {
        a.view.reveal(a.workspace.editor());
    }
}

fn settle(a: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        poll_find(a);
        if !a.panels.busy() {
            return;
        }
        assert!(Instant::now() < deadline, "Find did not settle");
        std::thread::yield_now();
    }
}

fn type_query(a: &mut App, value: &str) {
    assert!(a.panels.input().is_some());
    let viewport = a.viewport();
    assert!(a.panels.text_input(
        &mut a.workspace,
        TextInput::Insert {
            text: value.into(),
            replacement: None
        },
        viewport,
    ));
    a.changed_state(Instant::now(), false);
}

fn toggle(a: &mut App, option: &str) {
    named(a, "toggle_find_option", json!({"option":option}));
}

fn hits(a: &App) -> &[Range<usize>] {
    a.panels.visible_highlights()
}

fn selected(a: &App) -> Vec<Range<usize>> {
    a.workspace
        .editor()
        .regions()
        .iter()
        .map(|r| r.to_range())
        .collect()
}

fn select(a: &mut App, range: Range<usize>) {
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::from(range)]));
}

fn painted_message(a: &mut App, expected: &str) {
    let scene = a.scene();
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .any(|run| run.text == expected),
        "missing status {expected:?}; message was {:?}",
        a.panels.message
    );
}

#[test]
fn cmd_f_scope_follows_selection_panel_session_and_tab() {
    let mut a = find_app("begin\nhit\nend\nhit");
    let original = a.workspace.active;
    named(&mut a, "new_file", json!({}));
    input(&mut a, "hit other");
    let other = a.workspace.active;
    assert_ne!(original, other);
    a.workspace.select(original);
    set(&mut a, "auto_find_in_selection", json!(true));
    select(&mut a, 0..10);
    key(&mut a, "super+f");
    assert!(a.panels.in_selection);
    type_query(&mut a, "hit");
    settle(&mut a);
    assert_eq!(hits(&a), std::slice::from_ref(&(6..9)));
    key(&mut a, "enter");
    settle(&mut a);
    assert!(
        a.panels.in_selection,
        "a result selection is not a user collapse"
    );
    select(&mut a, 0..0);
    settle(&mut a);
    assert!(
        !a.panels.in_selection,
        "the user collapsed the original scope"
    );
    assert_eq!(hits(&a), [6..9, 14..17]);

    // An explicit in-session toggle is allowed without a selection. Reopening resets it.
    toggle(&mut a, "in_selection");
    settle(&mut a);
    assert!(a.panels.in_selection);
    assert!(hits(&a).is_empty());
    painted_message(&mut a, "No matches (In Selection on; no selection)");
    key(&mut a, "super+f");
    key(&mut a, "super+f");
    settle(&mut a);
    assert!(!a.panels.in_selection);
    assert_eq!(hits(&a), [6..9, 14..17]);

    toggle(&mut a, "in_selection");
    a.workspace.select(other);
    settle(&mut a);
    assert!(!a.panels.in_selection, "no scope is carried to another tab");
    assert_eq!(hits(&a), std::slice::from_ref(&(0..3)));
}

#[test]
fn cmd_f_reopen_resets_manual_scope_and_restart_retains_only_query() {
    let mut a = find_app("hit\nend\nhit");
    select(&mut a, 0..8);
    key(&mut a, "super+f");
    toggle(&mut a, "in_selection");
    type_query(&mut a, "hit");
    key(&mut a, "enter");
    settle(&mut a);
    assert_eq!(hits(&a), std::slice::from_ref(&(0..3)));
    key(&mut a, "escape");
    select(&mut a, 0..0);
    key(&mut a, "super+f");
    settle(&mut a);
    assert!(!a.panels.in_selection);
    assert_eq!(hits(&a), [0..3, 8..11]);

    let mut session = vel_session::Window::default();
    a.panels.capture_session(&mut session);
    let mut restored = find_app("hit\nend\nhit");
    restored
        .panels
        .restore_session(&restored.workspace, &session);
    key(&mut restored, "super+f");
    settle(&mut restored);
    assert!(!restored.panels.in_selection);
    assert_eq!(hits(&restored), [0..3, 8..11]);
}

#[test]
fn cmd_f_reports_restrictions_and_exact_count_in_scene() {
    let mut a = find_app("CAT CATS CAT");
    key(&mut a, "super+f");
    type_query(&mut a, "cat");
    settle(&mut a);
    painted_message(&mut a, "3 matches");
    toggle(&mut a, "whole_word");
    settle(&mut a);
    assert_eq!(hits(&a), [0..3, 9..12]);
    painted_message(&mut a, "2 matches");
    toggle(&mut a, "case_sensitive");
    settle(&mut a);
    assert!(hits(&a).is_empty());
    painted_message(&mut a, "No matches (Match Case on, Whole Word on)");
    key(&mut a, "enter");
    named(&mut a, "find_all", json!({}));
    settle(&mut a);
    painted_message(&mut a, "No matches (Match Case on, Whole Word on)");
}

#[test]
fn cmd_f_pending_navigation_keeps_every_key_and_direction() {
    let mut a = find_app(&"hit no\n".repeat(10_000));
    key(&mut a, "super+f");
    type_query(&mut a, "hit");
    key(&mut a, "enter");
    key(&mut a, "enter");
    key(&mut a, "shift+enter");
    settle(&mut a);
    assert_eq!(a.workspace.editor().selection(), 7..10);
    named(&mut a, "find_prev", json!({}));
    assert_eq!(a.workspace.editor().selection(), 0..3);
    named(&mut a, "find_prev", json!({}));
    assert_eq!(a.workspace.editor().selection(), 69_993..69_996);
    named(&mut a, "find_next", json!({}));
    assert_eq!(a.workspace.editor().selection(), 0..3);
}

#[test]
fn cmd_f_enter_and_find_all_wait_for_the_final_progressive_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("long.txt");
    let mut body =
        "padding padding padding padding padding padding padding padding\n".repeat(150_000);
    let tail = body.len();
    body.push_str("EOF-only hit\n");
    std::fs::write(&path, body).unwrap();
    for command in ["enter", "alt+enter"] {
        let mut a = find_app("");
        a.workspace.settings.set(
            vel_config::settings::Layer::User,
            "large_file.load_bytes",
            json!(1_000_000),
        );
        a.workspace
            .open(crate::cli::FileLocation {
                path: path.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        assert!(a.workspace.tabs[a.workspace.active].loading.is_some());
        a.sync_active_view();
        key(&mut a, "super+f");
        type_query(&mut a, "EOF-only");
        key(&mut a, command);
        // Do not deliver file IO yet. This reproduces a completed search of a partial snapshot.
        for _ in 0..10 {
            poll_find(&mut a);
        }
        assert_ne!(a.workspace.editor().selection(), tail..tail + 8);
        let deadline = Instant::now() + Duration::from_secs(20);
        while a.workspace.tabs[a.workspace.active].loading.is_some() {
            a.workspace.poll();
            poll_find(&mut a);
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        settle(&mut a);
        assert_eq!(
            a.workspace.editor().selection(),
            tail..tail + 8,
            "{command}"
        );
        if command == "alt+enter" {
            assert!(a.panels.panel.is_none());
            key(&mut a, "super+f");
            settle(&mut a);
        }
        assert_eq!(
            hits(&a),
            std::slice::from_ref(&(tail..tail + 8)),
            "{command}"
        );
        painted_message(&mut a, "1 match");
    }
}

#[test]
fn cmd_f_unicode_literal_offsets_and_case_folding() {
    let text = "前\t🦀 e\u{301} é ß ẞ SS İ i Σ σ ς Ａ ａ A\r\n後\t🦀 e\u{301}";
    let mut a = find_app(text);
    key(&mut a, "super+f");
    for (query, spellings) in [
        ("🦀", vec!["🦀", "🦀"]),
        ("e\u{301}", vec!["e\u{301}", "e\u{301}"]),
        ("é", vec!["é"]),
        ("ß", vec!["ß", "ẞ"]),
        ("i", vec!["i"]),
        ("İ", vec!["İ"]),
        ("Σ", vec!["Σ", "σ", "ς"]),
        ("Ａ", vec!["Ａ", "ａ"]),
        ("後", vec!["後"]),
    ] {
        key(&mut a, "super+a");
        type_query(&mut a, query);
        settle(&mut a);
        let mut end = 0;
        let expected: Vec<_> = spellings
            .into_iter()
            .map(|word| {
                let at = end + text[end..].find(word).unwrap();
                end = at + word.len();
                at..end
            })
            .collect();
        assert_eq!(hits(&a), expected, "{query}");
        named(&mut a, "find_all", json!({}));
        settle(&mut a);
        assert_eq!(selected(&a), expected, "{query}");
    }
}

#[test]
fn cmd_f_highlight_intersects_combining_cluster_on_wrapped_row() {
    let mut a = find_app(&format!(
        "{}e\u{301}\tend\nsecond e\u{301}",
        "wrap ".repeat(40)
    ));
    a.view.size.width = 320.;
    set(&mut a, "word_wrap", json!(true));
    let _ = a.scene();
    let text = a.workspace.editor().text.to_string();
    let first = text.find('\u{301}').unwrap();
    let second = text.rfind('\u{301}').unwrap();
    key(&mut a, "super+f");
    type_query(&mut a, "\u{301}");
    settle(&mut a);
    assert_eq!(hits(&a), [first..first + 2, second..second + 2]);
    assert_eq!(a.workspace.editor().selection(), first..first + 2);
    key(&mut a, "enter");
    settle(&mut a);
    assert_eq!(a.workspace.editor().selection(), second..second + 2);

    let e = a.workspace.editor();
    let expected = crate::view::color(
        &e.language.borrow().as_ref().unwrap().scheme,
        "find_highlight",
    );
    let target = a.view.rect_at(e, first - 1);
    let scene = a.scene();
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|layer| &layer.rects)
            .any(|rect| rect.color == expected
                && (rect.rect.y - target.y).abs() < a.view.line_height),
        "the nonselected match inside the grapheme must be painted on its wrapped row"
    );
}

#[test]
fn cmd_f_regex_whole_word_empty_invalid_and_multiline() {
    let text = "前猫 猫 猫_字 猫-字\r\nX\r\nY";
    let mut a = find_app(text);
    // Public docs describe word_separators for navigation, not regex \b.
    set(&mut a, "word_separators", json!("_"));
    key(&mut a, "super+f");
    toggle(&mut a, "whole_word");
    type_query(&mut a, "猫");
    settle(&mut a);
    assert_eq!(hits(&a), [7..10, 19..22]);
    toggle(&mut a, "whole_word");
    toggle(&mut a, "regex");
    key(&mut a, "super+a");
    type_query(&mut a, r"\b猫\b");
    settle(&mut a);
    assert_eq!(hits(&a), [7..10, 19..22]);
    key(&mut a, "super+a");
    type_query(&mut a, r"X\r?\nY");
    settle(&mut a);
    let start = text.find('X').unwrap();
    assert_eq!(a.workspace.editor().selection(), start..text.len());
    named(&mut a, "find_all", json!({}));
    settle(&mut a);
    assert_eq!(selected(&a), std::slice::from_ref(&(start..text.len())));
    key(&mut a, "super+a");
    type_query(&mut a, "[");
    settle(&mut a);
    assert!(hits(&a).is_empty());
    assert!(a.panels.message.contains("unclosed character class"));
    key(&mut a, "super+a");
    key(&mut a, "backspace");
    settle(&mut a);
    assert!(hits(&a).is_empty());
    assert!(a.panels.message.is_empty());
}

#[test]
fn cmd_f_does_not_cap_dense_matches_and_recomputes_after_edits() {
    let repetitions = 12_000;
    let mut a = find_app(&"hit no\n".repeat(repetitions));
    key(&mut a, "super+f");
    type_query(&mut a, "h");
    type_query(&mut a, "it");
    settle(&mut a);
    assert_eq!(hits(&a).len(), repetitions);
    named(&mut a, "find_prev", json!({}));
    let last = (repetitions - 1) * 7;
    assert_eq!(a.workspace.editor().selection(), last..last + 3);
    key(&mut a, "alt+enter");
    settle(&mut a);
    assert_eq!(selected(&a).len(), repetitions);
    assert_eq!(selected(&a).last(), Some(&(last..last + 3)));

    // Match offsets come from the newest document revision, not a syntax index.
    select(&mut a, 0..3);
    key(&mut a, "super+f");
    settle(&mut a);
    let _ = a.scene();
    assert!(!a.view.find_matches.is_empty());
    a.workspace
        .editor_mut()
        .run("insert", &json!({"characters":"前 no"}))
        .unwrap();
    let _ = a.scene();
    assert!(
        a.view.find_matches.is_empty(),
        "do not paint stale byte ranges before the poll"
    );
    named(&mut a, "find_all", json!({"_close_panel":false}));
    settle(&mut a);
    assert_eq!(hits(&a).len(), repetitions - 1);
    assert_eq!(selected(&a).first(), Some(&(10..13)));
    assert_eq!(selected(&a).last(), Some(&(last + 3..last + 6)));
}

#[test]
fn cmd_i_shift_enter_and_named_find_prev_respect_wrap() {
    let mut a = find_app("hit\n----\nhit\n----\nhit");
    key(&mut a, "super+i");
    type_query(&mut a, "hit");
    settle(&mut a);
    assert_eq!(a.workspace.editor().selection(), 0..3);
    key(&mut a, "shift+enter");
    settle(&mut a);
    assert_eq!(a.workspace.editor().selection(), 18..21);
    key(&mut a, "enter");
    settle(&mut a);
    assert!(a.panels.panel.is_none());
    assert_eq!(a.workspace.editor().selection(), 0..3);
    key(&mut a, "super+i");
    toggle(&mut a, "wrap");
    settle(&mut a);
    named(&mut a, "find_prev", json!({}));
    assert_eq!(a.workspace.editor().selection(), 0..3);
    assert_eq!(hits(&a).len(), 3);
}

#[test]
fn cmd_f_jump_reveals_nested_folds_and_preserves_unrelated_fold() {
    let text = "header\nouter\ninner hidden hit\ninner end\nouter end\nother fold\nfooter";
    let mut a = find_app(text);
    let other = text.find("other fold").unwrap();
    let footer = text.find("footer").unwrap();
    a.workspace.editor_mut().set_folded(vec![
        Region::new(7, other),
        Region::new(13, 40),
        Region::new(other, footer),
    ]);
    key(&mut a, "super+f");
    type_query(&mut a, "hit");
    settle(&mut a);
    let target = text.find("hit").unwrap();
    assert_eq!(hits(&a), std::slice::from_ref(&(target..target + 3)));
    assert_eq!(a.workspace.editor().selection(), target..target + 3);
    let _ = a.scene();
    assert_eq!(a.workspace.editor().folded(), [Region::new(other, footer)]);
}

#[test]
fn cmd_f_finds_a_case_insensitive_hit_inserted_at_a_chunk_boundary() {
    let source = "bookkeeper\n".repeat(20);
    let mut a = find_app(&source);
    select(&mut a, 4..4);
    input(&mut a, "k");
    key(&mut a, "super+f");
    type_query(&mut a, "k");
    settle(&mut a);
    assert_eq!(hits(&a).len(), 41);
    assert!(hits(&a).contains(&(4..5)));
    select(&mut a, 3..3);
    key(&mut a, "enter");
    key(&mut a, "enter");
    assert_eq!(a.workspace.editor().selection(), 4..5);
    key(&mut a, "alt+enter");
    settle(&mut a);
    assert!(selected(&a).contains(&(4..5)));
    select(&mut a, 0..0);
    key(&mut a, "super+alt+f");
    a.panels.replace.set("Q");
    named(&mut a, "replace_all", json!({"_close_panel":false}));
    assert!(!a.workspace.editor().text.to_string().contains('k'));
}

#[test]
fn pending_find_all_and_incremental_enter_release_input_immediately() {
    let doc = "hit\n".to_owned() + &"padding\n".repeat(8192) + "hit\n";
    for (open, accept) in [("super+f", "alt+enter"), ("super+i", "enter")] {
        let mut a = find_app(&doc);
        key(&mut a, open);
        type_query(&mut a, "no");
        settle(&mut a);
        key(&mut a, "super+a");
        type_query(&mut a, "hit");
        key(&mut a, accept);
        assert!(a.panels.input().is_none(), "{open} accepted while busy");
        input(&mut a, "X");
        assert_eq!(a.panels.query.value(), "hit");
        assert_eq!(a.workspace.editor().text.to_string(), format!("X{doc}"));
        settle(&mut a);
        assert!(a.panels.panel.is_none());
        let end = doc.len();
        let wanted = if open == "super+f" {
            vec![1..4, end - 3..end]
        } else {
            std::iter::once(end - 3..end).collect()
        };
        assert_eq!(selected(&a), wanted, "{open}");
    }
}

#[test]
fn replace_all_keeps_text_replaced_at_the_scope_start() {
    let mut a = find_app("hit a\nhit b\nhit c\nhit d\n");
    select(&mut a, 6..18);
    key(&mut a, "super+alt+f");
    toggle(&mut a, "in_selection");
    type_query(&mut a, "hit");
    settle(&mut a);
    assert_eq!(hits(&a), [6..9, 12..15]);
    a.panels.replace.set("HITS");
    named(&mut a, "replace_all", json!({"_close_panel":false}));
    key(&mut a, "super+a");
    type_query(&mut a, "HITS");
    settle(&mut a);
    assert_eq!(hits(&a), [6..10, 13..17]);
}

#[test]
fn ctrl_tab_switches_tabs_from_find_and_plain_tab_cycles_inputs() {
    let mut a = find_app("first");
    let first = a.workspace.active;
    named(&mut a, "new_file", json!({}));
    input(&mut a, "second");
    let second = a.workspace.active;
    assert_ne!(first, second);
    key(&mut a, "super+alt+f");
    key(&mut a, "tab");
    type_query(&mut a, "replacement");
    assert_eq!(a.panels.replace.value(), "replacement");
    key(&mut a, "shift+tab");
    type_query(&mut a, "query");
    assert_eq!(a.panels.query.value(), "query");
    key(&mut a, "ctrl+tab");
    assert_eq!(a.workspace.active, first);
    key(&mut a, "ctrl+shift+tab");
    assert_eq!(a.workspace.active, second);
}

#[test]
fn cmd_f_adjacent_matches_are_independent_edits() {
    let mut a = find_app("kk kk");
    key(&mut a, "super+f");
    type_query(&mut a, "k");
    key(&mut a, "alt+enter");
    assert_eq!(selected(&a), [0..1, 1..2, 3..4, 4..5]);
    input(&mut a, "Z");
    assert_eq!(a.workspace.editor().text.to_string(), "ZZ ZZ");
    named(&mut a, "undo", json!({}));
    assert_eq!(selected(&a), [0..1, 1..2, 3..4, 4..5]);
}

#[test]
#[ignore = "headless timing probe, including a 100 MB document"]
fn cmd_f_large_file_main_thread_timing() {
    let line_size = 10_000_000;
    let single_line = "x".repeat(line_size) + "\nKey Needle";
    let line = "a".repeat(984) + " Key Needle 123\n";
    assert_eq!(line.len(), 1000);
    let dense = line.repeat(100_000);
    for (source, query, regex, count, first, last) in [
        (
            single_line.as_str(),
            "Key Needle",
            false,
            1,
            line_size + 1..line_size + 11,
            line_size + 1..line_size + 11,
        ),
        (
            dense.as_str(),
            "Key Needle",
            false,
            100_000,
            985..995,
            dense.len() - 15..dense.len() - 5,
        ),
        (
            dense.as_str(),
            r"Key\s+Needle [0-9]{3}",
            true,
            100_000,
            985..999,
            dense.len() - 15..dense.len() - 1,
        ),
    ] {
        for case_sensitive in [false, true] {
            let mut a = find_app(source);
            let started = Instant::now();
            key(&mut a, "super+f");
            let open = started.elapsed();
            if case_sensitive {
                toggle(&mut a, "case_sensitive");
            }
            if regex {
                toggle(&mut a, "regex");
            }
            let mut max_key = Duration::ZERO;
            if source.len() < 11_000_000 {
                for letter in query.chars() {
                    let at = Instant::now();
                    type_query(&mut a, &letter.to_string());
                    max_key = max_key.max(at.elapsed());
                }
            } else {
                type_query(&mut a, query);
            }
            let at = Instant::now();
            key(&mut a, "alt+enter");
            let find_all = at.elapsed();
            settle(&mut a);
            let completed = started.elapsed();
            let selected = selected(&a);
            assert_eq!(selected.len(), count);
            assert_eq!(selected.first(), Some(&first));
            assert_eq!(selected.last(), Some(&last));
            println!(
                "FIND_TIMING bytes={} regex={regex} case={case_sensitive} selections={count} open={open:?} max_key={max_key:?} find_all={find_all:?} completed={completed:?}",
                source.len(),
            );
        }
    }
}

#[test]
fn super_nine_selects_the_last_tab_through_a_negative_index() {
    // The documented macOS Cmd+9 binding passes index -1: select the last tab.
    let mut a = find_app("first");
    for _ in 0..4 {
        named(&mut a, "new_file", json!({}));
    }
    let last = a.workspace.active;
    named(&mut a, "select_by_index", json!({"index": 0}));
    assert_ne!(a.workspace.active, last);
    key(&mut a, "super+9");
    assert_eq!(a.workspace.active, last);
    named(&mut a, "select_by_index", json!({"index": -2}));
    assert_ne!(a.workspace.active, last);
    named(&mut a, "select_by_index", json!({"index": -99}));
    assert_ne!(
        a.workspace.active, last,
        "an out-of-range index changes nothing"
    );
}
