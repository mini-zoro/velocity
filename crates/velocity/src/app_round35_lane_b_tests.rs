use super::*;
use vel_config::keymap::Key;
use vel_render::scene::{Scene, Surface};
use vel_text::Text;

fn fixture() -> App {
    fixture_with("some text\n", None)
}

fn fixture_with(text: &str, data: Option<PathBuf>) -> App {
    let options = Options::default();
    let now = Instant::now();
    App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        Workspace::new(Text::from(text), data, None, false).unwrap(),
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
    )
}

#[test]
fn r35_widget_settings_select_named_context_menu_only_for_widget() {
    let data = tempfile::tempdir().unwrap();
    let user = data.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Widget.sublime-settings"),
        r#"{"context_menu":"Fixture Context"}"#,
    )
    .unwrap();
    std::fs::write(
        user.join("Fixture Context.sublime-menu"),
        r#"[{"caption":"Only in widget","command":"command_note","args":{"message":"fixture"}}]"#,
    )
    .unwrap();
    let mut a = fixture_with("a line\n", Some(data.path().into()));
    a.run_named(None, "show_panel", &json!({"panel":"find"}));
    a.scene();
    let query = a.panels.query.bounds;
    let widget = a.headless_context_menu(Point {
        x: query.x + 5.,
        y: query.y + 5.,
    });
    assert_eq!(
        widget
            .iter()
            .map(|row| row.caption.as_str())
            .collect::<Vec<_>>(),
        ["Only in widget"]
    );
    let editor = a.headless_context_menu(Point {
        x: a.content.x + 100.,
        y: a.content.y + 65.,
    });
    assert!(!editor.is_empty());
    assert!(!editor.iter().any(|row| row.caption == "Only in widget"));
}

#[test]
fn r35_scrollbar_mousemap_is_scoped_to_visible_track() {
    let data = tempfile::tempdir().unwrap();
    let user = data.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("Scroll Bar.sublime-mousemap"),
        r#"[{"button":"button1","press_command":"command_note","press_args":{"message":"track press"},
          "command":"command_note","args":{"message":"track release"}}]"#).unwrap();
    let mut a = fixture_with(&"fixture text\n".repeat(240), Some(data.path().to_owned()));
    a.workspace
        .apply_user_preference("overlay_scroll_bars", Some(json!("disabled")));
    a.scene();
    let (track, _) = a.view.scrollbar_rects();
    assert!(track.width > 0.);
    let on_track = Point {
        x: a.content.x + track.x + track.width / 2.,
        y: a.content.y + track.y + track.height / 2.,
    };
    let on_text = Point {
        x: a.content.x + a.view.gutter(a.workspace.editor()) + 24.,
        y: a.content.y + track.y + 14.,
    };
    let mouse = |a: &mut App, kind, position| {
        a.handle_mouse_event(
            None,
            MouseEvent {
                button: 0,
                kind,
                position,
                click_count: 1,
                modifiers: Modifiers(0),
                timestamp: 0.,
            },
        )
    };
    mouse(&mut a, MouseKind::Down, on_text);
    mouse(&mut a, MouseKind::Up, on_text);
    assert_ne!(a.panels.message, "track press");
    assert_ne!(a.panels.message, "track release");
    let scroll_before = a.view.scroll_y;
    mouse(&mut a, MouseKind::Down, on_track);
    assert_eq!(a.panels.message, "track press");
    assert_eq!(a.view.scroll_y, scroll_before);
    mouse(&mut a, MouseKind::Up, on_text);
    assert_eq!(a.panels.message, "track release");
}

fn key(a: &mut App, key: &str) {
    assert!(a.dispatch_key_binding(None, &Key::parse(key).unwrap(), Instant::now(), 0..0));
}

fn wait_choices(a: &mut App, count: usize) {
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    while a.panels.quick.items.len() != count {
        a.panels.poll(&mut a.workspace, 0..0);
        assert!(
            Instant::now() < deadline,
            "quick panel did not publish {count} choices"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn r35_tab_chords_add_focus_and_deselect_without_closing() {
    let mut a = fixture();
    for _ in 0..3 {
        a.run_named(None, "new_file", &json!({}));
    }
    let sheets = a.workspace.workbench.groups[0].sheets.clone();
    a.workspace.select(1);
    key(&mut a, "super+j");
    key(&mut a, "super+shift+left");
    assert_eq!(a.workspace.workbench.groups[0].visible(), sheets[..2]);
    key(&mut a, "super+j");
    key(&mut a, "super+shift+right");
    assert_eq!(a.workspace.workbench.groups[0].visible(), sheets[..2]);
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, sheets[1]);

    a.workspace.click_sheet(3, true, false);
    key(&mut a, "super+j");
    key(&mut a, "super+shift+[");
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, sheets[1]);
    key(&mut a, "super+j");
    key(&mut a, "super+shift+]");
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, sheets[3]);
    assert_eq!(
        a.workspace.workbench.groups[0].visible(),
        [sheets[0], sheets[1], sheets[3]]
    );
    key(&mut a, "super+j");
    key(&mut a, "super+shift+[");
    key(&mut a, "super+j");
    key(&mut a, "super+left");
    assert_eq!(
        a.workspace.workbench.groups[0].visible(),
        [sheets[1], sheets[3]]
    );
    key(&mut a, "super+j");
    key(&mut a, "super+right");
    assert_eq!(a.workspace.workbench.groups[0].visible(), [sheets[1]]);
    assert_eq!(a.workspace.workbench.groups[0].sheets, sheets);
}

#[test]
fn r35_move_selected_commands_keep_order_focus_and_caret_move() {
    let mut a = fixture();
    for _ in 0..2 {
        a.run_named(None, "new_file", &json!({}));
    }
    let sheets = a.workspace.workbench.groups[0].sheets.clone();
    a.workspace.select(2);
    a.workspace.click_sheet(0, true, false);
    a.workspace
        .workbench
        .set_layout(vel_ui::workbench::Layout::grid(2, 1))
        .unwrap();
    a.run_named(None, "move_to", &json!({"group":1}));
    assert_eq!(a.workspace.workbench.active, 1);
    assert_eq!(a.workspace.workbench.groups[0].sheets, [sheets[1]]);
    assert_eq!(
        a.workspace.workbench.groups[1].visible(),
        [sheets[0], sheets[2]]
    );
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, sheets[0]);
    a.run_named(None, "move_to_neighbouring", &json!({"direction":"left"}));
    assert_eq!(a.workspace.workbench.active, 0);
    assert_eq!(
        a.workspace.workbench.groups[0].sheets,
        [sheets[1], sheets[0], sheets[2]]
    );
    assert_eq!(
        a.workspace.workbench.groups[0].visible(),
        [sheets[0], sheets[2]]
    );
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, sheets[0]);
    a.run_named(None, "move_to", &json!({"to":"eof"}));
    assert_eq!(a.workspace.editor().caret, "some text\n".len());
}

#[test]
fn r35_tab_dropdown_command_click_toggles_membership() {
    let mut a = fixture();
    for _ in 0..2 {
        a.run_named(None, "new_file", &json!({}));
    }
    let sheets = a.workspace.workbench.groups[0].sheets.clone();
    a.workspace.select(0);
    for selected in [true, false] {
        a.panels.tab_dropdown(
            &mut a.workspace,
            0,
            Rect {
                x: 300.,
                y: 30.,
                width: 20.,
                height: 20.,
            },
        );
        wait_choices(&mut a, 3);
        a.scene();
        let row = a
            .panels
            .quick
            .items
            .iter()
            .position(|item| item.value == 1)
            .unwrap();
        let point = Point {
            x: a.panels.quick_bounds.x + 12.,
            y: a.panels.quick_list_top() + (row as f64 + 0.5) * a.panels.quick.scroll.row_height,
        };
        a.handle_mouse_event(
            None,
            MouseEvent {
                button: 0,
                kind: MouseKind::Down,
                position: point,
                click_count: 1,
                modifiers: Modifiers(1 << 20),
                timestamp: 0.,
            },
        );
        assert_eq!(
            a.workspace.workbench.groups[0]
                .selected
                .contains(&sheets[1]),
            selected
        );
        assert!(
            a.workspace.workbench.groups[0]
                .selected
                .contains(&sheets[0])
        );
    }
}

#[test]
fn r35_goto_modifiers_keep_expected_tabs_on_acceptance() {
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("target.txt");
    std::fs::write(&path, "target").unwrap();
    let path = path.canonicalize().unwrap();
    for (key, retain) in [
        ("shift+enter", [true, true, true]),
        ("super+enter", [true, true, false]),
        ("alt+enter", [true, false, true]),
    ] {
        let mut a = fixture();
        for _ in 0..2 {
            a.run_named(None, "new_file", &json!({}));
        }
        a.workspace
            .open(crate::cli::FileLocation::path_only(path.clone()))
            .unwrap();
        let sheets = a.workspace.workbench.groups[0].sheets.clone();
        a.workspace.select(0);
        a.workspace.click_sheet(2, true, false);
        a.workspace.click_sheet(1, true, false);
        a.run_named(None, "show_overlay", &json!({"overlay":"goto"}));
        a.handle_text_input(
            None,
            TextInput::Insert {
                text: "target.txt".into(),
                replacement: None,
            },
        );
        wait_choices(&mut a, 1);
        let (used, _) = a
            .panels
            .key(&mut a.workspace, &Key::parse(key).unwrap(), 0..0)
            .unwrap();
        assert!(used, "{key}");
        let group = &a.workspace.workbench.groups[0];
        assert_eq!(group.focused, Some(sheets[3]));
        assert!(group.selected.contains(&sheets[3]));
        for (id, retain) in sheets[..3].iter().zip(retain) {
            assert_eq!(group.selected.contains(id), retain, "{key}");
        }
        if key == "super+enter" {
            assert_eq!(group.sheets, [sheets[0], sheets[1], sheets[3], sheets[2]]);
        }
    }
}

fn fixture_pixels(scene: &Scene) -> bool {
    scene
        .layers
        .iter()
        .flat_map(|l| &l.surfaces)
        .any(|s| matches!(s, Surface::Image(q) if q.image.id == 0xffff_ffff_1234_5678))
}

#[test]
fn r35_html_sheet_title_hover_is_painted_and_dismissed() {
    let mut a = fixture();
    a.run_named(
        None,
        "new_html_sheet",
        &json!({
            "name":"Tooltip test",
            "content":"<a title='Details &amp; more' href='https://example.invalid/'>Hover here</a>"
        }),
    );
    a.scene();
    let point = {
        let html = a.workspace.editor().html.borrow();
        let sheet = html.sheet.as_ref().unwrap();
        let tip = &sheet.display.tooltips[0];
        Point {
            x: a.content.x + sheet.bounds.x + f64::from(tip.rect.x) + 1.,
            y: a.content.y + sheet.bounds.y + f64::from(tip.rect.y) + 1.,
        }
    };
    let move_to = |a: &mut App, position| {
        a.handle_mouse_event(
            None,
            MouseEvent {
                button: 0,
                kind: MouseKind::Move,
                position,
                click_count: 0,
                modifiers: Modifiers(0),
                timestamp: 0.,
            },
        );
        a.scene()
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .any(|r| r.text == "Details & more")
    };
    assert!(move_to(&mut a, point));
    assert!(!move_to(&mut a, Point { x: 0., y: 0. }));
}

#[test]
fn r35_remote_minihtml_placeholder_then_pixels_on_all_three_surfaces() {
    use std::sync::{Mutex, mpsc};
    let (started, work) = mpsc::channel();
    let (release, hold) = mpsc::channel();
    let hold = Mutex::new(hold);
    let mut a = fixture();
    let url = "https://localhost/round35.gif";
    let markup = format!("<img src='{url}' width='16' height='16'>");
    vel_ui::minihtml::enable_remote_images(move |url| {
        started.send(url.to_owned()).unwrap();
        hold.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(30))
            .ok()?;
        Some(Arc::new(vel_render::scene::Image {
            logical_size: [1., 1.],
            id: 0xffff_ffff_1234_5678,
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255].into(),
        }))
    });
    a.run_named(None, "show_popup", &json!({"content":markup}));
    let loading = a.scene();
    assert_eq!(
        work.recv_timeout(crate::test_timing::HANG_GUARD).unwrap(),
        url
    );
    assert!(!fixture_pixels(&loading));
    assert!(
        loading
            .layers
            .iter()
            .flat_map(|l| &l.rects)
            .any(|r| r.color == [0.5, 0.5, 0.5, 0.2])
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    loop {
        if a.workspace.themes.ui.borrow().metrics.poll_images() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "remote image worker did not reply"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(fixture_pixels(&a.scene()));
    a.run_named(None, "hide_popup", &json!({}));
    a.run_named(
        None,
        "add_phantoms",
        &json!({
            "key":"remote", "items":[{"region":[0,0],"content":markup,"layout":"block"}]
        }),
    );
    assert!(fixture_pixels(&a.scene()));
    a.run_named(
        None,
        "new_html_sheet",
        &json!({"name":"Remote image","content":markup}),
    );
    assert!(fixture_pixels(&a.scene()));
    assert!(!a.workspace.themes.ui.borrow().metrics.images_pending());
    assert!(work.try_recv().is_err());
}

#[test]
fn r35_font_picker_preselects_current_and_writes_on_acceptance() {
    let mut a = fixture();
    a.workspace
        .apply_user_preference("font_face", Some(json!("Menlo")));
    let families = || vec!["Menlo".into(), "Courier New".into(), "Menlo".into()];
    a.font_choices(families());
    wait_choices(&mut a, 2);
    assert_eq!(
        a.panels.quick.items[a.panels.quick.selected].caption,
        "Menlo"
    );
    a.panels.cancel_overlay(&mut a.workspace);
    assert_eq!(a.workspace.settings.get("font_face"), Some(&json!("Menlo")));

    a.font_choices(families());
    wait_choices(&mut a, 2);
    a.panels.quick.selected = a
        .panels
        .quick
        .items
        .iter()
        .position(|item| item.caption == "Courier New")
        .unwrap();
    let (_, action) = a
        .panels
        .key(&mut a.workspace, &Key::parse("enter").unwrap(), 0..0)
        .unwrap();
    let action = action.unwrap();
    assert_eq!(action.command, "set_font_face");
    a.run_named(None, &action.command, &action.args);
    assert_eq!(
        a.workspace.settings.get("font_face"),
        Some(&json!("Courier New"))
    );
    assert_eq!(a.workspace.editor().preferences.font_face, "Courier New");
}
