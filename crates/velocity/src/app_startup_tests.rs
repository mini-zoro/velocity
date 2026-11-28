//! Exercise the first action before the native post-present hook can run.
use super::*;
use crate::{
    cli::FileLocation,
    instance::{Incoming, Request},
    session::Sessions,
};
use std::{cell::RefCell, fs, rc::Rc};
use vel_config::keymap::Key;
use vel_text::Text;

fn first_action_app(root: &Path, path: Option<&Path>, initialize_first: bool) -> App {
    let options = Options::default();
    let mut workspace = Workspace::new_for_startup(
        Text::new(),
        Some(root.to_owned()),
        None,
        true,
        &mut Default::default(),
    )
    .unwrap();
    if let Some(path) = path {
        workspace
            .open(FileLocation::path_only(path.into()))
            .unwrap();
    }
    let started = Instant::now();
    let mut app = App::new(
        options.clone(),
        Clock::new(started, std::time::SystemTime::now()),
        Reporter::for_options(&options, started).unwrap(),
        workspace,
        Arc::new(AtomicBool::new(false)),
        Rc::new(RefCell::new(Sessions::new_for_startup(Some(root)))),
    );
    assert!(app.startup_services_pending && app.workspace.palette_pending());
    assert!(app.workspace.watcher.is_none() && app.sessions.borrow().startup_writer_pending());
    if initialize_first {
        app.finish_startup_services("post_present_begin").unwrap();
        assert_ready(&app);
    }
    app
}

fn assert_ready(app: &App) {
    assert!(!app.startup_services_pending && !app.workspace.palette_pending());
    assert!(app.workspace.watcher.is_some() && !app.sessions.borrow().startup_writer_pending());
    assert!(!app.workspace.palette.is_empty());
}

fn fixture_file(root: &Path) -> PathBuf {
    let path = root.join("fixture.txt");
    fs::write(&path, "one\ntwo\n").unwrap();
    path.canonicalize().unwrap()
}

fn preferences_path(root: &Path) -> PathBuf {
    root.join("Packages/User/Preferences.sublime-settings")
}

#[test]
fn first_cmd_w_honors_close_windows_when_empty() {
    for initialize_first in [false, true] {
        for closes in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = preferences_path(dir.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, json!({"close_windows_when_empty":closes}).to_string()).unwrap();
            let mut app = first_action_app(dir.path(), None, initialize_first);
            assert!(app.workspace.is_empty());
            assert!(app.dispatch_key_binding(
                None,
                &Key::parse("super+w").unwrap(),
                Instant::now(),
                0..0,
            ));
            assert!(app.workspace.is_empty());
            assert_eq!(app.empty_close_requested, closes);
            assert_ready(&app);
        }
    }
}

#[test]
fn first_window_close_and_quit_persist_the_right_session() {
    for initialize_first in [false, true] {
        for quit in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = fixture_file(dir.path());
            let mut app = first_action_app(dir.path(), Some(&path), initialize_first);
            assert!(if quit {
                app.prepare_quit(None)
            } else {
                app.prepare_window_close(None)
            });
            assert_ready(&app);
            let saved = vel_session::load(&dir.path().join("Sessions/Session.sublime-session"));
            assert!(saved.messages.is_empty(), "{:?}", saved.messages);
            assert_eq!(saved.session.windows.len(), usize::from(quit));
            if quit {
                assert_eq!(saved.session.documents[0].path.as_ref(), Some(&path));
            } else {
                assert!(saved.session.documents.is_empty());
                assert!(saved.session.closed_window_files.contains(&path));
            }
        }
    }
}

#[test]
fn first_menu_command_and_palette_query_resolve_normally() {
    fn menu_item(items: &[MenuEntry], command: &str) -> Option<MenuEntry> {
        items.iter().find_map(|item| {
            if item.command.as_deref() == Some(command) {
                Some(item.clone())
            } else {
                menu_item(&item.children, command)
            }
        })
    }
    for initialize_first in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = first_action_app(dir.path(), None, initialize_first);
        let item = menu_item(&app.application_menu(), "new_file").unwrap();
        assert_eq!(app.startup_services_pending, !initialize_first);
        app.run_named_depth(
            None,
            "new_file",
            &serde_json::from_str(&item.args).unwrap(),
            0,
        )
        .unwrap();
        assert_eq!(app.workspace.tabs.len(), 1);
        assert!(!app.workspace.is_empty());
        assert_ready(&app);

        let dir = tempfile::tempdir().unwrap();
        let mut app = first_action_app(dir.path(), None, initialize_first);
        app.run_named_depth(
            None,
            "show_overlay",
            &json!({"overlay":"command_palette","text":"Open Crash Reports"}),
            0,
        )
        .unwrap();
        assert_ready(&app);
        let (_, invocation) = app
            .panels
            .key(&mut app.workspace, &Key::parse("enter").unwrap(), 0..4)
            .unwrap();
        assert_eq!(invocation.unwrap().command, "open_crash_reports");
    }
}

#[test]
fn first_finder_and_cli_opens_select_the_requested_document() {
    for initialize_first in [false, true] {
        for finder in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = fixture_file(dir.path());
            let mut app = first_action_app(dir.path(), None, initialize_first);
            if finder {
                app.open_received_paths(std::slice::from_ref(&path))
                    .unwrap();
            } else {
                let options = Options {
                    files: vec![FileLocation {
                        path: path.clone(),
                        line: 2,
                        column: 2,
                    }],
                    ..Options::default()
                };
                app.accept_request(Incoming::local(Request::new(&options, None).unwrap()))
                    .unwrap();
                assert_eq!(app.workspace.editor().selection(), 5..5);
            }
            assert_eq!(
                app.workspace.tabs[app.workspace.active].path.as_ref(),
                Some(&path)
            );
            assert_eq!(app.workspace.editor().text, "one\ntwo\n");
            assert_ready(&app);
        }
    }
}

#[test]
fn first_settings_change_applies_and_persists() {
    for initialize_first in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = first_action_app(dir.path(), None, initialize_first);
        app.run_named_depth(None, "set_ui_language", &json!({"language":"zh-Hans"}), 0)
            .unwrap();
        assert_eq!(
            app.workspace.translations.matched_language().as_str(),
            "zh-Hans"
        );
        assert_ready(&app);
        app.workspace.wait_resources();
        let saved = fs::read_to_string(preferences_path(dir.path())).unwrap();
        assert!(saved.contains("zh-Hans"), "{saved}");
    }
}

#[test]
fn headless_poll_and_scene_finish_startup_without_a_native_present() {
    for poll in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = first_action_app(dir.path(), None, false);
        app.render_scene();
        assert!(app.startup_services_pending && app.workspace.palette_pending());
        let revision = app.workspace.palette_revision;
        if poll {
            assert!(app.poll_workspace_model());
        } else {
            app.scene();
        }
        assert_ready(&app);
        assert_eq!(app.workspace.palette_revision, revision + 1);
        assert!(!app.ensure_startup_services(), "only publish once");
    }
}

#[test]
fn a_later_interactive_window_shares_the_first_windows_syntax_assets() {
    let dir = tempfile::tempdir().unwrap();
    let first = Workspace::new_for_startup(
        Text::new(),
        Some(dir.path().to_owned()),
        None,
        true,
        &mut Default::default(),
    )
    .unwrap();
    if !crate::language::Assets::packed_ready() {
        return; // A source archive built without the syntax packages has no pack.
    }
    // One syntax set and one preference set per process: the compiled contexts,
    // regex machines and metadata caches are shared, not built again per window.
    let second = Workspace::new_window(Some(dir.path().to_owned()), true).unwrap();
    assert!(Arc::ptr_eq(&first.assets.syntaxes, &second.assets.syntaxes));
    assert!(Arc::ptr_eq(&first.assets.metadata, &second.assets.metadata));
    let settings = first.for_settings_window().unwrap();
    assert!(Arc::ptr_eq(
        &first.assets.syntaxes,
        &settings.assets.syntaxes
    ));
    // Headless and test workspaces keep the eager loader.
    let unwatched = Workspace::new_window(Some(dir.path().to_owned()), false).unwrap();
    assert!(!Arc::ptr_eq(
        &first.assets.syntaxes,
        &unwatched.assets.syntaxes
    ));
}

#[test]
fn startup_cli_requests_open_before_the_first_frame_and_leave_services_deferred() {
    use crate::open_requests::Route;
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_file(dir.path());
    let second = dir.path().join("second.txt");
    fs::write(&second, "second\n").unwrap();
    let second = second.canonicalize().unwrap();
    let request = |path: &Path, new_window: bool| {
        let options = Options {
            files: vec![FileLocation {
                path: path.to_owned(),
                line: 2,
                column: 2,
            }],
            new_window,
            ..Options::default()
        };
        Request::new(&options, None).unwrap()
    };
    let mut app = first_action_app(dir.path(), None, false);
    // The CLI that launched the GUI waits for this confirmation; such requests are
    // routed to ready windows only.
    #[cfg(unix)]
    let (incoming, _client) = {
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        let incoming =
            Incoming::confirmed_fixture(request(&path, false), server, Duration::from_secs(30));
        (incoming, client)
    };
    #[cfg(not(unix))]
    let incoming = Incoming::local(request(&path, false));
    assert!(app.accept_startup_request(incoming));
    assert_eq!(
        app.workspace.tabs[app.workspace.active].path.as_ref(),
        Some(&path)
    );
    assert_eq!(app.workspace.editor().text, "one\ntwo\n");
    assert_eq!(app.workspace.editor().selection(), 5..5);
    let title = app.window_title();
    assert!(title.contains("fixture.txt"), "{title}");
    // The first frame does not start services; the watcher reconciles the file later.
    assert!(app.startup_services_pending && app.workspace.palette_pending());
    assert!(app.workspace.watcher.is_none());
    assert!(app.startup_requests.is_empty());

    // A new window opens once this one runs, and later requests keep their order.
    assert!(!app.accept_startup_request(Incoming::local(request(&second, true))));
    assert!(!app.accept_startup_request(Incoming::local(request(&path, false))));
    assert_eq!(app.startup_requests.len(), 2);
    assert_eq!(app.startup_requests[0].0, Route::New);
    assert_eq!(app.workspace.tabs.len(), 1);
    assert!(app.startup_services_pending);

    // After startup the queue is no longer used.
    app.startup_done = true;
    assert!(!app.accept_startup_requests());
}

#[test]
fn startup_input_ends_the_scanout_wait() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = first_action_app(dir.path(), None, false);
    app.note_startup_input();
    assert!(app.startup_interrupted);
    let other = tempfile::tempdir().unwrap();
    let mut done = first_action_app(other.path(), None, false);
    done.startup_done = true;
    done.note_startup_input();
    assert!(!done.startup_interrupted);
}

#[test]
fn the_first_frame_waits_for_the_highlighting_of_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("first.rs");
    let body: String = (0..400)
        .map(|i| format!("fn item_{i}(value: usize) -> usize {{ value.wrapping_mul({i}) }}\n"))
        .collect();
    fs::write(&path, body).unwrap();
    let mut app = first_action_app(dir.path(), Some(&path), false);
    // Generous: this only bounds a stuck highlighter in a loaded test run.
    let deadline = Instant::now() + Duration::from_secs(20);
    app.wait_for_startup_highlight(deadline, true);
    {
        let cell = app.workspace.editor().language.clone();
        let slot = cell.borrow();
        let language = slot.as_ref().expect("a language for a file");
        assert!(language.requested_rows_styled(), "{}", language.readiness());
        assert!(language.styled.contains_key(&0));
    }
    // The scene's own request is a subset of the rows already styled: it draws them
    // styled, and the second check neither waits nor rebuilds.
    app.render_scene();
    assert!(!app.wait_for_startup_highlight(deadline, false));
    assert!(
        app.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .requested_rows_styled()
    );
    assert!(app.startup_services_pending, "the wait starts no services");
}
