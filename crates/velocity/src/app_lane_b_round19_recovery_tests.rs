//! Failed-load, binary and path-identity verification through startup and App dispatch.
#[path = "app_lane_b_round19_special_tests.rs"]
mod special;
use super::*;
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
};

fn checkpoint(a: &mut App) {
    a.checkpoint(false).unwrap();
    a.sessions.borrow().flush().unwrap();
}
fn restore(data: &Path) -> App {
    restore_app(app(Some(data)), 0)
}
fn restore_app(mut a: App, index: usize) -> App {
    let session = a.sessions.borrow().restored.clone();
    let reserved = a.sessions.borrow_mut().reserve_recovered();
    let window = &session.windows[index];
    a.workspace
        .restore_window(window, &session.documents)
        .unwrap();
    a.restore_ui(window);
    a.restore_session_id(reserved[index]);
    a.sync_active_view();
    a.publish_resource_reports();
    a
}

#[test]
fn r19_recovery_failed_window_does_not_block_another_window_or_blank_fallback() {
    let data = tempfile::tempdir().unwrap();
    let mut first = app(Some(data.path()));
    named(&mut first, "select_all", json!({}));
    named(
        &mut first,
        "insert",
        json!({"characters":"recover first window"}),
    );
    let left = first.workspace.snapshot().unwrap();
    let mut second = app(None);
    named(&mut second, "select_all", json!({}));
    named(
        &mut second,
        "insert",
        json!({"characters":"keep second window"}),
    );
    let right = second.workspace.snapshot().unwrap();
    let session = vel_session::Session {
        windows: vec![left.0, right.0],
        documents: left.1.into_iter().chain(right.1).collect(),
        ..Default::default()
    };
    let path = data.path().join("Sessions/Session.sublime-session");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec(&session).unwrap()).unwrap();
    drop((first, second));
    // Startup reads the whole session once, before restoring individual windows.
    let first = app(Some(data.path()));
    let second = app(Some(data.path()));
    crate::session_restore::inject("validate");
    let mut first = restore_app(first, 0);
    let mut second = restore_app(second, 1);
    assert!(first.panels.message.contains(".bak"));
    assert_eq!(
        first.workspace.editor().text.to_string(),
        "recover first window"
    );
    assert_eq!(
        second.workspace.editor().text.to_string(),
        "keep second window"
    );
    assert!(first.workspace.messages.is_empty());
    assert!(second.workspace.messages.is_empty());
    validate_all(&mut first);
    validate_all(&mut second);

    // With no retained/disk-readable contents, the same boundary opens a blank
    // window while keeping the original session available in its backup.
    let mut empty = session;
    for document in &mut empty.documents {
        document.history = None;
    }
    std::fs::write(&path, serde_json::to_vec(&empty).unwrap()).unwrap();
    crate::session_restore::inject("groups");
    let mut blank = restore(data.path());
    assert!(blank.workspace.editor().text.is_empty());
    assert!(blank.panels.message.contains(".bak"));
    validate_all(&mut blank);
}

#[test]
fn r19_recovery_hidden_view_panic_is_contained_on_tab_dispatch() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("source.txt");
    std::fs::write(&path, "still recoverable").unwrap();
    drop(fixture(data.path(), &path, "all_lazy"));
    let mut a = restore(data.path());
    let canonical = path.canonicalize().unwrap();
    let index = a
        .workspace
        .tabs
        .iter()
        .position(|t| t.path.as_ref() == Some(&canonical))
        .unwrap();
    crate::session_restore::inject("restore_history");
    named(&mut a, "select_by_index", json!({"index":index}));
    a.publish_resource_reports();
    assert!(a.panels.message.contains(".bak"));
    assert!(a.workspace.messages.is_empty());
    assert!(a.workspace.editor().read_only);
    validate_all(&mut a);
    checkpoint(&mut a);
    drop(a);
    let mut a = restore(data.path());
    validate_all(&mut a);
}
fn loaded(a: &mut App) {
    let until = Instant::now() + Duration::from_secs(10);
    while a
        .workspace
        .tabs
        .iter()
        .any(|t| t.loading.is_some() || t.binary_loading.is_some())
    {
        a.workspace.poll();
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    a.workspace.sync_shared();
}
fn validate_all(a: &mut App) {
    validate(a);
    a.workspace.validate_document_views().unwrap();
    let (window, documents) = a.workspace.snapshot().unwrap();
    vel_session::Session {
        windows: vec![window],
        documents,
        ..Default::default()
    }
    .validate()
    .unwrap();
}
fn fixture(data: &Path, path: &Path, layout: &str) -> App {
    let mut a = app(Some(data));
    named(&mut a, "open_file", json!({"file":path}));
    loaded(&mut a);
    let original = a.workspace.tabs[a.workspace.active].sheet;
    named(&mut a, "clone_file", json!({}));
    if layout == "three" {
        named(&mut a, "split_view", json!({}));
        named(&mut a, "select_all_tabs", json!({}));
    } else if layout == "original" {
        let index = a.workspace.workbench.groups[0]
            .sheets
            .iter()
            .position(|s| *s == original)
            .unwrap();
        named(&mut a, "select_by_index", json!({"index":index}));
    } else if layout == "all_lazy" {
        named(&mut a, "new_file", json!({}));
        named(
            &mut a,
            "insert",
            json!({"characters":"unrelated dirty document"}),
        );
    }
    checkpoint(&mut a);
    a
}
fn damage(path: &Path, kind: &str) {
    match kind {
        "chmod" => {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
            assert!(
                std::fs::read(path).is_err(),
                "unreadable-file fixture must deny access"
            );
        }
        "directory" => {
            std::fs::remove_file(path).unwrap();
            std::fs::create_dir(path).unwrap();
        }
        "deleted" => {
            std::fs::remove_file(path).unwrap();
        }
        _ => unreachable!(),
    }
}

#[test]
fn r19_recovery_unreadable_clones_restore_and_materialise_without_panics() {
    for kind in ["chmod", "directory", "deleted"] {
        for layout in ["original", "clone", "three", "all_lazy"] {
            let data = tempfile::tempdir().unwrap();
            let path = data.path().join("source.txt");
            std::fs::write(&path, "clean original file\n").unwrap();
            let canonical = path.canonicalize().unwrap();
            drop(fixture(data.path(), &path, layout));
            damage(&path, kind);
            let mut a = restore(data.path());
            let sheets = a
                .workspace
                .tabs
                .iter()
                .filter(|t| t.path.as_ref() == Some(&canonical))
                .map(|t| t.sheet)
                .collect::<Vec<_>>();
            assert!(sheets.len() >= 2, "{kind}/{layout}: {}", a.panels.message);
            for sheet in sheets.clone() {
                let index = a.workspace.workbench.groups[0]
                    .sheets
                    .iter()
                    .position(|s| *s == sheet)
                    .unwrap();
                named(&mut a, "select_by_index", json!({"index":index}));
                a.scene();
                assert!(a.workspace.editor().read_only, "{kind}/{layout}");
                let before = a.workspace.editor().text.clone();
                named(
                    &mut a,
                    "insert",
                    json!({"characters":"must not edit failed load"}),
                );
                assert_eq!(a.workspace.editor().text, before);
                validate_all(&mut a);
            }
            checkpoint(&mut a);
            drop(a);
            let mut a = restore(data.path());
            // A second launch must retry the unavailable source, rather than
            // treating an empty error placeholder as recovered, editable text.
            for _ in sheets {
                let index = a
                    .workspace
                    .tabs
                    .iter()
                    .position(|t| t.path.as_ref() == Some(&canonical) && t.lazy.is_some());
                if let Some(index) = index {
                    named(&mut a, "select_by_index", json!({"index":index}));
                }
            }
            assert!(
                a.workspace
                    .tabs
                    .iter()
                    .filter(|t| t.path.as_ref() == Some(&canonical))
                    .all(|t| t.editor.read_only)
            );
            validate_all(&mut a);
            if layout == "all_lazy" {
                assert!(
                    a.workspace
                        .snapshot()
                        .unwrap()
                        .1
                        .iter()
                        .filter_map(|d| d.history.as_ref())
                        .filter_map(|h| h.current_text().ok())
                        .any(|text| text == "unrelated dirty document")
                );
            }
        }
    }
}

#[test]
fn r19_recovery_every_restore_step_backs_up_and_preserves_recoverable_text() {
    for step in crate::session_restore::STEPS {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("source.txt");
        std::fs::write(&path, "clean source").unwrap();
        let mut a = fixture(data.path(), &path, "three");
        named(&mut a, "new_file", json!({}));
        named(
            &mut a,
            "insert",
            json!({"characters":"RECOVERABLE DIRTY TEXT"}),
        );
        named(&mut a, "select_all_tabs", json!({}));
        // No history is stored for binary files, so use the file-loading step too.
        if *step == "restore_file" {
            std::fs::write(&path, vec![0; 4 * 1024 * 1024]).unwrap();
            let mut window = a.workspace.snapshot().unwrap();
            for document in &mut window.1 {
                if document.path.is_some() {
                    document.history = None;
                }
            }
            let session = vel_session::Session {
                windows: vec![window.0],
                documents: window.1,
                ..Default::default()
            };
            std::fs::write(
                data.path().join("Sessions/Session.sublime-session"),
                serde_json::to_vec(&session).unwrap(),
            )
            .unwrap();
        } else {
            checkpoint(&mut a);
        }
        drop(a);
        let session_path = data.path().join("Sessions/Session.sublime-session");
        let original = std::fs::read(&session_path).unwrap();
        crate::session_restore::inject(step);
        let mut a = restore(data.path());
        assert!(
            a.workspace.messages.is_empty(),
            "{step}: {:?}",
            a.workspace.messages
        );
        let backups = std::fs::read_dir(data.path().join("Sessions"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "bak"))
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1, "{step}: {}", a.panels.message);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), original);
        assert!(
            a.panels.message.contains(&backups[0].display().to_string()),
            "{step}"
        );
        assert!(
            a.workspace
                .tabs
                .iter()
                .any(|t| t.editor.text == "RECOVERABLE DIRTY TEXT"),
            "{step}"
        );
        validate_all(&mut a);
        checkpoint(&mut a);
        drop(a);
        let mut b = restore(data.path());
        validate_all(&mut b);
    }
}

#[test]
fn r19_recovery_binary_lazy_views_never_write_through_the_text_path() {
    for early in [false, true] {
        for layout in ["original", "clone", "all_lazy"] {
            let data = tempfile::tempdir().unwrap();
            let path = data.path().join("binary.dat");
            let bytes = (0..4 * 1024 * 1024)
                .map(|i| (i % 251) as u8)
                .collect::<Vec<_>>();
            std::fs::write(&path, &bytes).unwrap();
            drop(fixture(data.path(), &path, layout));
            let mut a = restore(data.path());
            if !early {
                loaded(&mut a);
            }
            let targets = a
                .workspace
                .tabs
                .iter()
                .filter(|t| t.path.as_ref() == Some(&path.canonicalize().unwrap()))
                .map(|t| t.sheet)
                .collect::<Vec<_>>();
            for sheet in targets {
                let index = a.workspace.workbench.groups[0]
                    .sheets
                    .iter()
                    .position(|s| *s == sheet)
                    .unwrap();
                named(&mut a, "select_by_index", json!({"index":index}));
                assert!(a.workspace.editor().read_only);
                key(&mut a, "super+s");
                named(&mut a, "insert", json!({"characters":"hello"}));
                key(&mut a, "super+s");
                loaded(&mut a);
                assert!(a.workspace.editor().binary.is_some());
                assert!(a.workspace.editor().read_only);
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                // Defence in depth: force the flag off, then use real Save dispatch.
                a.workspace.editor_mut().read_only = false;
                let snapshot = a.workspace.editor_mut().binary.take();
                named(&mut a, "save", json!({}));
                assert!(
                    a.panels.message.contains("binary/hex"),
                    "{}",
                    a.panels.message
                );
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                a.workspace.editor_mut().binary = snapshot;
                a.workspace.editor_mut().read_only = true;
                validate_all(&mut a);
            }
        }
    }
}

#[derive(Debug, PartialEq)]
struct Properties {
    path: Option<PathBuf>,
    format: vel_fileio::Format,
    saved_format: vel_fileio::Format,
    read_only: bool,
    binary: bool,
    bytes: u64,
    metrics: vel_largefile::DocumentMetrics,
    features: vel_largefile::FeaturePolicy,
    dirty: bool,
    deleted: bool,
    format_dirty: bool,
    disk: Option<vel_session::DiskStamp>,
    detection: bool,
    indentation: bool,
}
fn properties(a: &App, index: usize) -> Properties {
    let t = &a.workspace.tabs[index];
    Properties {
        path: t.path.clone(),
        format: t.format.clone(),
        saved_format: t.saved_format.clone(),
        read_only: t.editor.read_only,
        binary: t.editor.binary.is_some(),
        bytes: t.editor.source_bytes,
        metrics: t.editor.document_metrics,
        features: t.editor.features,
        dirty: a.workspace.document_contents_dirty(t),
        deleted: t.deleted,
        format_dirty: t.restored_format_dirty,
        disk: t.disk_saved.clone(),
        detection: t.needs_detection,
        indentation: t.indentation_initialized,
    }
}
#[test]
fn r19_recovery_clone_and_lazy_attachment_copy_the_same_document_properties() {
    for kind in ["text", "dirty", "deleted", "binary"] {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("source.txt");
        std::fs::write(
            &path,
            if kind == "binary" {
                &b"binary\0bytes"[..]
            } else {
                &b"text\r\n"[..]
            },
        )
        .unwrap();
        let canonical = path.canonicalize().unwrap();
        let mut a = app(Some(data.path()));
        named(&mut a, "open_file", json!({"file":path}));
        loaded(&mut a);
        if kind == "dirty" {
            named(&mut a, "insert", json!({"characters":"edited"}));
        }
        if kind == "deleted" {
            std::fs::remove_file(&path).unwrap();
            a.workspace.tabs[a.workspace.active].deleted = true;
        }
        named(&mut a, "clone_file", json!({}));
        let source = a
            .workspace
            .tabs
            .iter()
            .position(|t| {
                t.session_document == a.workspace.tabs[a.workspace.active].session_document
                    && t.sheet != a.workspace.tabs[a.workspace.active].sheet
            })
            .unwrap();
        assert_eq!(
            properties(&a, source),
            properties(&a, a.workspace.active),
            "clone {kind}"
        );
        checkpoint(&mut a);
        drop(a);
        let mut a = restore(data.path());
        loaded(&mut a);
        let target = a
            .workspace
            .tabs
            .iter()
            .position(|t| t.lazy.is_some() && t.path.as_ref() == Some(&canonical))
            .unwrap_or_else(|| panic!("no lazy view for {kind}: {}", a.panels.message));
        named(&mut a, "select_by_index", json!({"index":target}));
        loaded(&mut a);
        let source = a
            .workspace
            .tabs
            .iter()
            .position(|t| {
                t.session_document == a.workspace.tabs[target].session_document
                    && t.sheet != a.workspace.tabs[target].sheet
            })
            .unwrap();
        assert_eq!(
            properties(&a, source),
            properties(&a, target),
            "lazy {kind}"
        );
        validate_all(&mut a);
    }
}

#[test]
fn r19_recovery_save_as_symlink_has_one_identity_and_receives_external_changes() {
    let data = tempfile::tempdir().unwrap();
    let real = data.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let alias = data.path().join("alias");
    symlink(&real, &alias).unwrap();
    let via_alias = alias.join("new.txt");
    let canonical = real.canonicalize().unwrap().join("new.txt");
    let mut a = app(Some(data.path()));
    a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
    named(&mut a, "select_all", json!({}));
    named(&mut a, "insert", json!({"characters":"original"}));
    named(&mut a, "save", json!({"file":via_alias}));
    let sheet = a.workspace.tabs[a.workspace.active].sheet;
    assert_eq!(
        a.workspace.tabs[a.workspace.active].path.as_ref(),
        Some(&canonical)
    );
    assert_eq!(std::fs::read_to_string(&canonical).unwrap(), "original");
    std::fs::write(&canonical, "external contents changed on the real path").unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while a.workspace.editor().text != "external contents changed on the real path" {
        a.workspace.poll();
        assert!(Instant::now() < until, "external change missed");
        std::thread::sleep(Duration::from_millis(5));
    }
    let before = ids(&a);
    named(&mut a, "open_file", json!({"file":canonical}));
    assert_eq!(ids(&a), before);
    assert_eq!(a.workspace.tabs[a.workspace.active].sheet, sheet);
    checkpoint(&mut a);
    drop(a);
    let mut a = restore(data.path());
    named(&mut a, "open_file", json!({"file":via_alias}));
    assert_eq!(
        a.workspace
            .tabs
            .iter()
            .filter(|t| t.path.as_ref() == Some(&canonical))
            .count(),
        1
    );
    validate_all(&mut a);
}
