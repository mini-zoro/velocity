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

/// Runs a released tab's landing to its end and lets the image hand-off expire.
fn land(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(1);
    app.advance_tab_landing(None, end);
    app.release_landed_tab_image(None, end + Duration::from_secs(1), true);
}

fn dragged(app: &App) -> super::tab_drag::TabDrag {
    app.tab_press.as_ref().unwrap().drag.unwrap()
}

fn shared_slot(app: &App) -> Option<crate::session::DragSlot> {
    app.sessions.borrow().tab_drag.as_ref().unwrap().slot
}

#[test]
fn tab_press_threshold_floats_the_tab_and_escape_eases_it_home() {
    let mut a = app();
    a.workspace.new_file().unwrap();
    a.window_frame = Some([600., 300., a.window_size.width, a.window_size.height]);
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
    let press = a.tab_press.as_ref().unwrap();
    assert!(press.drag.is_none());
    // The image is ready before the threshold, so the first drag event only moves it.
    assert_eq!(press.image, Some(true));
    mouse(&mut a, MouseKind::Up, near);
    assert!(a.tab_press.is_none() && a.torn_off_windows.is_empty());
    assert!(a.sessions.borrow().tab_drag.is_none());
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, order[0]);

    mouse(&mut a, MouseKind::Down, start);
    let lifted = Point {
        x: start.x + 4.,
        ..start
    };
    mouse(&mut a, MouseKind::Drag, lifted);
    let drag = dragged(&a);
    assert!(drag.floating && !drag.detached);
    let on_screen = |p: Point| Point {
        x: p.x + 600.,
        y: p.y + 300.,
    };
    assert_eq!(drag.pointer, on_screen(lifted));
    // At the threshold the floating tab covers the strip tab exactly (the image's
    // grab offset puts the pointer back where it was pressed) ...
    let began = Instant::now() - Duration::from_secs(5);
    let fresh = |drag: super::tab_drag::TabDrag, at: Duration| {
        let mut drag = drag;
        drag.started_at(began);
        drag.anchor(began + at)
    };
    assert_eq!(fresh(drag, Duration::ZERO), on_screen(start));
    // ... then closes the threshold's lead on the pointer smoothly, at most half a
    // point ahead of a still pointer per 60 Hz frame, and keeps the press-time grab
    // offset from then on.
    let mut last = on_screen(start);
    for frame in 1..=13 {
        let at = fresh(drag, Duration::from_micros(16_667 * frame));
        assert!(
            (at.x - last.x).abs() < 0.5,
            "frame {frame}: {last:?} -> {at:?}"
        );
        assert_eq!(at.y, last.y);
        last = at;
    }
    assert_eq!(last, on_screen(lifted));
    // The tab leaves a gap exactly where it was; the floating image replaces it.
    assert_eq!(
        shared_slot(&a),
        Some(crate::session::DragSlot {
            window: a.session_id,
            group: 0,
            index: 0
        })
    );
    let label = a
        .sessions
        .borrow()
        .tab_drag
        .as_ref()
        .unwrap()
        .tab
        .label
        .clone();
    let label_runs = |scene: &vel_render::scene::Scene| {
        scene
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            // A tab narrower than its label paints it truncated ("first docum…").
            .filter(|run| {
                run.text == label
                    || run
                        .text
                        .strip_suffix('…')
                        .is_some_and(|prefix| !prefix.is_empty() && label.starts_with(prefix))
            })
            .count()
    };
    let scene = a.scene();
    assert_eq!(tab(&a, order[0]), original);
    assert!(scene.layers.iter().all(|layer| layer.shadows.is_empty()));
    let painted = label_runs(&scene);
    let shared = a.sessions.borrow_mut().tab_drag.take();
    // Without the floating drag the label is painted once more: the gap is real.
    let scene = a.scene();
    assert_eq!(label_runs(&scene), painted + 1);
    a.sessions.borrow_mut().tab_drag = shared;
    a.scene();

    // Both axes follow the pointer, including far below the strip.
    for (dx, dy) in [(9., 7.), (31., 45.), (55., 160.), (70., 12.)] {
        let point = Point {
            x: start.x + dx,
            y: start.y + dy,
        };
        mouse(&mut a, MouseKind::Drag, point);
        assert_eq!(
            dragged(&a).anchor(Instant::now() + Duration::from_secs(1)),
            Point {
                x: point.x + 600.,
                y: point.y + 300.
            }
        );
    }
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
    key(&mut a, "escape");
    assert!(a.tab_press.is_none());
    // On its way home the gap is back in its original slot and the order never changed.
    assert_eq!(shared_slot(&a).unwrap().index, 0);
    assert!(a.tab_landing.is_some());
    land(&mut a);
    assert!(a.tab_landing.is_none());
    assert!(a.sessions.borrow().tab_drag.is_none());
    assert!(a.sessions.borrow().drag_image_hide.is_none());
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
fn an_event_older_than_the_shown_pointer_never_pulls_the_floating_tab_back() {
    let mut a = app();
    a.workspace.new_file().unwrap();
    a.scene();
    let start = center(tab(&a, a.workspace.tabs[0].sheet));
    let at = |x: f64, y: f64, timestamp: f64| MouseEvent {
        button: 0,
        kind: MouseKind::Drag,
        position: Point {
            x: start.x + x,
            y: start.y + y,
        },
        click_count: 1,
        modifiers: Modifiers::default(),
        timestamp,
    };
    mouse(&mut a, MouseKind::Down, start);
    a.handle_mouse_event(None, at(10., 20., 5.));
    let shown = dragged(&a).pointer;
    // Queued behind a newer read: the slot logic still sees it, the tab does not move.
    a.handle_mouse_event(None, at(6., 12., 4.));
    assert_eq!(dragged(&a).pointer, shown);
    a.handle_mouse_event(None, at(30., 40., 6.));
    assert_eq!(
        dragged(&a).pointer,
        Point {
            x: start.x + 30.,
            y: start.y + 40.
        }
    );
    mouse(&mut a, MouseKind::Up, start);
}

#[test]
fn drop_on_the_strip_eases_into_the_gap_before_the_order_changes() {
    let mut a = app();
    a.workspace.new_file().unwrap();
    a.workspace.new_file().unwrap();
    a.window_frame = Some([600., 300., a.window_size.width, a.window_size.height]);
    a.scene();
    let order = a.workspace.workbench.groups[0].sheets.clone();
    let start = center(tab(&a, order[1]));
    let end = Point {
        x: center(tab(&a, order[2])).x + 4.,
        ..start
    };
    mouse(&mut a, MouseKind::Down, start);
    mouse(
        &mut a,
        MouseKind::Drag,
        Point {
            x: start.x + 40.,
            y: start.y + 60.,
        },
    );
    mouse(&mut a, MouseKind::Drag, end);
    mouse(&mut a, MouseKind::Up, end);
    // Released over slot 2: the order still waits for the landing.
    assert_eq!(a.workspace.workbench.groups[0].sheets, order);
    assert_eq!(shared_slot(&a).unwrap().index, 2);
    let landing = a.tab_landing.unwrap();
    assert_eq!(
        landing.action,
        super::tab_drag::Landing::Insert(shared_slot(&a).unwrap())
    );
    a.scene();
    let slot = tab(&a, order[1]);
    a.advance_tab_landing(None, Instant::now());
    assert_eq!(a.workspace.workbench.groups[0].sheets, order);
    a.advance_tab_landing(None, Instant::now() + Duration::from_millis(121));
    assert_eq!(
        a.workspace.workbench.groups[0].sheets,
        [order[0], order[2], order[1]]
    );
    assert!(a.sessions.borrow().tab_drag.is_none());
    // The image lands on the slot's pixels and waits for this window's next frame.
    let hide = a.sessions.borrow().drag_image_hide.unwrap();
    assert_eq!(hide.window, a.session_id);
    a.scene();
    assert_eq!(tab(&a, order[1]), slot);
    a.release_landed_tab_image(None, hide.after, true);
    assert!(a.sessions.borrow().drag_image_hide.unwrap().rendered);
    a.release_landed_tab_image(None, hide.after, true);
    assert!(a.sessions.borrow().drag_image_hide.is_none());
    assert!(a.torn_off_windows.is_empty());
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
    assert!(dragged(&a).detached);
    assert_eq!(shared_slot(&a), None);
    mouse(&mut a, MouseKind::Drag, right_strip);
    assert_eq!(shared_slot(&a).unwrap().group, 1);
    assert!(a.tab_slides.is_none());
    a.scene();
    assert!(vel_ui::contains(
        a.tab_strips[1].bounds,
        center(tab(&a, sheet))
    ));
    mouse(&mut a, MouseKind::Up, right_strip);
    // Reduce Motion lands at once.
    assert!(a.tab_landing.is_none());
    assert_eq!(a.workspace.workbench.group_of(sheet), Some(1));
    assert_eq!(a.workspace.workbench.groups[1].focused, Some(sheet));
    assert!(a.torn_off_windows.is_empty());
}

#[test]
fn an_off_strip_drop_inside_the_detach_band_eases_home_and_clicks_stay_clicks() {
    let mut a = app();
    a.workspace.new_file().unwrap();
    a.scene();
    let order = a.workspace.workbench.groups[0].sheets.clone();
    let strip = a.tab_strips[0];
    let start = center(tab(&a, order[0]));
    let over = Point {
        x: center(tab(&a, order[1])).x + 4.,
        ..start
    };
    // Just below the strip but within one strip height of it.
    let band = Point {
        x: over.x,
        y: strip.bounds.y + strip.bounds.height * 1.5,
    };
    mouse(&mut a, MouseKind::Down, start);
    mouse(&mut a, MouseKind::Drag, over);
    mouse(&mut a, MouseKind::Drag, band);
    assert!(!dragged(&a).detached);
    // The last gap stays, as the lifted tab's did, but a drop here changes nothing.
    assert_eq!(shared_slot(&a).unwrap().index, 1);
    mouse(&mut a, MouseKind::Up, band);
    assert_eq!(shared_slot(&a).unwrap().index, 0);
    assert_eq!(
        a.tab_landing.unwrap().action,
        super::tab_drag::Landing::Return
    );
    land(&mut a);
    assert_eq!(a.workspace.workbench.groups[0].sheets, order);
    assert!(a.torn_off_windows.is_empty());

    // A new press mid-landing completes the previous drop first.
    mouse(&mut a, MouseKind::Down, start);
    mouse(&mut a, MouseKind::Drag, over);
    mouse(&mut a, MouseKind::Up, over);
    assert!(a.tab_landing.is_some());
    let content = center(a.content);
    mouse(&mut a, MouseKind::Down, content);
    assert!(a.tab_landing.is_none());
    assert_eq!(a.workspace.workbench.groups[0].sheets, [order[1], order[0]]);
    mouse(&mut a, MouseKind::Up, content);
    // The image still waits for a presented frame; a frame that was not presented
    // only keeps the deadline, and the deadline ends the hand-off regardless.
    let hide = a.sessions.borrow().drag_image_hide.unwrap();
    a.release_landed_tab_image(None, hide.after, false);
    assert!(!a.sessions.borrow().drag_image_hide.unwrap().rendered);
    a.release_landed_tab_image(None, hide.after + Duration::from_millis(250), false);
    assert!(a.sessions.borrow().drag_image_hide.is_none());

    // A press on a tab retires a pending image before it loads its own.
    mouse(&mut a, MouseKind::Down, start);
    mouse(&mut a, MouseKind::Drag, over);
    mouse(&mut a, MouseKind::Up, over);
    a.advance_tab_landing(None, Instant::now() + Duration::from_secs(1));
    assert!(a.sessions.borrow().drag_image_hide.is_some());
    mouse(&mut a, MouseKind::Down, start);
    mouse(
        &mut a,
        MouseKind::Drag,
        Point {
            x: start.x + 1.,
            ..start
        },
    );
    assert!(a.sessions.borrow().drag_image_hide.is_none());
    assert_eq!(a.tab_press.as_ref().unwrap().image, Some(true));
    mouse(&mut a, MouseKind::Up, start);
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
    assert!(dragged(&a).detached && dragged(&a).floating);
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
    // The floating image stays visible over the target; its strip only opens a gap.
    assert!(
        !preview
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .any(|run| run.text.contains("unsaved"))
    );
    assert_eq!(
        b.tab_preview_order(0, &original_target).unwrap(),
        [original_target[0], sheet, original_target[1]]
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
    // The gap stays open in the target until the tab has eased into it.
    assert!(b.shared_tab_preview().is_some());
    assert!(b.workspace.sheet_index(sheet).is_none() && !b.receive_tabs());
    a.advance_tab_landing(None, Instant::now() + Duration::from_secs(1));
    assert!(a.sessions.borrow().tab_drag.is_none());
    // The target window, which now paints the tab, retires the image after a frame.
    let hide = a.sessions.borrow().drag_image_hide.unwrap();
    assert_eq!(hide.window, b.session_id);
    a.release_landed_tab_image(None, hide.after, true);
    assert!(!a.sessions.borrow().drag_image_hide.unwrap().rendered);
    b.release_landed_tab_image(None, hide.after, true);
    b.release_landed_tab_image(None, hide.after, true);
    assert!(a.sessions.borrow().drag_image_hide.is_none());
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
    assert!(dragged(&a).floating && b.shared_tab_preview().is_some());
    // The native deactivation and capture-loss callbacks use the same cancellation.
    assert!(a.cancel_tab_drag(None));
    assert!(a.tab_landing.is_none() && b.shared_tab_preview().is_none());
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
        assert!(dragged(&a).floating);
        key(&mut a, "escape");
        assert!(a.tab_press.is_none() && a.tab_landing.is_some());
        a.scene();
        land(&mut a);
        assert!(a.tab_landing.is_none());
        assert!(a.sessions.borrow().tab_drag.is_none());
        assert!(a.sessions.borrow().drag_image_hide.is_none());
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
        assert!(a.tab_landing.is_none() && a.tab_press.is_none());
        assert!(a.torn_off_windows.is_empty());
        assert_eq!(a.workspace.workbench.group_of(sheet), Some(0));
        assert!(
            b.tab_preview_order(0, &b.workspace.workbench.groups[0].sheets)
                .is_none()
        );
    }
}

#[test]
fn target_closing_mid_landing_keeps_the_tab_in_its_window() {
    let (mut a, mut b) = tab_drag_windows();
    let sheet = a.workspace.tabs[0].sheet;
    let start = center(tab(&a, sheet));
    let end = second_window_insertion_point(&a, &b);
    mouse(&mut a, MouseKind::Down, start);
    mouse(&mut a, MouseKind::Drag, end);
    mouse(&mut a, MouseKind::Up, end);
    assert!(a.tab_landing.is_some());
    a.sessions.borrow_mut().release_route(b.session_id);
    a.advance_tab_landing(None, Instant::now());
    assert!(a.tab_landing.is_none());
    assert!(a.sessions.borrow().drag_image_hide.is_none());
    assert_eq!(a.workspace.workbench.group_of(sheet), Some(0));
    assert!(!b.receive_tabs() && a.torn_off_windows.is_empty());
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
