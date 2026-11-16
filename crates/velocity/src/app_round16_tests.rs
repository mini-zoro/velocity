use super::*;
use vel_buffer::{Region, Selection};
use vel_config::keymap::Key;
use vel_text::Text;

fn app(source: &str, data: Option<&Path>) -> App {
    App::new(
        Options::default(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&Options::default(), Instant::now()).unwrap(),
        Workspace::new(Text::from(source), data.map(Path::to_path_buf), None, false).unwrap(),
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(data))),
    )
}
fn named(app: &mut App, name: &str, args: Value) {
    app.run_named(None, name, &args);
}
fn key(app: &mut App, key: &str) {
    let viewport = app.viewport();
    assert!(
        app.dispatch_key_binding(None, &Key::parse(key).unwrap(), Instant::now(), viewport),
        "{key}"
    );
}
fn wait_selection(app: &mut App, expected: std::ops::Range<usize>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let viewport = app.viewport();
        app.panels.poll(&mut app.workspace, viewport);
        if app.workspace.editor().selection() == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected:?}, got {:?}: {}",
            app.workspace.editor().selection(),
            app.panels.message
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
fn setup_find(app: &mut App, panel: &str, query: &str, caret: usize) {
    app.workspace.editor_mut().set_caret(caret, false);
    named(
        app,
        "show_panel",
        json!({"panel":panel,"text":if panel == "incremental_find" { "" } else { query }}),
    );
    if panel == "incremental_find" {
        named(app, "insert", json!({"characters":query}));
        wait_selection(app, 0..3);
    }
}

#[test]
fn named_dispatch_records_find_once_and_replays_in_order() {
    for (panel, find_key, from, hit, command) in [
        ("find", "super+g", 0, 0..3, "find_next"),
        ("find", "enter", 0, 0..3, "find_next"),
        ("find", "shift+enter", 11, 8..11, "find_prev"),
        ("incremental_find", "enter", 0, 8..11, "find_next"),
    ] {
        let mut recorded = app("one two one", None);
        setup_find(&mut recorded, panel, "one", from);
        let initial = recorded
            .workspace
            .editor()
            .buffer
            .view(recorded.workspace.editor().id)
            .sel()
            .clone();
        key(&mut recorded, "ctrl+q");
        key(&mut recorded, find_key);
        wait_selection(&mut recorded, hit.clone());
        if recorded.panels.panel.is_some() {
            key(&mut recorded, "escape");
        }
        named(&mut recorded, "insert", json!({"characters":"Z"}));
        key(&mut recorded, "ctrl+q");
        let commands = recorded.workspace.editor().registry.macro_commands();
        assert_eq!(
            commands.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            [command, "insert"],
            "{find_key}"
        );
        assert_eq!(commands[0].1["_macro_find"]["query"], "one");

        let mut manual = app("one two one", None);
        setup_find(&mut manual, panel, "one", from);
        key(&mut manual, find_key);
        wait_selection(&mut manual, hit);
        if manual.panels.panel.is_some() {
            key(&mut manual, "escape");
        }
        named(&mut manual, "insert", json!({"characters":"Z"}));
        let expected_text = manual.workspace.editor().text.to_string();
        let expected_regions = manual.workspace.editor().regions();
        assert_eq!(recorded.workspace.editor().text.to_string(), expected_text);
        assert_eq!(recorded.workspace.editor().regions(), expected_regions);

        named(&mut recorded, "undo", json!({}));
        // Leave a different query and a focused Find panel at playback. Recorded text
        // commands must still edit the document, and Find must restore its captured query.
        named(
            &mut recorded,
            "show_panel",
            json!({"panel":"find","text":"two"}),
        );
        recorded.workspace.editor_mut().set_selection(initial);
        key(&mut recorded, "ctrl+shift+q");
        assert_eq!(
            recorded.workspace.editor().text.to_string(),
            expected_text,
            "{panel}: {find_key}"
        );
        assert_eq!(recorded.workspace.editor().regions(), expected_regions);
        assert!(recorded.workspace.editor().actions.is_empty());
        assert_eq!(
            recorded.workspace.editor().registry.macro_commands(),
            commands
        );
    }
}

#[test]
fn named_dispatch_stops_user_macro_after_window_step_changes_view() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Packages/User")).unwrap();
    std::fs::write(
        dir.path().join("Packages/User/Mine.sublime-macro"),
        r#"[
        {"command":"find_next"}, {"command":"insert","args":{"characters":"Z"}},
        {"command":"new_file"}, {"command":"insert","args":{"characters":"next tab"}}
    ]"#,
    )
    .unwrap();
    for queued in [false, true] {
        let mut a = app("one two one", Some(dir.path()));
        setup_find(&mut a, "find", "one", 0);
        named(&mut a, "hide_panel", json!({}));
        let args = json!({"file":"res://Packages/User/Mine.sublime-macro"});
        if queued {
            // Exact host entry that previously failed before its actions could be forwarded.
            a.workspace
                .editor_mut()
                .run("run_macro_file", &args)
                .unwrap();
            assert_eq!(a.workspace.editor().actions.len(), 1);
            a.handle_named_actions(None).unwrap();
        } else {
            named(&mut a, "run_macro_file", args);
        }
        assert_eq!(a.workspace.tabs.len(), 2, "{}", a.panels.message);
        assert_eq!(a.workspace.tabs[0].editor.text.to_string(), "Z two one");
        assert_eq!(a.workspace.tabs[0].editor.regions(), [Region::caret(1)]);
        assert_eq!(a.workspace.editor().text.to_string(), "");
        assert_eq!(a.workspace.editor().regions(), [Region::caret(0)]);
        assert!(
            a.panels
                .message
                .contains("starting view is no longer active")
        );
        assert!(a.workspace.tabs.iter().all(|t| t.editor.actions.is_empty()));
    }
}

#[test]
fn named_dispatch_records_builtin_macros_across_tabs_once_per_caret() {
    let mut a = app("one two one", None);
    key(&mut a, "ctrl+q");
    named(&mut a, "insert", json!({"characters":"A"}));
    named(&mut a, "new_file", json!({}));
    assert!(a.workspace.editor().registry.is_recording());
    assert!(a.workspace.editor().context_matches(&[
        serde_json::from_value(json!({"key":"is_recording","operand":true})).unwrap()
    ]));
    named(
        &mut a,
        "run_macro_file",
        json!({"file":"res://Packages/Default/Add Line Before.sublime-macro"}),
    );
    named(&mut a, "insert", json!({"characters":"B"}));
    key(&mut a, "ctrl+q");
    assert_eq!(
        a.workspace
            .editor()
            .registry
            .macro_commands()
            .iter()
            .map(|c| c.0.as_str())
            .collect::<Vec<_>>(),
        ["insert", "run_macro_file", "insert"]
    );
    named(&mut a, "select_all", json!({}));
    named(&mut a, "insert", json!({"characters":"x\ny"}));
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([
            Region::caret(0),
            Region::caret(2),
        ]));
    key(&mut a, "ctrl+shift+q");
    assert_eq!(a.workspace.editor().text.to_string(), "B\nAx\nB\nAy");
    assert_eq!(a.workspace.editor().registry.macro_commands().len(), 3);
}

#[test]
fn named_dispatch_saves_loads_and_reloads_package_macros() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Packages/Other/Custom.sublime-macro");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"[{"command":"insert","args":{"characters":"猫"}}]"#,
    )
    .unwrap();
    let mut a = app("", Some(dir.path()));
    key(&mut a, "ctrl+q");
    named(
        &mut a,
        "run_macro_file",
        json!({"file":"res://Packages/Other/Custom.sublime-macro"}),
    );
    key(&mut a, "ctrl+q");
    assert_eq!(a.workspace.editor().registry.macro_commands().len(), 1);
    named(&mut a, "save_macro", json!({"name":"Mine"}));
    assert!(
        dir.path()
            .join("Packages/User/Mine.sublime-macro")
            .is_file(),
        "{}",
        a.panels.message
    );
    let items = a.panels.macro_file_items(&a.workspace).unwrap();
    for file in ["Other/Custom", "User/Mine"] {
        assert!(
            items.iter().any(
                |i| i.invocation.args["file"] == format!("res://Packages/{file}.sublime-macro")
            )
        );
    }
    named(
        &mut a,
        "run_macro_file",
        json!({"file":"Packages/User/Mine.sublime-macro"}),
    );
    assert_eq!(a.workspace.editor().text.to_string(), "猫猫");
    a.workspace.wait_resources();
    std::fs::write(&path, r#"[{"command":"insert","args":{"characters":"!"}}]"#).unwrap();
    a.workspace.request_resource_reload();
    a.workspace.wait_resources();
    named(
        &mut a,
        "run_macro_file",
        json!({"file":"res://Packages/Other/Custom.sublime-macro"}),
    );
    assert_eq!(a.workspace.editor().text.to_string(), "猫猫!");
    named(&mut a, "run_macro_file", json!({}));
    assert!(a.panels.overlay.is_some());
    key(&mut a, "escape");
    named(&mut a, "save_macro", json!({"name":"../bad"}));
    assert!(a.panels.message.contains("inside Packages/User"));
}

#[test]
fn named_dispatch_restores_recording_and_stops_after_macro_errors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Packages/User")).unwrap();
    std::fs::write(
        dir.path().join("Packages/User/Loop.sublime-macro"),
        r#"[{"command":"run_macro_file","args":{"file":"Packages/User/Loop.sublime-macro"}}]"#,
    )
    .unwrap();
    let mut a = app("original", Some(dir.path()));
    named(
        &mut a,
        "run_macro",
        json!({"commands":[{"command":"unknown"},{"command":"insert","args":{"characters":"wrong"}}]}),
    );
    assert!(a.panels.message.contains("unknown command"));
    assert_eq!(a.workspace.editor().text.to_string(), "original");
    assert_eq!(a.workspace.editor().registry.playback_depth(), 0);
    named(
        &mut a,
        "run_macro_file",
        json!({"file":"Packages/User/Loop.sublime-macro"}),
    );
    assert!(a.panels.message.contains("recursion"));
    assert_eq!(a.workspace.editor().registry.playback_depth(), 0);
    key(&mut a, "ctrl+q");
    named(&mut a, "insert", json!({"characters":"works"}));
    key(&mut a, "ctrl+q");
    assert_eq!(a.workspace.editor().registry.macro_commands().len(), 1);
}

#[test]
fn saving_recorded_macros_chooses_free_names_and_preserves_existing_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Packages/User");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Recorded.sublime-macro"), "existing user macro").unwrap();
    std::fs::write(root.join("Recorded.sublime-macro.bak"), "existing backup").unwrap();
    let mut a = app("", Some(dir.path()));
    for (i, text) in ["first", "second", "third"].into_iter().enumerate() {
        key(&mut a, "ctrl+q");
        named(&mut a, "insert", json!({"characters":text}));
        key(&mut a, "ctrl+q");
        // Use the real naming panel and Enter completion, accepting its default name.
        named(&mut a, "save_macro", json!({}));
        assert!(a.panels.input().is_some());
        key(&mut a, "enter");
        let path = root.join(format!("Recorded {}.sublime-macro", i + 2));
        let saved = std::fs::read_to_string(path).unwrap();
        assert_eq!(
            vel_commands::parse_macro(&saved).unwrap(),
            [("insert".into(), json!({"characters":text}))]
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.join("Recorded.sublime-macro")).unwrap(),
        "existing user macro"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("Recorded.sublime-macro.bak")).unwrap(),
        "existing backup"
    );
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 5);
}

#[test]
fn named_dispatch_finishes_replace_next_navigation_before_the_next_macro_edit() {
    let mut manual = app("one two one", None);
    setup_find(&mut manual, "replace", "one", 0);
    manual.panels.replace.set("X");
    manual
        .workspace
        .editor_mut()
        .set_selection(Selection::from(Region::new(0, 3)));
    key(&mut manual, "ctrl+q");
    named(&mut manual, "replace_next", json!({}));
    wait_selection(&mut manual, 6..9);
    named(&mut manual, "hide_panel", json!({}));
    named(&mut manual, "insert", json!({"characters":"Z"}));
    key(&mut manual, "ctrl+q");
    assert_eq!(manual.workspace.editor().text.to_string(), "X two Z");
    assert_eq!(
        manual
            .workspace
            .editor()
            .registry
            .macro_commands()
            .iter()
            .map(|c| c.0.as_str())
            .collect::<Vec<_>>(),
        ["replace_next", "insert"]
    );
    let mut replay = app("one two one", None);
    replay
        .workspace
        .editor_mut()
        .set_selection(Selection::from(Region::new(0, 3)));
    named(&mut replay, "run_macro", json!({}));
    assert_eq!(
        replay.workspace.editor().text.to_string(),
        manual.workspace.editor().text.to_string()
    );
    assert_eq!(
        replay.workspace.editor().regions(),
        manual.workspace.editor().regions()
    );
    assert!(replay.workspace.editor().actions.is_empty());
}

// These existing keymap regressions exercise built-in macro-backed keys at their new
// application owner instead of stopping at Editor's deliberately deferred action queue.
fn keys(app: &mut App, sequence: &str) {
    for name in sequence.split(' ') {
        key(app, name);
    }
}
fn select_app(app: &mut App, regions: &[(usize, usize)]) {
    app.workspace
        .editor_mut()
        .set_selection(Selection::from_regions(
            regions.iter().map(|&(a, b)| Region::new(a, b)),
        ));
}
#[test]
fn pairing_follows_the_surrounding_text_wraps_selections_and_deletes_empty_pairs() {
    let mut editor = app("don\nfoo;\nfoo bar\nabc\n", None);
    select_app(&mut editor, &[(3, 3)]);
    // After a letter the quote is not paired: the key falls through to plain text input.
    let viewport = editor.viewport();
    assert!(!editor.dispatch_key_binding(
        None,
        &Key::parse("'").unwrap(),
        Instant::now(),
        viewport
    ));
    select_app(&mut editor, &[(7, 7)]);
    keys(&mut editor, "(");
    assert_eq!(editor.workspace.editor().text.line(1).to_string(), "foo();");
    keys(&mut editor, "backspace");
    assert_eq!(
        editor.workspace.editor().text.line(1).to_string(),
        "foo;",
        "backspace deletes the pair"
    );
    select_app(&mut editor, &[(12, 12)]);
    keys(&mut editor, "(");
    assert_eq!(
        editor.workspace.editor().text.line(2).to_string(),
        "foo() bar"
    );
    let abc = editor.workspace.editor().text.line_to_byte(3);
    select_app(&mut editor, &[(abc, abc + 3)]);
    keys(&mut editor, "[");
    assert_eq!(
        editor.workspace.editor().text.line(3).to_string(),
        "[abc]",
        "a selection is wrapped"
    );
}
#[test]
fn line_editing_keys_match_the_macos_defaults() {
    let mut editor = app("\tone\ntwo\nthree\n", None);
    select_app(&mut editor, &[(2, 2)]);
    keys(&mut editor, "super+enter");
    assert_eq!(
        editor.workspace.editor().text.to_string(),
        "\tone\n\t\ntwo\nthree\n"
    );
    assert_eq!(editor.workspace.editor().caret, 6);
    named(&mut editor, "undo", json!({}));
    select_app(&mut editor, &[(2, 2)]);
    keys(&mut editor, "super+shift+enter");
    assert_eq!(
        editor.workspace.editor().text.to_string(),
        "\t\n\tone\ntwo\nthree\n"
    );
    assert_eq!(editor.workspace.editor().caret, 1);
    named(&mut editor, "undo", json!({}));
    select_app(&mut editor, &[(6, 6)]);
    keys(&mut editor, "ctrl+shift+k");
    assert_eq!(editor.workspace.editor().text.to_string(), "\tone\nthree\n");
    select_app(&mut editor, &[(7, 7)]);
    keys(&mut editor, "super+backspace");
    assert_eq!(editor.workspace.editor().text.to_string(), "\tone\nree\n");
    select_app(&mut editor, &[(2, 2)]);
    keys(&mut editor, "super+k super+k");
    assert_eq!(editor.workspace.editor().text.to_string(), "\to\nree\n");
    // ⌘⇧J joins; ⌘J alone is a chord prefix.
    select_app(&mut editor, &[(0, 0)]);
    keys(&mut editor, "super+shift+j");
    assert_eq!(editor.workspace.editor().text.to_string(), "\to ree\n");
    // Tab with a multi-line selection indents instead of replacing it.
    let mut editor = app("aaa\nbbb\nccc\n", None);
    select_app(&mut editor, &[(0, 8)]);
    keys(&mut editor, "tab");
    assert_eq!(
        editor.workspace.editor().text.to_string(),
        "\taaa\n\tbbb\nccc\n"
    );
    // ⌘← goes to the indentation first, then to column zero.
    let mut editor = app("    foo", None);
    select_app(&mut editor, &[(7, 7)]);
    keys(&mut editor, "super+left");
    assert_eq!(editor.workspace.editor().caret, 4);
    keys(&mut editor, "super+left");
    assert_eq!(editor.workspace.editor().caret, 0);
}

// Device-free timing of the real first macro key, excluding profile construction/reload.
// Reuse exactly the same 100 MB loose-file fixture before and after resource lookup changes.
#[test]
#[ignore = "manual headless timing with a 100 MB profile"]
fn macro_first_key_with_large_loose_package() {
    let root = Path::new("/tmp/velocity-lane-a-r16-review2/profile-100mb");
    let loose = root.join("Packages/Weight");
    std::fs::create_dir_all(&loose).unwrap();
    let bytes = vec![b'x'; 1_000_000];
    for i in 0..100 {
        let path = loose.join(format!("{i:03}.dat"));
        if !path.is_file() {
            std::fs::write(path, &bytes).unwrap();
        }
    }
    for sample in 1..=5 {
        let mut a = app("line", Some(root));
        a.workspace.editor_mut().set_caret(2, false);
        let started = Instant::now();
        key(&mut a, "super+enter");
        let first = started.elapsed();
        named(&mut a, "undo", json!({}));
        let started = Instant::now();
        key(&mut a, "super+enter");
        let warm = started.elapsed();
        named(&mut a, "undo", json!({}));
        a.workspace.request_resource_reload();
        a.workspace.wait_resources();
        let started = Instant::now();
        key(&mut a, "super+enter");
        let reloaded = started.elapsed();
        assert_eq!(a.workspace.editor().text.to_string(), "line\n");
        println!(
            "macro_100mb sample={sample} first_us={:.3} warm_us={:.3} reloaded_us={:.3}",
            first.as_secs_f64() * 1e6,
            warm.as_secs_f64() * 1e6,
            reloaded.as_secs_f64() * 1e6
        );
    }
}

fn macro_key(app: &mut App, name: &str, args: Value) {
    app.workspace
        .editor_mut()
        .keymap
        .append(
            &json!([
                {"keys":["ctrl+alt+f12"],"command":name,"args":args}
            ])
            .to_string(),
        )
        .unwrap();
    key(app, "ctrl+alt+f12");
}

#[test]
fn macro_editing_keys_target_the_focused_input_without_touching_the_document() {
    for panel in ["find", "palette", "macro_name"] {
        for (binding, input, expected) in [
            ("backspace", "abc", "ab"),
            ("super+backspace", "alpha beta", ""),
            ("ctrl+shift+k", "alpha beta", ""),
        ] {
            let mut a = app("call()\nhello world", None);
            a.workspace.editor_mut().set_caret(5, false);
            match panel {
                "find" => named(&mut a, "show_panel", json!({"panel":"find","text":""})),
                "palette" => named(&mut a, "show_overlay", json!({"overlay":"command_palette"})),
                _ => named(&mut a, "save_macro", json!({})),
            }
            let field = a.panels.input_mut().unwrap();
            field.set(input);
            field.run("move_to", &json!({"to":"eof"})).unwrap();
            let before = a.workspace.editor().regions();
            let change = a.workspace.editor().buffer.change_id();
            key(&mut a, binding);
            assert_eq!(
                a.panels.input().unwrap().value(),
                expected,
                "{panel}: {binding}"
            );
            assert_eq!(
                a.workspace.editor().text.to_string(),
                "call()\nhello world",
                "{panel}: {binding}"
            );
            assert_eq!(a.workspace.editor().regions(), before);
            assert_eq!(a.workspace.editor().buffer.change_id(), change);
            assert!(!a.workspace.tabs[a.workspace.active].dirty());
            key(&mut a, "super+z");
            assert_eq!(
                a.panels.input().unwrap().value(),
                input,
                "input undo: {panel}: {binding}"
            );
        }
    }
    // The same macro-backed binding still edits the document when its view owns focus.
    let mut a = app("call()", None);
    a.workspace.editor_mut().set_caret(5, false);
    key(&mut a, "backspace");
    assert_eq!(a.workspace.editor().text.to_string(), "call");
    assert_eq!(a.workspace.editor().regions(), [Region::caret(4)]);
}

#[test]
fn macro_keys_ignore_broken_packages_and_keep_the_user_catalog() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Installed Packages")).unwrap();
    std::fs::create_dir_all(dir.path().join("Packages/User")).unwrap();
    // A corrupt unrelated archive must not be opened by any of these macro keys.
    std::fs::write(
        dir.path().join("Installed Packages/Broken.sublime-package"),
        b"not a zip",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Packages/User/Mine.sublime-macro"),
        r#"[{"command":"insert","args":{"characters":"user"}}]"#,
    )
    .unwrap();
    let mut a = app("call()\nsecond", Some(dir.path()));
    assert!(
        a.workspace
            .messages
            .iter()
            .any(|m| m.contains("Broken.sublime-package"))
    );
    assert!(
        a.panels
            .macro_file_items(&a.workspace)
            .unwrap()
            .iter()
            .any(|i| i.invocation.args["file"] == "res://Packages/User/Mine.sublime-macro")
    );
    for (binding, expected) in [
        ("super+enter", "call()\n\nsecond"),
        ("ctrl+shift+k", "second"),
        ("super+backspace", ")\nsecond"),
        ("backspace", "call\nsecond"),
    ] {
        let mut a = app("call()\nsecond", Some(dir.path()));
        a.workspace.editor_mut().set_caret(5, false);
        key(&mut a, binding);
        assert_eq!(
            a.workspace.editor().text.to_string(),
            expected,
            "{binding}: {}",
            a.panels.message
        );
    }
    a.workspace.editor_mut().set_caret(5, false);
    macro_key(
        &mut a,
        "run_macro_file",
        json!({"file":"Packages/User/Mine.sublime-macro"}),
    );
    assert_eq!(a.workspace.editor().text.to_string(), "call(user)\nsecond");

    // A broken *supplying* archive warns and falls back to our clean-room Default macros.
    std::fs::write(
        dir.path()
            .join("Installed Packages/Default.sublime-package"),
        b"not a zip",
    )
    .unwrap();
    a.workspace.request_resource_reload();
    a.workspace.wait_resources();
    key(&mut a, "super+enter");
    assert_eq!(
        a.workspace.editor().text.to_string(),
        "call(user)\n\nsecond"
    );
    assert!(
        a.workspace
            .messages
            .iter()
            .any(|m| m.contains("Macro resource warning") && m.contains("Default.sublime-package"))
    );
}

#[cfg(unix)]
#[test]
fn macro_keys_do_not_read_unrelated_unreadable_loose_files() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let unreadable = dir.path().join("Packages/Unrelated/private.dat");
    std::fs::create_dir_all(unreadable.parent().unwrap()).unwrap();
    std::fs::write(&unreadable, "private unrelated bytes").unwrap();
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut a = app("line", Some(dir.path()));
    key(&mut a, "super+enter");
    assert_eq!(a.workspace.editor().text.to_string(), "line\n");
    assert!(
        !a.workspace
            .messages
            .iter()
            .any(|m| m.starts_with("Macro resource warning"))
    );
    std::fs::set_permissions(unreadable, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn macro_keys_load_only_the_requested_path_and_cache_parsed_commands_until_reload() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let installed = dir.path().join("Installed Packages");
    let loose = dir.path().join("Packages/Other");
    std::fs::create_dir_all(&installed).unwrap();
    std::fs::create_dir_all(&loose).unwrap();
    let path = loose.join("Chosen.sublime-macro");
    std::fs::write(&path, r#"[{"command":"insert","args":{"characters":"L"}}]"#).unwrap();
    let archive = std::fs::File::create(installed.join("Other.sublime-package")).unwrap();
    let mut archive = zip::ZipWriter::new(archive);
    archive
        .start_file(
            "Chosen.sublime-macro",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive
        .write_all(br#"[{"command":"insert","args":{"characters":"A"}}]"#)
        .unwrap();
    archive
        .start_file(
            "Unused.sublime-macro",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive
        .write_all(b"this must not be parsed on another key or in the chooser")
        .unwrap();
    archive.finish().unwrap();
    let mut a = app("", Some(dir.path()));
    let args = json!({"file":"res://Packages/Other/Chosen.sublime-macro"});
    macro_key(&mut a, "run_macro_file", args.clone());
    assert_eq!(a.workspace.editor().text.to_string(), "L");
    std::fs::remove_file(&path).unwrap();
    // The parsed macro, not loose bytes or the full archive, stays cached until reload.
    key(&mut a, "ctrl+alt+f12");
    assert_eq!(a.workspace.editor().text.to_string(), "LL");
    a.workspace.request_resource_reload();
    a.workspace.wait_resources();
    macro_key(&mut a, "run_macro_file", args);
    assert_eq!(a.workspace.editor().text.to_string(), "LLA");
    // The chooser uses the background index even if package files disappear afterwards.
    std::fs::remove_file(installed.join("Other.sublime-package")).unwrap();
    let items = a.panels.macro_file_items(&a.workspace).unwrap();
    assert!(
        items
            .iter()
            .any(|i| i.invocation.args["file"] == "res://Packages/Other/Unused.sublime-macro")
    );
    key(&mut a, "ctrl+alt+f12");
    assert_eq!(a.workspace.editor().text.to_string(), "LLAA");
    a.workspace.settings.set(
        vel_config::settings::Layer::User,
        "ignored_packages",
        json!(["Other"]),
    );
    key(&mut a, "ctrl+alt+f12");
    assert_eq!(a.workspace.editor().text.to_string(), "LLAA");
    assert!(a.panels.message.contains("not available"));
}

#[test]
fn macro_keys_stop_after_closing_the_origin_without_editing_or_saving_its_neighbor() {
    for chained in [false, true] {
        for file in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let neighbor = dir.path().join("neighbor.txt");
            std::fs::write(&neighbor, "saved neighbor").unwrap();
            let mut a = app("clean origin", Some(dir.path()));
            let origin = a.workspace.tabs[0].sheet;
            named(&mut a, "new_file", json!({}));
            named(&mut a, "insert", json!({"characters":"unsaved neighbor"}));
            a.workspace.tabs[1].path = Some(neighbor.clone());
            a.workspace.select(0);
            let commands = if chained {
                json!([{"command":"chain","args":{"commands":[["close",{}],["select_all",{}],["left_delete",{}],["save",{}]]}}])
            } else {
                json!([{"command":"close"},{"command":"select_all"},{"command":"left_delete"},{"command":"save"}])
            };
            if file {
                let root = dir.path().join("Packages/User");
                std::fs::create_dir_all(&root).unwrap();
                std::fs::write(root.join("Close.sublime-macro"), commands.to_string()).unwrap();
                macro_key(
                    &mut a,
                    "run_macro_file",
                    json!({"file":"Packages/User/Close.sublime-macro"}),
                );
            } else {
                macro_key(&mut a, "run_macro", json!({"commands":commands}));
            }
            assert!(a.workspace.sheet_index(origin).is_none());
            assert_eq!(a.workspace.tabs.len(), 1);
            assert_eq!(a.workspace.editor().text.to_string(), "unsaved neighbor");
            assert_eq!(a.workspace.editor().regions(), [Region::caret(16)]);
            assert!(a.workspace.tabs[0].dirty());
            assert_eq!(
                std::fs::read_to_string(&neighbor).unwrap(),
                "saved neighbor"
            );
            assert!(
                a.panels
                    .message
                    .contains("starting view is no longer active"),
                "{}",
                a.panels.message
            );
            assert_eq!(a.workspace.editor().registry.playback_depth(), 0);
            assert!(a.macro_origin.is_none());
            assert!(a.workspace.editor().actions.is_empty());
            // A later ordinary command is not left pinned to the closed sheet.
            named(&mut a, "insert", json!({"characters":"!"}));
            assert_eq!(a.workspace.editor().text.to_string(), "unsaved neighbor!");
        }
    }
}

#[test]
fn key_dispatch_preserves_shared_recorder_and_local_repeat_across_registry_handoffs() {
    let mut a = app("", None);
    key(&mut a, "ctrl+q");
    for _ in 0..3 {
        macro_key(&mut a, "insert", json!({"characters":"x"}));
    }
    named(&mut a, "new_file", json!({}));
    macro_key(&mut a, "insert", json!({"characters":"y"}));
    key(&mut a, "ctrl+q");
    assert_eq!(a.workspace.editor().registry.macro_commands().len(), 4);
    named(&mut a, "undo", json!({}));
    named(&mut a, "redo_or_repeat", json!({}));
    assert_eq!(a.workspace.editor().text.to_string(), "y");
    key(&mut a, "ctrl+shift+q");
    assert_eq!(a.workspace.editor().text.to_string(), "yxxxy");
    named(&mut a, "show_panel", json!({"panel":"find","text":"query"}));
    let input = a.panels.input_mut().unwrap();
    input.run("move_to", &json!({"to":"eof"})).unwrap();
    key(&mut a, "backspace");
    assert_eq!(a.panels.input().unwrap().value(), "quer");
    key(&mut a, "super+z");
    assert_eq!(a.panels.input().unwrap().value(), "query");
    assert_eq!(a.workspace.editor().registry.macro_commands().len(), 4);
}

#[test]
fn macro_keys_read_each_file_only_when_that_path_is_first_invoked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Packages/User");
    std::fs::create_dir_all(&root).unwrap();
    let first = root.join("First.sublime-macro");
    let later = root.join("Later.sublime-macro");
    std::fs::write(
        first,
        r#"[{"command":"insert","args":{"characters":"first"}}]"#,
    )
    .unwrap();
    std::fs::write(
        &later,
        r#"[{"command":"insert","args":{"characters":"stale"}}]"#,
    )
    .unwrap();
    let mut a = app("", Some(dir.path()));
    macro_key(
        &mut a,
        "run_macro_file",
        json!({"file":"Packages/User/First.sublime-macro"}),
    );
    let revision = a.workspace.resource_revision;
    // A cache miss on First must not capture Later's contents, even before a reload occurs.
    std::fs::write(
        later,
        r#"[{"command":"insert","args":{"characters":"fresh"}}]"#,
    )
    .unwrap();
    macro_key(
        &mut a,
        "run_macro_file",
        json!({"file":"Packages/User/Later.sublime-macro"}),
    );
    assert_eq!(a.workspace.resource_revision, revision);
    assert_eq!(a.workspace.editor().text.to_string(), "firstfresh");
}
