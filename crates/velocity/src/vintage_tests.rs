//! Full host routing, committed IME input and CPU scenes; no native services.
use crate::{
    panels::{Panel, Panels},
    view::View,
    workspace::Workspace,
};
use vel_buffer::{Region, Selection};
use vel_config::{json, keymap::Key, settings::Layer};
use vel_platform::{Size, TextInput, Utf16Range};
use vel_text::Text;

fn setup(text: &str) -> (Workspace, Panels, View) {
    let mut w = Workspace::new(Text::from(text), None, None, false).unwrap();
    w.editor_mut()
        .settings
        .set(Layer::View, "vintage_enabled", json!(true));
    w.editor_mut()
        .settings
        .set(Layer::View, "vintage_start_in_command_mode", json!(true));
    w.editor_mut().sync();
    let p = Panels::new(&w, false);
    let v = View::new(
        Size {
            width: 900.,
            height: 600.,
        },
        7.8,
        12.,
    );
    (w, p, v)
}
fn key(w: &mut Workspace, p: &mut Panels, v: &mut View, name: &str) {
    let key = if name.len() == 1 && name.chars().next().unwrap().is_ascii_uppercase() {
        Key::from_parts(&name.to_ascii_lowercase(), 4)
    } else {
        Key::parse(name).unwrap()
    };
    match p.resolve_key(w, &key, 0..usize::MAX).unwrap() {
        vel_config::keymap::Resolution::Command(action) => {
            w.editor_mut().run(&action.command, &action.args).unwrap()
        }
        vel_config::keymap::Resolution::Pending => {}
        vel_config::keymap::Resolution::Unbound => {
            let input = TextInput::Insert {
                text: name.into(),
                replacement: None,
            };
            if !p.text_input(w, input.clone(), 0..usize::MAX) {
                w.editor_mut().text_input(input);
            }
        }
    }
    p.sync_vintage_prompt(w, 0..usize::MAX).unwrap();
    let bounds = p.layout(Size {
        width: 900.,
        height: 600.,
    });
    v.size = Size {
        width: bounds.width,
        height: bounds.height,
    };
    let mut scene = v.scene(w.editor());
    p.paint(w, &mut scene);
    assert!(!scene.layers.is_empty());
}
fn text(w: &mut Workspace, p: &mut Panels, v: &mut View, chars: &str) {
    for c in chars.chars() {
        key(w, p, v, &c.to_string());
    }
}

#[test]
fn commands_counts_objects_registers_dot_macros_and_history_reach_scene() {
    let cases = [
        ("one two three", "dw", "two three"),
        ("one two three", "2dw", "three"),
        ("one two three", "wdiw", "one  three"),
        ("one\ntwo\nthree\n", "dd", "two\nthree\n"),
        ("one two three", "dw.", "three"),
        ("abc", "xup", "aabc"),
        ("one two three", "qaxq@a", "e two three"),
        ("one\ntwo\n", "J", "one two\n"),
        ("abc", "rZ", "Zbc"),
    ];
    for (before, keys, after) in cases {
        let (mut w, mut p, mut v) = setup(before);
        text(&mut w, &mut p, &mut v, keys);
        assert_eq!(w.editor().text.to_string(), after, "{keys}");
    }
    let (mut w, mut p, mut v) = setup("alpha beta gamma");
    text(&mut w, &mut p, &mut v, "2d");
    assert!(w.editor().vintage_status().contains("2d"));
    text(&mut w, &mut p, &mut v, "w");
    key(&mut w, &mut p, &mut v, "u");
    assert_eq!(w.editor().text.to_string(), "alpha beta gamma");
    w.editor_mut()
        .settings
        .set(Layer::View, "vintage_ctrl_keys", json!(true));
    w.editor_mut().sync();
    text(&mut w, &mut p, &mut v, "dw");
    key(&mut w, &mut p, &mut v, "u");
    key(&mut w, &mut p, &mut v, "ctrl+r");
    assert_eq!(w.editor().text.to_string(), "beta gamma");
}

#[test]
fn visual_block_carets_multicursor_clipboard_and_passthrough_contexts() {
    let (mut w, mut p, mut v) = setup("one two\nred blue\n");
    let e = w.editor_mut();
    e.set_selection(Selection::from_regions([
        Region::caret(0),
        Region::caret(8),
    ]));
    text(&mut w, &mut p, &mut v, "dw");
    assert_eq!(w.editor().text.to_string(), "two\nblue\n");
    key(&mut w, &mut p, &mut v, "u");
    assert_eq!(w.editor().regions().len(), 2);
    w.editor_mut().set_caret(0, false);
    text(&mut w, &mut p, &mut v, "Vj");
    assert!(w.editor().vintage_status().contains("VISUAL LINE"));
    let scene = v.scene(w.editor());
    assert!(scene.layers[1].rects.iter().any(|r| r.rect.width == v.cell));
    key(&mut w, &mut p, &mut v, "escape");
    w.editor_mut()
        .settings
        .set(Layer::View, "vintage_use_clipboard", json!(true));
    w.editor_mut().sync();
    w.editor_mut().set_caret(0, false);
    text(&mut w, &mut p, &mut v, "yw");
    assert_eq!(w.editor().clipboard, "one ");
    w.editor_mut().clipboard = "paste".into();
    key(&mut w, &mut p, &mut v, "p");
    assert!(w.editor().text.to_string().contains("paste"));
    w.editor_mut().keymap.append(r#"[{"keys":["ctrl+r"],"command":"insert","args":{"characters":"PASS"},"context":[{"key":"setting.command_mode"},{"key":"vi_mode_normal"}]}]"#).unwrap();
    key(&mut w, &mut p, &mut v, "ctrl+r");
    assert!(w.editor().text.to_string().contains("PASS"));
}

#[test]
fn insert_ime_multicaret_undo_and_live_enable_disable_keep_native_preview() {
    let (mut w, mut p, mut v) = setup("a🦀z\nb🦀q");
    w.editor_mut().set_selection(Selection::from_regions([
        Region::from(1..5),
        Region::from(8..12),
    ]));
    key(&mut w, &mut p, &mut v, "i");
    // Re-establish selected ranges after the normal-to-insert transition.
    w.editor_mut().set_selection(Selection::from_regions([
        Region::from(1..5),
        Region::from(8..12),
    ]));
    w.editor_mut().text_input(TextInput::Marked {
        text: "nihao".into(),
        selected: Utf16Range::new(5, 0),
        replacement: None,
    });
    assert!(w.editor().marked.is_some());
    assert!(w.editor().buffer.text().to_string().contains('🦀'));
    v.scene(w.editor());
    w.editor_mut().text_input(TextInput::Insert {
        text: "你好".into(),
        replacement: None,
    });
    assert_eq!(w.editor().text.to_string(), "a你好z\nb你好q");
    key(&mut w, &mut p, &mut v, "escape");
    key(&mut w, &mut p, &mut v, "u");
    assert_eq!(w.editor().text.to_string(), "a🦀z\nb🦀q");
    w.editor_mut().text_input(TextInput::Marked {
        text: "ignored".into(),
        selected: Utf16Range::new(0, 0),
        replacement: None,
    });
    assert!(w.editor().marked.is_none());
    w.editor_mut()
        .settings
        .set(Layer::View, "vintage_enabled", json!(false));
    w.editor_mut().sync();
    assert!(w.editor().vintage.is_none());
    w.editor_mut().set_caret(0, false);
    text(&mut w, &mut p, &mut v, "x");
    assert!(w.editor().text.to_string().starts_with('x'));
    key(&mut w, &mut p, &mut v, "ctrl+alt+v");
    // Toggle is a window command; headless registry preserves the action for its host.
    assert!(
        w.editor_mut()
            .actions
            .iter()
            .any(|(name, _)| name == "toggle_setting")
    );
}

#[test]
fn input_panel_search_ex_and_cancel_dispatch_registry_requests() {
    let (mut w, mut p, mut v) = setup("one two one\n");
    text(&mut w, &mut p, &mut v, "/one");
    assert_eq!(p.panel, Some(Panel::Input));
    key(&mut w, &mut p, &mut v, "enter");
    assert_eq!(w.editor().caret, 8);
    key(&mut w, &mut p, &mut v, "n");
    assert_eq!(w.editor().caret, 0);
    text(&mut w, &mut p, &mut v, "?two");
    key(&mut w, &mut p, &mut v, "enter");
    assert_eq!(w.editor().caret, 4);
    text(&mut w, &mut p, &mut v, ":w output.txt");
    key(&mut w, &mut p, &mut v, "enter");
    assert!(
        w.editor()
            .actions
            .iter()
            .any(|(name, args)| name == "save" && args["file"] == "output.txt")
    );
    text(&mut w, &mut p, &mut v, ":q!");
    key(&mut w, &mut p, &mut v, "enter");
    assert!(
        w.editor()
            .actions
            .iter()
            .any(|(name, args)| name == "close" && args["force"] == true)
    );
    key(&mut w, &mut p, &mut v, ":");
    key(&mut w, &mut p, &mut v, "escape");
    assert!(p.panel.is_none());
    assert!(
        w.editor()
            .vintage
            .as_ref()
            .unwrap()
            .engine
            .command_line()
            .is_none()
    );
}

#[test]
fn reindent_host_completion_is_one_vi_undo_and_restores_command_carets() {
    let original = "fn main() {\nprintln!(\"hi\");\n}\n";
    let (mut w, mut p, mut v) = setup(original);
    let syntax = w
        .assets
        .syntaxes
        .find_syntax_by_extension("rs")
        .unwrap()
        .path
        .clone();
    w.configure_language(0, Some(&syntax), false).unwrap();
    w.editor_mut()
        .settings
        .set(Layer::View, "translate_tabs_to_spaces", json!(true));
    w.editor_mut().sync();
    w.editor_mut().set_caret(12, false);
    text(&mut w, &mut p, &mut v, "==");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while w.editor().reindent_job.is_some() {
        w.editor_mut().poll_language();
        assert!(std::time::Instant::now() < deadline);
        std::thread::park_timeout(std::time::Duration::from_millis(1));
    }
    assert!(w.editor().text.to_string().contains("    println"));
    assert!(w.editor().regions().iter().all(|r| r.empty()));
    key(&mut w, &mut p, &mut v, "u");
    assert_eq!(w.editor().text.to_string(), original);
}

#[test]
fn runtime_user_preferences_and_prompt_focus_changes_preserve_text() {
    let temp = tempfile::tempdir().unwrap();
    let prefs = temp
        .path()
        .join("Packages/User/Preferences.sublime-settings");
    let mut w =
        Workspace::new(Text::from("one two"), Some(temp.path().into()), None, false).unwrap();
    assert!(w.editor().vintage.is_none());
    std::fs::write(
        &prefs,
        r#"{"vintage_enabled":true,"vintage_start_in_command_mode":true}"#,
    )
    .unwrap();
    w.poll();
    assert!(w.editor().command_mode());
    let mut p = Panels::new(&w, false);
    let mut v = View::new(
        Size {
            width: 900.,
            height: 600.,
        },
        7.8,
        12.,
    );
    key(&mut w, &mut p, &mut v, ":");
    std::fs::write(&prefs, r#"{"vintage_enabled":false}"#).unwrap();
    w.poll();
    p.poll(&mut w, 0..100);
    assert!(w.editor().vintage.is_none());
    assert!(p.panel.is_none());
    assert_eq!(w.editor().text.to_string(), "one two");
    std::fs::write(
        &prefs,
        r#"{"vintage_enabled":true,"vintage_start_in_command_mode":true,"vintage_ctrl_keys":true}"#,
    )
    .unwrap();
    w.poll();
    text(&mut w, &mut p, &mut v, "/two");
    p.action(&mut w, "show_panel", &json!({"panel":"find"}), 0..100)
        .unwrap();
    p.poll(&mut w, 0..100);
    assert_eq!(p.panel, Some(Panel::Find));
    assert!(
        w.editor()
            .vintage
            .as_ref()
            .unwrap()
            .engine
            .command_line()
            .is_none()
    );
    w.editor_mut().read_only = true;
    p.action(&mut w, "hide_panel", &json!({}), 0..100).unwrap();
    key(&mut w, &mut p, &mut v, "x");
    assert_eq!(w.editor().text.to_string(), "one two");
}

#[test]
fn visual_block_and_command_caret_highlight_use_the_editor_scene() {
    let (mut w, mut p, mut v) = setup("one two\nred blue\n");
    w.editor_mut()
        .settings
        .set(Layer::View, "vintage_ctrl_keys", json!(true));
    w.editor_mut()
        .settings
        .set(Layer::View, "highlight_line", json!(false));
    w.editor_mut().sync();
    key(&mut w, &mut p, &mut v, "ctrl+v");
    key(&mut w, &mut p, &mut v, "j");
    assert_eq!(w.editor().regions().len(), 2);
    assert!(w.editor().vintage_status().contains("VISUAL BLOCK"));
    let scene = v.scene(w.editor());
    assert!(scene.layers[1].rects.iter().all(|r| r.rect.width == v.cell));
    assert!(
        scene.layers[0]
            .rects
            .iter()
            .any(|r| r.rect.x == v.gutter(w.editor()) && r.rect.height == v.line_height)
    );
    key(&mut w, &mut p, &mut v, "escape");
    text(&mut w, &mut p, &mut v, "d/blue");
    key(&mut w, &mut p, &mut v, "enter");
    assert_eq!(w.editor().text.to_string(), "one two\nblue\n");
}

#[test]
fn passthrough_chords_do_not_extend_the_vi_pending_prefix() {
    let (mut w, _, _) = setup("one two");
    w.editor_mut().key(&Key::parse("d").unwrap()).unwrap();
    assert_eq!(w.editor().vintage_status(), "COMMAND d");
    assert!(
        !w.editor_mut()
            .key(&Key::parse("ctrl+f12").unwrap())
            .unwrap()
    );
    assert_eq!(w.editor().vintage_status(), "COMMAND d");
    assert!(!w.editor_mut().key(&Key::parse("ctrl+r").unwrap()).unwrap());
    assert_eq!(w.editor().vintage_status(), "COMMAND d");
}

#[test]
fn typing_supersedes_pending_reindent_without_losing_the_key_or_ime_mode() {
    let (mut w, _, _) = setup("one two");
    w.editor_mut().key(&Key::parse("=").unwrap()).unwrap();
    w.editor_mut().key(&Key::parse("=").unwrap()).unwrap();
    assert!(w.editor().reindent_job.is_some());
    w.editor_mut().key(&Key::parse("i").unwrap()).unwrap();
    assert!(!w.editor().command_mode());
    assert!(w.editor().reindent_job.is_none());
    w.editor_mut().text_input(TextInput::Insert {
        text: "你好".into(),
        replacement: None,
    });
    assert_eq!(w.editor().text.to_string(), "你好one two");
}
