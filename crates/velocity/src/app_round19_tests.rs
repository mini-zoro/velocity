//! Round 19 uses the application's named commands, key dispatch, mouse events and disk loader.
#[path = "app_round19_review_tests.rs"]
mod review;
use super::*;
use crate::panels::Overlay;

fn reset_history(a: &mut App) {
    *a.workspace.jump_history.borrow_mut() = Default::default();
    a.workspace.observe_navigation();
}
fn caret(a: &mut App, point: usize) {
    a.workspace.editor_mut().set_caret(point, false);
}
fn back(a: &mut App) {
    key(a, "ctrl+-");
}
fn forward(a: &mut App) {
    key(a, "ctrl+shift+-");
}
fn drive_until(a: &mut App, condition: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    loop {
        a.workspace.poll();
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        if condition(a) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{} / {:?}",
            a.panels.message,
            a.workspace.messages
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

#[test]
fn r19_history_goto_find_click_and_command_runs() {
    let mut a = app("one\nneedle\nthree\nfour\n", "Rust", None);
    caret(&mut a, 0);
    reset_history(&mut a);
    named(&mut a, "goto_line", json!({}));
    input(&mut a, "3");
    key(&mut a, "enter");
    assert_eq!(a.workspace.editor().caret, 11);
    back(&mut a);
    assert_eq!(a.workspace.editor().caret, 0);
    forward(&mut a);
    assert_eq!(a.workspace.editor().caret, 11);

    named(&mut a, "show_panel", json!({"panel":"find"}));
    input(&mut a, "needle");
    drive_until(&mut a, |a| a.workspace.editor().selection() == (4..10));
    named(&mut a, "hide_panel", json!({}));
    back(&mut a);
    assert_eq!(a.workspace.editor().caret, 11);
    forward(&mut a);
    assert_eq!(a.workspace.editor().selection(), 4..10);

    a.content = a.panels.layout(a.window_size);
    a.view.size = Size {
        width: a.content.width,
        height: a.content.height,
    };
    a.view.scene(a.workspace.editor());
    let rect = a.view.rect_at(a.workspace.editor(), 16);
    a.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind: MouseKind::Down,
            position: Point {
                x: a.content.x + rect.x + 1.,
                y: a.content.y + rect.y + a.view.line_height / 2.,
            },
            click_count: 1,
            modifiers: Modifiers(0),
            timestamp: 0.,
        },
    );
    assert_eq!(a.workspace.editor().caret, 16);
    back(&mut a);
    assert_eq!(a.workspace.editor().selection(), 4..10);

    caret(&mut a, 0);
    reset_history(&mut a);
    for _ in 0..3 {
        key(&mut a, "right");
    }
    assert_eq!(a.workspace.jump_history.borrow().back.len(), 1);
    back(&mut a);
    assert_eq!(a.workspace.editor().caret, 0);
    forward(&mut a);
    assert_eq!(a.workspace.editor().caret, 3);
}

#[test]
fn r19_history_tab_switch_edit_mapping_closed_view_and_primary() {
    let mut a = app("αbcd\n", "Rust", None);
    caret(&mut a, 4);
    let first = a.workspace.tabs[a.workspace.active].sheet;
    named(&mut a, "new_file", json!({}));
    input(&mut a, "second");
    let second = a.workspace.tabs[a.workspace.active].sheet;
    a.workspace.select(a.workspace.sheet_index(first).unwrap());
    caret(&mut a, 4);
    reset_history(&mut a);
    a.content = a.panels.layout(a.window_size);
    a.tab_hits = vec![(
        second,
        vel_platform::Rect {
            x: 100.,
            y: 0.,
            width: 100.,
            height: 24.,
        },
    )];
    a.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind: MouseKind::Down,
            position: Point { x: 120., y: 10. },
            click_count: 1,
            modifiers: Modifiers(0),
            timestamp: 0.,
        },
    );
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, second);
    back(&mut a);
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, first);
    assert_eq!(a.workspace.editor().caret, 4);

    caret(&mut a, 0);
    input(&mut a, "XYZ");
    let records = a.workspace.jump_history.borrow().back.len();
    back(&mut a);
    assert_eq!(
        a.workspace.jump_history.borrow().back.len() + 1,
        records,
        "typing adds no consumable origin"
    );
    assert_eq!(a.workspace.editor().caret, 7);
    forward(&mut a);
    assert_eq!(a.workspace.editor().caret, 3);

    caret(&mut a, 0);
    reset_history(&mut a);
    caret(&mut a, 8);
    named(&mut a, "left_delete", json!({}));
    back(&mut a);
    assert!(
        a.workspace
            .editor()
            .text
            .is_char_boundary(a.workspace.editor().caret)
    );
    assert!(a.workspace.editor().caret <= a.workspace.editor().text.len_bytes());

    a.workspace.select(a.workspace.sheet_index(second).unwrap());
    named(&mut a, "close_file", json!({"force":true}));
    back(&mut a);
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, first);
    assert!(a.workspace.tabs.iter().all(|t| t.sheet != second));

    let selection = Selection::from_regions([Region::caret(7), Region::new(3, 1)]);
    a.workspace.editor_mut().set_selection(selection.clone());
    reset_history(&mut a);
    key(&mut a, "right");
    back(&mut a);
    assert_eq!(
        a.workspace
            .editor()
            .buffer
            .view(a.workspace.editor().id)
            .sel(),
        &selection,
        "jump history preserves bounded multi-caret selections"
    );
}

#[test]
fn r19_cancel_goto_and_focused_input_do_not_add_document_jumps() {
    let mut a = app("first\nsecond\nthird\n", "Rust", None);
    caret(&mut a, 0);
    reset_history(&mut a);
    named(&mut a, "goto_line", json!({}));
    input(&mut a, "3");
    a.workspace.editor_mut().sync(); // Native idle/paint adapter access must not record a preview.
    let viewport = a.viewport();
    a.panels.poll(&mut a.workspace, viewport);
    assert!(a.workspace.jump_history.borrow().back.is_empty());
    key(&mut a, "escape");
    assert_eq!(a.workspace.editor().caret, 0);
    assert!(a.workspace.jump_history.borrow().back.is_empty());
    named(
        &mut a,
        "show_input_panel",
        json!({"caption":"Fixture", "initial_text":""}),
    );
    input(&mut a, "a long input");
    key(&mut a, "left");
    key(&mut a, "escape");
    assert_eq!(a.workspace.editor().caret, 0);
    assert!(a.workspace.jump_history.borrow().back.is_empty());
}

fn profile() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("Packages/Fixture");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::create_dir_all(dir.path().join("Packages/User")).unwrap();
    std::fs::write(dir.path().join("Packages/User/Preferences.sublime-settings"),
        "{ // keep this comment\n  \"font_size\": 13,\n  \"color_scheme\": \"auto\",\n  \"theme\": \"auto\",\n}\n").unwrap();
    std::fs::write(package.join("Amber.sublime-color-scheme"),
        r##"{"name":"Amber", "globals":{"background":"#242000","foreground":"#ffeeaa"},"rules":[]}"##).unwrap();
    std::fs::write(package.join("Broken.sublime-color-scheme"), "{ invalid").unwrap();
    std::fs::write(
        package.join("Secret.hidden-color-scheme"),
        r##"{"globals":{"background":"#112233","foreground":"#ffffff"}}"##,
    )
    .unwrap();
    std::fs::write(
        package.join("Invisible.sublime-color-scheme"),
        r##"{"hidden":true,"globals":{"background":"#112233","foreground":"#ffffff"}}"##,
    )
    .unwrap();
    std::fs::write(package.join("Legacy.tmTheme"),
        r##"<?xml version="1.0"?><plist version="1.0"><dict><key>name</key><string>Legacy</string><key>settings</key><array><dict><key>settings</key><dict><key>background</key><string>#112233</string><key>foreground</key><string>#ffffff</string></dict></dict></array></dict></plist>"##).unwrap();
    std::fs::write(package.join("Amber.sublime-theme"),
        r##"{"rules":[{"class":"sidebar_container","layer0.tint":"#242000","layer0.opacity":1}] }"##).unwrap();
    dir
}
fn scheme(a: &App) -> Arc<vel_color_scheme::ColorScheme> {
    a.workspace
        .editor()
        .language
        .borrow()
        .as_ref()
        .unwrap()
        .scheme
        .clone()
}
fn choose(a: &mut App, caption: &str, setting: &str) {
    let target = a
        .panels
        .quick
        .items
        .iter()
        .position(|item| item.caption == caption && item.annotation == setting)
        .unwrap_or_else(|| panic!("missing {caption}/{setting}: {:?}", a.panels.quick.items));
    while a.panels.quick.selected != target {
        key(a, "down");
    }
}

#[test]
fn r19_appearance_preview_commit_cancel_and_broken_scheme() {
    let dir = profile();
    let path = dir
        .path()
        .join("Packages/User/Preferences.sublime-settings");
    let original_file = std::fs::read(&path).unwrap();
    let mut a = app("fn fixture() {}", "Rust", Some(dir.path()));
    let original_scheme = scheme(&a);
    let assets = a.workspace.assets.clone();
    let revision = a.workspace.resource_revision;
    let view = a.workspace.editor().settings.layer(Layer::View).clone();
    named(&mut a, "select_color_scheme", json!({}));
    assert_eq!(a.panels.overlay, Some(Overlay::Appearance));
    assert_eq!(a.panels.quick.current(), Some(0));
    assert!(Arc::ptr_eq(&scheme(&a), &original_scheme));
    assert!(a.panels.quick.items.iter().any(|r| r.caption == "Legacy"));
    assert!(
        !a.panels
            .quick
            .items
            .iter()
            .any(|r| r.caption.contains("Secret") || r.caption.contains("Invisible"))
    );
    choose(&mut a, "Amber", "color_scheme");
    let amber = scheme(&a);
    assert!(!Arc::ptr_eq(&amber, &original_scheme));
    choose(&mut a, "Velocity Light", "color_scheme");
    choose(&mut a, "Amber", "color_scheme");
    assert!(
        Arc::ptr_eq(&amber, &scheme(&a)),
        "compiled scheme reused by resource path"
    );
    input(&mut a, "Broken");
    assert!(Arc::ptr_eq(&amber, &scheme(&a)));
    assert!(a.panels.message.contains("Broken") || a.panels.message.contains("invalid"));
    key(&mut a, "enter");
    assert_eq!(a.panels.overlay, Some(Overlay::Appearance));
    assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 0);
    key(&mut a, "escape");
    assert!(Arc::ptr_eq(&scheme(&a), &original_scheme));
    assert_eq!(a.workspace.editor().settings.layer(Layer::View), &view);
    assert_eq!(std::fs::read(&path).unwrap(), original_file);
    assert!(Arc::ptr_eq(&assets, &a.workspace.assets));
    assert_eq!(a.workspace.resource_revision, revision);

    named(&mut a, "select_color_scheme", json!({}));
    choose(&mut a, "Amber", "color_scheme");
    key(&mut a, "enter");
    assert!(a.panels.overlay.is_none());
    a.workspace.wait_resources();
    assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 1);
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("keep this comment"));
    assert_eq!(
        vel_config::json::Document::parse(&saved).unwrap().value["color_scheme"],
        "Amber.sublime-color-scheme"
    );
}

#[test]
fn r19_auto_variants_restore_exactly_and_commit_once() {
    for (command, root, target, key_name) in [
        (
            "select_color_scheme",
            "color_scheme",
            "Dark: Amber (applies in Dark mode)",
            "dark_color_scheme",
        ),
        (
            "select_color_scheme",
            "color_scheme",
            "Light: Amber (applies in Light mode)",
            "light_color_scheme",
        ),
        (
            "select_theme",
            "theme",
            "Dark: Amber (applies in Dark mode)",
            "dark_theme",
        ),
        (
            "select_theme",
            "theme",
            "Light: Amber (applies in Light mode)",
            "light_theme",
        ),
    ] {
        let dir = profile();
        let mut a = app("fn fixture() {}", "Rust", Some(dir.path()));
        let mut other = app("unrelated", "Rust", Some(dir.path()));
        other
            .workspace
            .editor_mut()
            .settings
            .set(Layer::View, root, json!("auto"));
        let other_view = other.workspace.editor().settings.layer(Layer::View).clone();
        let before = a.workspace.editor().settings.effective();
        named(&mut a, command, json!({}));
        choose(&mut a, target, key_name);
        key(&mut a, "escape");
        assert_eq!(a.workspace.editor().settings.effective(), before);
        assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 0);
        assert_eq!(
            other.workspace.editor().settings.layer(Layer::View),
            &other_view
        );
        named(&mut a, command, json!({}));
        choose(&mut a, target, key_name);
        key(&mut a, "enter");
        a.workspace.wait_resources();
        assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 1);
        let saved = std::fs::read_to_string(
            dir.path()
                .join("Packages/User/Preferences.sublime-settings"),
        )
        .unwrap();
        let saved = vel_config::json::Document::parse(&saved).unwrap().value;
        assert_eq!(saved[root], "auto");
        assert!(saved[key_name].as_str().unwrap().starts_with("Amber."));
        assert_eq!(
            other.workspace.editor().settings.layer(Layer::View),
            &other_view
        );
    }
}

#[test]
fn r19_appearance_dismissal_and_window_view_overrides() {
    let dir = profile();
    let mut a = app("fixture", "Rust", Some(dir.path()));
    set(
        &mut a,
        "color_scheme",
        json!("Velocity Light.sublime-color-scheme"),
    );
    let before = a.workspace.editor().settings.layer(Layer::View).clone();
    for replacement in ["show_overlay", "show_panel", "select_theme"] {
        named(&mut a, "select_color_scheme", json!({}));
        choose(&mut a, "Amber", "color_scheme");
        named(&mut a, replacement, json!({}));
        assert_eq!(a.workspace.editor().settings.layer(Layer::View), &before);
        key(&mut a, "escape");
    }
    assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 0);
    named(&mut a, "select_color_scheme", json!({}));
    choose(&mut a, "Amber", "color_scheme");
    // Clicking outside the quick panel closes through the same native/headless mouse route.
    a.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind: MouseKind::Down,
            position: Point { x: 1., y: 1. },
            click_count: 1,
            modifiers: Modifiers(0),
            timestamp: 0.,
        },
    );
    assert!(a.panels.overlay.is_none());
    assert_eq!(a.workspace.editor().settings.layer(Layer::View), &before);
}

#[test]
fn r19_picker_filters_input_preselects_fixed_and_survives_appearance_change() {
    let dir = profile();
    let mut a = app("fixture", "Rust", Some(dir.path()));
    a.workspace.set_appearance(false).unwrap();
    let light = scheme(&a);
    named(&mut a, "select_color_scheme", json!({}));
    input(&mut a, "Amber");
    assert!(
        a.panels
            .quick
            .items
            .iter()
            .all(|r| r.caption.contains("Amber"))
    );
    key(&mut a, "escape");
    assert!(Arc::ptr_eq(&light, &scheme(&a)));
    named(&mut a, "select_color_scheme", json!({}));
    choose(&mut a, "Amber", "color_scheme");
    a.workspace.set_appearance(true).unwrap();
    key(&mut a, "escape");
    assert_eq!(
        a.workspace.editor().settings.string("color_scheme", ""),
        "auto"
    );
    assert!(
        !Arc::ptr_eq(&light, &scheme(&a)),
        "auto follows new system appearance after cancel"
    );
    set(
        &mut a,
        "color_scheme",
        json!("Velocity Light.sublime-color-scheme"),
    );
    named(&mut a, "select_color_scheme", json!({}));
    let current = &a.panels.quick.items[a.panels.quick.selected];
    assert_eq!(current.caption, "Velocity Light");
    choose(&mut a, "Amber", "color_scheme");
    key(&mut a, "enter");
    a.workspace.wait_resources();
    assert_eq!(
        a.workspace.editor().settings.string("color_scheme", ""),
        "Velocity Light.sublime-color-scheme",
        "a genuine View override survives commit"
    );
    assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 1);
}

#[test]
#[ignore = "headless timing; run explicitly with --ignored --nocapture"]
fn r19_jump_history_10000_caret_timing() {
    let mut a = app(&"x\n".repeat(10000), "Plain text", None);
    let selection = Selection::from_regions((0..10000).map(|row| Region::caret(row * 2)));
    a.workspace.editor_mut().set_selection(selection);
    reset_history(&mut a);
    let mut samples = Vec::new();
    for _ in 0..21 {
        let start = Instant::now();
        for _ in 0..1000 {
            a.workspace.editor().observe_navigation("insert");
        }
        samples.push(start.elapsed().as_nanos() as f64 / 1000.);
    }
    samples.sort_by(f64::total_cmp);
    eprintln!(
        "10,000-caret jump observer median {:.1} ns/hook (no caret scan)",
        samples[10]
    );
    let start = Instant::now();
    input(&mut a, "y");
    eprintln!(
        "10,000-caret insert with history {:.3} ms",
        start.elapsed().as_secs_f64() * 1000.
    );
    assert_eq!(
        a.workspace
            .editor()
            .buffer
            .view(a.workspace.editor().id)
            .sel()
            .len(),
        10000
    );
}
