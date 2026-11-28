//! Review regressions through dispatch, disk events, restore and status inputs.
use super::round20::drive;
use super::*;

fn png() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0".to_vec()
}

#[test]
fn r20_detected_encoding_redetects_external_rewrites_and_restore() {
    for (original_codec, before, bytes, expected) in [
        (
            "UTF-16 LE with BOM",
            "old\n",
            b"hello!\n\n".to_vec(),
            Some(("hello!\n\n", "UTF-8")),
        ),
        (
            "Western (Windows 1252)",
            "café",
            "café".as_bytes().to_vec(),
            Some(("café", "UTF-8")),
        ),
        ("Western (Windows 1252)", "café", png(), None),
        ("UTF-8", "old\n", b"abc\0binary".to_vec(), None),
        (
            "UTF-8",
            "old\n",
            "hello!\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
            Some(("hello!\n", "UTF-16LE")),
        ),
    ] {
        for background in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("detected.txt");
            let format =
                vel_fileio::encoding_format(original_codec, vel_fileio::LineEnding::Lf).unwrap();
            vel_fileio::save(&path, &Text::from(before), &format, false).unwrap();
            let mut a = app("", "Plain Text", None);
            a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
            named(&mut a, "open_file", json!({"file":path}));
            assert!(!a.workspace.tabs[a.workspace.active].encoding_chosen);
            let (window, documents) = a.workspace.snapshot().unwrap();
            set(
                &mut a,
                "large_file.load_bytes",
                json!(if background { 1 } else { 64 * 1024 * 1024 }),
            );
            std::fs::write(&path, &bytes).unwrap();
            drive(&mut a, |a| match expected {
                Some((text, _)) => {
                    a.workspace.editor().text == text
                        && a.workspace.tabs[a.workspace.active].loading.is_none()
                }
                None => a.panels.message.contains("binary file"),
            });
            assert!(a.workspace.messages.is_empty());
            if let Some((text, codec)) = expected {
                let tab = &a.workspace.tabs[a.workspace.active];
                assert_eq!(tab.editor.text, text);
                assert_eq!(tab.format.encoding, codec);
                assert!(!tab.dirty());
            } else {
                assert_eq!(a.workspace.editor().text, before);
            }
            for history in [false, true] {
                let mut documents = documents.clone();
                if !history {
                    documents
                        .iter_mut()
                        .for_each(|document| document.history = None);
                }
                let wire = serde_json::to_string(&vel_session::Session {
                    windows: vec![window.clone()],
                    documents,
                    ..Default::default()
                })
                .unwrap();
                assert!(a.workspace.messages.is_empty());
                let session: vel_session::Session = serde_json::from_str(&wire).unwrap();
                let mut restored = app("", "Plain Text", None);
                restored.workspace.settings.set(
                    Layer::User,
                    "large_file.load_bytes",
                    json!(if background { 1 } else { 64 * 1024 * 1024 }),
                );
                restored
                    .workspace
                    .restore_window(&session.windows[0], &session.documents)
                    .unwrap();
                drive(&mut restored, |a| {
                    let tab = &a.workspace.tabs[a.workspace.active];
                    tab.loading.is_none() && tab.binary_loading.is_none()
                });
                let tab = &restored.workspace.tabs[restored.workspace.active];
                if let Some((text, codec)) = expected {
                    assert_eq!(tab.editor.text, text);
                    assert_eq!(tab.format.encoding, codec);
                } else {
                    assert!(tab.editor.binary.is_some());
                }
                assert!(!tab.dirty());
                assert!(!tab.encoding_chosen);
            }
        }
    }
}

#[test]
fn r20_user_choice_yields_to_conflicting_bom_and_binary() {
    for binary in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("choice.txt");
        std::fs::write(&path, b"caf\xe9").unwrap();
        let mut a = app("", "Plain Text", None);
        a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
        named(&mut a, "open_file", json!({"file":path}));
        named(&mut a, "reopen", json!({"encoding":"Western (ISO 8859-1)"}));
        let (window, documents) = a.workspace.snapshot().unwrap();
        std::fs::write(
            &path,
            if binary {
                png()
            } else {
                b"\xef\xbb\xbfhello!\n".to_vec()
            },
        )
        .unwrap();
        drive(&mut a, |a| {
            if binary {
                a.panels.message.contains("binary file")
            } else {
                a.workspace.editor().text == "hello!\n"
            }
        });
        if !binary {
            drive(&mut a, |a| a.panels.message.contains("BOM"));
        }
        let session: vel_session::Session = serde_json::from_str(
            &serde_json::to_string(&vel_session::Session {
                windows: vec![window],
                documents,
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let mut restored = app("", "Plain Text", None);
        restored
            .workspace
            .restore_window(&session.windows[0], &session.documents)
            .unwrap();
        drive(&mut restored, |a| {
            let tab = &a.workspace.tabs[a.workspace.active];
            tab.loading.is_none() && tab.binary_loading.is_none()
        });
        if binary {
            assert!(restored.workspace.editor().binary.is_some());
        } else {
            assert_eq!(restored.workspace.editor().text, "hello!\n");
        }
        drive(&mut restored, |a| {
            a.panels
                .message
                .contains(if binary { "binary file" } else { "BOM" })
        });
        assert!(restored.workspace.messages.is_empty());
    }
}

#[test]
fn r20_menu_encoding_bytes_and_selected_alias_survive_restore() {
    for (caption, marker, utf16) in [
        ("UTF-8", b"".as_slice(), None),
        ("UTF-8 with BOM", b"\xef\xbb\xbf", None),
        ("UTF-16 LE", b"", Some(true)),
        ("UTF-16 LE with BOM", b"\xff\xfe", Some(true)),
        ("UTF-16 BE", b"", Some(false)),
        ("UTF-16 BE with BOM", b"\xfe\xff", Some(false)),
        ("Western (ISO 8859-1)", b"", None),
        ("Turkish (ISO 8859-9)", b"", None),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("menu.txt");
        let mut a = app("hello!\n", "Plain Text", None);
        let menu =
            crate::panels::status_format_menu(&a.workspace, vel_ui::status::FormatItem::Encoding);
        let row = menu[1]
            .children
            .iter()
            .find(|row| row.caption == caption)
            .unwrap();
        let mut args = row.args.clone();
        args["file"] = json!(path);
        named(&mut a, row.command.unwrap(), args);
        let body: Vec<u8> = match utf16 {
            Some(le) => "hello!\n"
                .encode_utf16()
                .flat_map(|unit| {
                    if le {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    }
                })
                .collect(),
            None => b"hello!\n".to_vec(),
        };
        assert_eq!(
            std::fs::read(&path).unwrap(),
            [marker, &body].concat(),
            "{caption}"
        );
        named(&mut a, "reopen_encoding", json!({"encoding":caption}));
        assert_eq!(a.workspace.editor().text, "hello!\n");
        assert_eq!(
            a.workspace.tabs[a.workspace.active]
                .format
                .display_encoding(),
            caption
        );
        let menu =
            crate::panels::status_format_menu(&a.workspace, vel_ui::status::FormatItem::Encoding);
        let row = menu[1].children.iter().find(|row| row.checked).unwrap();
        named(&mut a, row.command.unwrap(), row.args.clone());
        assert_eq!(std::fs::read(&path).unwrap(), [marker, &body].concat());
        let (window, documents) = a.workspace.snapshot().unwrap();
        let session: vel_session::Session = serde_json::from_str(
            &serde_json::to_string(&vel_session::Session {
                windows: vec![window],
                documents,
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let mut restored = app("", "Plain Text", None);
        restored
            .workspace
            .restore_window(&session.windows[0], &session.documents)
            .unwrap();
        assert_eq!(
            restored.workspace.tabs[restored.workspace.active]
                .format
                .display_encoding(),
            caption
        );
        assert!(restored.workspace.tabs[restored.workspace.active].encoding_chosen);
    }
}

#[test]
fn r20_failed_explicit_save_cancel_restores_format_and_save_all_recovers_each_tab() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.txt");
    let mut a = app("你好\n", "Plain Text", None);
    named(&mut a, "save", json!({"file":path}));
    let previous = a.workspace.tabs[a.workspace.active].format.clone();
    named(&mut a, "save", json!({"encoding":"Western (Windows 1252)"}));
    drive(&mut a, |a| {
        a.panels
            .quick
            .items
            .iter()
            .any(|row| row.caption == "Save as UTF-8")
    });
    key(&mut a, "escape");
    assert_eq!(a.workspace.tabs[a.workspace.active].format, previous);
    assert!(!a.workspace.tabs[a.workspace.active].encoding_chosen);
    assert!(
        a.workspace.tabs[a.workspace.active]
            .preferred_encoding
            .is_none()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "你好\n");
    let mut paths = Vec::new();
    for index in 0..2 {
        let file = dir.path().join(format!("legacy-{index}"));
        std::fs::write(&file, b"caf\xe9").unwrap();
        named(&mut a, "open_file", json!({"file":file}));
        input(&mut a, "😀");
        paths.push(file);
    }
    named(&mut a, "save_all", json!({}));
    for _ in 0..2 {
        drive(&mut a, |a| {
            a.panels
                .quick
                .items
                .iter()
                .any(|row| row.caption == "Save as UTF-8")
        });
        key(&mut a, "enter");
    }
    for path in paths {
        assert!(std::fs::read_to_string(path).unwrap().contains('😀'));
    }
}

#[test]
fn r20_status_columns_remain_tab_expanded_across_long_line_and_edits() {
    let text = format!("{}abc", "\t".repeat(crate::large::LONG_LINE + 8));
    let mut a = app(&text, "Plain Text", None);
    set(&mut a, "tab_size", json!(8));
    for at in [
        crate::large::LONG_LINE - 1,
        crate::large::LONG_LINE,
        crate::large::LONG_LINE + 1,
        text.len(),
    ] {
        a.workspace.editor_mut().set_caret(at, false);
        let col = if at <= crate::large::LONG_LINE + 8 {
            at * 8 + 1
        } else {
            (crate::large::LONG_LINE + 8) * 8 + at - (crate::large::LONG_LINE + 8) + 1
        };
        assert_eq!(
            crate::panels::status_position(&a.workspace)
                .split(" ·")
                .next(),
            Some(format!("Ln 1, Col {col}").as_str())
        );
    }
    input(&mut a, "\t中");
    let col = (crate::large::LONG_LINE + 8) * 8 + 10;
    assert_eq!(
        crate::panels::status_position(&a.workspace)
            .split(" ·")
            .next(),
        Some(format!("Ln 1, Col {col}").as_str())
    );
    named(&mut a, "undo", json!({}));
    assert_eq!(
        crate::panels::status_position(&a.workspace)
            .split(" ·")
            .next(),
        Some(format!("Ln 1, Col {}", (crate::large::LONG_LINE + 8) * 8 + 4).as_str())
    );
}

#[test]
fn r20_large_restores_publish_utf8_and_shift_jis_prefix_before_completion() {
    use std::io::Write;
    for codec in ["UTF-8", "Japanese (Shift JIS)"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        // Short lines avoid spending the test on long-line display policy. The
        // file is larger than several publication intervals for both codecs.
        let block = "日本語 hello!\n".repeat(65536);
        let encoding = vel_fileio::encoding(codec).unwrap();
        let (bytes, _, errors) = encoding.encode(&block);
        assert!(!errors);
        {
            let mut file = std::fs::File::create(&path).unwrap();
            for _ in 0..128 {
                file.write_all(&bytes).unwrap();
            }
        }
        let mut a = app("", "Plain Text", None);
        set(&mut a, "large_file.load_bytes", json!(usize::MAX));
        // Build a small captured session without first reading the large file.
        let seed = dir.path().join("seed.txt");
        std::fs::write(&seed, b"hello\n").unwrap();
        named(&mut a, "open_file", json!({"file":seed}));
        named(&mut a, "reopen", json!({"encoding":codec}));
        let (window, mut documents) = a.workspace.snapshot().unwrap();
        for document in &mut documents {
            document.path = Some(path.clone());
            document.disk = vel_session::DiskStamp::read(&path);
        }
        for history in [false, true] {
            let mut documents = documents.clone();
            // Capture a disk-backed native history pointed at the actual file.
            for document in &mut documents {
                document.history = history.then(|| {
                    vel_session::History::capture_saved_with_options(
                        &a.workspace.editor().buffer,
                        path.clone(),
                        vel_fileio::DecodeOptions {
                            encoding: Some(codec.into()),
                            ..Default::default()
                        },
                    )
                });
            }
            let session: vel_session::Session = serde_json::from_str(
                &serde_json::to_string(&vel_session::Session {
                    windows: vec![window.clone()],
                    documents,
                    ..Default::default()
                })
                .unwrap(),
            )
            .unwrap();
            let mut restored = app("", "Plain Text", None);
            restored
                .workspace
                .restore_window(&session.windows[0], &session.documents)
                .unwrap();
            assert!(
                restored.workspace.tabs[restored.workspace.active]
                    .loading
                    .is_some(),
                "restore must return before decoding the complete file"
            );
            drive(&mut restored, |a| !a.workspace.editor().text.is_empty());
            let tab = &restored.workspace.tabs[restored.workspace.active];
            assert!(
                tab.loading.is_some(),
                "first prefix must precede completion"
            );
            assert!(tab.editor.read_only);
            assert!(tab.editor.text.len_bytes() < block.len() * 128);
            assert!(tab.editor.text.to_string().starts_with("日本語 hello!\n"));
            // Drop cancels the worker; this test asserts the first snapshot, not total throughput.
        }
    }
}

#[test]
fn r20_pure_insert_minimap_width_is_refreshed_by_next_publication() {
    let mut a = app("let x = 1;\n", "Rust", None);
    drive(&mut a, |a| {
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|language| !language.minimap.is_empty())
    });
    a.workspace.editor_mut().set_caret(10, false);
    input(&mut a, " addition");
    let revision = a.workspace.editor().buffer.change_id();
    let begin = Instant::now();
    drive(&mut a, |a| {
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|language| {
                language
                    .analysis
                    .as_ref()
                    .is_some_and(|analysis| analysis.version == revision)
            })
    });
    let language = a.workspace.editor().language.borrow();
    let language = language.as_ref().unwrap();
    let sample = language.minimap.iter().find(|line| line.row == 0).unwrap();
    let width = sample
        .tokens
        .iter()
        .map(|token| token.column + token.width)
        .max()
        .unwrap_or(0);
    assert_eq!(width, 19);
    // Publication itself is checked by `drive` (bounded by its hang guard); promptness
    // is a timing-run contract.
    crate::test_timing::within(
        begin.elapsed(),
        Duration::from_secs(1),
        "fresh minimap geometry after an ordinary insertion",
    );
}

#[test]
fn r20_clean_history_restore_preserves_soft_undo_and_saved_typing_undo() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.txt");
    let mut a = app("abc", "Plain Text", None);
    input(&mut a, "x");
    named(&mut a, "save", json!({"file":path}));
    a.workspace.editor_mut().set_caret(2, false);
    a.workspace.editor_mut().set_caret(1, false);
    let (window, documents) = a.workspace.snapshot().unwrap();
    let wire = serde_json::to_string(&vel_session::Session {
        windows: vec![window],
        documents,
        ..Default::default()
    })
    .unwrap();
    let commands = [
        "soft_undo",
        "soft_undo",
        "soft_redo",
        "soft_redo",
        "undo",
        "redo",
    ];
    let expected: Vec<_> = commands
        .iter()
        .map(|command| {
            named(&mut a, command, json!({}));
            (
                a.workspace.editor().text.to_string(),
                a.workspace.editor().regions().to_vec(),
                a.workspace.tabs[a.workspace.active].dirty(),
            )
        })
        .collect();
    for background in [false, true] {
        let session: vel_session::Session = serde_json::from_str(&wire).unwrap();
        let mut restored = app("", "Plain Text", None);
        restored.workspace.settings.set(
            Layer::User,
            "large_file.load_bytes",
            json!(if background { 1 } else { 64 * 1024 * 1024 }),
        );
        restored
            .workspace
            .restore_window(&session.windows[0], &session.documents)
            .unwrap();
        drive(&mut restored, |a| {
            a.workspace.tabs[a.workspace.active].loading.is_none()
        });
        for (command, expected) in commands.iter().zip(&expected) {
            named(&mut restored, command, json!({}));
            assert_eq!(
                (
                    restored.workspace.editor().text.to_string(),
                    restored.workspace.editor().regions().to_vec(),
                    restored.workspace.tabs[restored.workspace.active].dirty()
                ),
                *expected,
                "{command}, worker {background}"
            );
        }
    }
}

#[test]
fn r20_explicit_save_aliases_persist_encoding_provenance() {
    for command in ["save", "save_encoding", "prompt_save_as"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("choice.txt");
        let mut a = app("Ã©\n", "Plain Text", None);
        named(
            &mut a,
            command,
            json!({"file":path,"encoding":"Western (ISO 8859-1)"}),
        );
        assert_eq!(std::fs::read(&path).unwrap(), "é\n".as_bytes());
        assert!(
            a.workspace.tabs[a.workspace.active].encoding_chosen,
            "{command}"
        );
        let (window, documents) = a.workspace.snapshot().unwrap();
        let session: vel_session::Session = serde_json::from_str(
            &serde_json::to_string(&vel_session::Session {
                windows: vec![window],
                documents,
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let mut restored = app("", "Plain Text", None);
        restored
            .workspace
            .restore_window(&session.windows[0], &session.documents)
            .unwrap();
        assert_eq!(restored.workspace.editor().text, "Ã©\n", "{command}");
        assert_eq!(
            restored.workspace.tabs[restored.workspace.active]
                .format
                .display_encoding(),
            "Western (ISO 8859-1)"
        );
    }
}
