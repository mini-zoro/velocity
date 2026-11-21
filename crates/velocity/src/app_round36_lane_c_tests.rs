use super::*;
use std::{fs, path::Path};
use vel_buffer::Region;
use vel_config::keymap::Key;
use vel_text::Text;
use vel_ui::workbench::Layout;

fn app() -> App {
    let options = Options::default();
    let now = Instant::now();
    App::new(
        options.clone(),
        Clock::new(now, std::time::SystemTime::now()),
        Reporter::for_options(&options, now).unwrap(),
        Workspace::new(Text::new(), None, None, false).unwrap(),
        Arc::new(AtomicBool::new(false)),
        std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
    )
}

fn write(root: &Path, name: &str, text: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, text).unwrap();
    path.canonicalize().unwrap()
}

#[test]
fn r36_open_file_encoded_position_override_preserves_legacy_suffixes() {
    let dir = tempfile::tempdir().unwrap();
    let file = write(dir.path(), "notes.txt", "zero\né中文\n");
    let literal = write(dir.path(), "notes.txt:2", "literal colon\n");
    let encoded = format!("{}:2:3", file.display());
    let mut a = app();
    a.run_named(None, "open_file", &json!({"file":literal}));
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&literal)
    );
    a.run_named(
        None,
        "open_file",
        &json!({"file":encoded, "encoded_position":false}),
    );
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_deref(),
        Some(Path::new(&encoded))
    );
    a.run_named(
        None,
        "open_file",
        &json!({"file":encoded, "encoded_position":true}),
    );
    let editor = a.workspace.editor();
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&file)
    );
    assert_eq!(editor.caret, "zero\né中".len());
    a.run_named(None, "open_file", &json!({"file":file}));
    assert_eq!(a.workspace.editor().caret, "zero\né中".len());
    a.workspace.editor_mut().set_caret(0, false);
    a.run_named(None, "open_file", &json!({"file":encoded}));
    assert_eq!(a.workspace.editor().caret, "zero\né中".len());
}

#[test]
fn r36_open_file_flags_keep_group_views_and_unselect_without_closing() {
    let dir = tempfile::tempdir().unwrap();
    let first = write(dir.path(), "a", "text");
    let second = write(dir.path(), "b", "text");
    let third = write(dir.path(), "c", "text");
    let fourth = write(dir.path(), "d", "text");
    let mut a = app();
    a.run_named(None, "open_file", &json!({"file":first}));
    let original = a.workspace.tabs[a.workspace.active].sheet;
    a.workspace
        .workbench
        .set_layout(Layout::grid(2, 1))
        .unwrap();
    a.workspace.workbench.active = 1;
    a.run_named(
        None,
        "open_file",
        &json!({"file":first, "force_group":true}),
    );
    let clone = a.workspace.tabs[a.workspace.active].sheet;
    assert_ne!(clone, original);
    assert_eq!(a.workspace.workbench.group_of(clone), Some(1));
    assert_eq!(a.workspace.workbench.group_of(original), Some(0));
    assert_eq!(
        a.workspace.tabs[0].session_document,
        a.workspace.tabs[1].session_document
    );
    a.run_named(
        None,
        "open_file",
        &json!({"file":first, "force_group":true}),
    );
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, clone);
    a.run_named(
        None,
        "open_file",
        &json!({"file":first, "force_clone":true}),
    );
    assert_ne!(a.workspace.tabs[a.workspace.active].sheet, clone);
    assert_eq!(a.workspace.tabs.len(), 3);
    assert!(
        a.workspace
            .tabs
            .iter()
            .all(|tab| tab.session_document == a.workspace.tabs[0].session_document)
    );

    a.run_named(
        None,
        "open_file",
        &json!({"file":second, "add_to_selection":true}),
    );
    let left = a.workspace.tabs[a.workspace.active].sheet;
    a.run_named(
        None,
        "open_file",
        &json!({"file":third, "add_to_selection":true}),
    );
    let right = a.workspace.tabs[a.workspace.active].sheet;
    a.run_named(
        None,
        "open_file",
        &json!({"file":second, "add_to_selection":true}),
    );
    let before = a.workspace.tabs.len();
    a.run_named(
        None,
        "open_file",
        &json!({"file":fourth, "clear_to_right":true, "transient":true}),
    );
    let preview = &a.workspace.tabs[a.workspace.active];
    let group = &a.workspace.workbench.groups[1];
    assert!(group.selected.contains(&left));
    assert!(!group.selected.contains(&right));
    assert!(group.sheets.contains(&right));
    assert!(group.selected.contains(&preview.sheet));
    assert_eq!(a.workspace.tabs.len(), before + 1);
    assert!(preview.transient);
    assert!(!a.workspace.recent_files.contains(&fourth));
}

#[test]
fn r36_project_choice_modifier_and_companion_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let project = write(dir.path(), "demo.sublime-project", "{}");
    let companion = write(dir.path(), "demo.sublime-workspace", r#"{"external":true}"#);
    let mut a = app();
    a.workspace.recent_projects.push(project.clone());
    for (chord, separate) in [
        ("enter", false),
        ("ctrl+enter", false),
        ("super+enter", true),
    ] {
        a.run_named(None, "quick_switch_project", &json!({}));
        let (_, call) = a
            .panels
            .key(&mut a.workspace, &Key::parse(chord).unwrap(), 0..0)
            .unwrap();
        let call = call.unwrap();
        assert_eq!(call.command, "open_project");
        assert_eq!(call.args["file"], json!(project));
        assert_eq!(call.args["new_window"], separate);
    }
    a.run_named(
        None,
        "open_project_or_workspace",
        &json!({"file":companion}),
    );
    assert_eq!(a.workspace.project_path(), Some(project.as_path()));
    let initial = a.workspace.tabs[0].sheet;
    a.run_named(None, "open_project_or_workspace", &json!({"file":project}));
    assert_eq!(a.workspace.tabs[0].sheet, initial);

    let location = crate::cli::FileLocation::path_only(companion);
    let mut request = crate::instance::Request::new(&Options::default(), None).unwrap();
    request.files.push(location);
    crate::open_requests::RouteRequest::prepare(
        &mut request,
        crate::open_requests::OpenWindow::Never,
    );
    assert_eq!(request.project, Some(project));
    assert!(request.files.is_empty());
}

#[test]
fn r36_remove_deleted_projects_preserves_other_history_and_stale_windows() {
    let dir = tempfile::tempdir().unwrap();
    let good = write(dir.path(), "valid.sublime-project", "{}");
    let removed = write(dir.path(), "removed.sublime-project", "{}");
    let mut a = app();
    let mut b = app();
    b.session_id = a.sessions.borrow_mut().register();
    b.sessions = a.sessions.clone();
    a.workspace.recent_projects = vec![removed.clone(), good.clone()];
    a.workspace.recent_files.push(good.clone());
    a.sync_recents();
    b.sync_recents();
    assert_eq!(b.workspace.recent_projects.len(), 2);
    fs::remove_file(&removed).unwrap();
    a.run_named(None, "remove_deleted_recent_projects", &json!({}));
    assert_eq!(a.workspace.recent_files, std::slice::from_ref(&good));
    assert_eq!(a.workspace.recent_projects, std::slice::from_ref(&good));
    b.sync_recents();
    assert_eq!(b.workspace.recent_projects, [good]);
}

#[test]
fn r36_index_status_toggle_changes_effective_preference() {
    let mut a = app();
    for enabled in [false, true] {
        a.run_named(None, "toggle_indexing", &json!({}));
        assert_eq!(a.workspace.settings.bool("index_files", !enabled), enabled);
        assert_eq!(
            a.panels.intel.status_source.lock().unwrap().enabled,
            enabled
        );
        assert_eq!(
            a.workspace.settings.layer(Layer::User)["index_files"],
            enabled
        );
    }
}

#[test]
fn r36_block_command_and_report_a_bug_menu() {
    let mut a = app();
    a.workspace
        .new_text_tab(Text::from("start\nblock line\n\nnext block\n"))
        .unwrap();
    a.workspace.editor_mut().set_caret(9, false);
    a.run_named(None, "expand_selection", &json!({"to":"block"}));
    assert_eq!(a.workspace.editor().regions(), [Region::new(0, 17)]);

    let help = a
        .application_menu()
        .into_iter()
        .find(|item| item.caption == "Help")
        .unwrap();
    let report = help
        .children
        .iter()
        .find(|item| item.caption == "Report a Bug")
        .unwrap();
    assert_eq!(report.command.as_deref(), Some("open_url"));
    assert_eq!(
        serde_json::from_str::<Value>(&report.args).unwrap()["url"],
        format!("{}/issues", env!("CARGO_PKG_REPOSITORY"))
    );
}
