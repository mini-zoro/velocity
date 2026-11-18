//! Cross-lane regression coverage for the round-20 rebase.
use super::round20::drive;
use super::*;

#[test]
fn r20_merge_save_uses_raw_revision_for_touches_and_same_stamp_rewrites() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("revision.txt");
    std::fs::write(&path, "base\n").unwrap();
    let mut a = app("", "Plain Text", Some(directory.path()));
    named(&mut a, "open_file", json!({"file":path}));
    input(&mut a, "+mine");
    let index = a.workspace.active;
    let old = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(old + Duration::from_secs(2))
        .unwrap();
    assert!(
        !a.workspace.save_conflict(index, None).unwrap(),
        "mtime alone is not a raw rewrite"
    );
    let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
    let replacement = directory.path().join("replacement");
    std::fs::write(&replacement, "base\n").unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    assert!(
        !a.workspace.save_conflict(index, None).unwrap(),
        "inode alone is not a raw rewrite"
    );
    std::fs::write(&path, "new!\n").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    assert!(
        a.workspace.save_conflict(index, None).unwrap(),
        "same-stamp raw changes must conflict"
    );
    a.overwrite_decisions.push_back(false);
    named(&mut a, "save", json!({}));
    assert!(a.overwrite_decisions.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), b"new!\n");
    assert!(a.workspace.tabs[index].dirty());
    let expected = a.workspace.editor().text.to_string();
    a.overwrite_decisions.push_back(true);
    named(&mut a, "save", json!({}));
    assert!(a.overwrite_decisions.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    assert!(!a.workspace.tabs[index].dirty());
    assert_eq!(
        a.workspace.tabs[index].disk_content,
        vel_fileio::content_identity(&path).unwrap()
    );
}

#[test]
fn r20_merge_restored_clones_share_encoding_revision_and_native_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("shared.txt");
    vel_fileio::save(
        &path,
        &Text::from("日本語かな\n"),
        &vel_fileio::encoding_format("Japanese (Shift JIS)", vel_fileio::LineEnding::Lf).unwrap(),
        false,
    )
    .unwrap();
    let mut a = app("", "Plain Text", Some(directory.path()));
    named(&mut a, "open_file", json!({"file":path}));
    named(&mut a, "reopen", json!({"encoding":"Japanese (Shift JIS)"}));
    named(&mut a, "clone_file", json!({}));
    input(&mut a, "+edit");
    let (window, documents) = a.workspace.snapshot().unwrap();
    let raw = a.workspace.tabs[a.workspace.active].disk_content;
    let mut b = app("", "Plain Text", Some(directory.path()));
    b.workspace.restore_window(&window, &documents).unwrap();
    let shared = b.workspace.tabs[b.workspace.active].session_document;
    for index in 0..b.workspace.tabs.len() {
        b.workspace.select(index);
        let tab = &b.workspace.tabs[index];
        assert_eq!(tab.session_document, shared);
        assert_eq!(tab.format.encoding, "Shift_JIS");
        assert!(tab.encoding_chosen);
        assert_eq!(
            tab.preferred_encoding.as_deref(),
            Some("Japanese (Shift JIS)")
        );
        assert_eq!(tab.disk_content, raw);
        assert!(tab.editor.text.to_string().contains("+edit"));
        assert!(tab.dirty());
    }
    b.workspace.validate_document_views().unwrap();
    drive(&mut b, |a| {
        a.workspace.tabs.iter().all(|t| t.loading.is_none())
    });
    b.run_named_depth(None, "undo", &json!({}), 0).unwrap();
    b.workspace.sync_shared();
    for index in 0..b.workspace.tabs.len() {
        b.workspace.select(index);
        assert_eq!(b.workspace.editor().text, "日本語かな\n");
    }
    named(&mut b, "redo", json!({}));
    assert!(b.workspace.editor().text.to_string().contains("+edit"));
    drive(&mut b, |a| {
        a.workspace.tabs.iter().all(|t| t.loading.is_none())
    });
}

#[cfg(unix)]
#[test]
fn r20_merge_every_decode_boundary_rejects_special_files() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for kind in ["fifo", "socket", "device", "directory"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("special");
            match kind {
                "fifo" => assert!(
                    std::process::Command::new("mkfifo")
                        .arg(&path)
                        .status()
                        .unwrap()
                        .success()
                ),
                "socket" => {
                    drop(UnixListener::bind(&path).unwrap());
                }
                "device" => symlink("/dev/zero", &path).unwrap(),
                _ => std::fs::create_dir(&path).unwrap(),
            }
            for encoding in [None, Some("UTF-16 LE"), Some("Japanese (Shift JIS)")] {
                let options = vel_fileio::DecodeOptions {
                    encoding: encoding.map(str::to_owned),
                    ..Default::default()
                };
                assert!(
                    vel_fileio::decoding_for_path(&path, &options).is_err(),
                    "{kind}"
                );
                assert!(
                    vel_fileio::load_with_options(&path, &options, |_| {}).is_err(),
                    "{kind}"
                );
                assert!(
                    vel_fileio::load_background_with_options(path.clone(), options).is_err(),
                    "{kind}"
                );
                assert!(vel_fileio::content_identity(&path).is_err(), "{kind}");
            }
        }
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("special-file boundary blocked");
}

#[test]
fn r20_merge_status_hit_testing_and_popup_use_effective_captions() {
    let mut a = app("text\n", "Plain Text", None);
    for setting in ["show_encoding", "show_line_endings"] {
        set(&mut a, setting, json!(true));
    }
    for caption in ["UTF-8 with BOM", "UTF-16 LE", "Western (ISO 8859-1)"] {
        named(&mut a, "set_encoding", json!({"encoding":caption}));
        a.scene();
        assert_eq!(a.format_hits.len(), 2);
        let rows = a.format_menu(status_adapter::FormatItem::Encoding);
        let save = &rows[1].children;
        let checked: Vec<_> = save
            .iter()
            .filter(|row| a.command_checked(row.command.as_deref().unwrap(), &row.args))
            .collect();
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].caption, caption);
        let line_endings = a.format_menu(status_adapter::FormatItem::LineEndings);
        assert_eq!(line_endings.len(), 3);
        assert!(
            line_endings
                .iter()
                .all(|row| row.command.as_deref() == Some("set_line_ending"))
        );
    }
}

#[test]
fn r20_merge_error_placeholder_clones_keep_checkpoint_view_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("temporarily-unavailable.txt");
    std::fs::write(&path, "retained revision\n").unwrap();
    let mut a = app("", "Plain Text", Some(directory.path()));
    named(&mut a, "open_file", json!({"file":path}));
    named(&mut a, "clone_file", json!({}));
    let (window, documents) = a.workspace.snapshot().unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let mut b = app("", "Plain Text", Some(directory.path()));
    b.workspace.restore_window(&window, &documents).unwrap();
    assert!(b.workspace.editor().read_only);
    named(&mut b, "clone_file", json!({}));
    for index in 0..b.workspace.tabs.len() {
        named(&mut b, "select_by_index", json!({"index":index}));
        let (window, documents) = b.workspace.snapshot().unwrap();
        vel_session::Session {
            windows: vec![window],
            documents,
            ..Default::default()
        }
        .validate()
        .unwrap();
        b.workspace.validate_document_views().unwrap();
    }
    // The newly created clone is focused and owns no view in the old history.
    // A second startup must allocate its own view when the source recovers.
    let (window, documents) = b.workspace.snapshot().unwrap();
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, "retained revision\n").unwrap();
    let mut restored = app("", "Plain Text", None);
    restored
        .workspace
        .restore_window(&window, &documents)
        .unwrap();
    b = restored;
    named(&mut b, "open_file", json!({"file":path}));
    drive(&mut b, |a| {
        a.workspace.tabs.iter().all(|t| t.loading.is_none())
    });
    assert_eq!(b.workspace.editor().text, "retained revision\n");
    assert!(!b.workspace.editor().read_only);
    let (window, documents) = b.workspace.snapshot().unwrap();
    vel_session::Session {
        windows: vec![window],
        documents,
        ..Default::default()
    }
    .validate()
    .unwrap();
}

#[test]
fn r20_merge_hot_exit_keeps_edits_during_external_comparison() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("comparison.txt");
    let original = "saved line with spaces\n".repeat(2000);
    std::fs::write(&path, &original).unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
    named(&mut a, "open_file", json!({"file":path}));
    set(&mut a, "large_file.load_bytes", json!(1));
    input(&mut a, "+local");
    let old = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, format!("{original}external\n")).unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(old + Duration::from_secs(2))
        .unwrap();
    drive(&mut a, |a| {
        a.workspace.tabs[a.workspace.active].loading.is_some()
    });
    assert!(a.workspace.tabs[a.workspace.active].external_load);
    input(&mut a, "+latest");
    let expected = a.workspace.editor().text.clone();
    let (window, documents) = a.workspace.snapshot().unwrap();
    assert!(documents[0].history.as_ref().unwrap().retains_text());
    let mut b = app("", "Plain Text", None);
    b.workspace.restore_window(&window, &documents).unwrap();
    drive(&mut b, |a| {
        a.workspace.tabs.iter().all(|t| t.loading.is_none())
    });
    assert_eq!(b.workspace.editor().text, expected);
    assert!(b.workspace.tabs[b.workspace.active].dirty());
    assert!(b.workspace.tabs[b.workspace.active].external.is_some());
}

#[test]
fn r20_merge_background_diff_reference_read_failure_is_non_modal() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("reference.txt");
    std::fs::write(&path, "reference text\n").unwrap();
    let mut a = app("", "Plain Text", None);
    named(&mut a, "open_file", json!({"file":path}));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    a.workspace
        .editor_mut()
        .annotations
        .borrow_mut()
        .refresh_reference();
    a.scene();
    a.workspace.editor().annotations.borrow_mut().flush();
    drive(&mut a, |a| {
        let annotations = a.workspace.editor().annotations.borrow();
        annotations.pending == 0 && annotations.current.is_some()
    });
    assert!(
        a.workspace.messages.is_empty(),
        "{:?}",
        a.workspace.messages
    );
    a.publish_resource_reports();
    assert!(
        a.panels.message.contains("reference.txt"),
        "{}",
        a.panels.message
    );
    assert!(a.panels.message.contains("only regular files"));
    assert_eq!(a.workspace.editor().text, "reference text\n");
}

#[test]
fn r20_merge_legacy_codec_survives_directory_placeholder_and_watch_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.txt");
    let json = include_str!("fixtures/main-385dcdd-matrix/sjis_clean/session.json");
    let value: Value = serde_json::from_str(json).unwrap();
    let old = value["documents"][0]["path"].as_str().unwrap();
    // Only relocate paths in the real main-generated session fixture.
    let json = json.replace(old, path.to_str().unwrap());
    let session: vel_session::Session = serde_json::from_str(&json).unwrap();
    std::fs::create_dir(&path).unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
    a.workspace
        .restore_window(&session.windows[0], &session.documents)
        .unwrap();
    assert!(a.workspace.editor().read_only);
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(
        &path,
        include_bytes!("fixtures/main-385dcdd-matrix/sjis_clean/doc.txt"),
    )
    .unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(mtime + Duration::from_secs(2))
        .unwrap();
    let expected = include_str!("fixtures/main-385dcdd-matrix/sjis_clean/expected.txt");
    drive(&mut a, |a| {
        a.workspace.editor().text == expected && !a.workspace.editor().read_only
    });
    let tab = &a.workspace.tabs[a.workspace.active];
    assert_eq!(tab.format.encoding, "Shift_JIS");
    assert!(!tab.encoding_chosen);
    assert!(a.workspace.messages.is_empty());
}
