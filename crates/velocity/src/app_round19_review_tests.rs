//! Review regressions use the same command, input, mouse, watcher and focus routes as the app.
#[path = "app_round19_navigation_tests.rs"]
mod navigation_review;
use super::*;

fn goto(a: &mut App, line: usize) {
    named(a, "goto_line", json!({}));
    input(a, &line.to_string());
    key(a, "enter");
}
fn row(a: &App) -> usize {
    a.workspace
        .editor()
        .text
        .byte_to_line(a.workspace.editor().caret)
}
fn open(a: &mut App, path: &Path) {
    a.workspace
        .open(crate::cli::FileLocation {
            path: path.into(),
            line: 1,
            column: 1,
        })
        .unwrap();
    a.sync_active_view();
    drive_until(a, |a| a.workspace.tabs.iter().all(|t| t.loading.is_none()));
}
fn click(a: &mut App, at: usize, kind: MouseKind) {
    a.content = a.panels.layout(a.window_size);
    a.view.size = Size {
        width: a.content.width,
        height: a.content.height,
    };
    a.view.scene(a.workspace.editor());
    let rect = a.view.rect_at(a.workspace.editor(), at);
    a.handle_mouse_event(
        None,
        MouseEvent {
            button: 0,
            kind,
            position: Point {
                x: a.content.x + rect.x + 1.,
                y: a.content.y + rect.y + a.view.line_height / 2.,
            },
            click_count: 1,
            modifiers: Modifiers(0),
            timestamp: 0.,
        },
    );
}
#[test]
fn r19_background_reload_preserves_history_and_forward() {
    let files = tempfile::tempdir().unwrap();
    let bg = files.path().canonicalize().unwrap().join("bg.txt");
    let fg = files.path().canonicalize().unwrap().join("fg.txt");
    std::fs::write(&bg, "background\noriginal\n").unwrap();
    std::fs::write(&fg, "foreground\n".repeat(30)).unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
    open(&mut a, &bg);
    open(&mut a, &fg);
    reset_history(&mut a);
    goto(&mut a, 13);
    goto(&mut a, 20);
    back(&mut a);
    let before = (
        a.workspace.jump_history.borrow().back.len(),
        a.workspace.jump_history.borrow().forward.len(),
    );
    std::fs::write(&bg, "formatter changed background\nnew\n").unwrap();
    drive_until(&mut a, |a| {
        a.workspace
            .tabs
            .iter()
            .find(|t| t.path.as_ref() == Some(&bg))
            .is_some_and(|t| t.editor.text.to_string().starts_with("formatter"))
    });
    assert_eq!(
        (
            a.workspace.jump_history.borrow().back.len(),
            a.workspace.jump_history.borrow().forward.len()
        ),
        before
    );
    key(&mut a, "right");
    back(&mut a);
    back(&mut a);
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&fg)
    );
}
#[test]
fn r19_lazy_background_load_never_records_selection() {
    let files = tempfile::tempdir().unwrap();
    let bg = files.path().canonicalize().unwrap().join("bg.txt");
    let fg = files.path().canonicalize().unwrap().join("fg.txt");
    std::fs::write(&bg, "background\n".repeat(80)).unwrap();
    std::fs::write(&fg, "foreground\n".repeat(80)).unwrap();
    let mut a = app("", "Plain Text", None);
    open(&mut a, &bg);
    goto(&mut a, 12);
    open(&mut a, &fg);
    let (window, docs) = a.workspace.snapshot().unwrap();
    a.workspace.restore_window(&window, &docs).unwrap();
    let bg_index = a
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.as_ref() == Some(&bg))
        .unwrap();
    assert!(a.workspace.tabs[bg_index].lazy.is_some());
    // Force the lazy disk loader rather than the serialized undo-history path.
    a.workspace.tabs[bg_index].lazy.as_mut().unwrap().0.history = None;
    a.workspace.tabs[bg_index]
        .editor
        .settings
        .set(Layer::View, "large_file.load_bytes", json!(1));
    a.workspace.select(bg_index);
    assert!(a.workspace.tabs[bg_index].loading.is_some());
    let fg_index = a
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.as_ref() == Some(&fg))
        .unwrap();
    a.workspace.select(fg_index);
    reset_history(&mut a);
    goto(&mut a, 13);
    goto(&mut a, 20);
    back(&mut a);
    let before = (
        a.workspace.jump_history.borrow().back.len(),
        a.workspace.jump_history.borrow().forward.len(),
    );
    drive_until(&mut a, |a| a.workspace.tabs[bg_index].loading.is_none());
    assert_eq!(
        (
            a.workspace.jump_history.borrow().back.len(),
            a.workspace.jump_history.borrow().forward.len()
        ),
        before
    );
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&fg)
    );
    a.set_window_focused(false);
    caret(&mut a, 0);
    a.set_window_focused(true);
    assert_eq!(
        (
            a.workspace.jump_history.borrow().back.len(),
            a.workspace.jump_history.borrow().forward.len()
        ),
        before
    );
}
#[test]
fn r19_mouse_clicks_are_distinct_and_drag_is_one_jump() {
    let mut a = app(&"abcdefghij\n".repeat(30), "Plain Text", None);
    caret(&mut a, 0);
    reset_history(&mut a);
    for (i, line) in [3, 8, 13, 18].into_iter().enumerate() {
        let at = a.workspace.editor().text.line_to_byte(line);
        click(&mut a, at, MouseKind::Down);
        click(&mut a, at, MouseKind::Up);
        assert_eq!(a.workspace.jump_history.borrow().back.len(), i + 1);
    }
    for expected in [13, 8, 3, 0] {
        back(&mut a);
        assert_eq!(row(&a), expected);
    }
    reset_history(&mut a);
    click(&mut a, 22, MouseKind::Down);
    for at in [33, 44, 55] {
        click(&mut a, at, MouseKind::Drag);
    }
    click(&mut a, 55, MouseKind::Up);
    assert_eq!(a.workspace.jump_history.borrow().back.len(), 1);
    back(&mut a);
    assert_eq!(row(&a), 0);
}
#[test]
fn r19_vintage_parsed_motions_and_jump_origins() {
    let mut a = app(&"one two three\n".repeat(101), "Plain Text", None);
    set(&mut a, "vintage_enabled", json!(true));
    set(&mut a, "vintage_start_in_command_mode", json!(true));
    key(&mut a, "escape");
    caret(&mut a, 0);
    reset_history(&mut a);
    for _ in 0..3 {
        key(&mut a, "j");
    }
    assert_eq!(row(&a), 3);
    assert!(a.workspace.jump_history.borrow().back.is_empty());
    key(&mut a, "shift+g");
    assert!(row(&a) >= 100);
    assert_eq!(a.workspace.jump_history.borrow().back.len(), 1);
    back(&mut a);
    assert_eq!(row(&a), 3);
    key(&mut a, "shift+g");
    key(&mut a, "g");
    key(&mut a, "g");
    assert_eq!(row(&a), 0);
    back(&mut a);
    assert!(row(&a) >= 100);
}
#[test]
fn r19_history_survives_change_log_eviction_and_editing_sessions() {
    let mut a = app(&"abcdefghijklmnop\n".repeat(40), "Plain Text", None);
    caret(&mut a, 0);
    reset_history(&mut a);
    goto(&mut a, 15);
    goto(&mut a, 30);
    for _ in 0..10_050 {
        input(&mut a, "x");
    }
    assert_eq!(a.workspace.jump_history.borrow().back.len(), 2);
    back(&mut a);
    assert_eq!(row(&a), 14);
    back(&mut a);
    assert_eq!(row(&a), 0);

    reset_history(&mut a);
    goto(&mut a, 25);
    for _ in 0..5 {
        input(&mut a, "x");
        key(&mut a, "right");
    }
    assert_eq!(a.workspace.jump_history.borrow().back.len(), 1);
    back(&mut a);
    assert_eq!(row(&a), 0);
}
#[test]
fn r19_closed_file_reopens_at_saved_line_and_column() {
    let files = tempfile::tempdir().unwrap();
    let first = files.path().canonicalize().unwrap().join("first.txt");
    let second = files.path().canonicalize().unwrap().join("second.txt");
    std::fs::write(&first, "first line\nαsecond line\nthird line\n").unwrap();
    std::fs::write(&second, "elsewhere\n").unwrap();
    let mut a = app("", "Plain Text", None);
    open(&mut a, &first);
    caret(&mut a, 16);
    reset_history(&mut a);
    open(&mut a, &second);
    let at = a
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.as_ref() == Some(&first))
        .unwrap();
    a.workspace.close(at).unwrap();
    back(&mut a);
    drive_until(&mut a, |a| {
        a.workspace.tabs[a.workspace.active].loading.is_none()
    });
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&first)
    );
    assert_eq!(a.workspace.editor().caret, 16);
    forward(&mut a);
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&second)
    );
}
#[test]
fn r19_broken_theme_is_nonmodal_and_cannot_commit() {
    let dir = profile();
    std::fs::write(
        dir.path().join("Packages/Fixture/Broken.sublime-theme"),
        "{ invalid",
    )
    .unwrap();
    let mut a = app("fixture", "Rust", Some(dir.path()));
    a.workspace.messages.clear();
    let original = a.workspace.editor().settings.effective();
    let bytes = std::fs::read(
        dir.path()
            .join("Packages/User/Preferences.sublime-settings"),
    )
    .unwrap();
    named(&mut a, "select_theme", json!({}));
    choose(&mut a, "Broken", "theme");
    assert!(a.panels.message.contains("Broken"));
    assert!(a.workspace.messages.is_empty());
    let before = a.workspace.editor().settings.effective();
    key(&mut a, "enter");
    assert_eq!(a.panels.overlay, Some(Overlay::Appearance));
    assert_eq!(a.workspace.editor().settings.effective(), before);
    assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 0);
    assert_eq!(
        std::fs::read(
            dir.path()
                .join("Packages/User/Preferences.sublime-settings")
        )
        .unwrap(),
        bytes
    );
    key(&mut a, "escape");
    assert_eq!(a.workspace.editor().settings.effective(), original);
    assert!(a.workspace.messages.is_empty());
}
#[test]
fn r19_commit_preserves_view_and_syntax_overrides() {
    let dir = profile();
    std::fs::write(
        dir.path().join("Packages/User/Rust.sublime-settings"),
        r#"{"color_scheme":"Velocity Light.sublime-color-scheme"}"#,
    )
    .unwrap();
    let mut a = app("fixture", "Rust", Some(dir.path()));
    let overrides = a.workspace.editor().settings.layer(Layer::View).clone();
    named(&mut a, "select_color_scheme", json!({}));
    choose(&mut a, "Amber", "color_scheme");
    assert_eq!(
        a.workspace.editor().settings.string("color_scheme", ""),
        "Velocity Light.sublime-color-scheme"
    );
    let preview = scheme(&a);
    key(&mut a, "enter");
    drive_until(&mut a, |a| {
        a.workspace.appearance_writes.load(Ordering::Relaxed) == 1
            && a.panels.message.contains("overrides")
    });
    assert!(a.panels.message.contains("overrides"));
    assert_eq!(
        scheme(&a).globals.color("background"),
        preview.globals.color("background")
    );
    assert_eq!(
        a.workspace.editor().settings.string("color_scheme", ""),
        "Velocity Light.sublime-color-scheme"
    );
    assert_eq!(a.workspace.editor().settings.layer(Layer::View), &overrides);
}
#[test]
fn r19_variants_preview_the_committed_system_mode() {
    for dark in [false, true] {
        let dir = profile();
        let mut a = app("fixture", "Rust", Some(dir.path()));
        a.workspace.set_appearance(dark).unwrap();
        for (caption, key_name, active) in [
            (
                "Dark: Amber (applies in Dark mode)",
                "dark_color_scheme",
                dark,
            ),
            (
                "Light: Amber (applies in Light mode)",
                "light_color_scheme",
                !dark,
            ),
        ] {
            let before = scheme(&a);
            named(&mut a, "select_color_scheme", json!({}));
            choose(&mut a, caption, key_name);
            assert_eq!(!Arc::ptr_eq(&scheme(&a), &before), active);
            let preview = scheme(&a);
            key(&mut a, "enter");
            a.workspace.wait_resources();
            assert_eq!(
                scheme(&a).globals.color("background"),
                preview.globals.color("background")
            );
            assert_eq!(
                a.workspace.editor().settings.string("color_scheme", ""),
                "auto"
            );
            named(&mut a, "select_color_scheme", json!({}));
            choose(
                &mut a,
                "Automatic (follow system appearance)",
                "color_scheme",
            );
            key(&mut a, "escape");
        }
    }
}
#[test]
fn r19_malformed_preferences_roll_back_once() {
    let dir = profile();
    let mut a = app("fixture", "Rust", Some(dir.path()));
    let original = a.workspace.editor().settings.effective();
    let original_scheme = scheme(&a);
    let path = dir
        .path()
        .join("Packages/User/Preferences.sublime-settings");
    named(&mut a, "select_color_scheme", json!({}));
    choose(&mut a, "Amber", "color_scheme");
    std::fs::write(&path, "{ broken user json").unwrap();
    a.workspace.messages.clear();
    key(&mut a, "enter");
    drive_until(&mut a, |a| {
        a.panels.message.starts_with("could not save Preferences:")
    });
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{ broken user json"
    );
    assert_eq!(a.workspace.editor().settings.effective(), original);
    assert!(Arc::ptr_eq(&scheme(&a), &original_scheme));
    assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 0);
    assert!(
        a.workspace.messages.is_empty(),
        "{:?}",
        a.workspace.messages
    );
    assert!(
        a.workspace.appearance_notice.is_none(),
        "one status delivery"
    );
}
#[test]
fn r19_scheme_rows_deduplicate_user_overrides() {
    let dir = profile();
    std::fs::write(
        dir.path().join("Packages/User/Amber.sublime-color-scheme"),
        r##"{"globals":{"background":"#332200"}}"##,
    )
    .unwrap();
    let mut a = app("fixture", "Rust", Some(dir.path()));
    named(&mut a, "select_color_scheme", json!({}));
    for caption in [
        "Amber",
        "Dark: Amber (applies in Dark mode)",
        "Light: Amber (applies in Light mode)",
    ] {
        assert_eq!(
            a.panels
                .quick
                .items
                .iter()
                .filter(|r| r.caption == caption)
                .count(),
            1
        );
    }
    choose(&mut a, "Amber", "color_scheme");
    assert_eq!(scheme(&a).globals.color("background").unwrap().r, 0x33);
}
#[test]
fn r19_picker_dismisses_on_tab_commands_and_window_focus_loss() {
    let dir = profile();
    let mut a = app("fixture", "Rust", Some(dir.path()));
    named(&mut a, "new_file", json!({}));
    for action in [
        "ctrl+tab",
        "next_view",
        "select_by_index",
        "focus_loss",
        "activation",
    ] {
        let source = a.workspace.active;
        let before_scheme = scheme(&a);
        let before: Vec<_> = a
            .workspace
            .tabs
            .iter()
            .map(|t| t.editor.settings.effective())
            .collect();
        named(&mut a, "select_color_scheme", json!({}));
        choose(&mut a, "Amber", "color_scheme");
        match action {
            "ctrl+tab" | "next_view" | "select_by_index" => {
                if action == "ctrl+tab" {
                    key(&mut a, "ctrl+tab");
                } else {
                    named(&mut a, action, json!({"index":1 - source}));
                }
                // Native window commands are unavailable headlessly. Exercise the
                // same actual sheet selection after checking command dismissal.
                a.workspace.select(1 - source);
                assert_ne!(a.workspace.active, source);
            }
            "activation" => {
                a.workspace.select(1 - source);
                let viewport = a.viewport();
                a.panels.poll(&mut a.workspace, viewport);
            }
            _ => a.set_window_focused(false),
        }
        assert!(a.panels.overlay.is_none(), "{action}");
        for (tab, settings) in a.workspace.tabs.iter().zip(before) {
            assert_eq!(tab.editor.settings.effective(), settings);
        }
        a.set_window_focused(true);
        a.workspace.select(source);
        assert!(
            Arc::ptr_eq(&scheme(&a), &before_scheme),
            "hidden preview must refresh when activated"
        );
    }
}
#[test]
fn r19_preview_touches_only_visible_views_and_activation_is_lazy() {
    let dir = profile();
    let mut a = app("fn fixture() {}", "Rust", Some(dir.path()));
    for _ in 0..30 {
        named(&mut a, "new_file", json!({}));
    }
    let active = a.workspace.active;
    let versions: Vec<_> = a
        .workspace
        .tabs
        .iter()
        .map(|t| t.editor.settings.revision())
        .collect();
    let schemes: Vec<_> = a
        .workspace
        .tabs
        .iter()
        .map(|t| {
            t.editor
                .language
                .borrow()
                .as_ref()
                .map(|l| l.scheme.clone())
        })
        .collect();
    let mut timings = Vec::new();
    named(&mut a, "select_color_scheme", json!({}));
    choose(&mut a, "Amber", "color_scheme");
    for _ in 0..10 {
        let begin = Instant::now();
        key(&mut a, "down");
        timings.push(begin.elapsed());
        key(&mut a, "up");
    }
    for (index, tab) in a
        .workspace
        .tabs
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != active)
    {
        assert_eq!(tab.editor.settings.revision(), versions[index]);
        if let Some(before) = &schemes[index] {
            assert!(Arc::ptr_eq(
                before,
                &tab.editor.language.borrow().as_ref().unwrap().scheme
            ));
        }
    }
    timings.sort();
    eprintln!(
        "31-tab visible preview median {:.3}ms max {:.3}ms",
        timings[5].as_secs_f64() * 1000.,
        timings[9].as_secs_f64() * 1000.
    );
    choose(&mut a, "Amber", "color_scheme");
    key(&mut a, "enter");
    drive_until(&mut a, |a| {
        a.workspace.settings.string("color_scheme", "") == "Amber.sublime-color-scheme"
    });
    // Activate before a resource reload: the background adapter refreshes on demand.
    a.workspace.select(0);
    a.workspace.editor_mut().sync();
    assert_eq!(
        a.workspace.editor().settings.string("color_scheme", ""),
        "Amber.sublime-color-scheme"
    );
}
#[test]
fn r19_randomized_navigation_sweep() {
    let mut a = app(&"αbcdefghijklmnop\n".repeat(80), "Plain Text", None);
    caret(&mut a, 0);
    reset_history(&mut a);
    let mut seed = 0xa871_bed9_7384_u64;
    let mut switched = 0;
    for step in 0..3000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        match seed % 10 {
            0 => goto(&mut a, (seed as usize % 80) + 1),
            1 => back(&mut a),
            2 => forward(&mut a),
            3 => input(&mut a, "x"),
            4 => key(&mut a, "left"),
            5 => key(&mut a, "right"),
            6 => key(&mut a, "up"),
            7 => key(&mut a, "down"),
            8 => named(&mut a, "left_delete", json!({})),
            _ => {
                if step % 100 == 0 {
                    named(&mut a, "new_file", json!({}));
                    input(&mut a, "fixture\n");
                } else {
                    let next = (a.workspace.active + 1) % a.workspace.tabs.len();
                    switched += usize::from(next != a.workspace.active);
                    a.workspace.select(next);
                    a.sync_active_view();
                }
            }
        }
        let e = a.workspace.editor();
        assert!(
            e.caret <= e.text.len_bytes() && e.text.is_char_boundary(e.caret),
            "step {step}"
        );
        assert!(a.workspace.jump_history.borrow().back.len() <= 128);
        assert!(a.workspace.jump_history.borrow().forward.len() <= 128);
        assert!(
            a.workspace
                .workbench
                .validate(&a.workspace.tabs.iter().map(|t| t.sheet).collect())
                .is_ok()
        );
    }
    assert!(switched > 0, "the sweep must switch real sheets");
    eprintln!(
        "3,000 navigation steps, {switched} real sheet switches; bounded stacks and Unicode/workbench invariants passed"
    );
}

#[test]
fn r19_appearance_commit_persists_exact_pairs_after_inflight_reload() {
    let dir = profile();
    let mut a = app("fixture", "Rust", Some(dir.path()));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let blocker = crate::release::enqueue(move || {
        release_rx
            .recv_timeout(Duration::from_secs(20))
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .unwrap();
    named(&mut a, "select_color_scheme", json!({}));
    choose(
        &mut a,
        "Dark: Amber (applies in Dark mode)",
        "dark_color_scheme",
    );
    key(&mut a, "enter");
    let revision = a.workspace.resource_revision;
    a.workspace.request_resource_reload();
    drive_until(&mut a, |a| a.workspace.resource_revision != revision);
    assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 0);
    // The reload finished before the queued write: the committed value must survive it.
    assert_eq!(
        a.workspace.editor().settings.get("dark_color_scheme"),
        Some(&json!("Amber.sublime-color-scheme"))
    );
    release_tx.send(()).unwrap();
    blocker
        .recv_timeout(crate::test_timing::HANG_GUARD)
        .unwrap()
        .unwrap();
    a.workspace.wait_resources();
    let saved = vel_config::json::Document::parse(
        &std::fs::read_to_string(
            dir.path()
                .join("Packages/User/Preferences.sublime-settings"),
        )
        .unwrap(),
    )
    .unwrap()
    .value;
    assert_eq!(saved["color_scheme"], "auto");
    assert_eq!(saved["dark_color_scheme"], "Amber.sublime-color-scheme");
    assert_eq!(
        a.workspace.editor().settings.get("dark_color_scheme"),
        saved.get("dark_color_scheme")
    );
    assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 1);
    let path = dir
        .path()
        .join("Packages/User/Preferences.sublime-settings");
    let source = std::fs::read_to_string(&path).unwrap();
    let edited = vel_config::json::Document::parse(&source)
        .unwrap()
        .set(
            "dark_color_scheme",
            &json!("Velocity Light.sublime-color-scheme"),
        )
        .unwrap();
    std::fs::write(&path, edited).unwrap();
    a.workspace.request_resource_reload();
    a.workspace.wait_resources();
    assert_eq!(
        a.workspace
            .editor()
            .settings
            .string("dark_color_scheme", ""),
        "Velocity Light.sublime-color-scheme"
    );
}
