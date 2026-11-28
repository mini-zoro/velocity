use super::*;
use crate::{
    cli::FileLocation,
    instance::{Incoming, Request},
    open_requests::Route,
    session::Sessions,
};
use std::{cell::RefCell, fs, path::Path, rc::Rc};
use vel_config::settings::Layer;
use vel_text::Text;

type Profile = Rc<RefCell<Sessions>>;

fn sessions(root: &Path) -> Profile {
    Rc::new(RefCell::new(Sessions::new(Some(root))))
}

fn app(root: &Path, profile: &Profile, project: Option<PathBuf>) -> App {
    let options = Options::default();
    App::new(
        options.clone(),
        Clock::new(Instant::now(), std::time::SystemTime::now()),
        Reporter::for_options(&options, Instant::now()).unwrap(),
        Workspace::new(Text::new(), Some(root.into()), project, false).unwrap(),
        Arc::new(AtomicBool::new(false)),
        profile.clone(),
    )
}

fn location(path: &Path) -> FileLocation {
    FileLocation {
        path: path.into(),
        line: 1,
        column: 1,
    }
}

fn fixture_file(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, name).unwrap();
    path.canonicalize().unwrap()
}

fn session_path(root: &Path) -> PathBuf {
    root.join("Sessions/Session.sublime-session")
}

fn restored_app(root: &Path, profile: &Profile, window: &vel_session::Window, id: u64) -> App {
    let mut app = app(root, profile, window.project.clone());
    app.workspace
        .restore_window(window, &profile.borrow().restored.documents)
        .unwrap();
    app.restore_session_id(id);
    app.restore_ui(window);
    app
}

fn cmd_w(app: &mut App) {
    assert!(app.dispatch_key_binding(
        None,
        &vel_config::keymap::Key::parse("super+w").unwrap(),
        Instant::now(),
        0..0,
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_window_close_lifecycle_drops_the_editor_when_it_becomes_windowless() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let profile = sessions(root);
    let mut editor = app(root, &profile, None);
    let file = fixture_file(root, "closed.txt");
    editor.workspace.open(location(&file)).unwrap();
    editor.publish_open_route();
    let closed_id = editor.session_id;
    let editor_lifetime = Arc::downgrade(&editor.done);

    assert!(editor.prepare_window_close(None));
    let handler: Box<dyn AppHandler> = Box::new(editor);
    let template = handler.into_windowless().unwrap();

    assert!(editor_lifetime.upgrade().is_none());
    let profile = profile.borrow();
    assert!(!profile.is_live(closed_id));
    assert!(!profile.open_windows.contains_key(&closed_id));
    assert!(!profile.handles.contains_key(&closed_id));
    assert_eq!(profile.recent_items()[0], vec![file]);
    drop(template);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_window_close_lifecycle_builds_a_fresh_editor_and_settings_window() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let profile = sessions(root);
    let mut editor = app(root, &profile, None);
    let file = fixture_file(root, "closed.txt");
    editor.workspace.open(location(&file)).unwrap();
    assert!(editor.prepare_window_close(None));
    let template = windowless::Controller::new(&editor);
    let closed_id = editor.session_id;
    drop(editor);

    let mut commands = template.command_app().unwrap();
    assert!(commands.renderer.is_none() && commands.fonts.is_none());
    assert_eq!(commands.workspace.editor().text, "");
    assert!(commands.workspace.tabs.iter().all(|tab| tab.path.is_none()));
    assert!(!profile.borrow().has_live_windows());

    commands.open_settings_window(None).unwrap();
    assert!(commands.opened_preferences[0].settings_window);
    assert!(!profile.borrow().has_live_windows());
    let mut fresh = commands.window_app(None, None, true).unwrap();
    assert_ne!(fresh.session_id, closed_id);
    assert_eq!(fresh.workspace.editor().text, "");
    assert_eq!(fresh.workspace.recent_files, vec![file]);
    assert!(profile.borrow().is_live(fresh.session_id));
    assert!(!profile.borrow().is_live(closed_id));
    assert!(fresh.prepare_window_close(None));
}

#[test]
fn r32_cmd_w_closes_anonymous_window_without_quitting_or_restoring_it() {
    for hot in ["always", "only_on_quit", "disabled"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let profile = sessions(root);
        let mut a = app(root, &profile, None);
        a.workspace
            .settings
            .set(Layer::User, "hot_exit", json!(hot));
        a.workspace
            .open(location(&fixture_file(root, "closed.txt")))
            .unwrap();
        a.checkpoint(false).unwrap();
        let old_id = a.session_id;

        cmd_w(&mut a);
        assert!(a.empty_close_requested && a.workspace.is_empty(), "{hot}");
        // Complete the requested native close without running a native loop.
        assert!(a.prepare_window_close(None), "{hot}");
        assert!(!a.done.load(std::sync::atomic::Ordering::Relaxed));
        assert!(profile.borrow().active_window.is_none());
        profile.borrow().flush().unwrap();
        let disk = vel_session::load(&session_path(root)).session;
        assert!(
            disk.windows.is_empty() && disk.documents.is_empty(),
            "{hot}"
        );

        // Dock reopen and File > New Window use the same builder with different
        // new-window settings policies.
        for new_window in [false, true] {
            let mut fresh = a.window_app(None, None, new_window).unwrap();
            assert_ne!(fresh.session_id, old_id);
            assert_eq!(fresh.workspace.tabs.len(), 1);
            assert!(fresh.workspace.tabs[0].path.is_none());
            assert!(fresh.workspace.folders.is_empty());
            assert!(fresh.workspace.project_path().is_none());
            assert_eq!(fresh.workspace.editor().text, "");
            assert!(fresh.prepare_window_close(None));
        }
    }
}

#[test]
fn r32_cmd_w_default_keeps_project_windows_and_prompts_before_closing_dirty_tab() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("empty.sublime-project");
    fs::write(&path, "{}").unwrap();
    let mut named = app(root, &sessions(root), Some(path.clone()));
    cmd_w(&mut named);
    assert!(named.workspace.is_empty() && !named.empty_close_requested);

    fs::write(&path, r#"{"folders":[{"path":"."}]}"#).unwrap();
    let mut folders = app(root, &sessions(root), None);
    folders
        .panels
        .add_folder(&mut folders.workspace, root.to_owned())
        .unwrap();
    assert!(folders.workspace.project_path().is_none());
    cmd_w(&mut folders);
    assert!(folders.workspace.is_empty() && !folders.empty_close_requested);

    let mut dirty = app(root, &sessions(root), None);
    dirty.run_named(None, "insert", &json!({"characters":"unsaved"}));
    dirty
        .close_decisions
        .push_back(tab_adapter::CloseDecision::Cancel);
    cmd_w(&mut dirty);
    assert!(dirty.close_decisions.is_empty());
    assert!(!dirty.empty_close_requested);
    assert_eq!(dirty.workspace.editor().text, "unsaved");
    dirty
        .close_decisions
        .push_back(tab_adapter::CloseDecision::Discard);
    cmd_w(&mut dirty);
    assert!(dirty.close_decisions.is_empty());
    assert!(dirty.empty_close_requested && dirty.workspace.is_empty());
}

#[test]
fn closing_clean_window_then_quitting_only_restores_survivor_and_keeps_reopen_path() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let closed = fixture_file(root, "closed.txt");
    let survivor = fixture_file(root, "survivor.txt");
    let profile = sessions(root);
    let mut a = app(root, &profile, None);
    let mut b = app(root, &profile, None);
    a.workspace.open(location(&closed)).unwrap();
    b.workspace.open(location(&survivor)).unwrap();
    a.checkpoint(false).unwrap();
    b.checkpoint(false).unwrap();
    assert!(a.prepare_window_close(None));
    assert!(b.prepare_quit(None));
    drop((a, b, profile));

    let profile = sessions(root);
    let saved = profile.borrow().restored.clone();
    assert_eq!(saved.windows.len(), 1);
    assert_eq!(saved.documents.len(), 1);
    assert_eq!(saved.documents[0].path.as_ref(), Some(&survivor));
    assert!(saved.recent_files.contains(&closed));
    let ids = profile.borrow_mut().reserve_recovered();
    let mut b = restored_app(root, &profile, &saved.windows[0], ids[0]);
    assert_eq!(b.workspace.tabs.len(), 1);
    assert!(b.command_enabled_args("reopen_last_file", &json!({})));
    b.run_named(None, "reopen_last_file", &json!({}));
    assert_eq!(b.workspace.tabs.len(), 2);
    assert_eq!(
        b.workspace.tabs[b.workspace.active].path.as_ref(),
        Some(&closed)
    );
    assert_eq!(fs::read_to_string(&closed).unwrap(), "closed.txt");
}

#[test]
fn dirty_window_cancels_and_saves_before_closing_with_hot_exit() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let profile = sessions(root);
    let mut a = app(root, &profile, None);
    a.run_named(None, "insert", &json!({"characters":"unsaved text"}));
    a.checkpoint(false).unwrap();
    a.close_decisions
        .push_back(tab_adapter::CloseDecision::Cancel);
    assert!(!a.prepare_window_close(None));
    assert_eq!(a.workspace.editor().text, "unsaved text");
    assert_eq!(profile.borrow().test_snapshot().windows.len(), 1);
    assert!(!profile.borrow().has_closed_window_files());

    let path = root.join("saved.txt");
    a.close_decisions
        .push_back(tab_adapter::CloseDecision::Save(Some(path.clone())));
    assert!(a.prepare_window_close(None));
    assert!(a.close_decisions.is_empty());
    assert_eq!(fs::read_to_string(path).unwrap(), "unsaved text");
    let saved = vel_session::load(&session_path(root)).session;
    assert!(saved.windows.is_empty());
    assert!(saved.documents.is_empty());
}

#[test]
fn closing_last_window_leaves_nothing_to_restore_in_each_hot_exit_mode() {
    for hot in ["always", "only_on_quit", "disabled"] {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let profile = sessions(root);
        let mut a = app(root, &profile, None);
        a.workspace
            .settings
            .set(Layer::User, "hot_exit", json!(hot));
        let disk = fixture_file(root, "keep.txt");
        a.workspace.open(location(&disk)).unwrap();
        a.run_named(None, "insert", &json!({"characters":"unsaved edit"}));
        a.checkpoint(false).unwrap();
        a.close_decisions
            .push_back(tab_adapter::CloseDecision::Cancel);
        assert!(!a.prepare_window_close(None), "{hot}");
        a.close_decisions
            .push_back(tab_adapter::CloseDecision::Discard);
        assert!(a.prepare_window_close(None), "{hot}");
        // Native Quit visits live handlers only, so a windowless Quit cannot
        // republish the closed template.
        assert!(profile.borrow().active_window.is_none());
        profile.borrow().flush().unwrap();
        drop((a, profile));
        let restored = sessions(root);
        let saved = &restored.borrow().restored;
        assert!(saved.windows.is_empty(), "{hot}");
        assert!(saved.documents.is_empty(), "{hot}");
        assert_eq!(saved.closed_window_files, std::slice::from_ref(&disk));
        assert_eq!(fs::read_to_string(disk).unwrap(), "keep.txt");
    }
}

#[test]
fn project_close_keeps_unsaved_content_in_the_project_workspace_only() {
    for hot in ["always", "only_on_quit"] {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let profile = sessions(root);
        let project = root.join("saved.sublime-project");
        fs::write(&project, "{}").unwrap();
        let mut a = app(root, &profile, Some(project.clone()));
        a.workspace
            .settings
            .set(Layer::User, "hot_exit", json!(hot));
        a.run_named(
            None,
            "insert",
            &json!({"characters":"unsaved project text"}),
        );
        assert!(a.prepare_window_close(None));
        let saved = vel_session::load(&session_path(root)).session;
        assert!(saved.windows.is_empty());
        assert!(saved.documents.is_empty());
        let saved_project = vel_session::load_project_workspace(&project, Some(root)).session;
        assert_eq!(saved_project.windows.len(), 1);
        assert_eq!(
            saved_project.documents[0]
                .history
                .as_ref()
                .unwrap()
                .current_text()
                .unwrap(),
            "unsaved project text"
        );
    }
}

#[test]
fn legacy_ambiguous_windows_restore_once_and_can_be_closed_without_losing_data() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let clean = fixture_file(root, "old-closed.txt");
    let profile = sessions(root);
    let mut a = app(root, &profile, None);
    let mut b = app(root, &profile, None);
    a.run_named(
        None,
        "insert",
        &json!({"characters":"keep old unsaved text"}),
    );
    b.workspace.open(location(&clean)).unwrap();
    a.checkpoint(false).unwrap();
    b.checkpoint(false).unwrap();
    profile.borrow().flush().unwrap();
    let snapshot = profile.borrow().test_snapshot();
    drop((a, b, profile));

    // Old builds serialized the very same window shape for open and closed
    // windows. Neither the order nor dirtiness can identify closed windows.
    let mut legacy = serde_json::to_value(snapshot).unwrap();
    for window in legacy["windows"].as_array_mut().unwrap() {
        window.as_object_mut().unwrap().remove("retained_closed");
    }
    fs::write(session_path(root), serde_json::to_vec(&legacy).unwrap()).unwrap();
    let profile = sessions(root);
    let ids = profile.borrow_mut().reserve_recovered();
    assert_eq!(ids.len(), 2);
    let windows = profile.borrow().restored.windows.clone();
    let mut a = restored_app(root, &profile, &windows[0], ids[0]);
    let mut b = restored_app(root, &profile, &windows[1], ids[1]);
    assert_eq!(a.workspace.editor().text, "keep old unsaved text");
    assert!(b.prepare_window_close(None));
    assert!(a.prepare_quit(None));
    drop((a, b, profile));
    let reloaded = vel_session::load(&session_path(root)).session;
    assert_eq!(reloaded.windows.len(), 1);
    assert_eq!(reloaded.documents.len(), 1);
    assert_eq!(
        reloaded.documents[0]
            .history
            .as_ref()
            .unwrap()
            .current_text()
            .unwrap(),
        "keep old unsaved text"
    );
    assert_eq!(fs::read_to_string(clean).unwrap(), "old-closed.txt");
}

#[test]
fn restore_queues_one_window_per_snapshot_and_routes_file_requests_before_it_opens() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let profile = sessions(root);
    let files: Vec<_> = (0..3)
        .map(|i| {
            [
                fixture_file(root, &format!("{i}-a.txt")),
                fixture_file(root, &format!("{i}-b.txt")),
            ]
        })
        .collect();
    for [left, right] in &files {
        let mut a = app(root, &profile, None);
        a.workspace.open(location(left)).unwrap();
        a.workspace
            .workbench
            .set_layout(vel_ui::workbench::Layout::grid(2, 1))
            .unwrap();
        a.workspace.workbench.active = 1;
        a.workspace.open(location(right)).unwrap();
        assert!(a.prepare_quit(None));
    }
    drop(profile);
    let profile = sessions(root);
    let saved = profile.borrow().restored.clone();
    let ids = profile.borrow_mut().reserve_recovered();
    let mut first = restored_app(root, &profile, &saved.windows[0], ids[0]);
    first.restore_remaining_reserved(saved.windows.clone(), ids.clone(), Some(0));
    first.publish_open_route();

    let launch = Options {
        files: vec![location(&files[1][0])],
        ..Options::default()
    };
    let mut cli = Request::new(&launch, None).unwrap();
    assert_eq!(
        crate::open_requests::restored_window(&saved, &launch),
        Some(1)
    );
    assert_eq!(first.route_open(&mut cli), Route::Window(ids[1]));
    #[cfg(unix)]
    let client = {
        use crate::instance::{Instance, Start};
        let primary = Request::new(&Options::default(), None).unwrap();
        let Start::Primary(instance) = Instance::start_fixture(root, &primary).unwrap() else {
            panic!("fixture failed to claim its primary instance");
        };
        let root = root.to_owned();
        let client = std::thread::spawn(move || {
            assert!(matches!(
                Instance::start_fixture(&root, &cli).unwrap(),
                Start::Forwarded
            ));
        });
        let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
        loop {
            if let Some(incoming) = instance.drain().pop() {
                assert!(first.route_or_queue_open(incoming).unwrap().is_none());
                break;
            }
            assert!(Instant::now() < deadline, "forwarded request not delivered");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        first.set_instance(Some(instance));
        client
    };
    #[cfg(not(unix))]
    assert!(
        first
            .route_or_queue_open(Incoming::local(cli))
            .unwrap()
            .is_none()
    );
    // Finder also routes to the reserved ID; promotion must not accidentally
    // load the file into the first window while its own window is pending.
    first.open_received_paths(&[files[2][0].clone()]).unwrap();
    let new_files = [
        fixture_file(root, "incoming-a.txt"),
        fixture_file(root, "incoming-b.txt"),
    ];
    let launch = Options {
        files: new_files.iter().map(|path| location(path)).collect(),
        ..Options::default()
    };
    let incoming = Incoming::local(Request::new(&launch, None).unwrap());
    let (route, incoming) = first.route_or_queue_open(incoming).unwrap().unwrap();
    assert_eq!(route, Route::Window(ids[0]));
    first.accept_request(incoming).unwrap();
    assert_eq!(first.workspace.tabs.len(), 4);

    // Exercise the same restore plan consumed by poll_workbench, without creating
    // AppKit windows. Deliver each pending inbox after its controller is ready.
    let pending = std::mem::take(&mut first.restored_windows);
    assert_eq!(
        pending.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
        ids[1..]
    );
    for (window, id) in pending {
        let mut a = restored_app(root, &profile, &window, id);
        let requests = profile.borrow_mut().open_inbox.remove(&id).unwrap();
        assert_eq!(requests.len(), 1);
        for incoming in requests {
            a.accept_request(incoming).unwrap();
        }
        let workbench = &a.workspace.workbench;
        assert_eq!(a.workspace.tabs.len(), 2);
        assert_eq!(workbench.groups.len(), 2);
        assert!(workbench.groups.iter().all(|group| group.sheets.len() == 1));
    }
    #[cfg(unix)]
    client.join().unwrap();
    assert_eq!(profile.borrow().test_snapshot().windows.len(), 3);
}
