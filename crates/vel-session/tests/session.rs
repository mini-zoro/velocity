use std::{fs, time::Duration};
use vel_buffer::{Buffer, Region, Selection, ops};
use vel_session::{Document, History, HotExit, Session, Sheet, ViewState, Window};
fn sample() -> Session {
    let mut buffer = Buffer::new("α\n");
    let id = buffer.new_view();
    let mut history = History::new(&buffer, id);
    buffer.set_selection(id, Selection::from_regions([Region::caret(2)]));
    history.before_command(&buffer, id);
    ops::insert(&mut buffer, id, "你好");
    history.observe(&buffer, id, "paste");
    buffer.add_regions(id, "velocity.folds", vec![Region::new(0, 2)], true);
    let view = ViewState::capture(&buffer, id, (12., 34.));
    let mut workbench = vel_ui::workbench::Workbench::default();
    workbench.groups[0].add(7, false);
    Session {
        documents: vec![Document {
            id: 1,
            path: None,
            disk: None,
            encoding: "UTF-8".into(),
            bom: false,
            line_ending: "LF".into(),
            dirty: true,
            history: Some(history),
        }],
        windows: vec![Window {
            sheets: vec![Sheet {
                id: 7,
                document: 1,
                view,
                syntax: Some("Packages/Rust/Rust.sublime-syntax".into()),
                transient: false,
            }],
            workbench,
            find_history: vec!["find".into()],
            replace_history: vec!["replace".into()],
            ..Window::default()
        }],
        ..Session::default()
    }
}
#[test]
fn nonexistent_data_atomic_roundtrip_undo_folds_and_selection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("never/existed/Sessions/session");
    assert!(vel_session::load(&path).session.windows.is_empty());
    let session = sample();
    vel_session::save(&path, &session).unwrap();
    let recovery = vel_session::load(&path);
    assert!(recovery.messages.is_empty());
    let doc = &recovery.session.documents[0];
    let (mut buffer, id) = doc.history.as_ref().unwrap().restore(true, true);
    assert_eq!(buffer.text().to_string(), "α你好\n");
    assert!(buffer.is_dirty());
    assert!(buffer.undo(id));
    assert_eq!(buffer.text().to_string(), "α\n");
    assert_eq!(buffer.view(id).sel().primary().unwrap().b, 2);
    assert!(buffer.redo(id));
    let view = &recovery.session.windows[0].sheets[0].view;
    view.apply(&mut buffer, id);
    assert_eq!(buffer.view(id).sel().primary().unwrap().b, 8);
    assert_eq!(
        buffer
            .view(id)
            .get_regions("velocity.folds")
            .unwrap()
            .regions,
        vec![Region::new(0, 2)]
    );
    assert_eq!(view.scroll, (12., 34.));
}
#[test]
fn corrupt_and_wrong_schema_are_kept_aside_and_start_empty() {
    for source in [
        "{truncated",
        r#"{"version":999,"documents":[],"windows":[],"recent_files":[],"recent_folders":[],"recent_projects":[]}"#,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session");
        fs::write(&path, source).unwrap();
        let result = vel_session::load(&path);
        assert!(result.session.windows.is_empty());
        assert_eq!(result.messages.len(), 1);
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(
                fs::read_dir(dir.path())
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path()
            )
            .unwrap(),
            source
        );
    }
}
#[test]
fn failed_save_preserves_original_and_worker_reports_errors_without_panics() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data-is-a-file");
    fs::write(&file, "original").unwrap();
    let writer =
        vel_session::Writer::new(file.join("Sessions/session"), Duration::from_millis(1)).unwrap();
    writer.changed(sample()).unwrap();
    assert!(writer.flush().is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "original");
    let path = dir.path().join("session");
    fs::write(&path, "original").unwrap();
    assert!(vel_session::atomic_write(&path, |_| Err("injected encoding failure".into())).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "original");
}
#[cfg(unix)]
#[test]
fn readonly_data_directory_is_a_recoverable_io_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("new-data");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
    let result = vel_session::save(&root.join("Sessions/session"), &sample());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
}
#[test]
fn hot_exit_policies_and_changed_disk_drop_undo_keep_contents() {
    assert!(HotExit::Always.retain(false));
    assert!(!HotExit::OnlyOnQuit.retain(false));
    assert!(HotExit::OnlyOnQuit.retain(true));
    assert!(!HotExit::Disabled.retain(true));
    let original = sample();
    let history = original.documents[0].history.as_ref().unwrap();
    let (mut buffer, id) = history.restore(false, true);
    assert_eq!(buffer.text().to_string(), "α你好\n");
    assert!(buffer.is_dirty());
    let before = buffer.text().clone();
    if buffer.undo(id) {
        assert_eq!(*buffer.text(), before);
    }
    let mut session = original;
    session.apply_policy(HotExit::Disabled, false, true);
    assert!(session.documents.is_empty());
    assert!(session.windows[0].sheets.is_empty());
    session.validate().unwrap();
}
#[test]
fn redo_cursor_savepoint_unicode_and_limits_survive_wire_format() {
    let mut buffer = Buffer::new("a");
    let id = buffer.new_view();
    let mut history = History::new(&buffer, id);
    for text in ["你好", "🦀", "xyz"] {
        ops::paste(&mut buffer, id, text);
        history.observe(&buffer, id, "paste");
        buffer.break_undo_group();
    }
    buffer.mark_saved();
    history.observe(&buffer, id, "");
    buffer.undo(id);
    history.observe(&buffer, id, "undo");
    let encoded = serde_json::to_vec(&history).unwrap();
    let decoded: History = serde_json::from_slice(&encoded).unwrap();
    let (mut restored, view) = decoded.restore(true, true);
    assert_eq!(restored.text(), buffer.text());
    assert!(restored.is_dirty());
    assert!(restored.redo(view));
    assert!(!restored.is_dirty());
    buffer.set_history_limits(vel_buffer::HistoryLimits {
        entries: 1,
        ..Default::default()
    });
    ops::paste(&mut buffer, id, "!");
    history.observe(&buffer, id, "paste");
    assert!(history.groups() <= 1);
}
#[test]
fn project_comments_relative_rules_and_fresh_workspace_destination() {
    let source = r#"{//project
        "folders":[{"path":"src","name":"Code","follow_symlinks":true,"folder_exclude_patterns":["target"],"file_exclude_patterns":["*.tmp"],"binary_file_patterns":["*.bin"]}],
        "settings":{"tab_size":2},"build_systems":[{"name":"Check","cmd":["cargo","check"]}],"extension_key":42,
    }"#;
    let mut project = vel_session::project::Project::parse(source).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh/project.sublime-project");
    project.resolve(&path);
    project.save(&path).unwrap();
    let read = vel_session::project::Project::load(&path).unwrap();
    assert!(read.folders[0].path.is_absolute());
    assert!(read.folders[0].follow_symlinks);
    assert_eq!(read.extra["extension_key"], 42);
    assert_eq!(
        vel_session::project::workspace_path(&path),
        path.with_extension("sublime-workspace")
    );
}
