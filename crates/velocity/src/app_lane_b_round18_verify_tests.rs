//! Follow-up verifier reproductions through shared native/headless adapters.
use super::*;

#[test]
fn round18_verify_stalled_build_poll_preserves_tail_before_finished() {
    for (stall, count) in [(600, 1048), (1200, 1048), (2000, 1048), (3000, 7680)] {
        let mut a = app("", None);
        named(
            &mut a,
            "exec",
            json!({"cmd":["/usr/bin/awk",format!("BEGIN {{ for(i=0;i<{count};i++) printf \"%01024d\\n\", 0; print \"TAIL_MARKER\" }} ")]}),
        );
        std::thread::sleep(Duration::from_millis(stall));
        finish(&mut a);
        let tail = a
            .panels
            .output
            .find("TAIL_MARKER")
            .expect("consumer stall must not discard output");
        let finished = a.panels.output.find("[Finished").unwrap();
        assert!(tail < finished);
        assert_eq!(tail, count * 1025);
        assert!(!a.panels.output.contains("truncated"));
    }
}
fn go(a: &mut App, line: usize) {
    named(a, "goto_line", json!({}));
    a.panels.quick_input.set(&format!(":{line}"));
    let viewport = a.viewport();
    a.panels.edited(&mut a.workspace, viewport);
    a.scene();
    assert!(key(a, "enter"));
    a.scene();
    assert_eq!(
        a.workspace
            .editor()
            .text
            .byte_to_line(a.workspace.editor().caret),
        line - 1
    );
}
fn top(a: &mut App) -> usize {
    let anchor = a.view.text_scroll_anchor(a.workspace.editor());
    a.workspace.editor().text.byte_to_line(anchor.byte)
}
#[test]
fn round18_verify_jump_history_reveals_unchanged_destination_and_tabs_stay_parked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let paths = [root.join("f0"), root.join("f1")];
    for path in &paths {
        std::fs::write(path, "line\n".repeat(6000)).unwrap();
    }
    let mut a = app("", None);
    set(&mut a, "scroll_speed", json!(0));
    named(&mut a, "open_file", json!({"file":paths[0]}));
    go(&mut a, 3000);
    a.view.scroll(a.workspace.editor(), 0., -1e9);
    assert!(!caret_visible(&a));
    named(&mut a, "open_file", json!({"file":paths[1]}));
    go(&mut a, 100);
    go(&mut a, 5000);
    for _ in 0..3 {
        assert!(key(&mut a, "ctrl+-"));
        a.scene();
        assert!(caret_visible(&a));
    }
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&paths[0])
    );
    assert_eq!(
        a.workspace
            .editor()
            .text
            .byte_to_line(a.workspace.editor().caret),
        2999
    );
    for command in ["jump_forward", "jump_back"] {
        a.view.scroll(a.workspace.editor(), 0., -1e9);
        named(&mut a, command, json!({}));
        a.scene();
        assert!(caret_visible(&a));
    }
    a.view.scroll(a.workspace.editor(), 0., -1e9);
    let sheet = a.workspace.tabs[a.workspace.active].sheet;
    let parked = a.view.scroll_y;
    named(&mut a, "next_view", json!({}));
    assert_ne!(a.workspace.tabs[a.workspace.active].sheet, sheet);
    named(&mut a, "prev_view", json!({}));
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, sheet);
    assert_eq!(a.view.scroll_y, parked);
    assert!(!caret_visible(&a));
    // A bookmark/mark destination must reveal even when it is already selected.
    named(&mut a, "toggle_bookmark", json!({}));
    named(&mut a, "set_mark", json!({}));
    for command in [
        "next_bookmark",
        "prev_bookmark",
        "select_to_mark",
        "swap_with_mark",
    ] {
        a.view.scroll(a.workspace.editor(), 0., -1e9);
        named(&mut a, command, json!({}));
        assert!(caret_visible(&a), "{command}");
    }
}
fn geometry_app() -> App {
    let mut a = app(&format!("{}\n", "words ".repeat(30)).repeat(6000), None);
    a.options.mode = Mode::Interactive;
    set(&mut a, "word_wrap", json!(true));
    set(&mut a, "scroll_speed", json!(0));
    a.scene();
    a
}
fn park(a: &mut App, row: usize) {
    let at = a.workspace.editor().text.line_to_byte(row + 1);
    a.workspace.editor_mut().set_caret(at, false);
    a.view.apply_style(a.workspace.editor());
    let visual = a
        .workspace
        .editor()
        .layout
        .borrow()
        .visual_at(
            &a.workspace.editor().text,
            a.workspace.editor().text.line_to_byte(row),
        )
        .0;
    a.view.scroll(
        a.workspace.editor(),
        0.,
        visual as f64 * a.view.line_height - a.view.scroll_y,
    );
    a.scene();
    assert_eq!(top(a), row);
    assert!(caret_visible(a));
}
#[test]
fn round18_verify_geometry_sidebar_resize_and_single_group_zoom_anchor() {
    let mut a = geometry_app();
    park(&mut a, 4000);
    for _ in 0..3 {
        named(&mut a, "toggle_side_bar", json!({}));
        a.scene();
        assert_eq!(top(&mut a), 4000);
        assert!(caret_visible(&a));
        for width in [850., 1600., 1200.] {
            a.resize_views(
                Size {
                    width,
                    height: 800.,
                },
                1.,
            );
            a.scene();
            assert_eq!(top(&mut a), 4000);
            assert!(caret_visible(&a));
        }
        for command in [
            "increase_font_size",
            "increase_font_size",
            "decrease_font_size",
            "decrease_font_size",
        ] {
            named(&mut a, command, json!({}));
            a.scene();
            assert_eq!(top(&mut a), 4000, "{command}");
            assert!(caret_visible(&a));
        }
    }
}
fn pane_tops(a: &mut App) -> Vec<(u64, usize, bool)> {
    a.scene();
    let focused = a.workspace.tabs[a.workspace.active].sheet;
    a.workspace
        .tabs
        .iter()
        .filter(|t| a.sheet_bounds.contains_key(&t.sheet))
        .map(|tab| {
            let view = if tab.sheet == focused {
                &a.view
            } else {
                &a.sheet_views[&tab.sheet]
            };
            let at = view.hit_test(
                &tab.editor,
                Point {
                    x: view.gutter(&tab.editor) + 1.,
                    y: view.tab_height + 4.,
                },
            );
            let rect = view.rect_at(&tab.editor, tab.editor.caret);
            (
                tab.sheet,
                tab.editor.text.byte_to_line(at),
                rect.y >= view.tab_height + 4. - 1e-6
                    && rect.y + rect.height <= view.size.height - 4. + 1e-6,
            )
        })
        .collect()
}
#[test]
fn round18_verify_clone_zoom_resize_and_font_reload_anchor_every_pane() {
    let mut a = geometry_app();
    named(&mut a, "set_layout", json!({"preset":"columns2"}));
    park(&mut a, 4000);
    let first = a.workspace.tabs[a.workspace.active].sheet;
    named(&mut a, "clone_file", json!({}));
    named(&mut a, "move_to_group", json!({"group":1}));
    a.scene();
    park(&mut a, 5000);
    assert_eq!(a.workspace.workbench.groups.len(), 2);
    let expected = pane_tops(&mut a);
    assert_eq!(expected.len(), 2);
    assert!(expected.iter().all(|(_, _, visible)| *visible));
    assert!(
        expected
            .iter()
            .any(|(sheet, row, _)| *sheet == first && *row == 4000)
    );
    for _ in 0..3 {
        for command in [
            "increase_font_size",
            "increase_font_size",
            "decrease_font_size",
            "decrease_font_size",
        ] {
            named(&mut a, command, json!({}));
            assert_eq!(pane_tops(&mut a), expected, "{command}");
        }
        for width in [1500., 1000., 1200.] {
            a.resize_views(
                Size {
                    width,
                    height: 800.,
                },
                1.,
            );
            assert_eq!(pane_tops(&mut a), expected);
        }
        for face in ["Menlo", "Monaco"] {
            named(
                &mut a,
                "set_setting",
                json!({"setting":"font_face","value":face}),
            );
            let fonts = Fonts::load_named(face).unwrap();
            a.reload_font_views(fonts, 1.);
            assert_eq!(pane_tops(&mut a), expected, "{face}");
        }
        named(&mut a, "toggle_side_bar", json!({}));
        assert_eq!(pane_tops(&mut a), expected);
    }
}
#[test]
fn round18_verify_unwrap_long_line_keeps_horizontal_reading_position() {
    let mut a = geometry_app();
    named(&mut a, "new_file", json!({}));
    named(
        &mut a,
        "insert",
        json!({"characters":"word ".repeat(12000)}),
    );
    set(&mut a, "word_wrap", json!(true));
    a.workspace.editor_mut().set_caret(0, false);
    a.scene();
    a.view
        .scroll(a.workspace.editor(), 0., 1500. * a.view.line_height);
    let at = a.view.text_scroll_anchor(a.workspace.editor()).byte;
    assert!(at > 10000);
    set(&mut a, "word_wrap", json!(false));
    a.scene();
    assert_eq!(a.view.scroll_y, 0.);
    assert!(a.view.scroll_x > 0.);
    assert_eq!(a.view.text_scroll_anchor(a.workspace.editor()).byte, at);
    set(&mut a, "word_wrap", json!(true));
    a.scene();
    assert_eq!(a.view.text_scroll_anchor(a.workspace.editor()).byte, at);
}
#[test]
fn round18_verify_unwrap_keeps_visible_caret_and_horizontal_park() {
    let mut a = geometry_app();
    named(&mut a, "new_file", json!({}));
    named(
        &mut a,
        "insert",
        json!({"characters":"word ".repeat(12000)}),
    );
    set(&mut a, "word_wrap", json!(true));
    set(&mut a, "show_minimap", json!(false));
    a.scene();
    a.workspace.editor_mut().set_caret(0, false);
    a.view
        .scroll(a.workspace.editor(), 0., 1500. * a.view.line_height);
    let byte = a
        .workspace
        .editor()
        .layout
        .borrow()
        .offset(&a.workspace.editor().text, 1503, 8.);
    a.workspace.editor_mut().set_caret(byte, false);
    a.view.apply_style(a.workspace.editor());
    assert!(caret_visible(&a));
    set(&mut a, "word_wrap", json!(false));
    a.scene();
    let rect = a.view.rect_at(a.workspace.editor(), byte);
    assert!(caret_visible(&a));
    assert!(rect.x >= a.view.gutter(a.workspace.editor()));
    assert!(rect.x + rect.width <= a.view.size.width);
    a.workspace.editor_mut().set_caret(0, false);
    a.view.apply_style(a.workspace.editor());
    let parked = a.view.text_scroll_anchor(a.workspace.editor()).byte;
    assert!(parked > 10000);
    named(&mut a, "increase_font_size", json!({}));
    a.scene();
    assert_eq!(a.view.text_scroll_anchor(a.workspace.editor()).byte, parked);
    assert!(a.view.rect_at(a.workspace.editor(), 0).x < 0.);
}

#[test]
fn round18_verify_picker_remembers_multiple_matching_sets_and_signal_name() {
    let mut a = app("", None);
    a.workspace.build_systems = [
        ("RustA", "source.rust"),
        ("RustB", "source.rust"),
        ("PyA", "source.python"),
        ("PyB", "source.python"),
    ]
    .into_iter()
    .map(|(name, selector)| {
        vel_build::BuildSystem::from_value(
            name,
            json!({"selector":selector,"cmd":["/bin/echo",name],"quiet":true}),
        )
        .unwrap()
    })
    .collect();
    for (syntax, choice) in [("Rust/Rust", "RustB"), ("Python/Python", "PyA")] {
        set_syntax(&mut a, syntax);
        named(&mut a, "build", json!({"select":true}));
        choose_build(&mut a, choice);
        finish(&mut a);
        assert_eq!(a.panels.output, format!("{choice}\n"));
    }
    for (syntax, expected) in [
        ("Rust/Rust", "RustB"),
        ("Python/Python", "PyA"),
        ("Rust/Rust", "RustB"),
    ] {
        set_syntax(&mut a, syntax);
        assert!(key(&mut a, "super+b"));
        assert!(a.builds.task.is_some());
        finish(&mut a);
        assert_eq!(a.panels.output, format!("{expected}\n"));
    }
    named(
        &mut a,
        "exec",
        json!({"cmd":["/bin/sh","-c","kill -USR1 $$"]}),
    );
    finish(&mut a);
    assert!(
        a.panels
            .output
            .contains(&format!("with signal {} (SIGUSR1)", libc::SIGUSR1))
    );
    assert!(!a.panels.output.contains("Unknown"));
}

#[path = "app_lane_b_round18_final_tests.rs"]
mod final_verification;
