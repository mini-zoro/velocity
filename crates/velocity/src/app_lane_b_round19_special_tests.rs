//! Special-file, recovery and conflict regressions through startup/key/mouse dispatch.
use super::*;
use std::os::unix::net::UnixListener;

fn bounded(body: impl FnOnce() + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        let _ = tx.send(result);
    });
    match rx
        .recv_timeout(Duration::from_secs(15))
        .expect("document operation hung")
    {
        Ok(()) => {}
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
fn special(path: &Path, kind: &str) -> Option<UnixListener> {
    std::fs::remove_file(path).unwrap();
    match kind {
        "fifo" => {
            assert!(
                std::process::Command::new("mkfifo")
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
            None
        }
        "socket" => Some(UnixListener::bind(path).unwrap()),
        "device" => {
            symlink("/dev/zero", path).unwrap();
            None
        }
        _ => unreachable!(),
    }
}
fn source_tabs(a: &App) -> Vec<usize> {
    let docs = &a.sessions.borrow().restored.documents;
    let id = docs.iter().find(|d| d.path.is_some()).unwrap().id;
    a.workspace
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.restored_document == Some(id))
        .map(|(i, _)| i)
        .collect()
}
#[test]
fn r19_special_startup_and_lazy_click_reject_fifo_socket_and_device_in_every_layout() {
    for kind in ["fifo", "socket", "device"] {
        for layout in ["original", "clone", "three", "all_lazy"] {
            bounded(move || {
                let data = tempfile::tempdir().unwrap();
                let path = data.path().join("source.txt");
                std::fs::write(&path, "clean text").unwrap();
                drop(fixture(data.path(), &path, layout));
                let _socket = special(&path, kind);
                let mut a = restore(data.path());
                let indices = source_tabs(&a);
                assert!(indices.len() >= 2);
                for index in indices {
                    named(&mut a, "select_by_index", json!({"index":index}));
                    assert!(a.workspace.editor().read_only, "{kind}/{layout}");
                    assert!(a.workspace.tabs[index].load_failed);
                    assert!(!a.workspace.tabs[index].dirty());
                    key(&mut a, "super+s");
                    assert!(a.workspace.messages.is_empty());
                    validate_all(&mut a);
                }
                a.publish_resource_reports();
                assert!(
                    a.panels.message.contains("regular files"),
                    "{}",
                    a.panels.message
                );
                checkpoint(&mut a);
                drop(a);
                let mut a = restore(data.path());
                validate_all(&mut a);
            });
        }
    }
}
#[test]
fn r19_special_saves_save_all_and_save_on_close_never_open_a_fifo() {
    for command in ["save", "save_all", "close_file", "save_as"] {
        bounded(move || {
            let data = tempfile::tempdir().unwrap();
            let path = data.path().join("source.txt");
            std::fs::write(&path, "before").unwrap();
            let mut a = app(Some(data.path()));
            named(&mut a, "open_file", json!({"file":path}));
            named(&mut a, "insert", json!({"characters":"dirty "}));
            let _socket = special(&path, "fifo");
            let sheets = ids(&a);
            if command == "close_file" {
                a.close_decisions
                    .push_back(tab_adapter::CloseDecision::Save(None));
            }
            if command == "save_as" {
                named(&mut a, "save", json!({"file":path}));
            } else if command == "save" {
                key(&mut a, "super+s");
            } else {
                named(&mut a, command, json!({}));
            }
            assert_eq!(ids(&a), sheets);
            assert!(a.workspace.editor().text.to_string().contains("dirty"));
            assert!(
                a.panels.message.contains("regular files"),
                "{}",
                a.panels.message
            );
            assert!(a.workspace.messages.is_empty());
            assert!(!std::fs::metadata(path).unwrap().is_file());
        });
    }
}
#[test]
fn r19_special_restore_watchdog_backs_up_and_keeps_dirty_text() {
    bounded(|| {
        let data = tempfile::tempdir().unwrap();
        let mut a = app(Some(data.path()));
        named(&mut a, "insert", json!({"characters":"WATCHDOG KEEP"}));
        checkpoint(&mut a);
        drop(a);
        crate::restore_io::block_next();
        let at = Instant::now();
        let mut a = restore(data.path());
        assert!(at.elapsed() < Duration::from_secs(8));
        assert!(a.panels.message.contains("timed out"));
        assert!(a.panels.message.contains(".bak"));
        assert!(
            a.workspace
                .editor()
                .text
                .to_string()
                .contains("WATCHDOG KEEP")
        );
        assert!(a.workspace.messages.is_empty());
        validate_all(&mut a);
    });
}
#[test]
fn r19_special_lazy_watchdog_recovers_retained_edits_as_unsaved_text() {
    bounded(|| {
        let data = tempfile::tempdir().unwrap();
        let mut a = app(Some(data.path()));
        named(&mut a, "insert", json!({"characters":"LAZY RECOVERY"}));
        let text = a.workspace.editor().text.to_string();
        named(&mut a, "clone_file", json!({}));
        named(&mut a, "new_file", json!({}));
        checkpoint(&mut a);
        drop(a);
        let mut a = restore(data.path());
        assert!(a.workspace.tabs[0].lazy.is_some());
        crate::restore_io::block_next();
        named(&mut a, "select_by_index", json!({"index":0}));
        a.publish_resource_reports();
        assert!(a.panels.message.contains(".bak"));
        assert_eq!(a.workspace.editor().text, text.as_str());
        assert!(a.workspace.tabs[a.workspace.active].dirty());
        assert!(a.workspace.tabs[a.workspace.active].path.is_none());
        assert!(!a.workspace.editor().read_only);
        a.close_decisions
            .push_back(tab_adapter::CloseDecision::Cancel);
        named(&mut a, "close_all", json!({}));
        assert!(a.close_decisions.is_empty());
        assert_eq!(a.workspace.editor().text, text.as_str());
        let path = data.path().join("recovered.txt");
        named(&mut a, "save", json!({"file":path}));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        validate_all(&mut a);
    });
}
#[test]
fn r19_special_both_failed_windows_get_numbered_original_byte_backups() {
    let data = tempfile::tempdir().unwrap();
    let mut a = app(Some(data.path()));
    named(&mut a, "insert", json!({"characters":"first recover"}));
    let left = a.workspace.snapshot().unwrap();
    let mut b = app(None);
    named(&mut b, "insert", json!({"characters":"second recover"}));
    let right = b.workspace.snapshot().unwrap();
    let session = vel_session::Session {
        windows: vec![left.0, right.0],
        documents: left.1.into_iter().chain(right.1).collect(),
        ..Default::default()
    };
    let source = data.path().join("Sessions/Session.sublime-session");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    let bytes = serde_json::to_vec_pretty(&session).unwrap();
    std::fs::write(&source, &bytes).unwrap();
    drop((a, b));
    let a = app(Some(data.path()));
    let b = app(Some(data.path()));
    crate::session_restore::inject("groups");
    let mut a = restore_app(a, 0);
    let backups = std::fs::read_dir(source.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "bak"))
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 2, "all backups must precede source removal");
    // The second window must own the original even when unrelated launches evict it.
    crate::session_restore::evict_original(&source);
    crate::session_restore::inject("groups");
    let mut b = restore_app(b, 1);
    for path in backups {
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    assert_ne!(a.panels.message, b.panels.message);
    assert!(a.panels.message.contains("-1.bak"));
    assert!(b.panels.message.contains("-2.bak"));
    validate_all(&mut a);
    validate_all(&mut b);
}
#[test]
fn r19_special_reopen_retries_error_and_error_close_never_prompts() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("source.txt");
    std::fs::write(&path, "original").unwrap();
    drop(fixture(data.path(), &path, "clone"));
    let _socket = special(&path, "fifo");
    let mut a = restore(data.path());
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "repaired").unwrap();
    let before = ids(&a);
    named(&mut a, "open_file", json!({"file":path}));
    assert_eq!(ids(&a), before);
    assert_eq!(a.workspace.editor().text, "repaired");
    assert!(!a.workspace.editor().read_only);
    assert!(!a.workspace.tabs[a.workspace.active].load_failed);
    validate_all(&mut a);
    // Restore the old failure again and close the final error view after repair.
    drop(a);
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "placeholder").unwrap();
    drop(fixture(data.path(), &path, "clone"));
    special(&path, "fifo");
    let mut a = restore(data.path());
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "restored again").unwrap();
    a.close_decisions
        .push_back(tab_adapter::CloseDecision::Cancel);
    for _ in 0..2 {
        named(&mut a, "close_file", json!({}));
    }
    assert_eq!(a.close_decisions.len(), 1);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "restored again");
}
#[test]
fn r19_special_clean_text_becomes_hex_on_first_restore() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("source.txt");
    std::fs::write(&path, "old text").unwrap();
    drop(fixture(data.path(), &path, "clone"));
    let bytes = vec![0; 4 * 1024 * 1024];
    std::fs::write(&path, &bytes).unwrap();
    let mut a = restore(data.path());
    loaded(&mut a);
    assert!(a.workspace.editor().binary.is_some());
    assert!(!a.workspace.tabs[a.workspace.active].load_failed);
    key(&mut a, "super+s");
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    validate_all(&mut a);
}
#[test]
fn r19_special_changed_disk_requires_overwrite_and_cancel_preserves_both_versions() {
    for command in ["save", "save_all", "close_file"] {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("source.txt");
        std::fs::write(&path, "initial").unwrap();
        let mut a = app(Some(data.path()));
        named(&mut a, "open_file", json!({"file":path}));
        named(&mut a, "insert", json!({"characters":"my edit "}));
        let text = a.workspace.editor().text.to_string();
        let bytes = vec![0; 4 * 1024 * 1024];
        let replacement = data.path().join("new.bin");
        std::fs::write(&replacement, &bytes).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        a.overwrite_decisions.push_back(false);
        if command == "close_file" {
            a.close_decisions
                .push_back(tab_adapter::CloseDecision::Save(None));
        }
        named(&mut a, command, json!({}));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(a.workspace.editor().text, text.as_str());
        assert!(a.overwrite_decisions.is_empty());
        a.overwrite_decisions.push_back(true);
        if command == "close_file" {
            a.close_decisions
                .push_back(tab_adapter::CloseDecision::Save(None));
        }
        named(&mut a, command, json!({}));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert!(a.overwrite_decisions.is_empty());
    }
}
#[test]
fn r19_special_panes_keep_tab_bars_inside_their_group_when_split_dragged_and_resized() {
    let mut a = app(None);
    a.scene();
    for _ in 0..30 {
        named(&mut a, "new_pane", json!({}));
        a.scene();
    }
    assert!(a.workspace.workbench.groups.len() < 30);
    for r in &a.group_bounds {
        assert!(r.height + 0.01 >= a.workspace.pane_minimum.1);
    }
    let splitter = a
        .workspace
        .workbench
        .layout
        .dividers(a.workbench_bounds, 8.)
        .first()
        .copied()
        .unwrap();
    let rect = splitter.1;
    let at = Point {
        x: rect.x + rect.width / 2.,
        y: rect.y + rect.height / 2.,
    };
    for (kind, position) in [
        (MouseKind::Down, at),
        (MouseKind::Drag, Point { x: 1., y: 1. }),
        (MouseKind::Up, Point { x: 1., y: 1. }),
    ] {
        a.handle_mouse_event(
            None,
            MouseEvent {
                kind,
                position,
                button: 0,
                click_count: 1,
                modifiers: Modifiers(0),
                timestamp: 0.,
            },
        );
    }
    a.resize_views(
        Size {
            width: 320.,
            height: 200.,
        },
        1.,
    );
    a.scene();
    for r in &a.group_bounds {
        assert!(r.height + 0.01 >= a.workspace.pane_minimum.1);
    }
    for (sheet, rect) in a.tab_hits.clone() {
        let group = a.workspace.workbench.group_of(sheet).unwrap();
        assert!(
            rect.y + rect.height <= a.group_bounds[group].y + a.group_bounds[group].height + 0.01
        );
    }
    let (sheet, rect) = a.tab_hits.last().copied().unwrap();
    let before = ids(&a);
    a.handle_mouse_event(
        None,
        MouseEvent {
            kind: MouseKind::Down,
            position: Point {
                x: rect.x + rect.width / 2.,
                y: rect.y + rect.height / 2.,
            },
            button: 2,
            click_count: 1,
            modifiers: Modifiers(0),
            timestamp: 0.,
        },
    );
    assert_eq!(
        ids(&a),
        before.into_iter().filter(|id| *id != sheet).collect()
    );
    validate_all(&mut a);
}

#[test]
fn r19_special_open_and_external_reload_reject_nonregular_files_nonmodally() {
    for kind in ["fifo", "socket", "device"] {
        for dirty in [false, true] {
            bounded(move || {
                let data = tempfile::tempdir().unwrap();
                let path = data.path().join("source.txt");
                std::fs::write(&path, "clean contents").unwrap();
                let mut a = app(Some(data.path()));
                a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
                named(&mut a, "open_file", json!({"file":path}));
                if dirty {
                    named(&mut a, "insert", json!({"characters":"user edits"}));
                }
                let text = a.workspace.editor().text.to_string();
                let _socket = special(&path, kind);
                let until = Instant::now() + Duration::from_secs(5);
                loop {
                    a.workspace.poll();
                    a.publish_resource_reports();
                    if a.panels.message.contains("regular files") {
                        break;
                    }
                    assert!(Instant::now() < until, "reload did not report {kind}");
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert!(a.workspace.messages.is_empty());
                let path_text = path.to_string_lossy();
                assert_eq!(a.panels.message.matches(path_text.as_ref()).count(), 1);
                if dirty {
                    assert_eq!(a.workspace.editor().text, text.as_str());
                    assert!(a.workspace.tabs[a.workspace.active].dirty());
                    key(&mut a, "super+s");
                    assert!(a.panels.message.contains("regular files"));
                } else {
                    assert!(a.workspace.tabs[a.workspace.active].load_failed);
                    assert!(a.workspace.editor().read_only);
                    assert!(!a.workspace.tabs[a.workspace.active].dirty());
                }
                // A fresh Open File request follows the same regular-file boundary.
                let mut b = app(None);
                let before = ids(&b);
                named(&mut b, "open_file", json!({"file":path}));
                assert_eq!(ids(&b), before);
                assert!(b.panels.message.contains("regular files"));
                assert_eq!(b.panels.message.matches(path_text.as_ref()).count(), 1);
                assert!(b.workspace.messages.is_empty());
                validate_all(&mut a);
                validate_all(&mut b);
            });
        }
    }
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("unreadable.txt");
    let alias = data.path().join("alias.txt");
    std::fs::write(&path, "unreadable").unwrap();
    symlink(&path, &alias).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut a = app(None);
    named(&mut a, "open_file", json!({"file":alias}));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(a.panels.message.matches(path.to_str().unwrap()).count(), 1);
    assert!(!a.panels.message.contains(alias.to_str().unwrap()));
}
