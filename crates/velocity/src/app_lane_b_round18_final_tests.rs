//! Second-verifier reproductions through App dispatch, shared views and build polling.
use super::*;
fn numbered_app() -> App {
    let text = (0..6000)
        .map(|row| format!("ORIGINAL {row:05} {}\n", "words ".repeat(30)))
        .collect::<String>();
    let mut a = app(&text, None);
    a.options.mode = Mode::Interactive;
    set(&mut a, "word_wrap", json!(true));
    set(&mut a, "scroll_speed", json!(0));
    a.scene();
    a
}
fn top_text(a: &mut App) -> String {
    let row = top(a);
    a.workspace.editor().text.line(row).to_string()
}
fn input(a: &mut App, text: &str) {
    a.workspace.editor_mut().text_input(TextInput::Insert {
        text: text.into(),
        replacement: None,
    });
    a.changed_state(Instant::now(), true);
    a.scene();
}
fn work(a: &App) -> usize {
    a.view.anchor_queries.get()
        + a.sheet_views
            .values()
            .map(|v| v.anchor_queries.get())
            .sum::<usize>()
}
#[test]
fn round18_final_typing_and_wheel_skip_anchor_visibility_work() {
    for panes in [1, 4] {
        let mut a = numbered_app();
        set(&mut a, "word_wrap", json!(false));
        if panes == 4 {
            named(&mut a, "set_layout", json!({"preset":"grid4"}));
            for group in 1..4 {
                named(&mut a, "clone_file", json!({}));
                named(&mut a, "move_to_group", json!({"group":group}));
            }
        }
        input(&mut a, "warm");
        a.scene();
        let before = work(&a);
        for _ in 0..20 {
            input(&mut a, "x");
        }
        for _ in 0..20 {
            a.handle_scroll_event(
                None,
                ScrollEvent {
                    position: Point {
                        x: a.content.x + 200.,
                        y: a.content.y + 100.,
                    },
                    delta_x: 0.,
                    delta_y: -1.,
                    precise: false,
                    phase: 0,
                    momentum_phase: 0,
                    modifiers: Modifiers(0),
                    timestamp: 0.,
                },
            );
            a.scene();
        }
        assert_eq!(
            work(&a),
            before,
            "{panes} panes: ordinary input/scroll must not capture or measure caret visibility"
        );
        a.resize_views(
            Size {
                width: 1000.,
                height: 800.,
            },
            1.,
        );
        a.scene();
        assert!(work(&a) > before);
    }
}
#[test]
fn round18_final_shared_edits_anchor_parked_and_focused_receivers() {
    for focused in [false, true] {
        let mut a = numbered_app();
        named(&mut a, "set_layout", json!({"preset":"columns2"}));
        let receiver = a.workspace.active;
        let receiver_sheet = a.workspace.tabs[receiver].sheet;
        park(&mut a, 4000);
        let expected = top_text(&mut a);
        named(&mut a, "clone_file", json!({}));
        named(&mut a, "move_to_group", json!({"group":1}));
        a.scene();
        let writer = a.workspace.active;
        if focused {
            named(&mut a, "focus_group", json!({"group":0}));
            a.scene();
            // A previously revealed caret still must not follow a remote edit at BOF.
            named(&mut a, "show_at_center", json!({}));
            a.scene();
        }
        let expected = if focused { top_text(&mut a) } else { expected };
        a.workspace.tabs[writer].editor.set_caret(0, false);
        if focused {
            a.workspace.tabs[writer]
                .editor
                .run("insert", &json!({"characters":"INSERTED\n".repeat(1000)}))
                .unwrap();
            // Same shared post-event adapter as background buffer notifications.
            a.changed_state(Instant::now(), false);
        } else {
            named(&mut a, "move_to", json!({"to":"bof"}));
            input(&mut a, &"INSERTED\n".repeat(1000));
            assert!(caret_visible(&a));
        }
        a.scene();
        let (view, editor) = if focused {
            (&mut a.view, &a.workspace.tabs[receiver].editor)
        } else {
            (
                a.sheet_views.get_mut(&receiver_sheet).unwrap(),
                &a.workspace.tabs[receiver].editor,
            )
        };
        let at = view.text_scroll_anchor(editor).byte;
        let actual = editor.text.line(editor.text.byte_to_line(at)).to_string();
        assert_eq!(actual, expected, "focused={focused}");
        // A later wheel position becomes the new anchor, not the old reading mark.
        if focused {
            a.view
                .scroll(a.workspace.editor(), 0., a.view.line_height * 9.);
            a.scene();
            let now = top_text(&mut a);
            a.workspace.tabs[writer].editor.set_caret(0, false);
            a.workspace.tabs[writer]
                .editor
                .run("insert", &json!({"characters":"AGAIN\n".repeat(1000)}))
                .unwrap();
            a.changed_state(Instant::now(), false);
            a.scene();
            assert_eq!(top_text(&mut a), now);
        }
    }
}
#[test]
fn round18_final_long_line_paint_anchors_remote_edits() {
    let text = (0..120)
        .map(|row| {
            format!(
                "ORIGINAL {row:04} {}\n",
                "x".repeat(crate::large::LONG_LINE + 100)
            )
        })
        .collect::<String>();
    let mut a = app(&text, None);
    a.options.mode = Mode::Interactive;
    set(&mut a, "word_wrap", json!(false));
    set(&mut a, "scroll_speed", json!(0));
    named(&mut a, "set_layout", json!({"preset":"columns2"}));
    let at = a.workspace.editor().text.line_to_byte(80);
    a.workspace.editor_mut().set_caret(at, false);
    named(&mut a, "show_at_center", json!({}));
    a.scene();
    let receiver = a.workspace.active;
    let sheet = a.workspace.tabs[receiver].sheet;
    let expected = top_text(&mut a);
    named(&mut a, "clone_file", json!({}));
    named(&mut a, "move_to_group", json!({"group":1}));
    named(&mut a, "move_to", json!({"to":"bof"}));
    input(&mut a, &"INSERTED\n".repeat(1000));
    let view = a.sheet_views.get_mut(&sheet).unwrap();
    let editor = &a.workspace.tabs[receiver].editor;
    let anchor = view.text_scroll_anchor(editor);
    assert_eq!(
        editor
            .text
            .line(editor.text.byte_to_line(anchor.byte))
            .to_string(),
        expected
    );
    assert!(caret_visible(&a));
}
fn fold_above(a: &mut App) {
    // Even an explicit selection fold waits for the normal syntax publication.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        a.workspace.editor_mut().poll_language();
        if a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.analysis.is_some())
        {
            break;
        }
        assert!(Instant::now() < deadline, "syntax publication timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    let start = a.workspace.editor().text.line_to_byte(50);
    let end = a.workspace.editor().text.line_to_byte(3000);
    a.workspace
        .editor_mut()
        .set_selection(vel_buffer::Selection::from_regions([
            vel_buffer::Region::new(start, end),
        ]));
    named(a, "fold", json!({}));
    assert!(!a.workspace.editor().folded().is_empty());
    a.scene();
}
#[test]
fn round18_final_unfold_above_preserves_visible_text() {
    let mut a = numbered_app();
    fold_above(&mut a);
    park(&mut a, 4000);
    let expected = top_text(&mut a);
    let old = a.view.scroll_y;
    named(&mut a, "unfold_all", json!({}));
    a.scene();
    assert!(a.workspace.editor().folded().is_empty());
    assert_eq!(top_text(&mut a), expected);
    assert!(a.view.scroll_y > old);
    assert!(caret_visible(&a));
    named(&mut a, "unfold_all", json!({}));
    a.scene();
    assert_eq!(top_text(&mut a), expected);
}
#[test]
fn round18_final_reflow_finishes_animation_at_text_destination() {
    for change in ["resize", "sidebar", "wrap", "font", "folds"] {
        let mut a = numbered_app();
        if change == "folds" {
            fold_above(&mut a);
        }
        park(&mut a, 4000);
        set(&mut a, "scroll_speed", json!(0.1));
        assert!(key(&mut a, "pagedown"));
        assert!(a.view.scrolling());
        a.view.advance_scroll(
            a.workspace.editor(),
            Instant::now() + Duration::from_millis(40),
        );
        let caret = a.workspace.editor().caret;
        match change {
            "resize" => a.resize_views(
                Size {
                    width: 700.,
                    height: 800.,
                },
                1.,
            ),
            "sidebar" => named(&mut a, "toggle_side_bar", json!({})),
            "wrap" => set(&mut a, "word_wrap", json!(false)),
            "font" => named(&mut a, "increase_font_size", json!({})),
            "folds" => named(&mut a, "unfold_all", json!({})),
            _ => unreachable!(),
        }
        a.scene();
        assert!(!a.view.scrolling(), "{change}");
        assert!(caret_visible(&a), "{change}");
        assert_eq!(a.workspace.editor().caret, caret);
        let settled = (a.view.scroll_x, a.view.scroll_y);
        a.view.advance_scroll(
            a.workspace.editor(),
            Instant::now() + Duration::from_secs(3),
        );
        assert_eq!((a.view.scroll_x, a.view.scroll_y), settled);
        let r = a.view.rect_at(a.workspace.editor(), caret);
        assert_eq!(
            a.view.hit_test(
                a.workspace.editor(),
                Point {
                    x: r.x + 0.1,
                    y: r.y + r.height / 2.
                }
            ),
            caret
        );
    }
}
#[path = "../../vel-build/tests/support/detached.rs"]
mod detached_fixture;

#[test]
fn round18_final_continuous_daemon_finishes_and_cancels_promptly() {
    for cancel in [false, true] {
        let fixture = detached_fixture::DetachedFixture::new();
        let script = fixture.script(true, "sleep .05; echo ROOT_EXIT");
        let mut a = app("", None);
        named(&mut a, "exec", json!({"cmd":["/bin/sh","-c",script]}));
        let limit = Instant::now() + Duration::from_secs(10);
        while !a.panels.output.contains("ROOT_EXIT") {
            a.builds.poll(&mut a.panels);
            assert!(Instant::now() < limit);
            std::thread::sleep(Duration::from_millis(1));
        }
        let at = Instant::now();
        if cancel {
            named(&mut a, "cancel_build", json!({}));
        }
        finish(&mut a);
        let elapsed = at.elapsed();
        assert!(
            elapsed < Duration::from_millis(if cancel { 100 } else { 700 }),
            "cancel={cancel}: {elapsed:?}"
        );
        assert!(
            a.panels
                .output
                .contains(if cancel { "[Cancelled]" } else { "[Finished" })
        );
    }
}
#[test]
fn round18_final_output_spill_and_line_boundary_retention() {
    let mut a = app("", None);
    named(
        &mut a,
        "exec",
        json!({"cmd":["/usr/bin/awk","BEGIN {for(i=1;i<=24000;i++) printf \"fixture.rs:%d:1: WHOLE %01000d\\n\",i,i; print \"TAIL_MARKER\"}"],"file_regex":"^([^:]+):([0-9]+):1: (.*)$"}),
    );
    std::thread::sleep(Duration::from_secs(1));
    finish(&mut a);
    assert!(a.panels.output.len() <= 8 * 1024 * 1024);
    let mut lines = a.panels.output.lines();
    assert!(lines.next().unwrap().starts_with("[output truncated]"));
    assert!(lines.next().unwrap().starts_with("fixture.rs:"));
    assert!(a.panels.output.contains("TAIL_MARKER"));
    assert!(a.panels.message.contains("24000 errors"));
    let lines = a.panels.output.lines().collect::<Vec<_>>();
    for diagnostic in &a.builds.diagnostics {
        assert!(
            lines[diagnostic.output_line]
                .starts_with(&format!("fixture.rs:{}:1: WHOLE ", diagnostic.line))
        );
    }
    assert_eq!(a.builds.diagnostics.first().unwrap().output_line, 1);
}
#[test]
fn round18_final_oversized_output_line_discards_its_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let release = dir.path().join("release");
    let program = format!(
        r#"/usr/bin/awk 'BEGIN {{for(i=0;i<9216;i++) printf "%01024d",0}}'; while [ ! -e '{}' ]; do sleep .01; done; printf 'FRAGMENT\nWHOLE_LINE\nTAIL_MARKER\n'"#,
        release.display()
    );
    let mut a = app("", None);
    named(&mut a, "exec", json!({"cmd":["/bin/sh","-c",program]}));
    let deadline = Instant::now() + Duration::from_secs(30);
    while !a.panels.output.starts_with("[output truncated]") {
        a.builds.poll(&mut a.panels);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(a.panels.output.lines().count(), 1);
    assert!(a.panels.output.starts_with("[output truncated]"));
    std::fs::write(release, "").unwrap();
    finish(&mut a);
    assert!(
        a.panels
            .output
            .split_once('\n')
            .unwrap()
            .1
            .starts_with("WHOLE_LINE\nTAIL_MARKER\n"),
        "{}",
        a.panels.output
    );
}
fn with_login_app(name: &str, sleep: &str, ignored: bool, body: impl FnOnce(&Path)) {
    if let Some(root) = std::env::var_os("VELOCITY_R18_LOGIN_FIXTURE") {
        body(Path::new(&root));
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let shell = dir.path().join("shell");
    let delay = if sleep == "barrier" {
        format!(
            "while [ ! -f '{}/release' ]; do sleep .01; done",
            dir.path().display()
        )
    } else {
        format!("sleep {sleep}")
    };
    std::fs::write(&shell,format!("#!/bin/sh\nprintf x >> '{}/calls'\n{delay}\nexport PROFILE_READY=ready\nexec /bin/sh -c \"$3\"\n",dir.path().display())).unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    let module = module_path!().split_once("::").unwrap().1;
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", &format!("{module}::{name}"), "--nocapture"]);
    if ignored {
        command.arg("--ignored");
    }
    let result = command
        .env("VELOCITY_R18_LOGIN_FIXTURE", dir.path())
        .env("SHELL", shell)
        .env("HOME", dir.path())
        .env_remove("ZDOTDIR")
        .env_remove("BASH_ENV")
        .env_remove("ENV")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&result.stderr));
}
#[test]
fn round18_final_login_double_build_keeps_shared_capture() {
    with_login_app(
        "round18_final_login_double_build_keeps_shared_capture",
        "barrier",
        false,
        |root| {
            let mut a = app("", None);
            a.workspace.build_systems = vec![
                vel_build::BuildSystem::from_value(
                    "fixture",
                    json!({"cmd":["/usr/bin/printenv","PROFILE_READY"],"quiet":true}),
                )
                .unwrap(),
            ]
            .into();
            // Selection prewarms the same cache; the fixture holds startup at a
            // barrier so both actual Cmd+B requests happen during that capture.
            named(&mut a, "set_build_system", json!({"name":"fixture"}));
            let deadline = Instant::now() + Duration::from_secs(10);
            while std::fs::read_to_string(root.join("calls"))
                .unwrap_or_default()
                .is_empty()
            {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(key(&mut a, "super+b"));
            assert!(a.builds.task.is_some());
            assert!(key(&mut a, "super+b"));
            std::fs::write(root.join("release"), "").unwrap();
            finish(&mut a);
            // Process scheduling can exceed the production fallback budget under
            // parallel load; a late success must still come from this one capture.
            while a.panels.output != "ready\n" {
                assert!(
                    a.panels.message.contains("startup still running"),
                    "{}",
                    a.panels.message
                );
                assert!(Instant::now() < deadline, "shared capture never completed");
                std::thread::sleep(Duration::from_millis(10));
                assert!(key(&mut a, "super+b"));
                finish(&mut a);
            }
            for _ in 0..2 {
                assert!(key(&mut a, "super+b"));
                finish(&mut a);
                assert_eq!(a.panels.output, "ready\n");
            }
            assert_eq!(std::fs::read_to_string(root.join("calls")).unwrap(), "x");
        },
    );
}
#[test]
fn round18_final_login_alternating_builds_share_base() {
    with_login_app(
        "round18_final_login_alternating_builds_share_base",
        "3",
        false,
        |root| {
            let mut a = app("", None);
            a.workspace.build_systems=[("A","/a"),("B","/b")].into_iter().map(|(name,path)|
            vel_build::BuildSystem::from_value(name,json!({"cmd":["/usr/bin/env"],"quiet":true,"env":{"BUILD_FLAG":name,"SHELL":"/does/not/exist"},"path":format!("$PATH:{path}")})).unwrap()).collect();
            for name in ["A", "B"] {
                named(&mut a, "set_build_system", json!({"name":name}));
                assert!(key(&mut a, "super+b"));
                finish(&mut a);
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                assert!(key(&mut a, "super+b"));
                finish(&mut a);
                if a.panels.output.lines().any(|l| l == "PROFILE_READY=ready") {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "late capture never became available"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
            for name in ["A", "B"].into_iter().cycle().take(8) {
                named(&mut a, "set_build_system", json!({"name":name}));
                assert!(key(&mut a, "super+b"));
                finish(&mut a);
                let values: std::collections::BTreeMap<_, _> = a
                    .panels
                    .output
                    .lines()
                    .filter_map(|l| l.split_once('='))
                    .collect();
                assert_eq!(values.get("PROFILE_READY"), Some(&"ready"));
                assert_eq!(values.get("BUILD_FLAG"), Some(&name));
                assert_eq!(values.get("SHELL"), Some(&"/does/not/exist"));
                assert!(values["PATH"].ends_with(if name == "A" { ":/a" } else { ":/b" }));
            }
            assert_eq!(std::fs::read_to_string(root.join("calls")).unwrap(), "x");
        },
    );
}
#[test]
#[ignore = "isolated three-second quiet-build CPU measurement"]
fn round18_final_quiet_build_cpu_timing() {
    with_login_app("round18_final_quiet_build_cpu_timing", "0", true, |_| {
        let mut a = app("", None);
        named(&mut a, "exec", json!({"cmd":["/bin/true"]}));
        finish(&mut a);
        fn cpu() -> f64 {
            // SAFETY: rusage is an initialized writable value; getrusage retains no pointer.
            let mut r: libc::rusage = unsafe { std::mem::zeroed() };
            // SAFETY: r points to the initialized writable rusage value above.
            assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut r) }, 0);
            (r.ru_utime.tv_sec + r.ru_stime.tv_sec) as f64
                + (r.ru_utime.tv_usec + r.ru_stime.tv_usec) as f64 / 1e6
        }
        let before = cpu();
        named(&mut a, "exec", json!({"cmd":["/bin/sleep","3"]}));
        finish(&mut a);
        let ms = (cpu() - before) * 1000.;
        eprintln!("R18_QUIET_BUILD cpu_ms={ms:.3}");
        assert!(ms < 30., "quiet build CPU {ms:.3} ms");
    });
}

#[path = "app_lane_b_round18_third_tests.rs"]
mod third_verification;
