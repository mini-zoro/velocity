use super::*;
use std::{cell::RefCell, rc::Rc};
use vel_config::keymap::Key;
use vel_platform::accessibility::Role;
use vel_text::Text;

type Sessions = Rc<RefCell<crate::session::Sessions>>;

fn startup(data: Option<&Path>, project: Option<PathBuf>) -> Workspace {
    Workspace::new_for_startup(
        Text::new(),
        data.map(Path::to_owned),
        project,
        false,
        &mut Default::default(),
    )
    .unwrap()
}

fn app(workspace: Workspace, sessions: Sessions) -> App {
    let options = Options::default();
    let now = Instant::now();
    let reporter = Reporter::for_options(&options, now).unwrap();
    App::new(
        options,
        Clock::new(now, std::time::SystemTime::now()),
        reporter,
        workspace,
        Arc::new(AtomicBool::new(false)),
        sessions,
    )
}

fn sessions(data: Option<&Path>) -> Sessions {
    Rc::new(RefCell::new(crate::session::Sessions::new(data)))
}

fn key(app: &mut App, chord: &str) {
    assert!(app.dispatch_key_binding(None, &Key::parse(chord).unwrap(), Instant::now(), 0..0));
}

fn assert_tabless_editor(app: &mut App) {
    let scene = app.scene();
    assert!(app.tab_hits.is_empty());
    assert!(
        app.workspace
            .workbench
            .groups
            .iter()
            .all(|g| g.sheets.is_empty())
    );
    assert!(app.workspace.editor().text.is_empty());
    assert!(
        scene
            .layers
            .iter()
            .flat_map(|layer| &layer.text)
            .any(|run| run.text == "1")
    );
    let nodes = app.accessible_tree().nodes;
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::TextArea && n.editable && n.frame.width > 0.)
    );
    assert!(
        !nodes
            .iter()
            .any(|n| matches!(n.role, Role::Tab | Role::TabGroup))
    );
}

#[test]
fn r41_fresh_input_is_not_a_document_until_edited() {
    let mut app = app(startup(None, None), sessions(None));
    let input = app.workspace.tabs[0].sheet;
    assert!(!app.panels.side_visible);
    assert!(app.panels.panel.is_none() && app.panels.overlay.is_none());
    assert_tabless_editor(&mut app);
    let window = app
        .application_menu()
        .into_iter()
        .find(|entry| entry.id == "window")
        .unwrap();
    assert!(
        !window
            .children
            .iter()
            .any(|entry| entry.caption.eq_ignore_ascii_case("untitled")
                || entry.command.as_deref() == Some("select_by_index"))
    );
    for name in [
        "next_view",
        "prev_view",
        "next_view_in_stack",
        "prev_view_in_stack",
    ] {
        app.run_named(None, name, &json!({}));
        assert_eq!(app.workspace.tabs[0].sheet, input);
        assert!(app.workspace.is_empty());
    }
    key(&mut app, "super+p");
    assert_eq!(app.panels.overlay, Some(crate::panels::Overlay::Goto));
    assert!(app.panels.quick.items.is_empty());
    key(&mut app, "escape");
    let (window, docs) = app.capture_session_window().unwrap();
    assert!(window.sheets.is_empty() && docs.is_empty());

    app.handle_text_input(
        None,
        TextInput::Insert {
            text: "first edit".into(),
            replacement: None,
        },
    );
    app.scene();
    assert_eq!(app.tab_hits.len(), 1);
    assert_eq!(app.tab_hits[0].0, input);
    assert!(
        app.accessible_tree()
            .nodes
            .iter()
            .any(|n| n.role == Role::Tab)
    );
    let (window, docs) = app.capture_session_window().unwrap();
    assert_eq!((window.sheets.len(), docs.len()), (1, 1));
    let mut restored = startup(None, None);
    restored.restore_window(&window, &docs).unwrap();
    assert!(!restored.is_empty());
    assert_eq!(restored.editor().text, "first edit");
}

#[test]
fn r41_open_files_and_folders_reuse_the_startup_input() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project-root");
    std::fs::create_dir(&root).unwrap();
    let files = ["a.rs", "zh.md", "b.txt"].map(|name| {
        let file = root.join(name);
        std::fs::write(&file, "text\n").unwrap();
        file
    });

    for count in [1, files.len()] {
        let mut app = app(startup(None, None), sessions(None));
        app.open_received_paths(&files[..count]).unwrap();
        app.scene();
        assert!(!app.panels.side_visible);
        assert_eq!(app.tab_hits.len(), count);
        assert!(app.workspace.tabs.iter().all(|tab| tab.path.is_some()));
    }
    let mut folder = app(startup(None, None), sessions(None));
    folder
        .open_received_paths(std::slice::from_ref(&root))
        .unwrap();
    assert!(folder.panels.side_visible);
    assert_eq!(folder.workspace.window_title(), "untitled — project-root");
    assert_tabless_editor(&mut folder);
    folder.open_received_paths(&files[..1]).unwrap();
    folder.scene();
    assert_eq!(folder.tab_hits.len(), 1);
    assert!(folder.workspace.tabs[0].path.is_some());

    let project = dir.path().join("project.sublime-project");
    std::fs::write(&project, r#"{"folders":[{"path":"project-root"}]}"#).unwrap();
    let mut project_app = app(startup(None, Some(project)), sessions(None));
    assert!(project_app.panels.side_visible);
    assert_tabless_editor(&mut project_app);
}

#[test]
fn r41_new_file_and_close_last_tab_keep_the_empty_editor_editable() {
    let mut app = app(startup(None, None), sessions(None));
    let reuse_startup = |app: &App| {
        app.publish_open_route();
        app.sessions.borrow().open_windows[&app.session_id].untouched
    };
    assert!(reuse_startup(&app));
    app.workspace
        .apply_user_preference("close_windows_when_empty", Some(json!(false)));
    key(&mut app, "super+n");
    app.scene();
    assert!(!app.workspace.is_empty());
    assert_eq!(app.tab_hits.len(), 1);
    assert!(!reuse_startup(&app));
    let (window, docs) = app.capture_session_window().unwrap();
    assert_eq!((window.sheets.len(), docs.len()), (1, 1));
    key(&mut app, "super+w");
    assert!(!app.empty_close_requested);
    assert_tabless_editor(&mut app);
    app.workspace
        .apply_user_preference("close_windows_when_empty", Some(json!(true)));
    key(&mut app, "super+w");
    assert!(app.empty_close_requested);
}

#[test]
fn r41_macro_can_edit_the_tabless_editor() {
    let mut app = app(startup(None, None), sessions(None));
    app.run_named(
        None,
        "run_macro",
        &json!({"commands": [
            {"command": "insert", "args": {"characters": "first"}},
            {"command": "insert", "args": {"characters": " edit"}}
        ]}),
    );
    assert_eq!(app.workspace.editor().text, "first edit");
    app.scene();
    assert_eq!(app.tab_hits.len(), 1);
}

#[test]
fn r41_sidebar_auto_and_per_window_overrides_survive_restore() {
    let profile = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = crate::workspace::absolute_folder(project.path()).unwrap();
    let live = sessions(Some(profile.path()));
    let mut states = [
        app(startup(None, None), live.clone()),
        app(startup(None, None), live.clone()),
        app(startup(None, None), live.clone()),
    ];
    let [automatic, shown, hidden] = &mut states;
    automatic
        .panels
        .add_folder(&mut automatic.workspace, root.clone())
        .unwrap();
    assert!(automatic.panels.side_visible);
    automatic.panels.side_focused = true;
    automatic
        .panels
        .remove_sidebar_folder(&mut automatic.workspace, &root)
        .unwrap();
    assert!(!automatic.panels.side_visible);
    assert!(!automatic.panels.side_focused);
    key(shown, "super+k");
    key(shown, "super+b");
    assert!(shown.panels.side_visible);
    shown
        .panels
        .add_folder(&mut shown.workspace, root.clone())
        .unwrap();
    shown
        .panels
        .remove_sidebar_folder(&mut shown.workspace, &root)
        .unwrap();
    assert!(shown.panels.side_visible);
    hidden
        .panels
        .add_folder(&mut hidden.workspace, root.clone())
        .unwrap();
    hidden.run_named(None, "toggle_side_bar", &json!({}));
    assert!(!hidden.panels.side_visible);
    for app in &mut states {
        app.checkpoint(true).unwrap();
    }
    live.borrow().flush().unwrap();
    let recovered = sessions(Some(profile.path()));
    let saved = recovered.borrow().restored.clone();
    assert!(saved.documents.is_empty());
    assert_eq!(saved.windows.len(), 3);
    for window in &saved.windows {
        let mut workspace = startup(None, None);
        workspace.restore_window(window, &saved.documents).unwrap();
        let mut app = app(workspace, recovered.clone());
        app.restore_ui(window);
        let explicit = window.normal_ui.get("show_side_bar").copied();
        assert_eq!(app.panels.side_visible, explicit.unwrap_or(false));
        assert_tabless_editor(&mut app);
        app.panels
            .remove_sidebar_folder(&mut app.workspace, &root)
            .unwrap();
        app.panels
            .add_folder(&mut app.workspace, root.clone())
            .unwrap();
        assert_eq!(app.panels.side_visible, explicit.unwrap_or(true));
    }
    assert!(
        saved
            .windows
            .iter()
            .any(|w| !w.normal_ui.contains_key("show_side_bar"))
    );
    assert!(
        saved
            .windows
            .iter()
            .any(|w| w.normal_ui.get("show_side_bar") == Some(&true))
    );
    assert!(
        saved
            .windows
            .iter()
            .any(|w| w.normal_ui.get("show_side_bar") == Some(&false))
    );
}

#[test]
fn r41_user_sidebar_preference_overrides_auto_until_toggled() {
    let profile = tempfile::tempdir().unwrap();
    let user = profile.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    let root = tempfile::tempdir().unwrap();
    for visible in [true, false] {
        std::fs::write(
            user.join("Preferences.sublime-settings"),
            json!({"show_side_bar":visible}).to_string(),
        )
        .unwrap();
        let mut app = app(startup(Some(profile.path()), None), sessions(None));
        assert_eq!(app.panels.side_visible, visible);
        app.panels
            .add_folder(&mut app.workspace, root.path().to_owned())
            .unwrap();
        assert_eq!(app.panels.side_visible, visible);
        app.run_named(None, "toggle_side_bar", &json!({}));
        assert_eq!(app.panels.side_visible, !visible);
    }
}
