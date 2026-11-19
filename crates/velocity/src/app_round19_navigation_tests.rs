//! Follow-up verification through the real input, request, tab and command routes.
use super::*;

#[test]
fn goto_line_with_argument_jumps_but_no_argument_opens_goto() {
    let mut a = app("a\nβ\nthird\nfourth\n", "Plain Text", None);
    named(&mut a, "goto_line", json!({"line":3}));
    assert_eq!(a.workspace.editor().selection(), 5..5);
    assert!(a.panels.overlay.is_none());
    key(&mut a, "ctrl+g");
    assert_eq!(a.panels.overlay, Some(crate::panels::Overlay::Goto));
    assert_eq!(a.panels.quick_input.value(), ":");
}

#[test]
fn split_into_lines_splits_a_single_line_selection_into_words() {
    let mut a = app("alpha beta gamma\n", "Plain Text", None);
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(16, 0)]));
    key(&mut a, "super+shift+l");
    assert_eq!(
        a.workspace.editor().regions(),
        [Region::new(5, 0), Region::new(10, 6), Region::new(16, 11)]
    );
}

#[test]
fn r19_repeated_editing_commands_preserve_goto_origin() {
    for (command, count) in [
        ("pagedown", 5),
        ("super+d", 4),
        ("super+d", 140),
        ("ctrl+shift+down", 3),
        ("super+l", 3),
        ("paste", 4),
        ("swap_line_down", 3),
        ("alt+backspace", 3),
        ("toggle_comment", 3),
        ("ctrl+shift+k", 3),
        ("undo", 4),
        ("limits", 3),
        ("super+down", 3),
        ("home", 3),
        ("end", 3),
    ] {
        let mut a = app(&"needle word line\n".repeat(300), "Rust", None);
        input(&mut a, "undo fixture");
        caret(&mut a, 0);
        reset_history(&mut a);
        goto(&mut a, 40);
        a.content = a.panels.layout(a.window_size);
        a.view.size = Size {
            width: a.content.width,
            height: a.content.height,
        };
        a.view.scene(a.workspace.editor());
        let before = a
            .workspace
            .editor()
            .buffer
            .view(a.workspace.editor().id)
            .sel()
            .clone();
        let revision = a.workspace.editor().buffer.change_id();
        a.workspace.editor_mut().clipboard = "pasted".into();
        let mut exercised = false;
        for _ in 0..count {
            match command {
                "paste" | "swap_line_down" | "toggle_comment" | "undo" => {
                    named(&mut a, command, json!({}));
                }
                "limits" => {
                    key(&mut a, "super+right");
                    exercised |= a
                        .workspace
                        .editor()
                        .buffer
                        .view(a.workspace.editor().id)
                        .sel()
                        != &before;
                    key(&mut a, "super+left");
                }
                _ => key(&mut a, command),
            }
            exercised |= a
                .workspace
                .editor()
                .buffer
                .view(a.workspace.editor().id)
                .sel()
                != &before
                || a.workspace.editor().buffer.change_id() != revision;
        }
        assert!(exercised, "{command} must exercise its document path");
        assert_eq!(a.workspace.jump_history.borrow().back.len(), 1, "{command}");
        back(&mut a);
        assert_eq!(
            a.workspace.editor().caret,
            0,
            "{command} retains the pre-Goto origin"
        );
    }
}

#[test]
fn r19_find_preview_is_one_session_and_cancel_is_silent() {
    for (panel, accept) in [("find", true), ("find", false), ("incremental_find", false)] {
        let source = (0..100)
            .map(|i| format!("needle {i}\n"))
            .collect::<String>();
        let mut a = app(&source, "Plain Text", None);
        caret(&mut a, 0);
        reset_history(&mut a);
        goto(&mut a, 40);
        named(&mut a, "show_panel", json!({"panel":panel, "text":""}));
        for ch in "needle 27".chars() {
            input(&mut a, &ch.to_string());
            drive_until(&mut a, |a| !a.panels.busy());
            assert_eq!(
                a.workspace.jump_history.borrow().back.len(),
                1,
                "{panel}: {ch}"
            );
        }
        assert_eq!(row(&a), 27, "the preview actually moved the caret");
        key(&mut a, if accept { "enter" } else { "escape" });
        if panel == "incremental_find" {
            assert_eq!(row(&a), 39);
            assert_eq!(a.workspace.jump_history.borrow().back.len(), 1);
        } else {
            assert_eq!(a.workspace.jump_history.borrow().back.len(), 2);
            let accepted = a.workspace.editor().selection();
            back(&mut a);
            assert_eq!(row(&a), 39);
            forward(&mut a);
            assert_eq!(a.workspace.editor().selection(), accepted);
            back(&mut a);
        }
        back(&mut a);
        assert_eq!(row(&a), 0);
    }
}

#[test]
fn r19_counted_vintage_nonjump_motions_never_record() {
    let mut a = app(&"one two three\n".repeat(101), "Plain Text", None);
    set(&mut a, "vintage_enabled", json!(true));
    set(&mut a, "vintage_start_in_command_mode", json!(true));
    key(&mut a, "escape");
    caret(&mut a, 0);
    reset_history(&mut a);
    for motion in ["j", "k", "j"] {
        key(&mut a, "2");
        key(&mut a, "0");
        key(&mut a, motion);
        assert!(
            a.workspace.jump_history.borrow().back.is_empty(),
            "20{motion}"
        );
    }
    assert_eq!(row(&a), 20);
    key(&mut a, "shift+g");
    assert!(row(&a) >= 100);
    back(&mut a);
    assert_eq!(
        row(&a),
        20,
        "explicit vi jump still remembers the counted-motion destination"
    );
}

#[test]
fn r19_nearby_bookmarks_and_marks_are_explicit_jumps() {
    let mut a = app(&"words\n".repeat(30), "Plain Text", None);
    caret(&mut a, 0);
    named(&mut a, "set_mark", json!({}));
    for line in [3, 5] {
        let at = a.workspace.editor().text.line_to_byte(line);
        caret(&mut a, at);
        named(&mut a, "toggle_bookmark", json!({}));
    }
    let at = a.workspace.editor().text.line_to_byte(8);
    caret(&mut a, at);
    reset_history(&mut a);
    for expected in [3, 5] {
        named(&mut a, "next_bookmark", json!({}));
        assert_eq!(row(&a), expected);
    }
    back(&mut a);
    assert_eq!(row(&a), 3);
    back(&mut a);
    assert_eq!(row(&a), 8);
    for command in ["swap_with_mark", "select_to_mark"] {
        // Swap exchanges the saved mark; restore it through the command as a user would.
        caret(&mut a, 0);
        named(&mut a, "set_mark", json!({}));
        caret(&mut a, at);
        reset_history(&mut a);
        named(&mut a, command, json!({}));
        assert_eq!(row(&a), 0);
        back(&mut a);
        assert_eq!(row(&a), 8, "{command} records even within ten lines");
    }
}

#[test]
fn r19_find_all_accepts_session_without_closing_panel() {
    let mut a = app(&"needle\n".repeat(80), "Plain Text", None);
    caret(&mut a, 0);
    reset_history(&mut a);
    goto(&mut a, 40);
    named(&mut a, "show_panel", json!({"panel":"find", "text":""}));
    input(&mut a, "needle");
    drive_until(&mut a, |a| !a.panels.busy());
    assert_eq!(a.workspace.jump_history.borrow().back.len(), 1);
    named(&mut a, "find_all", json!({"_close_panel":false}));
    assert_eq!(a.workspace.editor().regions().len(), 80);
    assert_eq!(a.workspace.jump_history.borrow().back.len(), 2);
    named(&mut a, "hide_panel", json!({}));
    back(&mut a);
    assert_eq!(row(&a), 39);
    back(&mut a);
    assert_eq!(row(&a), 0);
}

#[cfg(unix)]
#[test]
fn r19_unfocused_cli_finder_and_open_preserve_origin() {
    for route in ["cli", "finder", "open"] {
        let files = tempfile::tempdir().unwrap();
        let root = files.path().canonicalize().unwrap();
        let one = root.join("one.txt");
        let two = root.join("two.txt");
        std::fs::write(&one, "one\n".repeat(100)).unwrap();
        std::fs::write(&two, "two\n".repeat(100)).unwrap();
        let mut a = app("", "Plain Text", None);
        open(&mut a, &one);
        caret(&mut a, 0);
        reset_history(&mut a);
        goto(&mut a, 30);
        a.set_window_focused(false);
        let location = crate::cli::FileLocation {
            path: two.clone(),
            line: 50,
            column: 1,
        };
        if route == "open" {
            a.workspace.open(location).unwrap();
        } else {
            let mut request = crate::instance::Request::new(&Options::default(), None).unwrap();
            request.files = vec![location];
            request.native_open = route == "finder";
            a.accept_request(crate::instance::Incoming::local(request))
                .unwrap();
        }
        a.sync_active_view();
        drive_until(&mut a, |a| {
            a.workspace.tabs.iter().all(|t| t.loading.is_none())
        });
        a.set_window_focused(true);
        assert_eq!(row(&a), if route == "finder" { 0 } else { 49 }, "{route}");
        back(&mut a);
        assert_eq!(
            a.workspace.tabs[a.workspace.active].path.as_ref(),
            Some(&one),
            "{route}"
        );
        assert_eq!(
            row(&a),
            29,
            "{route} records the origin before focus returns"
        );
        forward(&mut a);
        assert_eq!(
            a.workspace.tabs[a.workspace.active].path.as_ref(),
            Some(&two),
            "{route}"
        );
    }
}

#[cfg(unix)]
#[test]
fn r19_existing_tab_open_requests_preserve_unfocused_origin() {
    for route in ["finder", "native_accept", "cli", "open_file"] {
        let files = tempfile::tempdir().unwrap();
        let root = files.path().canonicalize().unwrap();
        let one = root.join("one.txt");
        let two = root.join("two.txt");
        for path in [&one, &two] {
            std::fs::write(path, "line\n".repeat(100)).unwrap();
        }
        let mut a = app("", "Plain Text", None);
        open(&mut a, &one);
        open(&mut a, &two);
        goto(&mut a, 17);
        let index = a
            .workspace
            .tabs
            .iter()
            .position(|tab| tab.path.as_ref() == Some(&one))
            .unwrap();
        a.workspace.select(index);
        caret(&mut a, 0);
        reset_history(&mut a);
        goto(&mut a, 30);
        a.set_window_focused(false);
        match route {
            "finder" => crate::open_requests::open_paths(
                &mut a.workspace,
                &mut a.panels,
                std::slice::from_ref(&two),
            )
            .unwrap(),
            "open_file" => named(
                &mut a,
                "open_file",
                json!({"file":format!("{}:50:1", two.display())}),
            ),
            _ => {
                let mut request = crate::instance::Request::new(&Options::default(), None).unwrap();
                request.files = vec![crate::cli::FileLocation {
                    path: two.clone(),
                    line: 50,
                    column: 1,
                }];
                request.native_open = route == "native_accept";
                a.accept_request(crate::instance::Incoming::local(request))
                    .unwrap();
            }
        }
        a.sync_active_view();
        a.set_window_focused(true);
        assert_eq!(
            a.workspace.tabs[a.workspace.active].path.as_ref(),
            Some(&two),
            "{route}"
        );
        assert_eq!(
            row(&a),
            if matches!(route, "finder" | "native_accept") {
                16
            } else {
                49
            },
            "existing Finder opens retain their destination: {route}"
        );
        back(&mut a);
        assert_eq!(
            a.workspace.tabs[a.workspace.active].path.as_ref(),
            Some(&one),
            "{route}"
        );
        assert_eq!(row(&a), 29, "{route}");
        forward(&mut a);
        assert_eq!(
            a.workspace.tabs[a.workspace.active].path.as_ref(),
            Some(&two),
            "{route}"
        );
    }
}

#[test]
fn r19_bounded_multicaret_back_forward_maps_all_regions() {
    for count in [5, 64, 65] {
        let mut a = app(&"abcdefghij\n".repeat(100), "Plain Text", None);
        let origin =
            Selection::from_regions((0..count).map(|i| Region::new(i * 11 + 1, i * 11 + 3)));
        a.workspace.editor_mut().set_selection(origin.clone());
        reset_history(&mut a);
        goto(&mut a, 95);
        let newest =
            Selection::from_regions((0..5).rev().map(|i| Region::caret((90 + i) * 11 + 2)));
        a.workspace.editor_mut().set_selection(newest.clone());
        // The interactive selection fixture is itself a distinct command; return
        // through it to the Goto origin, then forward to the five-caret destination.
        back(&mut a);
        back(&mut a);
        let expected = if count <= 64 {
            origin.clone()
        } else {
            Selection::from_regions(origin.primary())
        };
        assert_eq!(
            a.workspace
                .editor()
                .buffer
                .view(a.workspace.editor().id)
                .sel(),
            &expected,
            "{count} regions"
        );
        forward(&mut a);
        forward(&mut a);
        assert_eq!(
            a.workspace
                .editor()
                .buffer
                .view(a.workspace.editor().id)
                .sel(),
            &newest
        );

        // Rebase retained multi-selections before their maps are evicted.
        a.workspace
            .editor_mut()
            .buffer
            .set_history_limits(vel_buffer::HistoryLimits {
                changes: 160,
                ..Default::default()
            });
        for _ in 0..300 {
            input(&mut a, "z");
        }
        back(&mut a);
        back(&mut a);
        assert_eq!(
            a.workspace
                .editor()
                .buffer
                .view(a.workspace.editor().id)
                .sel(),
            &expected,
            "rebased {count} regions"
        );
    }
}

#[test]
fn r19_typing_arguments_merge_and_preserve_forward_history() {
    let mut a = app(&"line\n".repeat(100), "Plain Text", None);
    caret(&mut a, 0);
    reset_history(&mut a);
    goto(&mut a, 30);
    goto(&mut a, 70);
    back(&mut a);
    let back_len = a.workspace.jump_history.borrow().back.len();
    let forward_len = a.workspace.jump_history.borrow().forward.len();
    assert_eq!(forward_len, 1);
    for line in 0..60 {
        input(&mut a, &format!("typed {line}\n"));
        named(
            &mut a,
            "insert_snippet",
            json!({"contents":format!("pair {line} " )}),
        );
        named(&mut a, "left_delete", json!({}));
        named(&mut a, "right_delete", json!({}));
        // A committed IME insert uses the same document input route.
        a.workspace.editor_mut().text_input(TextInput::Marked {
            text: "字".into(),
            selected: vel_platform::Utf16Range {
                location: 1,
                length: 0,
            },
            replacement: None,
        });
        input(&mut a, "字");
        assert_eq!(
            a.workspace.jump_history.borrow().back.len(),
            back_len,
            "typing line {line}"
        );
        assert_eq!(
            a.workspace.jump_history.borrow().forward.len(),
            forward_len,
            "typing retains Forward at line {line}"
        );
    }
    forward(&mut a);
    assert!(row(&a) >= 69, "mapped Forward destination stays reachable");
    back(&mut a);
    goto(&mut a, 5);
    assert!(
        a.workspace.jump_history.borrow().forward.is_empty(),
        "an explicit jump branches history"
    );
}

#[test]
fn r19_editing_distance_is_measured_from_session_start() {
    let mut a = app(&"line\n".repeat(100), "Plain Text", None);
    caret(&mut a, 0);
    reset_history(&mut a);
    goto(&mut a, 20);
    goto(&mut a, 80);
    back(&mut a);
    let forward_len = a.workspace.jump_history.borrow().forward.len();
    for i in 0..10 {
        input(&mut a, &i.to_string());
        key(&mut a, "down");
        assert_eq!(a.workspace.jump_history.borrow().back.len(), 1);
        assert_eq!(a.workspace.jump_history.borrow().forward.len(), forward_len);
    }
    input(&mut a, "x");
    key(&mut a, "down");
    assert_eq!(a.workspace.jump_history.borrow().back.len(), 2);
    assert!(a.workspace.jump_history.borrow().forward.is_empty());
    back(&mut a);
    assert_eq!(
        row(&a),
        19,
        "distance split remembers the editing session's start"
    );
}

#[test]
fn r19_vintage_ex_line_number_is_a_jump() {
    let mut a = app(&"one two three\n".repeat(100), "Plain Text", None);
    set(&mut a, "vintage_enabled", json!(true));
    set(&mut a, "vintage_start_in_command_mode", json!(true));
    key(&mut a, "escape");
    caret(&mut a, 0);
    reset_history(&mut a);
    for line in [30, 70] {
        key(&mut a, "shift+;");
        // Native vi uses the focused command input panel, including Enter submission.
        input(&mut a, &line.to_string());
        key(&mut a, "enter");
        assert_eq!(row(&a), line - 1);
    }
    back(&mut a);
    assert_eq!(row(&a), 29);
    back(&mut a);
    assert_eq!(row(&a), 0);
    forward(&mut a);
    assert_eq!(row(&a), 29);
    forward(&mut a);
    assert_eq!(row(&a), 69);
}

#[test]
fn r19_multiclick_is_one_gesture() {
    let mut a = app(&"one two three\n".repeat(30), "Plain Text", None);
    caret(&mut a, 0);
    reset_history(&mut a);
    a.content = a.panels.layout(a.window_size);
    a.view.size = Size {
        width: a.content.width,
        height: a.content.height,
    };
    a.view.scene(a.workspace.editor());
    let at = a.workspace.editor().text.line_to_byte(8) + 1;
    let rect = a.view.rect_at(a.workspace.editor(), at);
    let point = Point {
        x: a.content.x + rect.x + 1.,
        y: a.content.y + rect.y + a.view.line_height / 2.,
    };
    for count in 1..=3 {
        for kind in [MouseKind::Down, MouseKind::Up] {
            a.handle_mouse_event(
                None,
                MouseEvent {
                    button: 0,
                    kind,
                    position: Point {
                        x: point.x + (count - 1) as f64,
                        y: point.y + (count - 1) as f64 * 0.25,
                    },
                    click_count: count,
                    modifiers: Modifiers(0),
                    timestamp: 0.,
                },
            );
        }
        assert_eq!(
            a.workspace.jump_history.borrow().back.len(),
            1,
            "click count {count}"
        );
    }
    let accepted = a
        .workspace
        .editor()
        .buffer
        .view(a.workspace.editor().id)
        .sel()
        .clone();
    back(&mut a);
    assert_eq!(a.workspace.editor().caret, 0);
    forward(&mut a);
    assert_eq!(
        a.workspace
            .editor()
            .buffer
            .view(a.workspace.editor().id)
            .sel(),
        &accepted
    );
}

#[test]
fn r19_deleted_closed_history_file_is_skipped() {
    let files = tempfile::tempdir().unwrap();
    let root = files.path().canonicalize().unwrap();
    let one = root.join("one.txt");
    let two = root.join("two.txt");
    std::fs::write(&one, "one\n".repeat(60)).unwrap();
    std::fs::write(&two, "two\n".repeat(60)).unwrap();
    let mut a = app("", "Plain Text", None);
    open(&mut a, &one);
    goto(&mut a, 30);
    reset_history(&mut a);
    open(&mut a, &two);
    let index = a
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.as_ref() == Some(&one))
        .unwrap();
    a.workspace.close(index).unwrap();
    std::fs::remove_file(&one).unwrap();
    back(&mut a);
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&two)
    );
    assert!(
        a.workspace
            .tabs
            .iter()
            .all(|t| t.path.as_ref() != Some(&one))
    );
    assert_eq!(
        a.panels.message,
        format!("{} no longer exists", one.display())
    );
    assert!(a.workspace.jump_history.borrow().back.is_empty());
    assert!(!one.exists());
}

// Hold the serial writer so the test observes Enter before any file write can finish.
// Drop releases it even after an assertion failure, avoiding stalls in other tests.
struct BlockedWrite(Option<std::sync::mpsc::Sender<()>>);
impl BlockedWrite {
    fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        crate::release::enqueue(move || {
            rx.recv_timeout(Duration::from_secs(30))
                .map_err(|e| e.to_string())
        })
        .unwrap();
        Self(Some(tx))
    }
}
impl Drop for BlockedWrite {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

#[cfg(unix)]
#[test]
fn r19_failed_appearance_write_revokes_reload_override_before_error_frame() {
    use std::os::unix::fs::PermissionsExt;
    struct WritableAgain(std::path::PathBuf, std::fs::Permissions);
    impl Drop for WritableAgain {
        fn drop(&mut self) {
            std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
        }
    }
    for root in ["color_scheme", "theme", "dark_color_scheme"] {
        let dir = profile();
        let mut a = app("fixture", "Rust", Some(dir.path()));
        let user = dir.path().join("Packages/User");
        let file = user.join("Preferences.sublime-settings");
        // A real zoom writes a separate key before the barrier, but its tracker
        // completion is still pending when the appearance commit is registered.
        named(&mut a, "increase_font_size", json!({}));
        let font_size = a.workspace.editor().settings.get("font_size").cloned();
        let blocked = BlockedWrite::new();
        let original = a.workspace.editor().settings.get(root).cloned();
        let original_root = a.workspace.editor().settings.get("color_scheme").cloned();
        let background = scheme(&a).globals.color("background");
        named(
            &mut a,
            if root == "theme" {
                "select_theme"
            } else {
                "select_color_scheme"
            },
            json!({}),
        );
        choose(
            &mut a,
            if root.starts_with("dark_") {
                "Dark: Amber (applies in Dark mode)"
            } else {
                "Amber"
            },
            root,
        );
        key(&mut a, "enter");
        // Let the real zoom finish before denying atomic writes. A second barrier
        // on the queue is unnecessary: reading the file observes that first write.
        let until = Instant::now() + Duration::from_secs(20);
        while !std::fs::read_to_string(&file)
            .unwrap()
            .contains("font_size")
            || vel_config::json::Document::parse(&std::fs::read_to_string(&file).unwrap())
                .unwrap()
                .value
                .get("font_size")
                != font_size.as_ref()
        {
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let _writable = WritableAgain(
            user.clone(),
            std::fs::metadata(&user).unwrap().permissions(),
        );
        std::fs::set_permissions(&user, std::fs::Permissions::from_mode(0o555)).unwrap();
        let revision = a.workspace.resource_revision;
        a.workspace.request_resource_reload();
        a.workspace.poll();
        assert_eq!(
            a.workspace.resource_revision, revision,
            "resource load starts while the writer is blocked"
        );
        drop(blocked);
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut errors = 0;
        loop {
            a.workspace.poll();
            if a.workspace
                .appearance_notice
                .as_ref()
                .is_some_and(|s| s.starts_with("could not save Preferences:"))
            {
                errors += 1;
            }
            let viewport = a.viewport();
            a.panels.poll(&mut a.workspace, viewport);
            if errors > 0 {
                assert_eq!(
                    a.workspace.editor().settings.get(root),
                    original.as_ref(),
                    "failed {root} must not reappear in the error frame"
                );
                assert_eq!(
                    a.workspace.editor().settings.get("color_scheme"),
                    original_root.as_ref()
                );
                assert_eq!(scheme(&a).globals.color("background"), background);
                assert_eq!(
                    a.workspace.editor().settings.get("font_size"),
                    font_size.as_ref(),
                    "zoom override survives revocation"
                );
            }
            assert!(
                a.workspace.messages.is_empty(),
                "{:?}",
                a.workspace.messages
            );
            if errors > 0 && !a.workspace.resources_busy() {
                break;
            }
            assert!(Instant::now() < deadline, "{}", a.panels.message);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert_eq!(errors, 1);
        assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 0);
        assert!(a.workspace.resource_revision != revision);
    }
}

#[test]
fn r19_commit_keeps_preview_visible_until_write_completes() {
    for root in ["color_scheme", "theme"] {
        let dir = profile();
        let mut a = app("fixture", "Rust", Some(dir.path()));
        let blocked = BlockedWrite::new();
        named(&mut a, &format!("select_{root}"), json!({}));
        choose(&mut a, "Amber", root);
        let preview = scheme(&a);
        let selected = a.workspace.editor().settings.get(root).cloned().unwrap();
        key(&mut a, "enter");
        assert_eq!(a.panels.overlay, None);
        assert_eq!(a.workspace.editor().settings.get(root), Some(&selected));
        assert_eq!(
            a.workspace.settings.layer(Layer::User).get(root),
            Some(&selected)
        );
        assert!(
            Arc::ptr_eq(&preview, &scheme(&a)),
            "Enter must not restore the old colors"
        );
        for _ in 0..10 {
            a.workspace.poll();
            let viewport = a.viewport();
            a.panels.poll(&mut a.workspace, viewport);
            assert!(Arc::ptr_eq(&preview, &scheme(&a)));
            assert_eq!(a.workspace.editor().settings.get(root), Some(&selected));
        }
        assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 0);
        drop(blocked);
        a.workspace.wait_resources();
        assert_eq!(a.workspace.appearance_writes.load(Ordering::Relaxed), 1);
        assert_eq!(a.workspace.editor().settings.get(root), Some(&selected));
    }
}

fn multicaret_tabs(entries: usize) -> App {
    let mut a = app(&"x\n".repeat(10_000), "Plain Text", None);
    let selection = Selection::from_regions((0..10_000).rev().map(|row| Region::caret(row * 2)));
    a.workspace.editor_mut().set_selection(selection);
    a.workspace.clone_file().unwrap();
    reset_history(&mut a);
    for _ in 0..entries {
        a.workspace.select(1 - a.workspace.active);
        a.sync_active_view();
    }
    assert_eq!(a.workspace.jump_history.borrow().back.len(), entries);
    for jump in &a.workspace.jump_history.borrow().back {
        assert_eq!(jump.selection.len(), 1);
        assert_eq!(jump.caret_count, 10_000);
    }
    a
}

#[test]
fn r19_multicaret_history_records_only_primary() {
    let mut a = multicaret_tabs(60);
    input(&mut a, "y");
    back(&mut a);
    assert_eq!(
        a.workspace
            .editor()
            .buffer
            .view(a.workspace.editor().id)
            .sel()
            .len(),
        1
    );
    assert_eq!(
        a.workspace.editor().caret,
        1,
        "primary maps through the edit in the shared buffer"
    );
}

#[test]
#[ignore = "isolated paired App timing: 10,000 carets, 60 records, 300 keys"]
fn r19_multicaret_history_300_key_stall_gate() {
    use crate::editor::navigation::timing;
    let mut a = multicaret_tabs(60);
    let mut baseline = multicaret_tabs(60);
    baseline.workspace.jump_history.borrow_mut().disabled = true;
    for app in [&mut a, &mut baseline] {
        // Trigger real retention pressure and eviction during this run.
        app.workspace
            .editor_mut()
            .buffer
            .set_history_limits(vel_buffer::HistoryLimits {
                changes: 160,
                ..Default::default()
            });
    }
    let type_key = |app: &mut App, text: &str| {
        // Native text input ends with this shared App state/reveal adapter. The
        // former gate omitted it and never synchronized the shared-buffer clone.
        input(app, text);
        app.changed_state(Instant::now(), true);
    };
    for _ in 0..10 {
        type_key(&mut a, "w");
        type_key(&mut baseline, "w");
    }
    timing::take();
    let mut samples = Vec::with_capacity(300);
    let mut base_samples = Vec::with_capacity(300);
    let mut hooks = Vec::with_capacity(300);
    for key in 0..300 {
        // Alternate order to reduce thermal/scheduling bias at the same document
        // length, rather than comparing a later key against an earlier median.
        for history in if key % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            let app = if history { &mut a } else { &mut baseline };
            timing::take();
            let begin = Instant::now();
            type_key(app, "y");
            let elapsed = begin.elapsed();
            let hook_time = timing::take();
            if history {
                samples.push(elapsed);
                hooks.push(hook_time);
            } else {
                base_samples.push(elapsed);
            }
        }
    }
    let history_worst = *hooks.iter().max().unwrap();
    samples.sort_unstable();
    base_samples.sort_unstable();
    let median = samples[samples.len() / 2];
    let base_median = base_samples[base_samples.len() / 2];
    let total: Duration = samples.iter().sum();
    let base_total: Duration = base_samples.iter().sum();
    eprintln!(
        "10,000 carets / 60 entries / 300 paired App keys: history hooks worst {:.3} ms; full-path median {:.3} ms / disabled {:.3} ms; totals {:.3} s / {:.3} s ({:+.3}%)",
        history_worst.as_secs_f64() * 1000.,
        median.as_secs_f64() * 1000.,
        base_median.as_secs_f64() * 1000.,
        total.as_secs_f64(),
        base_total.as_secs_f64(),
        (total.as_secs_f64() / base_total.as_secs_f64() - 1.) * 100.
    );
    assert!(
        history_worst <= Duration::from_millis(1),
        "history maintenance/observation must stay bounded on every key"
    );
    assert!(
        total.as_secs_f64() <= base_total.as_secs_f64() * 1.10,
        "paired full-App overhead exceeds 10%"
    );
    for recall in 0..60 {
        let departed = a.workspace.active;
        back(&mut a);
        assert_ne!(a.workspace.active, departed, "recall {recall}");
        assert_eq!(
            a.workspace
                .editor()
                .buffer
                .view(a.workspace.editor().id)
                .sel()
                .len(),
            1
        );
        assert_eq!(
            a.workspace.editor().caret,
            310,
            "every anchor survives eviction: {recall}"
        );
    }
    assert!(a.workspace.jump_history.borrow().back.is_empty());
    for _ in 0..60 {
        let departed = a.workspace.active;
        forward(&mut a);
        assert_ne!(a.workspace.active, departed);
        assert_eq!(a.workspace.editor().caret, 310);
    }
}
