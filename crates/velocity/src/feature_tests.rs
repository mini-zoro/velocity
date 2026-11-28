//! Language/view integration regressions; every case runs without a native window or GPU.
use crate::{editor::Editor, language::Assets, panels::Panels, view::View, workspace::Workspace};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::{Region, Selection};
use vel_color_scheme::ColorScheme;
use vel_config::{json, keymap::Key, settings::Layer};
use vel_platform::{Point, Size};
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
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        e.poll_language();
        if e.language.borrow().as_ref().is_some_and(|l| l.ready()) {
            break;
        }
        assert!(Instant::now() < deadline, "language results timed out");
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert!(e.language.borrow().as_ref().unwrap().errors.is_empty());
}
fn view() -> View {
    View::new(
        Size {
            width: 400.,
            height: 240.,
        },
        8.,
        12.,
    )
}
#[test]
fn real_scopes_drive_quote_pairing_and_brackets_ignore_strings_comments() {
    let source = "fn alpha() {\n let s = \"[inside]\"; // {ignored}\n}\n";
    let mut e = editor(source, "Rust");
    let string = source.find("inside").unwrap();
    let comment = source.find("ignored").unwrap();
    assert!(
        e.language
            .borrow()
            .as_ref()
            .unwrap()
            .selector(&e.text, "string", string)
    );
    assert!(
        e.language
            .borrow()
            .as_ref()
            .unwrap()
            .selector(&e.text, "comment", comment)
    );
    assert_eq!(
        e.language
            .borrow()
            .as_ref()
            .unwrap()
            .analysis
            .as_ref()
            .unwrap()
            .pairs
            .len(),
        2
    );
    e.set_caret(comment, false);
    assert!(!e.key(&Key::parse("\"").unwrap()).unwrap());
    e.run("insert", &json!({"characters":"\""})).unwrap();
    assert_eq!(e.text.len_bytes(), source.len() + 1);
    e.run("undo", &json!({})).unwrap();
    ready(&mut e);
    assert_eq!(e.text.to_string(), source);
}
#[test]
fn toggling_comments_ignores_blank_lines_when_deciding_to_uncomment() {
    let mut e = editor("// a\n\n// b\n", "Rust");
    e.set_selection(Selection::from_regions([Region::new(0, 10)]));
    e.run("toggle_comment", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "a\n\nb\n");
    e.run("toggle_comment", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "// a\n\n// b\n");
}
#[test]
fn multi_cursor_comments_are_deduplicated_and_undoable() {
    let mut e = editor("fn a() {}\nfn b() {}\n", "Rust");
    e.set_selection(Selection::from_regions([
        Region::caret(0),
        Region::caret(3),
        Region::caret(10),
    ]));
    e.run("toggle_comment", &json!({"block":false})).unwrap();
    assert_eq!(e.text.to_string(), "// fn a() {}\n// fn b() {}\n");
    e.run("toggle_comment", &json!({"block":false})).unwrap();
    assert_eq!(e.text.to_string(), "fn a() {}\nfn b() {}\n");
    e.set_selection(Selection::from_regions([
        Region::new(3, 4),
        Region::new(13, 14),
    ]));
    e.run("toggle_comment", &json!({"block":true})).unwrap();
    assert_eq!(e.text.to_string(), "fn /*a*/() {}\nfn /*b*/() {}\n");
    assert_eq!(
        e.regions()
            .iter()
            .map(|r| e.text.slice(r.to_range()).to_string())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    e.run("toggle_comment", &json!({"block":true})).unwrap();
    assert_eq!(e.text.to_string(), "fn a() {}\nfn b() {}\n");
    e.run("toggle_comment", &json!({"block":true})).unwrap();
    e.run("undo", &json!({})).unwrap();
    assert_eq!(e.text.to_string(), "fn a() {}\nfn b() {}\n");
    e.set_caret(3, false);
    e.run("toggle_comment", &json!({"block":true})).unwrap();
    assert_eq!(e.caret, 5);
    e.run("toggle_comment", &json!({"block":true})).unwrap();
    assert_eq!(e.text.to_string(), "fn a() {}\nfn b() {}\n");
    assert_eq!(e.caret, 3);
}
#[test]
fn metadata_autoindent_reindent_and_stale_reindent_preserve_edits() {
    let mut e = editor("if true {}", "Rust");
    e.settings
        .set(Layer::View, "translate_tabs_to_spaces", json!(true));
    e.sync();
    e.set_caret(9, false);
    e.run("insert", &json!({"characters":"\n"})).unwrap();
    assert_eq!(e.text.to_string(), "if true {\n    \n}");
    assert_eq!(e.caret, 14);
    let mut e = editor("fn a() {\nwork();\n}\n", "Rust");
    e.settings
        .set(Layer::View, "translate_tabs_to_spaces", json!(true));
    e.sync();
    e.run("reindent", &json!({})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while e.reindent_job.is_some() {
        e.poll_language();
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert_eq!(e.text.to_string(), "fn a() {\n    work();\n}\n");
    e.run("reindent", &json!({})).unwrap();
    e.run("insert", &json!({"characters":"// newest\n"}))
        .unwrap();
    while e.reindent_job.is_some() {
        e.poll_language();
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert!(e.text.to_string().starts_with("// newest\n"));
}
#[test]
fn transformed_symbols_populate_goto_and_preview_cancellation() {
    let mut workspace = Workspace::new(
        Text::from("fn alpha() {}\nfn beta() {}\n"),
        None,
        None,
        false,
    )
    .unwrap();
    workspace.set_file_type("Rust").unwrap();
    ready(workspace.editor_mut());
    let names = workspace
        .editor()
        .language
        .borrow()
        .as_ref()
        .unwrap()
        .analysis
        .as_ref()
        .unwrap()
        .symbols
        .clone();
    assert!(names.iter().any(|(n, _)| n.contains("alpha")));
    assert!(names.iter().any(|(n, _)| n.contains("beta")));
    let mut panels = Panels::new(&workspace, false);
    let viewport = 0..workspace.editor().text.len_bytes();
    panels
        .action(
            &mut workspace,
            "show_overlay",
            &json!({"overlay":"goto","text":"@beta"}),
            viewport.clone(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while panels.quick.items.is_empty() {
        panels.poll(&mut workspace, viewport.clone());
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert_eq!(
        workspace
            .editor()
            .text
            .byte_to_line(workspace.editor().caret),
        1
    );
    panels
        .key(&mut workspace, &Key::parse("escape").unwrap(), viewport)
        .unwrap();
    assert_eq!(workspace.editor().caret, 0);
}
#[test]
fn syntax_detection_settings_status_picker_and_auto_scheme_live_switch() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Rust.sublime-settings"),
        r#"{"tab_size":2,"word_wrap":true}"#,
    )
    .unwrap();
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn a() {\n  work();\n}\n").unwrap();
    let mut w = Workspace::new(Text::new(), Some(dir.path().into()), None, false).unwrap();
    w.open(crate::cli::FileLocation {
        path,
        line: 1,
        column: 1,
    })
    .unwrap();
    assert_eq!(w.editor().language.borrow().as_ref().unwrap().name, "Rust");
    assert_eq!(w.editor().preferences.tab_size, 2);
    let dark = w
        .editor()
        .language
        .borrow()
        .as_ref()
        .unwrap()
        .scheme
        .default_style();
    w.set_appearance(false).unwrap();
    assert_ne!(
        w.editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .scheme
            .default_style(),
        dark
    );
    w.set_file_type("Python").unwrap();
    assert_eq!(
        w.editor().language.borrow().as_ref().unwrap().name,
        "Python"
    );
    let mut p = Panels::new(&w, false);
    p.layout(Size {
        width: 800.,
        height: 600.,
    });
    // Status items are hit where the last paint placed them.
    p.paint(&w, &mut vel_render::scene::Scene::default());
    let item = p
        .status_item(crate::panels::StatusItem::Syntax)
        .expect("syntax item painted");
    let click = Point {
        x: item.x + item.width / 2.,
        y: 590.,
    };
    assert!(p.mouse(&mut w, click, true, false, false, 0..0).unwrap());
    assert_eq!(p.overlay, Some(crate::panels::Overlay::Syntax));
    assert!(w.palette.iter().any(|p| p.caption == "Set Syntax: Rust"));
}
#[test]
fn folds_track_edits_and_undo_and_minimap_drag_preserves_selection() {
    let mut e = editor("fn a() {\n    work();\n}\n", "Rust");
    let mut v = view();
    v.apply_style(&e);
    e.run("fold_by_level", &json!({"level":1})).unwrap();
    assert_eq!(e.folded().len(), 1);
    v.apply_style(&e);
    assert_eq!(e.layout.borrow().total(), 3);
    e.set_caret(0, false);
    e.run("insert", &json!({"characters":"// comment\n"}))
        .unwrap();
    assert_eq!(e.text.byte_to_line(e.folded()[0].begin()), 1);
    e.run("undo", &json!({})).unwrap();
    assert_eq!(e.text.byte_to_line(e.folded()[0].begin()), 0);
    e.run("unfold_all", &json!({})).unwrap();
    v.apply_style(&e);
    assert_eq!(e.layout.borrow().total(), 4);
    let before = e.regions();
    assert!(
        v.pointer(&mut e, Point { x: 399., y: 190. }, true, false, false)
            .unwrap()
    );
    assert_eq!(e.regions(), before);
    assert!(
        v.pointer(&mut e, Point { x: 200., y: 150. }, false, true, false)
            .unwrap()
    );
    assert!(
        v.pointer(&mut e, Point { x: 200., y: 150. }, false, false, true)
            .unwrap()
    );
}
#[test]
fn wrap_vertical_movement_uses_visual_rows_and_keeps_graphemes_intact() {
    let mut e = editor("ab 你好 👩‍💻 long words continue here\nsecond", "Plain Text");
    e.settings.set(Layer::View, "wrap_width", json!(10));
    e.settings.set(Layer::View, "word_wrap", json!(true));
    e.sync();
    let mut v = view();
    v.apply_style(&e);
    e.set_caret(3, false);
    let before = e.layout.borrow().visual_at(&e.text, e.caret).0;
    e.run("move", &json!({"by":"lines","forward":true}))
        .unwrap();
    assert_eq!(e.layout.borrow().visual_at(&e.text, e.caret).0, before + 1);
    assert!(e.text.is_char_boundary(e.caret));
    let rect = v.rect_at(&e, e.caret);
    assert_eq!(
        v.hit_test(
            &e,
            Point {
                x: rect.x,
                y: rect.y
            }
        ),
        e.caret
    );
}
#[test]
fn scheme_decorations_and_font_options_reach_scene_and_minimap() {
    let mut e = editor("fn name() {} // comment\n", "Rust");
    let scheme=Arc::new(ColorScheme::parse(r##"{"globals":{"background":"#010203","foreground":"#abcdef","caret":"#fedcba","selection":"#334455","invisibles":"#998877","gutter":"#112233"},"rules":[{"scope":"keyword, storage","foreground":"#ff0000","font_style":"bold italic squiggly_underline"}]}"##).unwrap());
    e.language
        .borrow_mut()
        .as_mut()
        .unwrap()
        .recolor(scheme.clone(), e.text.clone(), 4);
    ready(&mut e);
    let mut v = view();
    let deadline = Instant::now() + Duration::from_secs(10);
    let scene = loop {
        let scene = v.scene(&e);
        if scene.layers[0].text.iter().any(|r| r.style.bold) {
            break scene;
        }
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    };
    assert_eq!(
        scene.background,
        crate::language::rgba(scheme.default_style().background)
    );
    assert!(scene.layers[0].underlines.iter().any(|u| u.squiggle));
    assert!(scene.layers.last().unwrap().rects.len() > 6);
    e.settings
        .set(Layer::View, "font_options", json!(["no_bold", "no_italic"]));
    e.sync();
    let scene = v.scene(&e);
    assert!(
        scene.layers[0]
            .text
            .iter()
            .all(|r| !r.style.bold && !r.style.italic)
    );
}

#[test]
fn tags_match_independently_and_folded_attributes_keep_the_tag_body() {
    let source = "<div class=\"wide\">\n  <span>body</span>\n</div>\n";
    let mut e = editor(source, "HTML");
    e.set_caret(2, false);
    e.settings.set(Layer::View, "match_brackets", json!(false));
    e.sync();
    let mut v = view();
    let scene = v.scene(&e);
    assert_eq!(scene.layers[0].underlines.len(), 2);
    e.run("fold_tag_attributes", &json!({})).unwrap();
    let scene = v.scene(&e);
    let rendered = scene.layers[0]
        .text
        .iter()
        .map(|r| r.text.as_str())
        .collect::<String>();
    assert!(rendered.contains('…'));
    assert!(rendered.contains("body"));
    assert!(!rendered.contains("wide"));
}

#[test]
fn minimap_samples_relocate_after_newlines_and_keep_unmodified_token_colors() {
    let mut e = editor(&"fn f() {}\n".repeat(50), "Rust");
    let rows: Vec<_> = e
        .language
        .borrow()
        .as_ref()
        .unwrap()
        .minimap
        .iter()
        .map(|l| l.row)
        .collect();
    let original = e.language.borrow().as_ref().unwrap().minimap[20]
        .tokens
        .clone();
    e.set_caret(3, false);
    e.run("insert", &json!({"characters":"x"})).unwrap();
    assert_eq!(
        e.language
            .borrow()
            .as_ref()
            .unwrap()
            .minimap
            .iter()
            .map(|l| l.row)
            .collect::<Vec<_>>(),
        rows
    );
    assert_eq!(
        e.language.borrow().as_ref().unwrap().minimap[20].tokens[0].color,
        original[0].color
    );
    e.set_caret(0, false);
    e.run("insert", &json!({"characters":"// header\n"}))
        .unwrap();
    assert_eq!(e.language.borrow().as_ref().unwrap().minimap[20].row, 21);
    assert_eq!(
        e.language.borrow().as_ref().unwrap().minimap[20].tokens[0].color,
        original[0].color
    );
}

#[test]
fn wrapped_ime_first_fragment_and_commit_use_the_correct_snapshot() {
    use vel_platform::{TextInput, Utf16Range};
    let mut e = editor("", "Plain Text");
    e.settings.set(Layer::View, "word_wrap", json!(true));
    e.settings.set(Layer::View, "wrap_width", json!(4));
    e.sync();
    e.text_input(TextInput::Marked {
        text: "你好你好\nnext".into(),
        selected: Utf16Range::new(4, 0),
        replacement: None,
    });
    let mut v = view();
    v.scene(&e);
    let (rect, actual) = v.range_rect(&e, Utf16Range::new(0, 4));
    assert_eq!(actual, Utf16Range::new(0, 2));
    assert_eq!(rect.width, 32.);
    e.text_input(TextInput::Insert {
        text: "你好你好\nnext".into(),
        replacement: None,
    });
    v.scene(&e);
    assert_eq!(e.layout.borrow().total(), 3);
    assert_eq!(e.text.to_string(), "你好你好\nnext");
    e.run("undo", &json!({})).unwrap();
    v.scene(&e);
    assert_eq!(e.layout.borrow().total(), 1);
}

#[test]
fn soft_tab_backspace_rewraps_only_the_edited_line() {
    let mut e = Editor::new(Text::from("    work();\n".repeat(100).as_str()));
    e.settings
        .set(Layer::View, "translate_tabs_to_spaces", json!(true));
    e.settings.set(Layer::View, "word_wrap", json!(true));
    e.settings.set(Layer::View, "wrap_width", json!(8));
    e.sync();
    let mut v = view();
    v.scene(&e);
    e.set_caret(1, false);
    e.run("left_delete", &json!({})).unwrap();
    v.scene(&e);
    assert!(e.text.to_string().starts_with("   work();"));
    assert_eq!(e.layout.borrow().rewrapped, 1);
}

#[test]
fn block_only_line_comments_survive_two_immediate_toggles() {
    for syntax in ["HTML", "CSS"] {
        let mut e = editor("", syntax);
        e.run("toggle_comment", &json!({})).unwrap();
        e.run("toggle_comment", &json!({})).unwrap();
        assert_eq!(e.text.to_string(), "", "{syntax}");
    }
}
#[cfg(target_os = "macos")]
#[test]
fn content_background_is_the_painted_editor_background() {
    let e = editor("fn name() {}\n", "Rust");
    let scheme = Arc::new(
        ColorScheme::parse(
            r##"{"globals":{"background":"#0a141e","foreground":"#abcdef"},"rules":[]}"##,
        )
        .unwrap(),
    );
    e.language
        .borrow_mut()
        .as_mut()
        .unwrap()
        .recolor(scheme.clone(), e.text.clone(), 4);
    let mut v = view();
    let expected = crate::language::rgba(scheme.default_style().background);
    assert_eq!(v.editor_background(&e), expected);
    assert_eq!(v.scene(&e).background, expected);
    // Not the composed window scene's default, which painted a near-black backdrop.
    assert_ne!(expected, vel_render::scene::Scene::default().background);
}
