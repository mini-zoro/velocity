//! Third-verifier repros through real App commands and frame/poll paths.
use super::*;
fn receiver_top(a: &mut App, receiver: usize) -> (usize, usize) {
    let sheet = a.workspace.tabs[receiver].sheet;
    let e = &a.workspace.tabs[receiver].editor;
    let view = a.sheet_views.get_mut(&sheet).unwrap();
    // The frame already configured this pane under the shared document owner.
    // Read its public layout directly: applying a view to the inactive shadow
    // editor here would manufacture a new buffer identity outside the UI path.
    let layout = e.layout.borrow();
    let byte = layout.offset(
        &e.text,
        (view.scroll_y / view.line_height + 1e-8).floor() as usize,
        view.scroll_x / view.cell,
    );
    (e.text.byte_to_line(byte), byte)
}
fn two_panes(lines: usize, wrap: bool, hidden: bool) -> (App, usize, usize) {
    let text = (0..lines)
        .map(|row| {
            format!(
                "{row:05}#0 {}\n",
                (1..32)
                    .map(|n| format!("{row:05}#{n} "))
                    .collect::<String>()
            )
        })
        .collect::<String>();
    let mut a = app(&text, None);
    a.options.mode = Mode::Interactive;
    set(&mut a, "word_wrap", json!(wrap));
    set(&mut a, "scroll_speed", json!(0));
    named(&mut a, "set_layout", json!({"preset":"columns2"}));
    park(&mut a, if hidden { 4002 } else { 5000 });
    if hidden {
        let at = a.workspace.editor().text.line_to_byte(4001);
        a.workspace.editor_mut().set_caret(at, false);
        if wrap {
            a.view
                .scroll(a.workspace.editor(), 0., a.view.line_height * 3.);
        }
        a.scene();
        assert!(!caret_visible(&a));
    }
    let anchor = a.view.text_scroll_anchor(a.workspace.editor()).byte;
    let receiver = a.workspace.active;
    named(&mut a, "clone_file", json!({}));
    named(&mut a, "move_to_group", json!({"group":1}));
    a.scene();
    (a, receiver, anchor)
}
#[test]
fn round18_third_remote_edit_and_geometry_use_pre_edit_coordinates() {
    for wrap in [false, true] {
        for change in ["gutter", "sidebar", "resize", "font"] {
            for hidden in [false, true] {
                let (mut a, receiver, anchor) = two_panes(9990, wrap, hidden);
                a.workspace.editor_mut().set_caret(0, false);
                let inserted = "INSERTED\n".repeat(100);
                // Actual editor input and shared-view notification, intentionally
                // no frame before the following geometry change.
                a.workspace.editor_mut().text_input(TextInput::Insert {
                    text: inserted.clone(),
                    replacement: None,
                });
                a.changed_state(Instant::now(), true);
                match change {
                    "sidebar" => named(&mut a, "toggle_side_bar", json!({})),
                    "resize" => a.resize_views(
                        Size {
                            width: 1100.,
                            height: 700.,
                        },
                        1.,
                    ),
                    "font" => {
                        assert!(key(&mut a, "super+="));
                    }
                    _ => {}
                }
                a.scene();
                let (_, actual) = receiver_top(&mut a, receiver);
                let e = &a.workspace.tabs[receiver].editor;
                let layout = e.layout.borrow();
                assert_eq!(
                    layout.visual_at(&e.text, actual).0,
                    layout.visual_at(&e.text, anchor + inserted.len()).0,
                    "wrap={wrap}, change={change}, hidden={hidden}: {}",
                    e.text.line(e.text.byte_to_line(actual))
                );
            }
        }
    }
}
#[test]
fn round18_third_undo_swallowed_viewport_restores_exact_reading_position() {
    for wrap in [false, true] {
        for all in [false, true] {
            for resize in [false, true] {
                let (mut a, receiver, anchor) = two_panes(6000, wrap, false);
                let start = if all {
                    0
                } else {
                    a.workspace.editor().text.line_to_byte(10)
                };
                let end = a.workspace.editor().text.len_bytes();
                a.workspace
                    .editor_mut()
                    .set_selection(vel_buffer::Selection::from_regions([
                        vel_buffer::Region::new(start, end),
                    ]));
                named(&mut a, "right_delete", json!({}));
                a.scene();
                let collapsed = receiver_top(&mut a, receiver).0;
                assert!(collapsed <= 10);
                if resize {
                    // Repeated resize must not revive the unreachable canonical anchor.
                    a.resize_views(
                        Size {
                            width: 1050.,
                            height: 710.,
                        },
                        1.,
                    );
                    a.scene();
                }
                named(&mut a, "undo", json!({}));
                a.scene();
                let (row, actual) = receiver_top(&mut a, receiver);
                let expected = a.workspace.tabs[receiver].editor.text.byte_to_line(anchor);
                assert!(
                    row.abs_diff(expected) <= 1,
                    "wrap={wrap} all={all}: {row} expected {expected}, byte={actual}"
                );
                a.resize_views(
                    Size {
                        width: 1000.,
                        height: 700.,
                    },
                    1.,
                );
                a.scene();
                assert!(receiver_top(&mut a, receiver).0.abs_diff(expected) <= 1);
                named(&mut a, "redo", json!({}));
                a.scene();
                assert!(receiver_top(&mut a, receiver).0 <= 10);
                named(&mut a, "undo", json!({}));
                a.scene();
                assert!(receiver_top(&mut a, receiver).0.abs_diff(expected) <= 1);
            }
        }
    }
}
#[test]
fn round18_third_burst_is_bounded_and_publishes_tail_after_exit() {
    let dir = tempfile::tempdir().unwrap();
    let exited = dir.path().join("exited");
    let mut a = app("", None);
    let program = format!(
        r#"my $s=('x' x 8191)."\n"; for(1..51200) {{print $s}} print "fixture.rs:7:1: FINAL_ERROR\nTAIL_MARKER\n"; close STDOUT; open(F,">{}"); print F "done"; close F;"#,
        exited.display()
    );
    named(
        &mut a,
        "exec",
        json!({"cmd":["/usr/bin/perl","-e",program.replace('$',r"\$")],"file_regex":"^(fixture.rs):([0-9]+):([0-9]+): (.*)$"}),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    while !exited.exists() {
        assert!(Instant::now() < deadline, "producer timeout");
        std::thread::sleep(Duration::from_millis(1));
    }
    let at = Instant::now();
    let task = a.builds.task.as_ref().unwrap();
    let stats = task.events.stats();
    assert!(stats.buffered_bytes <= 8 * 1024 * 1024);
    assert!(stats.peak_bytes <= 8 * 1024 * 1024);
    assert_eq!(stats.spill_file_bytes, 0, "transport creates no spill file");
    assert!(stats.dropped_bytes > 380 * 1024 * 1024);
    finish(&mut a);
    assert!(
        at.elapsed() < Duration::from_millis(500),
        "{:?}",
        at.elapsed()
    );
    eprintln!(
        "400 MiB burst: {:?} after exit, stats={stats:?}",
        at.elapsed()
    );
    assert!(a.panels.output.len() <= 8 * 1024 * 1024);
    assert!(a.panels.output.contains("TAIL_MARKER"));
    assert!(a.panels.output.contains("bytes dropped)"));
    assert!(
        a.panels.output.find("TAIL_MARKER").unwrap() < a.panels.output.find("[Finished").unwrap()
    );
    assert!(a.panels.message.contains("1 error"));
    let d = a.builds.diagnostics.last().unwrap();
    assert_eq!(
        a.panels.output.lines().nth(d.output_line),
        Some("fixture.rs:7:1: FINAL_ERROR")
    );
}
#[test]
fn round18_third_cancel_discards_burst_backlog_immediately() {
    let mut a = app("", None);
    named(
        &mut a,
        "exec",
        json!({"cmd":["/usr/bin/perl","-e",r#"$|=1; my $s=('x' x 8191)."\n"; while(1) {print $s}"#.replace('$',r"\$")]}),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while a.builds.task.as_ref().unwrap().events.stats().dropped_bytes == 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let at = Instant::now();
    named(&mut a, "cancel_build", json!({}));
    while a.builds.task.is_some() {
        a.builds.poll(&mut a.panels);
        assert!(at.elapsed() < Duration::from_millis(100));
        std::thread::yield_now();
    }
    eprintln!("burst cancel: {:?}", at.elapsed());
    assert!(a.panels.output.contains("[Cancelled]"));
    assert!(!a.panels.output.contains("[Finished"));
    assert!(a.panels.output.len() < 1024);
}
#[test]
fn round18_third_default_build_capture_uses_fixture_home_and_shell() {
    let mut a = app("", None);
    named(&mut a, "exec", json!({"cmd":["/usr/bin/env"]}));
    finish(&mut a);
    let env = a
        .panels
        .output
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect::<std::collections::BTreeMap<_, _>>();
    let home = Path::new(env["HOME"]);
    let shell = Path::new(env["SHELL"]);
    assert_eq!(shell.parent(), Some(home));
    assert!(shell.is_file());
    assert_ne!(home, Path::new(&std::env::var("HOME").unwrap()));
    assert!(
        std::fs::read_to_string(shell)
            .unwrap()
            .contains("exec /bin/sh -c")
    );
}

#[test]
fn round18_third_cancel_after_producer_exit_discards_retained_tail() {
    let mut a = app("", None);
    let source =
        r#"my $s=('x' x 8191)."\n"; for(1..15360) {print $s} print "TAIL\n";"#.replace('$', r"\$");
    named(&mut a, "exec", json!({"cmd":["/usr/bin/perl","-e",source]}));
    let deadline = Instant::now() + Duration::from_secs(30);
    while !a.builds.task.as_ref().unwrap().finished() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(a.builds.task.as_ref().unwrap().events.stats().dropped_bytes > 0);
    let at = Instant::now();
    named(&mut a, "cancel_build", json!({}));
    while a.builds.task.is_some() {
        a.builds.poll(&mut a.panels);
        assert!(at.elapsed() < Duration::from_millis(100));
        std::thread::yield_now();
    }
    assert!(a.panels.output.contains("[Cancelled]"));
    assert!(!a.panels.output.contains("TAIL"));
}

#[test]
fn round18_third_diagnostics_arrive_before_completion_without_replay() {
    let dir = tempfile::tempdir().unwrap();
    let release = dir.path().join("release");
    let mut a = app("", None);
    let source = format!(
        "/usr/bin/awk 'BEGIN {{for(i=1;i<=40000;i++) printf \"fixture.rs:%d:1: diagnostic\\n\",i}}'; while [ ! -f '{}' ]; do sleep .01; done",
        release.display()
    );
    named(
        &mut a,
        "exec",
        json!({"shell_cmd":source,"file_regex":"^([^:]+):([0-9]+):([0-9]+): (.*)$"}),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while a.builds.diagnostics.last().is_none_or(|d| d.line != 40000) {
        assert!(a.builds.task.is_some());
        a.builds.poll(&mut a.panels);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        a.builds.task.is_some(),
        "diagnostics must publish before completion"
    );
    assert_eq!(a.builds.diagnostics.len(), 32768);
    let rows = a.panels.output.lines().collect::<Vec<_>>();
    for d in &a.builds.diagnostics {
        assert_eq!(
            rows[d.output_line],
            format!("fixture.rs:{}:1: diagnostic", d.line)
        );
    }
    std::fs::write(release, "").unwrap();
    finish(&mut a);
    assert!(a.panels.message.contains("40000 errors"));
}

#[test]
fn round18_third_deletion_bookmarks_do_not_capture_without_reflow() {
    let (mut a, receiver, anchor) = two_panes(6000, false, false);
    let before = work(&a);
    a.workspace
        .editor_mut()
        .set_selection(vel_buffer::Selection::from_regions([
            vel_buffer::Region::new(anchor, anchor + 5),
        ]));
    named(&mut a, "right_delete", json!({}));
    a.scene();
    assert_eq!(
        work(&a),
        before,
        "a parked pane's deletion bookmark must not capture visibility or shape an old layout"
    );
    assert_eq!(receiver_top(&mut a, receiver).0, 5000);
    named(&mut a, "undo", json!({}));
    a.scene();
    assert_eq!(receiver_top(&mut a, receiver).0, 5000);
    assert_eq!(work(&a), before);
}

#[test]
fn round18_third_offscreen_reflow_uses_old_row_bounds() {
    for wrap in [false, true] {
        let (mut a, receiver, anchor) = two_panes(9990, wrap, false);
        named(&mut a, "focus_group", json!({"group":0}));
        let caret = a.workspace.editor().text.line_to_byte(9000);
        a.workspace.editor_mut().set_caret(caret, false);
        a.scene();
        assert!(!caret_visible(&a));
        named(&mut a, "focus_group", json!({"group":1}));
        a.scene();
        let sheet = a.workspace.tabs[receiver].sheet;
        let before = a.sheet_views[&sheet].anchor_queries.get();
        a.workspace.editor_mut().set_caret(0, false);
        let inserted = "INSERTED\n".repeat(100);
        a.workspace.editor_mut().text_input(TextInput::Insert {
            text: inserted.clone(),
            replacement: None,
        });
        a.changed_state(Instant::now(), true);
        a.scene();
        let (_, actual) = receiver_top(&mut a, receiver);
        let e = &a.workspace.tabs[receiver].editor;
        let layout = e.layout.borrow();
        assert_eq!(
            layout.visual_at(&e.text, actual).0,
            layout.visual_at(&e.text, anchor + inserted.len()).0
        );
        assert_eq!(
            a.sheet_views[&sheet].anchor_queries.get(),
            before,
            "offscreen carets need no old-layout capture, wrap={wrap}"
        );
    }
}
