//! Round 16 Lane B behavior checks; no AppKit, window server, font or GPU initialization.
use super::*;
use crate::{language::Assets, panels::Panels, workspace::Workspace};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::{Region, Selection};
use vel_config::{
    json,
    keymap::{Key, Resolution},
    settings::Layer,
};
use vel_text::Text;
fn editor(source: &str, syntax: &str) -> Editor {
    let mut e = Editor::new(Text::from(source));
    e.attach_language(
        Assets::bundled().unwrap(),
        syntax,
        Arc::new(ColorScheme::velocity_dark()),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while !e.language.borrow().as_ref().unwrap().ready() {
        e.poll_language();
        assert!(Instant::now() < until);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    e
}
fn view() -> View {
    View::new(
        Size {
            width: 600.,
            height: 260.,
        },
        8.,
        12.,
    )
}
#[test]
fn nested_folds_survive_outer_unfold_in_wrapped_and_unwrapped_layouts() {
    let source = "outer:\n    inner:\n        hello\n    tail\nend\n";
    for wrap in [false, true] {
        let mut e = editor(source, "Python");
        e.settings.set(Layer::View, "word_wrap", json!(wrap));
        e.sync();
        let inner = Region::new(
            source.find("inner:").unwrap() + 6,
            source.find("hello").unwrap() + 5,
        );
        let outer = Region::new(6, source.find("tail").unwrap() + 4);
        e.set_folded(vec![inner]);
        e.set_folded(vec![outer, inner]);
        let mut v = view();
        v.apply_style(&e);
        assert_eq!(e.folded(), [outer, inner]);
        assert_eq!(
            e.layout.borrow().locate(1).0,
            4,
            "inner row leaked through outer"
        );
        e.set_caret(outer.begin(), false);
        e.run("unfold", &json!({})).unwrap();
        v.apply_style(&e);
        assert_eq!(e.folded(), [inner]);
        assert_eq!(e.layout.borrow().locate(1).0, 1);
    }
}
#[test]
fn fold_navigation_reveals_endpoints_steps_over_markers_and_edits_visible_text() {
    let mut e = editor("a\n  hidden\nz\n", "Python");
    let fold = Region::new(1, 10);
    e.set_folded(vec![fold]);
    e.set_caret(1, false);
    e.run("move", &json!({"by":"characters","forward":true}))
        .unwrap();
    assert_eq!(e.caret, 10);
    assert_eq!(e.folded(), [fold]);
    e.run(
        "move",
        &json!({"by":"characters","forward":false,"extend":true}),
    )
    .unwrap();
    assert_eq!(e.regions(), [Region::new(10, 1)]);
    e.set_caret(5, false);
    e.set_caret(0, false);
    assert!(
        e.folded().is_empty(),
        "a reveal survives later caret movement before painting"
    );
    e.set_folded(vec![fold]);
    e.set_selection(Selection::from_regions([Region::new(5, 7)]));
    e.run("insert", &json!({"characters":"XX"})).unwrap();
    assert!(e.folded().is_empty());
    assert_eq!(e.text.to_string(), "a\n  hXXden\nz\n");
    e.run("undo", &json!({})).unwrap();
    assert!(e.folded().is_empty());
}
#[test]
fn fold_levels_replace_the_forest_and_zero_unfolds_without_analysis() {
    let mut e = editor(
        "outer:\n    inner:\n        hello\n    tail\nend\n",
        "Python",
    );
    e.run("fold_by_level", &json!({"level":1})).unwrap();
    let first = e.folded();
    assert!(!first.is_empty());
    e.run("fold_by_level", &json!({"level":2})).unwrap();
    assert!(e.folded().iter().all(|r| !first.contains(r)));
    e.run("fold_by_level", &json!({"level":0})).unwrap();
    assert!(e.folded().is_empty());
    let mut e = Editor::new(Text::from("abc"));
    e.set_folded(vec![Region::new(1, 2)]);
    e.run("fold_by_level", &json!({"level":0})).unwrap();
    assert!(e.folded().is_empty());
    for level in 1..=9 {
        let mut e = Editor::new(Text::new());
        assert!(matches!(
            e.resolve_key(&Key::parse("super+k").unwrap()),
            Resolution::Pending
        ));
        assert!(
            matches!(e.resolve_key(&Key::parse(&format!("super+{level}")).unwrap()), Resolution::Command(i) if i.command=="fold_by_level" && i.args["level"]==level)
        );
    }
}
#[test]
fn indentation_folds_leave_trailing_blank_lines_and_final_newline_visible() {
    for source in [
        "if x:\n    body\n\n\nnext\n",
        "if x:\n    body\n\n",
        "if x:\n    body",
    ] {
        let e = editor(source, "Python");
        let language = e.language.borrow();
        let f = language
            .as_ref()
            .unwrap()
            .analysis
            .as_ref()
            .unwrap()
            .folds
            .iter()
            .find(|f| f.range.start == 5)
            .unwrap();
        assert_eq!(f.range.end, source.find("body").unwrap() + 4);
        assert!(!source[f.range.clone()].ends_with('\n'));
    }
}
#[test]
fn fold_marker_mouse_unfolds_and_bookmarks_have_a_gutter_symbol() {
    for width in [0, 1, 80] {
        let mut e = editor("a\n  hidden\nz\n", "Python");
        let mut v = view();
        e.settings.set(Layer::View, "word_wrap", json!(width != 0));
        e.settings.set(Layer::View, "wrap_width", json!(width));
        e.sync();
        let fold = Region::new(1, 10);
        e.set_folded(vec![fold]);
        v.apply_style(&e);
        let scene = v.scene(&e);
        let marker = scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .find(|r| r.text.contains('…'))
            .unwrap();
        let prefix = marker.text.split('…').next().unwrap().chars().count();
        let marker_x = marker.x as f64 + prefix as f64 * v.cell;
        let marker_y = marker.baseline as f64 - v.ascent - 3.;
        let before = v.rect_at(&e, fold.begin());
        assert!(
            (before.x - marker_x).abs() < 0.01,
            "caret misses drawn marker at width {width}"
        );
        assert!((before.y - marker_y).abs() < 0.01);
        e.set_caret(fold.begin(), false);
        e.run("move", &json!({"by":"characters","forward":true}))
            .unwrap();
        v.scene(&e);
        let after = v.rect_at(&e, fold.end());
        assert_eq!(after.y, before.y);
        assert!((after.x - before.x - v.cell).abs() < 0.01);
        assert_eq!(
            v.hit_test(
                &e,
                Point {
                    x: after.x,
                    y: after.y + 1.
                }
            ),
            fold.end()
        );
        let query = Utf16Range::from_bytes(&e.text, fold.end()..fold.end());
        let (ime, actual) = v.range_rect(&e, query);
        assert_eq!(actual, query);
        assert_eq!(ime.x, after.x);
        e.run("move", &json!({"by":"characters","forward":false}))
            .unwrap();
        let selection = e.regions();
        assert!(
            v.pointer(
                &mut e,
                Point {
                    x: marker_x + 1.,
                    y: marker_y + 2.
                },
                true,
                false,
                false
            )
            .unwrap()
        );
        assert!(e.folded().is_empty());
        assert_eq!(e.regions(), selection);
        e.buffer.add_regions(
            e.id,
            "velocity.bookmarks",
            vec![Region::new(0, 1), Region::new(2, 10)],
            false,
        );
        let scene = v.scene(&e);
        assert_eq!(
            scene
                .layers
                .iter()
                .flat_map(|l| &l.rects)
                // Bookmarks are round dots at the gutter's left edge.
                .filter(|r| r.rect.x == 3. && r.rect.width == r.rect.height && r.radius > 0.)
                .count(),
            2
        );
    }
}
#[test]
fn guide_geometry_stays_in_its_block_and_supports_stippled_and_solid() {
    let e = editor("top:\n    a\n\n    b\nend\n    other\n", "Python");
    let language = e.language.borrow();
    let g = &language.as_ref().unwrap().analysis.as_ref().unwrap().guides;
    assert_eq!(g.columns(2), 4);
    assert_eq!(g.active(1, 4).unwrap().rows, 1..4);
    let rect = Rect {
        x: 10.,
        y: 0.,
        width: 1.,
        height: 20.,
    };
    let mut l = vel_render::scene::Layer::default();
    paint_guide(&mut l, rect, rect, [1.; 4], false);
    assert_eq!(l.rects.len(), 1);
    assert_eq!(l.rects[0].rect.height, 20.);
    assert!(l.rects[0].is_stippled_guide());
    l.rects.clear();
    paint_guide(&mut l, rect, rect, [1.; 4], true);
    assert_eq!(l.rects.len(), 1);
    assert_eq!(l.rects[0].rect.height, 20.);
}
#[test]
fn occurrence_outlines_are_whole_words_visible_and_cancel_immediately_on_edit() {
    assert_eq!(
        word_occurrences("one lone one_ one One one", "one").collect::<Vec<_>>(),
        [0..3, 14..17, 22..25]
    );
    assert_eq!(
        word_occurrences("界 界界 界", "界").collect::<Vec<_>>(),
        [0..3, 11..14]
    );
    let mut e = Editor::new(Text::from("one one lone\none\n"));
    e.set_selection(Selection::from_regions([Region::new(0, 3)]));
    assert_eq!(selected_word(&e, &e.regions()).as_deref(), Some("one"));
    let mut v = view();
    let with = v
        .scene(&e)
        .layers
        .iter()
        .map(|l| l.rects.len())
        .sum::<usize>();
    e.settings.set(Layer::View, "match_selection", json!(false));
    e.sync();
    let without = v
        .scene(&e)
        .layers
        .iter()
        .map(|l| l.rects.len())
        .sum::<usize>();
    assert_eq!(with - without, 2);
    e.settings.set(Layer::View, "match_selection", json!(true));
    e.set_selection(Selection::from_regions([Region::new(1, 3)]));
    assert!(selected_word(&e, &e.regions()).is_none());
    e.set_selection(Selection::from_regions([Region::new(0, 3)]));
    e.run("insert", &json!({"characters":"new"})).unwrap();
    assert!(selected_word(&e, &e.regions()).is_none());
}
#[test]
fn verified_visual_defaults_and_whitespace_array_and_string_forms() {
    let mut workspace = Workspace::new(Text::from("  a "), None, None, false).unwrap();
    let e = workspace.editor_mut();
    let mut v = view();
    assert_eq!(
        e.settings.get("draw_white_space"),
        Some(&json!(["selection"]))
    );
    assert!(!e.preferences.highlight_line);
    assert_eq!(
        e.settings.get("indent_guide_options"),
        Some(&json!(["draw_normal"]))
    );
    e.set_selection(Selection::from_regions([Region::new(0, 2)]));
    for value in [json!("selection"), json!(["selection"])] {
        e.settings.set(Layer::View, "draw_white_space", value);
        e.sync();
        assert_eq!(
            v.scene(e)
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .filter(|r| r.text == "·")
                .count(),
            2
        );
    }
}
#[test]
fn vintage_insert_routes_ordinary_commands_and_records_host_changes_for_dot_and_counts() {
    let mut e = editor("fn a() {}", "Rust");
    e.settings.set(Layer::User, "ignored_packages", json!([]));
    e.sync();
    assert!(e.vintage.is_some());
    assert!(!e.command_mode());
    for key in [
        "enter",
        "backspace",
        "left",
        "right",
        "shift+left",
        "up",
        "down",
    ] {
        let resolution = e.resolve_key(&Key::parse(key).unwrap());
        assert!(
            matches!(resolution, Resolution::Command(_)),
            "{key}: {resolution:?}"
        );
    }
    e.set_caret(8, false);
    e.key(&Key::parse("enter").unwrap()).unwrap();
    assert!(e.text.to_string().contains('\n'));
    e.key(&Key::parse("escape").unwrap()).unwrap();
    assert!(e.command_mode());
    let mut e = Editor::new(Text::from("abc"));
    e.settings.set(Layer::View, "vintage_enabled", json!(true));
    e.settings
        .set(Layer::View, "vintage_start_in_command_mode", json!(true));
    e.sync();
    for key in ["3", "i"] {
        e.key(&Key::parse(key).unwrap()).unwrap();
    }
    e.text_input(vel_platform::TextInput::Insert {
        text: "界".into(),
        replacement: None,
    });
    e.key(&Key::parse("escape").unwrap()).unwrap();
    assert_eq!(e.text.to_string(), "界界界abc");
    e.key(&Key::parse("u").unwrap()).unwrap();
    assert_eq!(e.text.to_string(), "abc");
    e.key(&Key::parse(".").unwrap()).unwrap();
    assert_eq!(e.text.to_string(), "界界界abc");
}
#[test]
fn vintage_external_selections_are_visual_and_documented_keys_reach_the_host() {
    let mut e = Editor::new(Text::from("one two"));
    e.settings.set(Layer::View, "vintage_enabled", json!(true));
    e.settings
        .set(Layer::View, "vintage_start_in_command_mode", json!(true));
    e.settings
        .set(Layer::View, "vintage_ctrl_keys", json!(true));
    e.sync();
    e.set_selection(Selection::from_regions([Region::new(0, 3)]));
    e.key(&Key::parse("d").unwrap()).unwrap();
    assert_eq!(e.text.to_string(), " two");
    e.key(&Key::parse("u").unwrap()).unwrap();
    assert_eq!(e.text.to_string(), "one two");
    for key in ["ctrl+e", "ctrl+y"] {
        assert!(
            matches!(e.resolve_key(&Key::parse(key).unwrap()),Resolution::Command(i) if i.command=="scroll_lines" && i.args["amount"].is_number())
        );
    }
    assert!(matches!(
        e.resolve_key(&Key::parse("ctrl+alt+w").unwrap()),
        Resolution::Unbound
    ));
    e.key(&Key::parse("alt+w").unwrap()).unwrap();
    assert!(e.caret > 0);
}
#[test]
fn spelling_navigation_does_not_enable_spell_check() {
    let mut w = Workspace::new(Text::from("hello mispelled\n"), None, None, false).unwrap();
    let mut p = Panels::new(&w, false);
    let mut v = view();
    assert!(!w.editor().settings.bool("spell_check", false));
    p.review_action(&mut w, "navigate_misspelling", &json!({"forward":true}))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while w.editor().selection().is_empty() {
        v.scene(w.editor());
        w.poll();
        p.poll(&mut w, 0..100);
        assert!(Instant::now() < until, "spelling navigation timed out");
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert_eq!(
        w.editor().text.slice(w.editor().selection()).to_string(),
        "mispelled"
    );
    assert!(!w.editor().settings.bool("spell_check", false));
    assert!(
        !w.editor()
            .settings
            .get("spell_check")
            .unwrap()
            .as_bool()
            .unwrap()
    );
    assert!(
        w.editor()
            .annotations
            .borrow()
            .current
            .as_ref()
            .unwrap()
            .spelling
            .is_empty()
    );
}
#[test]
fn menus_and_palette_have_registered_commands_and_matching_arguments() {
    use vel_config::ui::{self, CommandState, MenuItem, PaletteItem};
    let menu: Vec<MenuItem> = ui::parse(include_str!(
        "../resources/Packages/Default/Main.sublime-menu"
    ))
    .unwrap();
    let palette: Vec<PaletteItem> = ui::parse(include_str!(
        "../resources/Packages/Default/Velocity.sublime-commands"
    ))
    .unwrap();
    let e = Editor::new(Text::from("abc"));
    let mut registry = vel_commands::Registry::standard();
    // These commands are hosted by the application, not the standalone registry.
    for command in ["new_build_system", "new_snippet", "copy_as_html"] {
        registry.register(command, vel_commands::Kind::WindowCommand, None);
    }
    let resolved = ui::resolve_menu(&menu, "OSX", &json!({}), |i| CommandState {
        enabled: registry.is_enabled(&i.command, &i.args, &e),
        ..Default::default()
    });
    for name in ["Edit", "Selection", "Find", "Goto", "Tools"] {
        assert!(resolved.iter().any(|r| r.caption == name));
    }
    fn visit(items: &[MenuItem], p: &[PaletteItem], r: &vel_commands::Registry) {
        for row in items {
            if let Some(command) = &row.command {
                if row.caption == "-" {
                    continue;
                }
                assert!(
                    p.iter()
                        .any(|i| i.invocation.command == *command && i.invocation.args == row.args),
                    "missing palette: {}",
                    row.caption
                );
                assert!(
                    r.kind(command).is_some(),
                    "unregistered menu command: {command}"
                );
            }
            visit(&row.children, p, r);
        }
    }
    for group in menu.iter().filter(|r| {
        matches!(
            r.caption.as_str(),
            "Edit" | "Selection" | "Find" | "Goto" | "Tools"
        )
    }) {
        visit(&group.children, &palette, &registry);
    }
}
#[test]
fn spelling_selector_checks_prose_comments_and_strings_but_skips_markup_and_embedded_code() {
    let source = "<strangetag badattr='wrongvalue'>mispelled prose</strangetag><script>badcode; // mispelled comment\n</script><style>.badselector { color: red; }</style>";
    let e = editor(source, "HTML");
    let language = e.language.borrow();
    let l = language.as_ref().unwrap();
    for (word, expected) in [
        ("strangetag", false),
        ("badattr", false),
        ("wrongvalue", false),
        ("mispelled prose", true),
        ("badcode", false),
        ("mispelled comment", true),
        ("badselector", false),
    ] {
        assert_eq!(
            l.selector(
                &e.text,
                crate::annotations::SPELLING_SELECTOR,
                source.find(word).unwrap()
            ),
            expected,
            "{word}"
        );
    }
    let selector = vel_scope::Selector::parse(crate::annotations::SPELLING_SELECTOR).unwrap();
    for scopes in [
        vec!["text.html", "markup.underline.link"],
        vec![
            "source.c",
            "meta.preprocessor.c.include",
            "string.quoted.double.c",
        ],
        vec!["text.html", "keyword.control"],
    ] {
        let scopes: Vec<_> = scopes
            .into_iter()
            .map(|s| vel_scope::Scope::new(s).unwrap())
            .collect();
        assert!(!selector.matches(&scopes));
    }
}
#[test]
fn vintage_insert_pair_deletion_arrows_and_indent_match_ordinary_editing() {
    let mut normal = editor("fn a() {}", "Rust");
    let mut vintage = editor("fn a() {}", "Rust");
    for e in [&mut normal, &mut vintage] {
        e.settings
            .set(Layer::View, "translate_tabs_to_spaces", json!(true));
        e.sync();
        e.set_caret(8, false);
    }
    vintage
        .settings
        .set(Layer::View, "vintage_enabled", json!(true));
    vintage.sync();
    for key in ["enter", "(", "backspace", "left", "shift+right", "x"] {
        for e in [&mut normal, &mut vintage] {
            if !e.key(&Key::parse(key).unwrap()).unwrap() {
                assert_eq!(key.chars().count(), 1);
                e.text_input(vel_platform::TextInput::Insert {
                    text: key.into(),
                    replacement: None,
                });
            }
        }
        assert_eq!(vintage.text.to_string(), normal.text.to_string(), "{key}");
        assert_eq!(vintage.regions(), normal.regions(), "{key}");
    }
    let mut e = editor("al", "Rust");
    e.set_caret(1, false);
    e.settings.set(Layer::View, "vintage_enabled", json!(true));
    e.settings
        .set(Layer::View, "vintage_start_in_command_mode", json!(true));
    // Editor::new is a bare host; Workspace loads this packaged setting normally.
    e.settings
        .set(Layer::View, "auto_complete_commit_on_tab", json!(false));
    e.sync();
    e.key(&Key::parse("a").unwrap()).unwrap();
    let mut engine = vel_complete::CompletionEngine::new();
    engine
        .add_completions(r#"{"scope":"source.rust","completions":["alpha","alpine"]}"#)
        .unwrap();
    e.completion.set_engine(Arc::new(engine));
    e.run("auto_complete", &json!({})).unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while e.completion.busy() || !e.completion.visible() {
        e.poll_language();
        e.poll_completion();
        assert!(Instant::now() < until, "Vintage completion timed out");
        std::thread::park_timeout(Duration::from_millis(1));
    }
    e.key(&Key::parse("down").unwrap()).unwrap();
    assert_eq!(e.completion.selected, 1);
    e.key(&Key::parse("up").unwrap()).unwrap();
    assert_eq!(e.completion.selected, 0);
    assert_eq!(e.caret, 2);
    e.key(&Key::parse("enter").unwrap()).unwrap();
    assert_eq!(e.text.to_string(), "alpha");
    e.key(&Key::parse("escape").unwrap()).unwrap();
    e.key(&Key::parse("u").unwrap()).unwrap();
    assert_eq!(e.text.to_string(), "al");
    e.key(&Key::parse(".").unwrap()).unwrap();
    assert_eq!(e.text.to_string(), "alpha");
}
#[test]
fn vintage_external_change_and_yank_use_the_exact_exclusive_selection() {
    for operator in ["c", "y"] {
        let mut e = Editor::new(Text::from("one two"));
        e.settings.set(Layer::View, "vintage_enabled", json!(true));
        e.settings
            .set(Layer::View, "vintage_start_in_command_mode", json!(true));
        e.sync();
        e.set_selection(Selection::from_regions([Region::new(3, 0)]));
        e.key(&Key::parse(operator).unwrap()).unwrap();
        assert_eq!(
            e.vintage
                .as_ref()
                .unwrap()
                .engine
                .register('"')
                .unwrap()
                .fragments,
            ["one"]
        );
        if operator == "c" {
            assert_eq!(e.text.to_string(), " two");
            assert!(!e.command_mode());
        } else {
            assert_eq!(e.text.to_string(), "one two");
        }
    }
    let mut e = Editor::new(Text::from("abc"));
    e.settings.set(Layer::User, "ignored_packages", json!([]));
    e.sync();
    assert_eq!(
        e.vintage
            .as_ref()
            .unwrap()
            .engine
            .context("setting.vintage_ctrl_keys"),
        cfg!(target_os = "macos")
    );
}
#[test]
fn default_character_motions_and_boundary_deletion_do_not_edit_hidden_bytes() {
    let mut e = Editor::new(Text::from("a hidden z"));
    e.set_folded(vec![Region::new(1, 8)]);
    e.set_caret(1, false);
    e.run("move", &json!({"forward":true})).unwrap();
    assert_eq!(e.caret, 8);
    e.run("left_delete", &json!({})).unwrap();
    assert!(e.folded().is_empty());
    assert_eq!(e.text.to_string(), "a hidde z");
    e.run("undo", &json!({})).unwrap();
    e.set_folded(vec![Region::new(1, 8)]);
    e.set_selection(Selection::from_regions([Region::new(1, 8)]));
    e.run("insert", &json!({"characters":"new"})).unwrap();
    assert!(e.folded().is_empty());
    assert_eq!(e.text.to_string(), "anew z");
}
#[test]
fn trimming_indentation_does_not_duplicate_syntax_folds_at_a_new_level() {
    let e = editor(
        "fn f() {\n    work();\n}\nfn g() {\n    work();\n}\n",
        "Rust",
    );
    let language = e.language.borrow();
    let folds = &language.as_ref().unwrap().analysis.as_ref().unwrap().folds;
    assert_eq!(folds.len(), 2);
    assert!(folds.iter().all(|f| f.level == 1));
}
#[test]
fn lazy_guide_storage_preserves_leading_and_intervening_blank_runs() {
    for (source, expected) in [
        ("zero\n\nzero\n\n    body\n", vec![0, 0, 0, 4, 4, 4]),
        ("\n\n    body\n", vec![4, 4, 4, 4]),
        ("\nzero\n\nzero\n\n", vec![0, 0, 0, 0, 0, 0]),
    ] {
        let e = editor(source, "Plain Text");
        let language = e.language.borrow();
        let guides = &language.as_ref().unwrap().analysis.as_ref().unwrap().guides;
        assert_eq!(
            (0..e.text.len_lines())
                .map(|row| guides.columns(row))
                .collect::<Vec<_>>(),
            expected
        );
    }
}
