//! Round 20 exercises named commands, native input adapters, disk notifications and restore.
use super::*;

pub(super) fn drive(a: &mut App, ready: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + crate::test_timing::HANG_GUARD;
    loop {
        a.workspace.poll();
        let viewport = a.viewport();
        a.panels.poll(&mut a.workspace, viewport);
        a.sync_active_view();
        if ready(a) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{} / {:?}",
            a.panels.message,
            a.workspace.messages
        );
        std::thread::park_timeout(Duration::from_millis(2));
    }
}

#[test]
fn r20_encoding_and_line_ending_command_dispatch_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("commands.txt");
    std::fs::write(&path, "café\n").unwrap();
    let mut a = app("", "Plain Text", None);
    named(&mut a, "open_file", json!({"file":path}));
    for command in ["set_line_ending", "set_line_endings"] {
        for (value, expected) in [
            ("windows", vel_fileio::LineEnding::CrLf),
            ("unix", vel_fileio::LineEnding::Lf),
            ("cr", vel_fileio::LineEnding::Cr),
        ] {
            assert!(a.command_enabled(command, &json!({"type":value}).to_string()));
            named(&mut a, command, json!({"type":value}));
            assert_eq!(
                a.workspace.tabs[a.workspace.active].format.line_ending,
                expected
            );
            assert!(a.command_checked(command, &json!({"type":value}).to_string()));
            assert!(!a.command_checked(command, "{\"type\":\"invalid\"}"));
        }
    }
    for command in ["save", "save_encoding"] {
        named(
            &mut a,
            command,
            json!({"encoding":"Western (Windows 1252)"}),
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"caf\xe9\r");
        assert!(!a.workspace.tabs[a.workspace.active].dirty());
        named(&mut a, command, json!({"encoding":"UTF-8 with BOM"}));
        assert_eq!(std::fs::read(&path).unwrap(), b"\xef\xbb\xbfcaf\xc3\xa9\r");
    }
    for command in ["reopen", "reopen_encoding"] {
        named(&mut a, command, json!({"encoding":"UTF-8"}));
        assert_eq!(a.workspace.editor().text.to_string(), "café\n");
        assert!(a.workspace.tabs[a.workspace.active].format.bom);
        assert!(!a.workspace.tabs[a.workspace.active].dirty());
    }
    // A copied user binding takes the same alias dispatch, not just direct calls.
    a.workspace
        .editor_mut()
        .keymap
        .append(r#"[{"keys":["ctrl+alt+l"],"command":"set_line_ending","args":{"type":"unix"}}]"#)
        .unwrap();
    key(&mut a, "ctrl+alt+l");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].format.line_ending,
        vel_fileio::LineEnding::Lf
    );
}

#[test]
fn r20_app_encoding_roundtrip_table_and_save_defaults() {
    let dir = tempfile::tempdir().unwrap();
    for (index, (encoding, bom, contents)) in [
        ("UTF-8", false, "ascii 中文 😀\n"),
        ("UTF-8", true, "ascii 中文 😀\n"),
        ("UTF-16LE", false, "ascii 中文 😀\n"),
        ("UTF-16BE", false, "ascii 中文 😀\n"),
        ("UTF-16LE", true, "ascii 中文 😀\n"),
        ("UTF-16BE", true, "ascii 中文 😀\n"),
        ("windows-1252", false, "café €\n"),
    ]
    .into_iter()
    .enumerate()
    {
        for ending in [
            vel_fileio::LineEnding::Lf,
            vel_fileio::LineEnding::CrLf,
            vel_fileio::LineEnding::Cr,
        ] {
            let path = dir.path().join(format!("roundtrip-{index}"));
            let format = vel_fileio::Format {
                encoding: encoding.into(),
                display_name: None,
                bom,
                line_ending: ending,
            };
            vel_fileio::save(&path, &Text::from(contents), &format, false).unwrap();
            let original = std::fs::read(&path).unwrap();
            let mut a = app("", "Plain Text", None);
            named(&mut a, "open_file", json!({"file":path}));
            assert_eq!(a.workspace.editor().text.to_string(), contents);
            assert!(a.workspace.editor().binary.is_none());
            assert_eq!(a.workspace.tabs[a.workspace.active].format, format);
            named(&mut a, "save", json!({}));
            assert_eq!(std::fs::read(path).unwrap(), original);
        }
    }
    let path = dir.path().join("no-break.txt");
    std::fs::write(&path, "abc").unwrap();
    let mut a = app("", "Plain Text", None);
    a.workspace
        .settings
        .set(Layer::User, "default_line_ending", json!("windows"));
    named(&mut a, "open_file", json!({"file":path}));
    assert_eq!(
        a.workspace.tabs[a.workspace.active].format.line_ending,
        vel_fileio::LineEnding::CrLf
    );
    input(&mut a, "\n");
    named(&mut a, "save", json!({}));
    assert_eq!(std::fs::read(path).unwrap(), b"\r\nabc");

    let path = dir.path().join("trim.txt");
    std::fs::write(&path, "untouched  \nchanged\n").unwrap();
    named(&mut a, "open_file", json!({"file":path}));
    set(&mut a, "trim_trailing_white_space_on_save", json!(true));
    assert!(
        a.workspace
            .editor()
            .preferences
            .trim_only_modified_white_space
    );
    a.workspace.editor_mut().set_caret(19, false);
    input(&mut a, "  ");
    named(&mut a, "save", json!({}));
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "untouched  \nchanged\n"
    );
}

#[test]
fn r20_external_rewrites_keep_encoding_and_respect_reload_opt_out() {
    for background in [false, true] {
        for auto in [false, true] {
            for dirty in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("japanese.txt");
                let format =
                    vel_fileio::encoding_format("Japanese (Shift JIS)", vel_fileio::LineEnding::Lf)
                        .unwrap();
                vel_fileio::save(&path, &Text::from("日本語\n"), &format, false).unwrap();
                let mut a = app("", "Plain Text", None);
                a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
                named(&mut a, "open_file", json!({"file":path}));
                named(&mut a, "reopen", json!({"encoding":"Japanese (Shift JIS)"}));
                assert_eq!(a.workspace.editor().text.to_string(), "日本語\n");
                if dirty {
                    input(&mut a, "local ");
                    a.changed_state(Instant::now(), true);
                }
                let before = a.workspace.editor().text.to_string();
                set(
                    &mut a,
                    "large_file.load_bytes",
                    json!(if background { 1 } else { 64 * 1024 * 1024 }),
                );
                set(&mut a, "reload_file_on_change", json!(auto));
                let histories = a.workspace.jump_history.borrow().back.len();
                vel_fileio::save(&path, &Text::from("日本語の変更\n"), &format, false).unwrap();
                drive(&mut a, |a| {
                    if auto && !dirty {
                        a.workspace.editor().text == "日本語の変更\n"
                    } else {
                        a.workspace.tabs[a.workspace.active].external.is_some()
                    }
                });
                let tab = &a.workspace.tabs[a.workspace.active];
                assert_eq!(tab.format.encoding, "Shift_JIS");
                assert_eq!(
                    a.workspace.jump_history.borrow().back.len(),
                    histories,
                    "external reload is not a navigation edit"
                );
                if !auto || dirty {
                    assert_eq!(tab.editor.text.to_string(), before);
                    assert_eq!(tab.dirty(), dirty);
                    assert_eq!(
                        tab.external.as_ref().unwrap().text.to_string(),
                        "日本語の変更\n"
                    );
                    a.workspace.reload_external(a.workspace.active, true);
                    assert_eq!(a.workspace.editor().text.to_string(), "日本語の変更\n");
                }
                assert!(!a.workspace.tabs[a.workspace.active].dirty());
            }
        }
    }
}

#[test]
fn r20_session_restore_uses_stored_encoding_without_becoming_dirty() {
    for (encoding, contents) in [
        ("Japanese (Shift JIS)", "日本語\n"),
        ("Western (Windows 1252)", "Ã©\n"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.txt");
        let format = vel_fileio::encoding_format(encoding, vel_fileio::LineEnding::Lf).unwrap();
        vel_fileio::save(&path, &Text::from(contents), &format, false).unwrap();
        let mut a = app("", "Plain Text", None);
        named(&mut a, "open_file", json!({"file":path}));
        named(&mut a, "reopen", json!({"encoding":encoding}));
        let (window, documents) = a.workspace.snapshot().unwrap();
        for history in [false, true] {
            for background in [false, true] {
                let mut documents = documents.clone();
                if !history {
                    documents.iter_mut().for_each(|doc| doc.history = None);
                }
                let wire = serde_json::to_string(&vel_session::Session {
                    windows: vec![window.clone()],
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
                restored
                    .workspace
                    .restore_window(&session.windows[0], &session.documents)
                    .unwrap();
                drive(&mut restored, |a| {
                    a.workspace.tabs[a.workspace.active].loading.is_none()
                });
                assert_eq!(
                    restored.workspace.editor().text.to_string(),
                    contents,
                    "{encoding} / history {history} / background {background}"
                );
                let tab = &restored.workspace.tabs[restored.workspace.active];
                assert_eq!(tab.format.encoding, format.encoding);
                assert_eq!(tab.saved_format, tab.format);
                assert!(!tab.dirty(), "restoring is not an edit");
            }
        }
    }
}

#[test]
fn r20_unrepresentable_save_offers_explicit_utf8_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.txt");
    std::fs::write(&path, b"\xe9").unwrap();
    let mut a = app("", "Plain Text", None);
    named(&mut a, "open_file", json!({"file":path}));
    input(&mut a, "😀");
    named(&mut a, "save", json!({}));
    assert_eq!(std::fs::read(&path).unwrap(), b"\xe9");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].format.encoding,
        "windows-1252"
    );
    assert!(a.panels.message.contains("cannot be represented"));
    assert!(a.panels.message.contains("Save as UTF-8"));
    assert_eq!(a.panels.overlay, Some(crate::panels::Overlay::Choices));
    drive(&mut a, |a| {
        a.panels
            .quick
            .items
            .iter()
            .any(|item| item.caption == "Save as UTF-8")
    });
    key(&mut a, "escape");
    assert_eq!(std::fs::read(&path).unwrap(), b"\xe9");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].format.encoding,
        "windows-1252"
    );
    assert!(a.workspace.tabs[a.workspace.active].dirty());
    named(&mut a, "save", json!({}));
    drive(&mut a, |a| {
        a.panels
            .quick
            .items
            .iter()
            .any(|item| item.caption == "Save as UTF-8")
    });
    key(&mut a, "enter");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "😀é");
    assert_eq!(
        a.workspace.tabs[a.workspace.active].format.encoding,
        "UTF-8"
    );
    assert!(!a.workspace.tabs[a.workspace.active].dirty());
}

#[test]
fn r20_status_text_and_format_menu_table() {
    let mut a = app("\tab中", "Plain Text", None);
    set(&mut a, "show_encoding", json!(true));
    set(&mut a, "show_line_endings", json!(true));
    for (encoding, bom, ending, expected) in [
        ("UTF-8", false, vel_fileio::LineEnding::Lf, "UTF-8 · Unix"),
        (
            "UTF-8",
            true,
            vel_fileio::LineEnding::CrLf,
            "UTF-8 with BOM · Windows",
        ),
        (
            "UTF-16LE",
            true,
            vel_fileio::LineEnding::Cr,
            "UTF-16 LE with BOM · Mac OS 9",
        ),
        (
            "windows-1252",
            false,
            vel_fileio::LineEnding::Lf,
            "Western (Windows 1252) · Unix",
        ),
    ] {
        a.workspace.tabs[a.workspace.active].format = vel_fileio::Format {
            encoding: encoding.into(),
            display_name: None,
            bom,
            line_ending: ending,
        };
        for (tab, col) in [(4, 8), (8, 12)] {
            set(&mut a, "tab_size", json!(tab));
            assert_eq!(
                crate::panels::status_position(&a.workspace),
                format!("Ln 1, Col {col} · {expected}")
            );
        }
        let menu =
            crate::panels::status_format_menu(&a.workspace, vel_ui::status::FormatItem::Encoding);
        assert_eq!(menu.len(), 2);
        assert!(!menu[0].enabled, "unsaved file cannot reopen");
        assert!(menu[1].enabled);
        assert_eq!(menu[1].children.iter().filter(|row| row.checked).count(), 1);
        let row = menu[1].children.iter().find(|row| row.checked).unwrap();
        assert_eq!(row.command, Some("save"));
        assert_eq!(
            row.args["encoding"],
            a.workspace.tabs[a.workspace.active]
                .format
                .display_encoding()
        );
        let endings = crate::panels::status_format_menu(
            &a.workspace,
            vel_ui::status::FormatItem::LineEndings,
        );
        assert_eq!(endings.len(), 3);
        assert_eq!(endings.iter().filter(|row| row.checked).count(), 1);
        assert!(
            endings
                .iter()
                .all(|row| row.command == Some("set_line_ending"))
        );
    }
    // B1 keeps tab expansion beyond the long-line threshold. The App caches prefixes.
    for (text, tab, expected) in [
        ("\tab", 4, 7),
        ("\tab", 8, 11),
        ("中\t中", 4, 6),
        ("\t\t", 4, 9),
    ] {
        let source = Text::from(text);
        assert_eq!(
            vel_ui::status::column(&source, source.len_bytes(), tab, crate::large::LONG_LINE),
            expected
        );
    }
    let long = Text::from("\t".repeat(crate::large::LONG_LINE * 8));
    assert_eq!(
        vel_ui::status::column(&long, long.len_bytes(), 8, crate::large::LONG_LINE),
        long.len_chars() * 8 + 1
    );
}

#[test]
fn r20_typing_undo_and_soft_undo_keep_their_boundaries() {
    let source = "x\n".repeat(64);
    let mut a = app(&source, "Plain Text", None);
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions(
            (0..64).map(|row| Region::caret(row * 2)),
        ));
    for _ in 0..12 {
        input(&mut a, "a");
        a.changed_state(Instant::now(), true);
    }
    named(&mut a, "undo", json!({}));
    assert_eq!(a.workspace.editor().text.to_string(), source);
    named(&mut a, "redo", json!({}));
    assert_eq!(
        a.workspace.editor().text.len_bytes(),
        source.len() + 12 * 64
    );
    named(&mut a, "soft_undo", json!({}));
    assert_eq!(
        a.workspace.editor().text.len_bytes(),
        source.len() + 11 * 64
    );
    named(&mut a, "soft_redo", json!({}));
    assert_eq!(
        a.workspace.editor().text.len_bytes(),
        source.len() + 12 * 64
    );
    named(&mut a, "undo", json!({}));
    assert_eq!(
        a.workspace.editor().text.len_bytes(),
        source.len() + 11 * 64
    );
    named(&mut a, "undo", json!({}));
    assert_eq!(a.workspace.editor().text.to_string(), source);

    // Preserve the existing word/space/punctuation runs as well as the single
    // word above. CJK is a word run; a space does not join two Latin words.
    for (typing, runs) in [
        ("let x = 1; ", vec!["let ", "x ", "=", " 1", ";", " "]),
        ("你好世界中文输入", vec!["你好世界中文输入"]),
    ] {
        let mut a = app(&source, "Plain Text", None);
        a.workspace
            .editor_mut()
            .set_selection(Selection::from_regions(
                (0..64).map(|row| Region::caret(row * 2)),
            ));
        for ch in typing.chars() {
            input(&mut a, &ch.to_string());
            a.changed_state(Instant::now(), true);
        }
        let mut prefix = typing.len();
        for run in runs.into_iter().rev() {
            prefix -= run.len();
            named(&mut a, "undo", json!({}));
            assert_eq!(
                a.workspace.editor().text.to_string(),
                format!("{}x\n", &typing[..prefix]).repeat(64)
            );
        }
    }
}

// The exact same timing harness is injected into clean 385dcdd for comparison.
#[test]
#[ignore = "1000 full App keys, code with spaces and CJK, wrapped/unwrapped, 1/10000 carets; see MANUAL_TEST.md"]
fn r20_typing_cost_stays_flat() {
    let record = std::env::var_os("VELOCITY_R20_TYPING_RECORD");
    let baseline: Value = std::env::var_os("VELOCITY_R20_TYPING_BASELINE")
        .map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap())
        .unwrap_or(Value::Null);
    assert!(
        record.is_some() || baseline.is_object(),
        "supply matching main measurements; see MANUAL_TEST.md"
    );
    let filter = std::env::var("VELOCITY_R20_TYPING_FILTER").unwrap_or_default();
    let mut report = serde_json::Map::new();
    let mut regressions = Vec::new();
    for (pattern, text) in [("code", "let x = 1; "), ("cjk", "你好世界中文输入")] {
        for (mode, wrap) in [("wrapped", true), ("unwrapped", false)] {
            for carets in [1, 10_000] {
                let name = format!("{pattern}/{mode}/{carets}");
                if !filter.is_empty() && !name.contains(&filter) {
                    continue;
                }
                let source = "x\n".repeat(carets);
                let mut a = app(&source, "Plain Text", None);
                set(&mut a, "word_wrap", json!(wrap));
                set(&mut a, "wrap_width", json!(80));
                set(&mut a, "wrap_width_style", json!("fixed"));
                let editor = a.workspace.editor_mut();
                editor.set_selection(Selection::from_regions(
                    (0..carets).map(|row| Region::caret(row * 2)),
                ));
                editor.buffer.set_history_limits(vel_buffer::HistoryLimits {
                    bytes: 1024 * 1024 * 1024,
                    ..Default::default()
                });
                a.sync_active_view();
                let keys: Vec<_> = text
                    .chars()
                    .map(|c| {
                        let text = c.to_string();
                        let key = Key::parse(if c == ' ' { "space" } else { &text }).unwrap();
                        (key, text)
                    })
                    .collect();
                let mut samples = Vec::new();
                for i in 0..1000 {
                    let (key, text) = &keys[i % keys.len()];
                    let start = Instant::now();
                    let viewport = a.viewport();
                    assert!(!a.dispatch_key_binding(None, key, start, viewport));
                    input(&mut a, text);
                    a.changed_state(start, true);
                    a.workspace.poll();
                    let viewport = a.viewport();
                    a.panels.poll(&mut a.workspace, viewport);
                    samples.push(start.elapsed().as_secs_f64() * 1000.);
                    if samples.len() % 100 == 0 {
                        let mut block = samples[samples.len() - 100..].to_vec();
                        block.sort_by(f64::total_cmp);
                        eprintln!(
                            "r20 App typing: {name}, keys {}-{}, median {:.4} ms",
                            samples.len() - 99,
                            samples.len(),
                            block[50]
                        );
                    }
                }
                let median = |block: &[f64]| {
                    let mut block = block.to_vec();
                    block.sort_by(f64::total_cmp);
                    block[block.len() / 2]
                };
                let first = median(&samples[..100]);
                let last = median(&samples[900..]);
                report.insert(name.clone(), json!({"first100_ms":first,"last100_ms":last,"blocks_ms":samples.chunks(100).map(median).collect::<Vec<_>>()}));
                // Record mode measures main without asserting its known bad growth.
                if baseline.is_object() {
                    let ratio_pass = last <= first * 1.5;
                    let absolute_pass = !wrap && last <= first + 1.0;
                    if carets == 10_000 {
                        eprintln!(
                            "r20 flatness: {name}: {}",
                            if ratio_pass {
                                "ratio <= 1.5"
                            } else if absolute_pass {
                                "unwrapped increase <= 1 ms"
                            } else {
                                "FAIL"
                            }
                        );
                    }
                    if carets == 10_000 && !ratio_pass && !absolute_pass {
                        regressions.push(format!("{name}: first {first:.4} ms, last {last:.4} ms"));
                    }
                    let base = &baseline[&name];
                    let base_first = base["first100_ms"]
                        .as_f64()
                        .expect("matching baseline first100_ms");
                    let base_last = base["last100_ms"]
                        .as_f64()
                        .expect("matching baseline last100_ms");
                    if first > base_first * if carets == 1 { 1.05 } else { 1.0 }
                        || (carets == 1 && last > base_last * 1.05)
                    {
                        regressions.push(format!("{name} regressed: {first:.4}/{last:.4} ms; main {base_first:.4}/{base_last:.4} ms"));
                    }
                }
                let bytes = a.workspace.editor().text.len_bytes();
                let last_bytes = keys[999 % keys.len()].1.len() * carets;
                let editor = a.workspace.editor_mut();
                assert!(editor.buffer.soft_undo(editor.id));
                assert_eq!(editor.buffer.text().len_bytes(), bytes - last_bytes);
                assert!(editor.buffer.soft_redo(editor.id));
                assert_eq!(editor.buffer.text().len_bytes(), bytes);
                // Words/whitespace retain today's separate typing runs. The
                // dedicated command-path test checks their exact undo boundary.
                while editor.buffer.undo(editor.id) {}
                assert_eq!(editor.buffer.text().to_string(), source);
            }
        }
    }
    if let Some(path) = record {
        std::fs::write(path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
    assert!(regressions.is_empty(), "{}", regressions.join("; "));
}
