//! Follow-up review: codec preference, status-only diagnostics and no-op disk events.
use super::round20::drive;
use super::*;

fn drive_load(a: &mut App, ready: impl Fn(&App) -> bool) {
    drive(a, |a| {
        assert!(
            a.workspace.messages.is_empty(),
            "modal queue: {:?}",
            a.workspace.messages
        );
        ready(a)
    });
}
fn encoded(text: &str, encoding: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encoded.txt");
    vel_fileio::save(
        &path,
        &Text::from(text),
        &vel_fileio::encoding_format(encoding, vel_fileio::LineEnding::Lf).unwrap(),
        false,
    )
    .unwrap();
    std::fs::read(path).unwrap()
}
fn restore(a: &mut App, background: bool) -> App {
    let (window, documents) = a.workspace.snapshot().unwrap();
    let wire = serde_json::to_string(&vel_session::Session {
        windows: vec![window],
        documents,
        ..Default::default()
    })
    .unwrap();
    let session: vel_session::Session = serde_json::from_str(&wire).unwrap();
    let mut restored = app("", "Plain Text", None);
    restored.workspace.settings.set(
        Layer::User,
        "large_file.load_bytes",
        json!(if background { 1 } else { 64 * 1024 * 1024 }),
    );
    restored.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
    restored
        .workspace
        .restore_window(&session.windows[0], &session.documents)
        .unwrap();
    drive_load(&mut restored, |a| {
        let tab = &a.workspace.tabs[a.workspace.active];
        tab.loading.is_none() && tab.binary_loading.is_none()
    });
    restored
}

#[test]
fn r20_failed_decode_keeps_preference_across_rewrites_and_restore() {
    for background in [false, true] {
        for restart in [false, true] {
            for choice in ["Japanese (Shift JIS)", "UTF-8"] {
                let original = if choice == "UTF-8" {
                    "original UTF-8 ✓\n"
                } else {
                    "日本語かな\n"
                };
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("preference.txt");
                let original_bytes = encoded(original, choice);
                std::fs::write(&path, &original_bytes).unwrap();
                let mut a = app("", "Plain Text", None);
                a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
                named(&mut a, "open_file", json!({"file":path}));
                named(&mut a, "reopen", json!({"encoding":choice}));
                assert_eq!(a.workspace.editor().text, original);
                set(
                    &mut a,
                    "large_file.load_bytes",
                    json!(if background { 1 } else { 64 * 1024 * 1024 }),
                );
                let (first_bytes, first_text, first_codec) = if choice == "UTF-8" {
                    (b"caf\xe9\n".to_vec(), "café\n", "windows-1252")
                } else {
                    (
                        "naïve café ✓!\n".as_bytes().to_vec(),
                        "naïve café ✓!\n",
                        "UTF-8",
                    )
                };
                let steps = [
                    (first_bytes, first_text, first_codec, false),
                    (
                        encoded("hello UTF-16 中文\n", "UTF-16 LE"),
                        "hello UTF-16 中文\n",
                        "UTF-16LE",
                        false,
                    ),
                    (
                        original_bytes,
                        original,
                        if choice == "UTF-8" {
                            "UTF-8"
                        } else {
                            "Shift_JIS"
                        },
                        true,
                    ),
                    (
                        "naïve café ✓! again\n".as_bytes().to_vec(),
                        "naïve café ✓! again\n",
                        "UTF-8",
                        choice == "UTF-8",
                    ),
                ];
                for (step, (bytes, expected, codec, chosen)) in steps.into_iter().enumerate() {
                    a.panels.set_message(String::new());
                    std::fs::write(&path, bytes).unwrap();
                    drive_load(&mut a, |a| {
                        let tab = &a.workspace.tabs[a.workspace.active];
                        tab.editor.text == expected
                            && tab.loading.is_none()
                            && (chosen || !a.panels.message.is_empty())
                    });
                    let tab = &a.workspace.tabs[a.workspace.active];
                    assert_eq!(tab.format.encoding, codec, "{choice}/{background}/{step}");
                    assert!(tab.encoding_chosen);
                    assert_eq!(tab.preferred_encoding.as_deref(), Some(choice));
                    assert!(!tab.dirty());
                    if !chosen {
                        assert!(!a.panels.message.is_empty());
                    }
                    let (_, documents) = a.workspace.snapshot().unwrap();
                    let document = documents
                        .iter()
                        .find(|d| d.id == a.workspace.tabs[a.workspace.active].session_document)
                        .unwrap();
                    assert_eq!(document.encoding, codec);
                    assert!(document.encoding_chosen);
                    assert_eq!(document.preferred_encoding.as_deref(), Some(choice));
                    if restart && step == 1 {
                        a = restore(&mut a, background);
                        assert_eq!(a.workspace.editor().text, expected);
                        assert!(a.workspace.tabs[a.workspace.active].encoding_chosen);
                    }
                }
            }
        }
    }
}

#[test]
fn r20_load_override_notices_are_status_only_and_identical_across_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notices.txt");
    for (choice, bytes, needle) in [
        (
            None,
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0".to_vec(),
            "binary file",
        ),
        (
            Some("Western (ISO 8859-1)"),
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0".to_vec(),
            "binary file",
        ),
        (
            Some("Western (ISO 8859-1)"),
            b"\xef\xbb\xbfBOM override\n".to_vec(),
            "BOM",
        ),
    ] {
        let mut notices = Vec::new();
        for background in [false, true] {
            std::fs::write(&path, "original\n").unwrap();
            let mut a = app("", "Plain Text", None);
            a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
            named(&mut a, "open_file", json!({"file":path}));
            if let Some(choice) = choice {
                named(&mut a, "reopen", json!({"encoding":choice}));
            }
            let (window, documents) = a.workspace.snapshot().unwrap();
            set(
                &mut a,
                "large_file.load_bytes",
                json!(if background { 1 } else { 64 * 1024 * 1024 }),
            );
            std::fs::write(&path, &bytes).unwrap();
            drive_load(&mut a, |a| a.panels.message.contains(needle));
            assert!(a.panels.overlay.is_none());
            assert!(!a.panels.message.contains("stored encoding ignored") || choice.is_some());
            if needle == "binary file" {
                assert_eq!(a.workspace.editor().text, "original\n");
            } else {
                assert_eq!(a.workspace.editor().text, "BOM override\n");
            }
            notices.push(a.panels.message.clone());
            let mut restored = app("", "Plain Text", None);
            restored.workspace.settings.set(
                Layer::User,
                "large_file.load_bytes",
                json!(if background { 1 } else { 64 * 1024 * 1024 }),
            );
            restored
                .workspace
                .restore_window(&window, &documents)
                .unwrap();
            drive_load(&mut restored, |a| {
                let tab = &a.workspace.tabs[a.workspace.active];
                tab.loading.is_none()
                    && tab.binary_loading.is_none()
                    && (choice.is_none() || !a.panels.message.is_empty())
            });
            assert!(restored.workspace.messages.is_empty());
            assert!(restored.panels.overlay.is_none());
        }
        assert_eq!(notices[0], notices[1], "foreground/background wording");
    }
}

#[test]
fn r20_large_file_touch_keeps_clean_and_dirty_history_and_selections() {
    for background in [false, true] {
        for dirty in [false, true] {
            for auto in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("touch.txt");
                let source = "original 中文\n".repeat(512);
                std::fs::write(&path, &source).unwrap();
                let mut a = app("", "Plain Text", None);
                a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
                named(&mut a, "open_file", json!({"file":path}));
                set(
                    &mut a,
                    "large_file.load_bytes",
                    json!(if background { 1 } else { 64 * 1024 * 1024 }),
                );
                set(&mut a, "reload_file_on_change", json!(auto));
                input(&mut a, "saved");
                a.changed_state(Instant::now(), false);
                named(&mut a, "save", json!({}));
                let saved = a.workspace.editor().text.to_string();
                if dirty {
                    input(&mut a, " local");
                    a.changed_state(Instant::now(), false);
                }
                let caret = a.workspace.editor().text.floor_char_boundary(25);
                a.workspace
                    .editor_mut()
                    .set_selection(Selection::from_regions([
                        Region::new(1, 3),
                        Region::caret(caret),
                    ]));
                let before = a.workspace.editor().text.to_string();
                let buffer = a.workspace.editor().buffer.id();
                let revision = a.workspace.editor().buffer.change_id();
                let view = a.workspace.editor().id;
                let selection = a.workspace.editor().buffer.view(view).sel().clone();
                let old_stamp = a.workspace.tabs[a.workspace.active].disk_saved.clone();
                let modified =
                    std::fs::metadata(&path).unwrap().modified().unwrap() + Duration::from_secs(2);
                std::fs::File::open(&path)
                    .unwrap()
                    .set_modified(modified)
                    .unwrap();
                let expected_stamp = vel_session::DiskStamp::read(&path);
                assert_ne!(expected_stamp, old_stamp);
                drive_load(&mut a, |a| {
                    let tab = &a.workspace.tabs[a.workspace.active];
                    assert!(
                        tab.external.is_none(),
                        "touch must never conflict with edits"
                    );
                    tab.loading.is_none() && tab.disk_saved == expected_stamp
                });
                let tab = &a.workspace.tabs[a.workspace.active];
                assert_eq!(tab.editor.buffer.id(), buffer);
                assert_eq!(tab.editor.buffer.change_id(), revision);
                assert_eq!(tab.editor.id, view);
                assert_eq!(tab.editor.buffer.view(view).sel(), &selection);
                assert_eq!(tab.editor.text, before.as_str());
                assert_eq!(tab.dirty(), dirty);
                assert!(a.panels.message.is_empty(), "{}", a.panels.message);
                named(&mut a, "undo", json!({}));
                if dirty {
                    assert_ne!(a.workspace.editor().text, before.as_str());
                } else {
                    assert_eq!(a.workspace.editor().text, source.as_str());
                }
                named(&mut a, "redo", json!({}));
                assert_eq!(a.workspace.editor().text, before.as_str());
                // A same-sized actual rewrite still takes the conflict/reload path.
                let rewritten = saved.replacen("original", "external", 1);
                assert_eq!(rewritten.len(), saved.len());
                std::fs::write(&path, &rewritten).unwrap();
                drive_load(&mut a, |a| {
                    let tab = &a.workspace.tabs[a.workspace.active];
                    if dirty || !auto {
                        tab.external.is_some()
                    } else {
                        tab.editor.text == rewritten.as_str() && tab.loading.is_none()
                    }
                });
                if dirty || !auto {
                    assert_eq!(a.workspace.editor().text, before.as_str());
                }
            }
        }
    }
}
