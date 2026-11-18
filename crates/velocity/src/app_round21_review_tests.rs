use super::*;
use vel_buffer::{Region, Selection};
use vel_config::keymap::Key;
use vel_text::Text;

fn fixture(text: &str, syntax: &str) -> App {
    crate::test_login::install();
    let mut workspace = Workspace::new(Text::from(text), None, None, false).unwrap();
    workspace.set_file_type(syntax).unwrap();
    let options = Options::default();
    let mut app = App::new(
        options.clone(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&options, Instant::now()).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
    );
    app.panels.side_visible = false;
    app.sync_active_view();
    app.workspace.editor_mut().set_caret(text.len(), false);
    app
}

fn key(app: &mut App, key: &str) -> bool {
    let viewport = app.viewport();
    app.dispatch_key_binding(None, &Key::parse(key).unwrap(), Instant::now(), viewport)
}

fn selector(app: &mut App, selector: &str) {
    app.run_editor_selector(None, Command::from_selector(selector).unwrap());
}

#[test]
fn round21_ctrl_k_then_native_yank_restores_each_kill() {
    let mut app = fixture("abc\n\nxyz\n", "Plain Text");
    let e = app.workspace.editor_mut();
    e.set_selection(Selection::from_regions([
        Region::caret(1),
        Region::caret(4),
        Region::caret(6),
    ]));
    assert!(key(&mut app, "ctrl+k"));
    assert_eq!(app.workspace.editor().text.to_string(), "a\nx\n");
    assert!(!key(&mut app, "ctrl+y"));
    selector(&mut app, "yank:");
    assert_eq!(app.workspace.editor().text.to_string(), "abc\n\nxyz\n");
    app.workspace.editor_mut().set_caret(4, false);
    assert!(key(&mut app, "ctrl+k"));
    assert_eq!(app.workspace.editor().text.to_string(), "abc\nxyz\n");
    selector(&mut app, "yank:");
    assert_eq!(app.workspace.editor().text.to_string(), "abc\n\nxyz\n");
}

#[test]
fn round21_consecutive_kills_keep_the_latest_entry_separate_from_clipboard() {
    let mut app = fixture("abc\ndef", "Plain Text");
    app.workspace.editor_mut().clipboard = "clipboard".into();
    app.workspace.editor_mut().set_caret(1, false);
    assert!(key(&mut app, "ctrl+k"));
    assert!(key(&mut app, "ctrl+k"));
    assert_eq!(app.workspace.editor().text.to_string(), "adef");
    selector(&mut app, "yank:");
    assert_eq!(app.workspace.editor().text.to_string(), "a\ndef");
    assert_eq!(app.workspace.editor().clipboard, "clipboard");
}

#[test]
fn round21_visual_ctrl_k_exits_visual_but_normal_ctrl_k_still_kills() {
    let mut app = fixture("abcdef\ntail", "Plain Text");
    for setting in [
        "vintage_start_in_command_mode",
        "vintage_ctrl_keys",
        "vintage_enabled",
    ] {
        app.run_named(
            None,
            "set_setting",
            &json!({"setting":setting,"value":true}),
        );
    }
    app.workspace.editor_mut().set_caret(1, false);
    assert!(key(&mut app, "v"));
    assert!(key(&mut app, "l"));
    assert_eq!(app.workspace.editor().selection(), 1..3);
    assert!(key(&mut app, "ctrl+k"));
    assert_eq!(app.workspace.editor().text.to_string(), "adef\ntail");
    assert!(
        app.workspace
            .editor()
            .vintage_status()
            .starts_with("COMMAND")
    );
    assert!(app.workspace.editor().regions().iter().all(|r| r.empty()));
    assert!(key(&mut app, "ctrl+k"));
    assert_eq!(app.workspace.editor().text.to_string(), "a\ntail");
    assert!(
        app.workspace
            .editor()
            .vintage_status()
            .starts_with("COMMAND")
    );
}

#[test]
fn round21_eof_fold_swap_keeps_its_own_separator_and_round_trips() {
    let mut app = fixture("a\nb\nx\ny", "Plain Text");
    let e = app.workspace.editor_mut();
    e.set_folded(vec![Region::new(5, 7)]);
    e.set_caret(4, false);
    assert!(key(&mut app, "ctrl+super+up"));
    assert_eq!(app.workspace.editor().text.to_string(), "a\nx\ny\nb");
    assert_eq!(app.workspace.editor().folded(), [Region::new(3, 5)]);
    assert!(key(&mut app, "super+z"));
    assert_eq!(app.workspace.editor().text.to_string(), "a\nb\nx\ny");
    assert_eq!(app.workspace.editor().folded(), [Region::new(5, 7)]);
    assert!(key(&mut app, "super+shift+z"));
    assert_eq!(app.workspace.editor().folded(), [Region::new(3, 5)]);

    // A fold of just the final separator has no physical range while its
    // line is last. Undo and a fresh move in the opposite direction recover it.
    let mut app = fixture("a\nb\n", "Plain Text");
    app.workspace
        .editor_mut()
        .set_folded(vec![Region::new(3, 4)]);
    app.workspace.editor_mut().set_caret(2, false);
    assert!(key(&mut app, "ctrl+super+down"));
    assert_eq!(app.workspace.editor().text.to_string(), "a\n\nb");
    assert!(app.workspace.editor().folded().is_empty());
    assert!(key(&mut app, "super+z"));
    assert_eq!(app.workspace.editor().folded(), [Region::new(3, 4)]);
    assert!(key(&mut app, "super+shift+z"));
    assert!(key(&mut app, "ctrl+super+up"));
    assert_eq!(app.workspace.editor().text.to_string(), "a\nb\n");
    assert_eq!(app.workspace.editor().folded(), [Region::new(3, 4)]);

    // At EOF the inner and outer folds can temporarily have equal physical
    // ranges. Their distinct newline ownership must survive undo.
    let mut app = fixture("a\nb\nz", "Plain Text");
    app.workspace
        .editor_mut()
        .set_folded(vec![Region::new(2, 4), Region::new(2, 3)]);
    app.workspace.editor_mut().set_caret(2, false);
    assert!(key(&mut app, "ctrl+super+down"));
    assert_eq!(
        app.workspace.editor().folded(),
        [Region::new(4, 5), Region::new(4, 5)]
    );
    assert!(key(&mut app, "super+z"));
    assert_eq!(
        app.workspace.editor().folded(),
        [Region::new(2, 4), Region::new(2, 3)]
    );
}

#[test]
fn round21_unindent_undo_restores_a_column_zero_fold() {
    let mut app = fixture("    header\n    hidden\nend", "Plain Text");
    let e = app.workspace.editor_mut();
    e.set_folded(vec![Region::new(0, "    header\n    hidden\n".len())]);
    e.set_caret(0, false);
    assert!(key(&mut app, "super+["));
    assert_eq!(
        app.workspace.editor().text.to_string(),
        "header\n    hidden\nend"
    );
    assert_eq!(
        app.workspace.editor().folded(),
        [Region::new(0, "header\n    hidden\n".len())]
    );
    assert!(key(&mut app, "super+z"));
    assert_eq!(
        app.workspace.editor().folded(),
        [Region::new(0, "    header\n    hidden\n".len())]
    );
    assert!(key(&mut app, "super+shift+z"));
    assert_eq!(
        app.workspace.editor().folded(),
        [Region::new(0, "header\n    hidden\n".len())]
    );
}

#[test]
fn round21_vintage_gets_ctrl_keys_before_native_editing() {
    let mut app = fixture("abc\nxyz", "Plain Text");
    app.workspace.editor_mut().set_caret(5, false);
    assert!(key(&mut app, "ctrl+k")); // Make the kill ring nonempty.
    app.run_named(
        None,
        "set_setting",
        &json!({"setting": "vintage_start_in_command_mode","value": true}),
    );
    app.run_named(
        None,
        "set_setting",
        &json!({"setting": "vintage_ctrl_keys","value": true}),
    );
    app.run_named(
        None,
        "set_setting",
        &json!({"setting": "vintage_enabled","value": true}),
    );
    app.workspace.editor_mut().set_caret(1, false);
    assert_eq!(
        app.workspace
            .editor()
            .vintage
            .as_ref()
            .unwrap()
            .engine
            .mode(),
        vel_vintage::Mode::Normal
    );
    assert!(key(&mut app, "ctrl+o"));
    assert_eq!(app.workspace.editor().caret, 1);
    selector(&mut app, "insertNewlineIgnoringFieldEditor:");
    assert_eq!(app.workspace.editor().text.to_string(), "abc\nx");
    assert!(key(&mut app, "ctrl+y")); // Vintage scroll, not yank.
    selector(&mut app, "yank:"); // Also safe if a user supplies a native binding.
    assert_eq!(app.workspace.editor().text.to_string(), "abc\nx");
    assert!(key(&mut app, "i"));
    selector(&mut app, "yank:");
    assert_eq!(app.workspace.editor().text.to_string(), "ayzbc\nx");
}

#[test]
fn round21_native_visual_lines_paragraphs_and_deletes() {
    let mut app = fixture("abcdefghijklmnopqrstuvwxyz\n    tail\nz", "Plain Text");
    for (setting, value) in [
        ("word_wrap", json!(true)),
        ("wrap_width", json!(8)),
        ("translate_tabs_to_spaces", json!(true)),
    ] {
        app.run_named(
            None,
            "set_setting",
            &json!({"setting":setting,"value":value}),
        );
    }
    app.workspace.editor_mut().set_caret(12, false);
    selector(&mut app, "moveToBeginningOfLine:");
    assert_eq!(app.workspace.editor().caret, 8);
    app.workspace.editor_mut().set_caret(12, false);
    selector(&mut app, "moveToEndOfLine:");
    assert_eq!(app.workspace.editor().caret, 15);
    selector(&mut app, "moveToEndOfParagraph:");
    assert_eq!(app.workspace.editor().caret, 26);
    selector(&mut app, "moveParagraphForwardAndModifySelection:");
    assert_eq!(app.workspace.editor().selection(), 26..35);
    selector(&mut app, "moveParagraphBackwardAndModifySelection:");
    assert_eq!(app.workspace.editor().selection(), 26..27);
    app.workspace.editor_mut().set_caret(12, false);
    selector(&mut app, "deleteToBeginningOfLine:");
    assert_eq!(
        app.workspace.editor().text.to_string(),
        "abcdefghmnopqrstuvwxyz\n    tail\nz"
    );
    selector(&mut app, "yank:");
    assert!(
        app.workspace
            .editor()
            .text
            .to_string()
            .starts_with("abcdefghijklmnopqrstuvwxyz")
    );
    app.workspace.editor_mut().set_caret(32, false);
    selector(&mut app, "insertBacktab:");
    assert_eq!(
        app.workspace.editor().text.to_string(),
        "abcdefghijklmnopqrstuvwxyz\ntail\nz"
    );

    let mut app = fixture("é e\u{301}", "Plain Text");
    app.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([
            Region::caret(2),
            Region::caret(6),
        ]));
    selector(&mut app, "deleteBackwardByDecomposingPreviousCharacter:");
    assert_eq!(app.workspace.editor().text.to_string(), "e e");
}
