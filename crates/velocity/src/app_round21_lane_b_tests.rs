use super::*;
use crate::{cli::FileLocation, instance::Request, open_requests::Route, session::Sessions};
use std::{cell::RefCell, path::Path, rc::Rc, time::Duration};
use vel_platform::AppHandler;
use vel_text::Text;

fn fixture(workspace: Workspace, sessions: Rc<RefCell<Sessions>>) -> App {
    let options = Options::default();
    let clock = Clock::new(Instant::now(), std::time::SystemTime::now());
    let reporter = Reporter::for_options(&options, Instant::now()).unwrap();
    App::new(
        options,
        clock,
        reporter,
        workspace,
        Arc::new(AtomicBool::new(false)),
        sessions,
    )
}
fn sessions() -> Rc<RefCell<Sessions>> {
    Rc::new(RefCell::new(Sessions::new(None)))
}
fn location(path: &Path) -> FileLocation {
    FileLocation {
        path: path.into(),
        line: 1,
        column: 1,
    }
}
#[track_caller]
fn spin(mut finish: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(30);
    while !finish() {
        assert!(Instant::now() < until, "fixture did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn two_apps_route_to_existing_folder_project_and_file_without_replacing_dirty_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("shared.txt");
    std::fs::write(&file, "existing").unwrap();
    let project = root.join("fixture.sublime-project");
    std::fs::write(&project, r#"{"folders":[{"path":"."}]}"#).unwrap();
    let sessions = sessions();
    let mut first = fixture(
        Workspace::new(Text::new(), None, Some(project.clone()), false).unwrap(),
        sessions.clone(),
    );
    first.workspace.open(location(&file)).unwrap();
    first
        .workspace
        .editor_mut()
        .run("insert", &json!({"characters":"unsaved "}))
        .unwrap();
    first.publish_open_route();
    let second = fixture(
        Workspace::new(Text::new(), None, None, false).unwrap(),
        sessions,
    );
    second.publish_open_route();
    for path in [&root, &project, &file] {
        let mut request = crate::open_requests::native_request(std::slice::from_ref(path)).unwrap();
        assert_eq!(
            second.route_open(&mut request),
            Route::Window(first.session_id),
            "{path:?}"
        );
    }
    let mut request = Request::new(
        &Options {
            new_window: true,
            ..Options::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        second.route_open(&mut request),
        Route::Window(second.session_id)
    );
    assert!(first.workspace.tabs[first.workspace.active].dirty());
}

#[test]
fn two_apps_route_cli_and_finder_symlink_alias_to_the_original_window() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let folder = root.join("real");
    std::fs::create_dir(&folder).unwrap();
    let link = root.join("linked");
    std::os::unix::fs::symlink(&folder, &link).unwrap();
    let sessions = sessions();
    let mut first = fixture(
        Workspace::new(Text::new(), None, None, false).unwrap(),
        sessions.clone(),
    );
    first
        .panels
        .add_folder(&mut first.workspace, link.clone())
        .unwrap();
    first.publish_open_route();
    assert_eq!(first.workspace.folders[0].path, link);
    let second = fixture(
        Workspace::new(Text::new(), None, None, false).unwrap(),
        sessions,
    );
    second.publish_open_route();
    for (path, native_open) in [(&link, false), (&folder, false), (&link, true)] {
        let mut request = Request::new(
            &Options {
                files: vec![location(path)],
                ..Options::default()
            },
            None,
        )
        .unwrap();
        request.native_open = native_open;
        assert_eq!(
            second.route_open(&mut request),
            Route::Window(first.session_id)
        );
    }
}

#[test]
fn mru_key_dispatch_survives_a_session_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        let path = dir.path().join(name);
        std::fs::write(&path, name).unwrap();
        w.open(location(&path)).unwrap();
    }
    w.select(0);
    w.select(2);
    let (window, documents) = w.snapshot().unwrap();
    let restored: vel_session::Window =
        serde_json::from_slice(&serde_json::to_vec(&window).unwrap()).unwrap();
    let mut target = Workspace::new(Text::new(), None, None, false).unwrap();
    target.restore_window(&restored, &documents).unwrap();
    let mut app = fixture(target, sessions());
    let tab = vel_config::keymap::Key::parse("ctrl+tab").unwrap();
    assert!(app.dispatch_key_binding(None, &tab, Instant::now(), 0..1));
    assert_eq!(app.workspace.tabs[app.workspace.active].name(), "a.txt");
    app.run_named(None, "prev_view_in_stack", &json!({}));
    assert_eq!(app.workspace.tabs[app.workspace.active].name(), "b.txt");
}

#[test]
fn revert_cancel_confirm_and_failed_load_preserve_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.txt");
    std::fs::write(&path, b"caf\xe9\r\n").unwrap();
    let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
    w.open(location(&path)).unwrap();
    w.reopen_encoding("Western (ISO 8859-1)").unwrap();
    let mut app = fixture(w, sessions());
    app.workspace
        .editor_mut()
        .run("insert", &json!({"characters":"unsaved "}))
        .unwrap();
    let text = app.workspace.editor().text.clone();
    app.run_named(None, "revert", &json!({}));
    assert_eq!(app.workspace.editor().text, text);
    app.workspace.revert(|| true).unwrap();
    spin(|| {
        app.workspace.poll();
        !app.workspace.tabs[app.workspace.active].dirty()
    });
    assert_eq!(app.workspace.editor().text.to_string(), "café\n");
    assert_eq!(
        app.workspace.tabs[app.workspace.active].format.line_ending,
        vel_fileio::LineEnding::CrLf
    );
    app.run_named(None, "undo", &json!({}));
    assert_eq!(app.workspace.editor().text.to_string(), "café\n");
    std::fs::remove_file(path).unwrap();
    app.workspace
        .editor_mut()
        .run("insert", &json!({"characters":"keep "}))
        .unwrap();
    let edited = app.workspace.editor().text.clone();
    assert!(app.workspace.revert(|| true).is_err());
    assert_eq!(app.workspace.editor().text, edited);
}

#[test]
fn definition_uses_open_tabs_with_index_disabled_and_current_file_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
    workspace.settings.set(
        vel_config::settings::Layer::User,
        "index_files",
        json!(false),
    );
    let paths = [dir.path().join("a.rs"), dir.path().join("b.rs")];
    for path in &paths {
        std::fs::write(path, "fn foo() {}\nfn caller() { foo(); }\n").unwrap();
        workspace.open(location(path)).unwrap();
    }
    let mut app = fixture(workspace, sessions());
    spin(|| {
        app.workspace.poll();
        app.panels.poll(&mut app.workspace, 0..2);
        app.workspace.tabs.iter().all(|tab| {
            tab.editor
                .language
                .borrow()
                .as_ref()
                .and_then(|l| l.analysis.as_ref())
                .is_some_and(|a| a.index_symbols.complete)
        })
    });
    let call = app
        .workspace
        .editor()
        .text
        .to_string()
        .rfind("foo")
        .unwrap()
        + 1;
    app.workspace.editor_mut().set_caret(call, false);
    app.run_named(None, "goto_definition", &json!({}));
    spin(|| {
        app.panels.poll(&mut app.workspace, 0..2);
        app.panels.overlay == Some(crate::panels::Overlay::Locations)
    });
    assert_eq!(
        app.workspace.tabs[app.workspace.active].path.as_deref(),
        Some(paths[1].canonicalize().unwrap().as_path())
    );
    assert_eq!(
        app.workspace
            .editor()
            .text
            .byte_to_line(app.workspace.editor().caret),
        0
    );
}

#[test]
fn stdin_starts_clean_detects_python_and_writes_legacy_bytes() {
    let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
    let raw = b"#!/usr/bin/env python3\r\n# caf\xe9\r\n";
    workspace.open_stdin(raw).unwrap();
    assert_eq!(
        workspace.editor().text.to_string(),
        "#!/usr/bin/env python3\n# café\n"
    );
    assert_eq!(
        workspace.editor().language.borrow().as_ref().unwrap().name,
        "Python"
    );
    assert!(
        !workspace
            .editor()
            .settings
            .editor()
            .translate_tabs_to_spaces
    );
    workspace.editor_mut().run("undo", &json!({})).unwrap();
    let mut encoded = Vec::new();
    vel_fileio::encode_to(
        &mut encoded,
        &workspace.editor().text,
        &workspace.tabs[workspace.active].format,
    )
    .unwrap();
    assert_eq!(encoded, raw);
}

#[test]
fn cli_command_runs_on_the_opened_document() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("target.txt");
    std::fs::write(&path, "disk").unwrap();
    let options = Options::parse([
        "--command".to_owned(),
        r#"insert {"characters":"first "}"#.to_owned(),
        path.display().to_string(),
    ])
    .unwrap()
    .unwrap();
    let workspace = Workspace::new(Text::from("other buffer"), None, None, false).unwrap();
    let mut app = fixture(workspace, sessions());
    let request = Request::new(&options, None).unwrap();
    app.accept_request(crate::instance::Incoming::local(request))
        .unwrap();
    assert_eq!(app.workspace.editor().text.to_string(), "disk");
    app.poll_cli_commands(None);
    assert_eq!(app.workspace.editor().text.to_string(), "first disk");
    assert_eq!(
        app.workspace.tabs[0].editor.text.to_string(),
        "other buffer"
    );
}

#[test]
fn switch_compound_file_and_round21_menu_rows_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("foo.pb.cc");
    let b = dir.path().join("foo.pb.h");
    for path in [&a, &b] {
        std::fs::write(path, "/* fixture */").unwrap();
    }
    let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
    workspace.open(location(&a)).unwrap();
    let mut app = fixture(workspace, sessions());
    let key = vel_config::keymap::Key::parse("super+alt+up").unwrap();
    assert!(app.dispatch_key_binding(None, &key, Instant::now(), 0..1));
    assert_eq!(app.workspace.tabs[app.workspace.active].name(), "foo.pb.h");
    let menu = app.application_menu();
    for path in [
        &["File", "Revert File"][..],
        &["Edit", "Tag", "Close Tag"],
        &["Edit", "Tag", "Wrap Selection With Tag"],
        &["Tools", "Developer", "Show Scope Name"],
        &["Goto", "Switch File", "Switch Header/Implementation"],
    ] {
        let mut items = menu.as_slice();
        for caption in path {
            let entry = items
                .iter()
                .find(|e| e.caption == *caption)
                .unwrap_or_else(|| panic!("missing {path:?}"));
            items = &entry.children;
            if let Some(command) = &entry.command {
                assert!(
                    app.workspace.editor().registry.kind(command).is_some(),
                    "{command}"
                );
                assert!(
                    app.workspace
                        .palette
                        .iter()
                        .any(|p| p.invocation.command == *command)
                );
            }
        }
    }
    let view = menu.iter().find(|item| item.caption == "View").unwrap();
    let syntax = view
        .children
        .iter()
        .find(|item| item.caption == "Syntax")
        .unwrap();
    assert!(!syntax.children.is_empty());
    assert_eq!(view.children.last().unwrap().caption, "Syntax");
    let edit = menu.iter().find(|item| item.caption == "Edit").unwrap();
    assert_eq!(edit.children.first().unwrap().caption, "Undo");
    assert_eq!(edit.children.last().unwrap().caption, "Tag");
    let tools = menu.iter().find(|item| item.caption == "Tools").unwrap();
    assert_eq!(tools.children.last().unwrap().caption, "Developer");
    for name in [
        "prompt_add_folder",
        "prompt_select_project",
        "open_recent_project",
        "clear_recent_files",
        "clear_recent_projects",
        "hide_overlay",
        "exit",
    ] {
        assert!(
            app.workspace.editor().registry.kind(name).is_some(),
            "{name}"
        );
    }
    assert!(app.command_enabled("revert", "{}"));
}
