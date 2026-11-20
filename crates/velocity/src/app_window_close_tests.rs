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
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
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
