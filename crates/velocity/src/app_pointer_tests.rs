use super::*;
use std::{cell::RefCell, rc::Rc};
use vel_config::keymap::Key;
use vel_text::Text;

fn app() -> App {
    let options = Options::default();
    let now = Instant::now();
    App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        Workspace::new(Text::from("first document\nsecond line"), None, None, false).unwrap(),
        Arc::new(AtomicBool::new(false)),
        Rc::new(RefCell::new(crate::session::Sessions::new(None))),
    )
}

fn center(rect: Rect) -> Point {
    Point {
        x: rect.x + rect.width / 2.,
        y: rect.y + rect.height / 2.,
    }
}

fn tab(app: &App, sheet: u64) -> Rect {
    app.tab_hits.iter().find(|(id, _)| *id == sheet).unwrap().1
}

fn mouse(app: &mut App, kind: MouseKind, position: Point) {
    app.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind,
            position,
            click_count: 1,
            modifiers: Modifiers::default(),
            timestamp: 0.,
        },
    );
}

fn key(app: &mut App, name: &str) {
    let viewport = app.viewport();
    assert!(app.dispatch_key_binding(None, &Key::parse(name).unwrap(), Instant::now(), viewport));
}

#[test]
fn edit_click_and_cursor_move_reset_the_visible_caret_without_reveal() {
    let mut a = app();
    a.scene();
    let half = vel_platform::DEFAULT_CARET_BLINK_INTERVAL;
    let expire = |a: &mut App| {
        a.view.reset_caret_after_input(Instant::now() - half);
        assert_eq!(
            a.view.caret_opacity(a.workspace.editor(), Instant::now()),
            0.
        );
    };
    let visible = |a: &App| {
        assert_eq!(
            a.view.caret_opacity(a.workspace.editor(), Instant::now()),
            1.
        );
    };
    expire(&mut a);
    a.handle_text_input(
        None,
        TextInput::Insert {
            text: "!".into(),
            replacement: None,
        },
    );
    visible(&a);
    expire(&mut a);
    key(&mut a, "right");
    visible(&a);
    expire(&mut a);
    let point = Point {
        x: a.content.x + a.view.gutter(a.workspace.editor()) + 10.,
        y: a.content.y + a.view.tab_height + a.view.line_height * 1.5,
    };
    mouse(&mut a, MouseKind::Down, point);
    mouse(&mut a, MouseKind::Up, point);
    visible(&a);
    expire(&mut a);
    a.workspace.editor_mut().set_caret(3, false);
    a.changed_state(Instant::now(), false);
    visible(&a);
}

#[test]
fn inactive_occluded_and_system_disabled_carets_have_no_deadline() {
    let mut a = app();
    let now = Instant::now();
    let half = vel_platform::DEFAULT_CARET_BLINK_INTERVAL;
    for style in ["blink", "smooth", "phase"] {
        a.workspace
            .editor_mut()
            .settings
            .set(Layer::View, "caret_style", json!(style));
        a.update_caret_policy(true, Some(half), now);
        assert!(a.view.next_caret_frame(a.workspace.editor(), now).is_some());
        // The native active flag is false for an occluded or unfocused window.
        a.update_caret_policy(false, Some(half), now);
        assert_eq!(a.view.next_caret_frame(a.workspace.editor(), now), None);
        assert_eq!(a.view.caret_opacity(a.workspace.editor(), now), 1.);
        a.update_caret_policy(true, None, now);
        assert_eq!(a.view.next_caret_frame(a.workspace.editor(), now), None);
    }
    a.workspace
        .editor_mut()
        .settings
        .set(Layer::View, "caret_style", json!("smooth"));
    a.accessibility_preferences.reduce_motion = true;
    a.update_caret_policy(true, Some(half), now);
    assert_eq!(
        a.view.next_caret_frame(a.workspace.editor(), now),
        Some(now + half)
    );
    a.set_window_focused(false);
    a.scene();
    assert!(a.view.show_inactive_caret);
}

#[test]
fn tab_press_threshold_lift_and_escape_restore_the_original_strip() {
    let mut a = app();
    a.workspace.new_file().unwrap();
    a.scene();
    let order = a.workspace.workbench.groups[0].sheets.clone();
    let original = tab(&a, order[0]);
    let start = center(original);
    let near = Point {
        x: start.x + 3.,
        ..start
    };
    mouse(&mut a, MouseKind::Down, start);
    mouse(&mut a, MouseKind::Drag, near);
    assert!(a.tab_press.as_ref().unwrap().drag.is_none());
    mouse(&mut a, MouseKind::Up, near);
    assert!(a.tab_press.is_none() && a.torn_off_windows.is_empty());
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, order[0]);

    mouse(&mut a, MouseKind::Down, start);
    let passed = Point {
        x: center(tab(&a, order[1])).x + 4.,
        ..start
    };
    mouse(&mut a, MouseKind::Drag, passed);
    assert_eq!(
        a.tab_preview_order(0, &order).unwrap(),
        [order[1], order[0]]
    );
    assert_eq!(a.workspace.workbench.groups[0].sheets, order);
    let scene = a.scene();
    let lifted = tab(&a, order[0]);
    assert!((lifted.x - (original.x + passed.x - start.x)).abs() < 0.01);
    assert_eq!(lifted.y, original.y);
    assert!(scene.layers.iter().any(|layer| !layer.shadows.is_empty()));
    key(&mut a, "escape");
    mouse(
        &mut a,
        MouseKind::Up,
        Point {
            x: passed.x,
            y: 800.,
        },
    );
    assert!(a.tab_press.is_none() && a.torn_off_windows.is_empty());
    assert_eq!(a.workspace.workbench.groups[0].sheets, order);
}

#[test]
fn dropping_only_targets_other_strips_and_reduce_motion_skips_slides() {
    let mut a = app();
    a.workspace.workbench.new_pane(true).unwrap();
    a.accessibility_preferences.reduce_motion = true;
    a.scene();
    let sheet = a.workspace.tabs[0].sheet;
    let start = center(tab(&a, sheet));
    let right_editor = center(a.group_bounds[1]);
    let right_strip = center(a.tab_strips[1].bounds);
    mouse(&mut a, MouseKind::Down, start);
    mouse(&mut a, MouseKind::Drag, right_editor);
    assert_eq!(a.tab_press.as_ref().unwrap().drag.unwrap().group, 0);
    mouse(&mut a, MouseKind::Drag, right_strip);
    assert_eq!(a.tab_press.as_ref().unwrap().drag.unwrap().group, 1);
    assert!(a.tab_slides.is_none());
    a.scene();
    assert!(vel_ui::contains(
        a.tab_strips[1].bounds,
        center(tab(&a, sheet))
    ));
    mouse(&mut a, MouseKind::Up, right_strip);
    assert_eq!(a.workspace.workbench.group_of(sheet), Some(1));
    assert_eq!(a.workspace.workbench.groups[1].focused, Some(sheet));
    assert!(a.torn_off_windows.is_empty());
}

#[test]
fn tear_off_inside_or_outside_the_window_moves_the_live_document_and_undo() {
    for outside_window in [false, true] {
        let mut a = app();
        a.run_named(None, "insert", &json!({"characters":"unsaved "}));
        let original = a.workspace.editor().text.to_string();
        let sheet = a.workspace.tabs[a.workspace.active].sheet;
        let doc = a.workspace.tabs[a.workspace.active].session_document;
        a.workspace.editor_mut().set_caret(1, false);
        a.workspace.editor_mut().set_caret(5, true);
        if !outside_window {
            a.workspace
                .settings
                .set(Layer::User, "close_windows_when_empty", json!(false));
            a.workspace.editor_mut().settings.set(
                Layer::View,
                "close_windows_when_empty",
                json!(false),
            );
        }
        a.scene();
        let start = center(tab(&a, sheet));
        let destination = if outside_window {
            Point { x: -60., y: -40. }
        } else {
            center(a.content)
        };
        mouse(&mut a, MouseKind::Down, start);
        let selection = a.workspace.editor().regions();
        mouse(&mut a, MouseKind::Drag, destination);
        mouse(&mut a, MouseKind::Up, destination);
        assert!(a.workspace.is_empty());
        assert_eq!(a.empty_close_requested, outside_window);
        assert!(a.workspace.sheet_index(sheet).is_none());
        assert_eq!(a.torn_off_windows.len(), 1);
        let b = &mut a.torn_off_windows[0];
        assert_eq!(b.workspace.tabs.len(), 1);
        let tab = &b.workspace.tabs[0];
        assert_eq!((tab.sheet, tab.session_document), (sheet, doc));
        assert!(tab.dirty());
        assert_eq!(b.workspace.editor().text.to_string(), original);
        assert_eq!(b.workspace.editor().regions(), selection);
        b.run_named(None, "undo", &json!({}));
        assert_eq!(
            b.workspace.editor().text.to_string(),
            "first document\nsecond line"
        );
    }
}

fn tab_drag_windows() -> (App, App) {
    let mut a = app();
    let options = a.options.clone();
    let now = Instant::now();
    let mut b = App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        Workspace::new(Text::from("target"), None, None, false).unwrap(),
        Arc::new(AtomicBool::new(false)),
        a.sessions.clone(),
    );
    b.workspace.new_file().unwrap();
    b.window_frame = Some([
        a.window_size.width + 100.,
        200.,
        b.window_size.width,
        b.window_size.height,
    ]);
    a.scene();
    b.scene();
    (a, b)
}

fn second_window_insertion_point(a: &App, b: &App) -> Point {
    let sessions = a.sessions.borrow();
    let strip = &sessions.tab_strips[&b.session_id][0];
    Point {
        x: strip.origin + strip.step + 2.,
        y: strip.bounds.y + strip.bounds.height / 2.,
    }
}

#[test]
fn detached_tab_hits_another_headless_window_and_inserts_at_the_published_index() {
    let (mut a, mut b) = tab_drag_windows();
    let inserted = format!("unsaved {}\n", "\nlong line".repeat(160));
    a.run_named(None, "insert", &json!({"characters": inserted}));
    let sheet = a.workspace.tabs[0].sheet;
    let doc = a.workspace.tabs[0].session_document;
    let start = center(tab(&a, sheet));
    let original_target = b.workspace.workbench.groups[0].sheets.clone();
    let target_point = second_window_insertion_point(&a, &b);
    mouse(&mut a, MouseKind::Down, start);
    a.workspace.editor_mut().set_caret(2, false);
    a.workspace.editor_mut().set_caret(6, true);
    let selection = a.workspace.editor().regions();
    mouse(
        &mut a,
        MouseKind::Drag,
        Point {
            x: -80.,
            y: start.y,
        },
    );
    assert!(a.tab_press.as_ref().unwrap().drag.unwrap().detached);
    assert!(a.drag_image.is_some());
    a.scene();
    assert!(a.tab_hits.is_empty());
    a.view.scroll_y = 64.;
    mouse(&mut a, MouseKind::Drag, target_point);
    let shared = a.sessions.borrow().tab_drag.clone().unwrap();
    assert_eq!(
        shared.slot,
        Some(crate::session::DragSlot {
            window: b.session_id,
            group: 0,
            index: 1
        })
    );
    assert_eq!(
        b.tab_preview_order(0, &original_target).unwrap(),
        [original_target[0], sheet, original_target[1]]
    );
    let preview = b.scene();
    assert!(
        preview
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .any(|run| run.text.contains("unsaved"))
    );
    assert!(b.tab_slides.is_some());
    assert!(b.workspace.sheet_index(sheet).is_none());
    mouse(
        &mut a,
        MouseKind::Drag,
        Point {
            y: target_point.y + 80.,
            ..target_point
        },
    );
    assert!(
        a.sessions
            .borrow()
            .tab_drag
            .as_ref()
            .unwrap()
            .slot
            .is_none()
    );
    assert!(b.tab_preview_order(0, &original_target).is_none());
    mouse(&mut a, MouseKind::Drag, target_point);
    mouse(&mut a, MouseKind::Up, target_point);
    assert!(a.tab_press.is_none());
    assert!(a.drag_image.is_none());
    assert!(a.sessions.borrow().tab_drag.is_none());
    assert!(b.receive_tabs());
    assert_eq!(b.workspace.workbench.groups[0].sheets[1], sheet);
    assert_eq!(b.workspace.workbench.groups[0].focused, Some(sheet));
    assert_eq!(b.workspace.tabs[b.workspace.active].session_document, doc);
    assert!(b.workspace.tabs[b.workspace.active].dirty());
    assert_eq!(b.workspace.tabs[b.workspace.active].scroll.1, 64.);
    assert_eq!(b.workspace.editor().regions(), selection);
    assert!(a.empty_close_requested);
    assert_eq!(b.sessions.borrow().active_window, Some(b.session_id));
    b.run_named(None, "undo", &json!({}));
    assert_eq!(
        b.workspace.editor().text.to_string(),
        "first document\nsecond line"
    );
}

#[test]
fn detached_tab_preview_obeys_animation_disabled_in_both_windows() {
    let (mut a, mut b) = tab_drag_windows();
    for app in [&mut a, &mut b] {
        app.workspace
            .editor_mut()
            .settings
            .set(Layer::View, "animation_enabled", json!(false));
    }
    let sheet = a.workspace.tabs[0].sheet;
    let start = center(tab(&a, sheet));
    let end = second_window_insertion_point(&a, &b);
    mouse(&mut a, MouseKind::Down, start);
    mouse(&mut a, MouseKind::Drag, end);
    b.scene();
    assert!(a.tab_slides.is_none() && b.tab_slides.is_none());
    assert!(a.drag_image.is_some() && b.shared_tab_preview().is_some());
    // The native deactivation and capture-loss callbacks use the same cancellation.
    assert!(a.cancel_tab_drag(None));
    assert!(a.drag_image.is_none() && b.shared_tab_preview().is_none());
    mouse(&mut a, MouseKind::Up, end);
    assert!(a.torn_off_windows.is_empty());
    assert_eq!(a.workspace.tabs[0].sheet, sheet);
}

#[test]
fn detached_drag_escape_releases_image_and_shared_state_for_repeated_cycles() {
    let mut a = app();
    a.scene();
    let sheet = a.workspace.tabs[0].sheet;
    let start = center(tab(&a, sheet));
    for _ in 0..100 {
        mouse(&mut a, MouseKind::Down, start);
        mouse(
            &mut a,
            MouseKind::Drag,
            Point {
                x: -80.,
                y: start.y,
            },
        );
        assert!(a.drag_image.is_some());
        key(&mut a, "escape");
        assert!(a.tab_press.is_none());
        assert!(a.drag_image.is_none());
        assert!(a.sessions.borrow().tab_drag.is_none());
        a.scene();
    }
}

#[test]
fn empty_space_drop_keeps_the_original_grab_offset_in_screen_space() {
    let mut a = app();
    a.window_frame = Some([600., 300., a.window_size.width, a.window_size.height]);
    a.scene();
    let rect = tab(&a, a.workspace.tabs[0].sheet);
    let start = Point {
        x: rect.x + 13.,
        y: rect.y + 8.,
    };
    let end = Point { x: 40., y: -200. };
    mouse(&mut a, MouseKind::Down, start);
    mouse(&mut a, MouseKind::Drag, end);
    mouse(&mut a, MouseKind::Up, end);
    assert_eq!(
        a.torn_off_windows[0].tear_off_anchor,
        Some(Point { x: 627., y: 92. })
    );
}

#[test]
fn closing_either_drag_window_cancels_without_dropping_on_mouse_up() {
    for source_closes in [false, true] {
        let (mut a, b) = tab_drag_windows();
        let sheet = a.workspace.tabs[0].sheet;
        let start = center(tab(&a, sheet));
        let end = second_window_insertion_point(&a, &b);
        mouse(&mut a, MouseKind::Down, start);
        mouse(&mut a, MouseKind::Drag, end);
        assert_eq!(
            a.sessions
                .borrow()
                .tab_drag
                .as_ref()
                .unwrap()
                .slot
                .unwrap()
                .window,
            b.session_id
        );
        a.sessions.borrow_mut().release_route(if source_closes {
            a.session_id
        } else {
            b.session_id
        });
        assert!(a.sessions.borrow().tab_drag.is_none());
        mouse(&mut a, MouseKind::Up, end);
        assert!(a.drag_image.is_none() && a.tab_press.is_none());
        assert!(a.torn_off_windows.is_empty());
        assert_eq!(a.workspace.workbench.group_of(sheet), Some(0));
        assert!(
            b.tab_preview_order(0, &b.workspace.workbench.groups[0].sheets)
                .is_none()
        );
    }
}

#[test]
fn cursors_follow_text_fields_chrome_and_both_split_orientations() {
    use CursorShape::*;
    let mut a = app();
    a.run_named(None, "toggle_side_bar", &json!({}));
    a.scene();
    let sheet = a.workspace.tabs[0].sheet;
    assert_eq!(a.cursor_at(center(tab(&a, sheet))), Arrow);
    assert_eq!(a.cursor_at(center(a.panels.side_bounds)), Arrow);
    assert_eq!(
        a.cursor_at(Point {
            x: 10.,
            y: a.window_size.height - 2.
        }),
        Arrow
    );
    let point = center(a.content);
    assert_eq!(a.cursor_at(point), IBeam);
    assert_eq!(
        a.cursor_at(Point {
            x: a.content.x + 8.,
            ..point
        }),
        Arrow
    );
    assert_eq!(
        a.cursor_at(Point {
            x: a.content.x + a.content.width - 20.,
            ..point
        }),
        Arrow
    );
    assert_eq!(
        a.cursor_at(Point {
            x: a.panels.sidebar_edge(),
            ..point
        }),
        ResizeHorizontal
    );
    a.run_named(None, "show_panel", &json!({"panel":"replace"}));
    a.scene();
    assert_eq!(a.cursor_at(center(a.panels.query.bounds)), IBeam);
    assert_eq!(a.cursor_at(center(a.panels.replace.bounds)), IBeam);
    assert_eq!(a.cursor_at(center(a.panels.tree.nodes[&50].rect)), Arrow);
    a.run_named(None, "show_overlay", &json!({"overlay":"command_palette"}));
    a.scene();
    assert_eq!(a.cursor_at(center(a.panels.quick_input.bounds)), IBeam);
    assert_eq!(a.cursor_at(point), Arrow);
    a.panels.cancel_overlay(&mut a.workspace);
    for (vertical, shape) in [(true, ResizeHorizontal), (false, ResizeVertical)] {
        let mut a = app();
        a.workspace.workbench.new_pane(vertical).unwrap();
        a.scene();
        let (_, divider) = a
            .workspace
            .workbench
            .layout
            .dividers(a.workbench_bounds, 1.)[0];
        assert_eq!(a.cursor_at(center(divider)), shape);
    }
}
