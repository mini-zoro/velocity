//! Decode policy through commands, watchers and the actual session serializer.
use super::round20::drive;
use super::*;
#[path = "../../vel-fileio/tests/support/decoding.rs"]
mod model;

fn settled(a: &mut App) {
    drive(a, |a| {
        assert!(
            a.workspace.messages.is_empty(),
            "{:?}",
            a.workspace.messages
        );
        let t = &a.workspace.tabs[a.workspace.active];
        t.loading.is_none() && t.binary_loading.is_none() && t.load_warning.is_none()
    });
}
fn rewrite(a: &mut App, path: &Path, bytes: &[u8]) {
    let old = std::fs::metadata(path).unwrap().modified().unwrap();
    std::fs::write(path, bytes).unwrap();
    let modified = old + Duration::from_secs(2);
    std::fs::File::open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    drive(a, |a| {
        assert!(
            a.workspace.messages.is_empty(),
            "{:?}",
            a.workspace.messages
        );
        let t = &a.workspace.tabs[a.workspace.active];
        t.loading.is_none()
            && t.binary_loading.is_none()
            && t.load_warning.is_none()
            && t.disk_stamp.is_some_and(|(_, time)| time == modified)
    });
    // Notices produced at completion are drained on the following frame.
    settled(a);
}
fn session(a: &mut App) -> vel_session::Session {
    let (window, documents) = a.workspace.snapshot().unwrap();
    serde_json::from_slice(
        &serde_json::to_vec(&vel_session::Session {
            windows: vec![window],
            documents,
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap()
}
fn restore(wire: &vel_session::Session, fallback: &str, background: bool) -> App {
    let mut a = app("", "Plain Text", None);
    a.workspace
        .settings
        .set(Layer::User, "fallback_encoding", json!(fallback));
    a.workspace.settings.set(
        Layer::User,
        "large_file.load_bytes",
        json!(if background { 1 } else { 64 * 1024 * 1024 }),
    );
    a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
    a.workspace
        .restore_window(&wire.windows[0], &wire.documents)
        .unwrap();
    settled(&mut a);
    a
}
fn chosen(a: &mut App, path: &Path, choice: Option<&'static str>, fallback: &str) {
    a.workspace
        .settings
        .set(Layer::User, "fallback_encoding", json!(fallback));
    a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
    let source = model::sample(
        "initial",
        "original text\n",
        choice.unwrap_or("UTF-8"),
        false,
    );
    std::fs::write(path, source.bytes).unwrap();
    named(a, "open_file", json!({"file":path}));
    if let Some(choice) = choice {
        named(a, "reopen", json!({"encoding":choice}));
    }
    settled(a);
    assert_eq!(a.workspace.editor().text, "original text\n");
}
#[test]
fn r20_decode_policy_app_path_table() {
    let samples = model::samples();
    let mut cases = 0;
    // Each meaningful tab state crosses both reload sizes and clean restore with
    // and without native history. Open and Reopen share these same byte samples.
    for sample in &samples {
        for state in 0..8 {
            let choice = match state {
                0 => None,
                1 | 6 | 7 => Some("Japanese (Shift JIS)"),
                2 => Some("UTF-8"),
                3 => Some("Western (Windows 1252)"),
                4 => Some("UTF-16 LE"),
                _ => Some("Chinese Simplified (GBK)"),
            };
            let fallback = if matches!(sample.name, "Shift JIS" | "GBK") {
                sample.codec
            } else {
                "Western (Windows 1252)"
            };
            for background in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("policy-app.txt");
                let mut a = app("", "Plain Text", None);
                chosen(&mut a, &path, choice, fallback);
                if state >= 6 {
                    rewrite(&mut a, &path, "naïve café ✓!".as_bytes());
                    assert!(a.workspace.tabs[a.workspace.active].encoding_chosen);
                    if state == 7 {
                        named(&mut a, "save", json!({}));
                    }
                }
                let preference = a.workspace.tabs[a.workspace.active]
                    .preferred_encoding
                    .clone();
                let expected = model::expected(sample, preference.as_deref(), fallback);
                let before = a.workspace.editor().text.clone();
                let same_bytes = std::fs::read(&path).unwrap() == sample.bytes;
                let mut wire = session(&mut a);
                assert_eq!(
                    wire.documents[0]
                        .history
                        .as_ref()
                        .unwrap()
                        .current_text()
                        .unwrap(),
                    before
                );
                set(
                    &mut a,
                    "large_file.load_bytes",
                    json!(if background { 1 } else { 64 * 1024 * 1024 }),
                );
                a.panels.set_message(String::new());
                rewrite(&mut a, &path, &sample.bytes);
                match &expected {
                    Some((text, codec, user_choice)) => {
                        assert_eq!(
                            &a.workspace.editor().text.to_string(),
                            text,
                            "{} state={state} bg={background}",
                            sample.name
                        );
                        let tab = &a.workspace.tabs[a.workspace.active];
                        assert_eq!(&tab.format.encoding, codec);
                        assert_eq!(tab.encoding_chosen, preference.is_some());
                        assert!(!tab.dirty());
                        if !user_choice && preference.is_some() && !same_bytes {
                            assert!(
                                a.panels.message.contains("policy-app.txt"),
                                "{} state={state} bg={background}: {}",
                                sample.name,
                                a.panels.message
                            );
                        }
                    }
                    None => {
                        assert_eq!(a.workspace.editor().text, before);
                        assert!(a.panels.message.contains("policy-app.txt: binary file"));
                    }
                }
                let notice = a.panels.message.clone();
                for history in [true, false] {
                    if !history {
                        for document in &mut wire.documents {
                            document.history = None;
                        }
                    }
                    let restored = restore(&wire, fallback, background);
                    assert!(restored.panels.overlay.is_none());
                    match &expected {
                        Some((text, codec, _)) => {
                            assert_eq!(
                                &restored.workspace.editor().text.to_string(),
                                text,
                                "restore {} state={state} bg={background} hist={history}",
                                sample.name
                            );
                            assert_eq!(
                                &restored.workspace.tabs[restored.workspace.active]
                                    .format
                                    .encoding,
                                codec
                            );
                            assert_eq!(
                                restored.workspace.tabs[restored.workspace.active].encoding_chosen,
                                preference.is_some()
                            );
                        }
                        None => assert!(restored.workspace.editor().binary.is_some()),
                    }
                    assert_eq!(
                        restored.panels.message, notice,
                        "wording: {} state={state}",
                        sample.name
                    );
                    cases += 1;
                }
                // Explicit Reopen uses the policy as well; it is a new choice,
                // independent of acceptance by a previous ordinary save.
                if let Some(choice) = choice {
                    let mut reopened = app("", "Plain Text", None);
                    chosen(&mut reopened, &path, None, fallback);
                    std::fs::write(&path, &sample.bytes).unwrap();
                    named(&mut reopened, "reopen", json!({"encoding":choice}));
                    settled(&mut reopened);
                    match model::expected(sample, Some(choice), fallback) {
                        Some((text, codec, _)) => {
                            assert_eq!(
                                reopened.workspace.editor().text.to_string(),
                                text,
                                "reopen {} choice={choice}",
                                sample.name
                            );
                            assert_eq!(
                                reopened.workspace.tabs[reopened.workspace.active]
                                    .format
                                    .encoding,
                                codec
                            );
                            assert!(
                                reopened.workspace.tabs[reopened.workspace.active].encoding_chosen
                            );
                        }
                        None => assert!(
                            reopened
                                .panels
                                .message
                                .contains("policy-app.txt: binary file")
                        ),
                    }
                } else {
                    let mut opened = app("", "Plain Text", None);
                    opened.workspace.settings.set(
                        Layer::User,
                        "fallback_encoding",
                        json!(fallback),
                    );
                    opened.workspace.settings.set(
                        Layer::User,
                        "large_file.load_bytes",
                        json!(if background { 1 } else { 64 * 1024 * 1024 }),
                    );
                    named(&mut opened, "open_file", json!({"file":path}));
                    settled(&mut opened);
                    match model::expected(sample, None, fallback) {
                        Some((text, _, _)) => {
                            assert_eq!(opened.workspace.editor().text.to_string(), text)
                        }
                        None => assert!(opened.workspace.editor().binary.is_some()),
                    }
                }
                cases += 2; // reload plus open/Reopen
            }
        }
    }
    assert_eq!(cases, 1344);
}

#[test]
fn r20_save_after_fallback_retains_choice_and_revision_codec_on_touch_and_restart() {
    for background in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accepted.txt");
        let mut a = app("", "Plain Text", None);
        chosen(
            &mut a,
            &path,
            Some("Japanese (Shift JIS)"),
            "Western (Windows 1252)",
        );
        set(
            &mut a,
            "large_file.load_bytes",
            json!(if background { 1 } else { 64 * 1024 * 1024 }),
        );
        rewrite(&mut a, &path, "naïve café ✓!".as_bytes());
        let start = "naïve café".len();
        let end = a.workspace.editor().text.len_bytes();
        a.workspace
            .editor_mut()
            .set_selection(Selection::from_regions([Region::new(start, end)]));
        key(&mut a, "backspace");
        named(&mut a, "save", json!({}));
        assert_eq!(std::fs::read(&path).unwrap(), "naïve café".as_bytes());
        let wire = session(&mut a);
        assert_eq!(
            wire.documents[0].preferred_encoding.as_deref(),
            Some("Japanese (Shift JIS)")
        );
        let id = a.workspace.editor().buffer.id();
        let revision = a.workspace.editor().buffer.change_id();
        rewrite(&mut a, &path, "naïve café".as_bytes());
        assert_eq!(a.workspace.editor().buffer.id(), id);
        assert_eq!(a.workspace.editor().buffer.change_id(), revision);
        assert!(a.workspace.tabs[a.workspace.active].external.is_none());
        assert_eq!(
            a.workspace.tabs[a.workspace.active].format.encoding,
            "UTF-8"
        );
        named(&mut a, "undo", json!({}));
        assert_eq!(a.workspace.editor().text, "naïve café ✓!");
        let mut restored = restore(&wire, "Western (Windows 1252)", background);
        assert_eq!(restored.workspace.editor().text, "naïve café");
        assert_eq!(
            restored.workspace.tabs[restored.workspace.active]
                .format
                .encoding,
            "UTF-8"
        );
        named(&mut restored, "undo", json!({}));
        assert_eq!(restored.workspace.editor().text, "naïve café ✓!");
    }
}

#[test]
fn r20_hot_exit_dirty_touch_uses_persisted_raw_identity() {
    for background in [false, true] {
        for save_first in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("dirty.txt");
            let mut a = app("", "Plain Text", None);
            chosen(&mut a, &path, None, "Western (Windows 1252)");
            if save_first {
                input(&mut a, "saved ");
                named(&mut a, "save", json!({}));
            }
            let disk = std::fs::read(&path).unwrap();
            input(&mut a, "dirty ");
            a.changed_state(Instant::now(), false);
            let expected = a.workspace.editor().text.to_string();
            let wire = session(&mut a);
            assert!(wire.documents[0].dirty);
            assert!(wire.documents[0].disk_content.is_some());
            let mut restored = restore(&wire, "Western (Windows 1252)", background);
            let id = restored.workspace.editor().buffer.id();
            let revision = restored.workspace.editor().buffer.change_id();
            let view = restored.workspace.editor().id;
            let selections = restored.workspace.editor().buffer.view(view).sel().clone();
            rewrite(&mut restored, &path, &disk);
            assert!(
                restored.workspace.tabs[restored.workspace.active]
                    .external
                    .is_none()
            );
            assert_eq!(restored.workspace.editor().buffer.id(), id);
            assert_eq!(restored.workspace.editor().buffer.change_id(), revision);
            assert_eq!(
                restored.workspace.editor().buffer.view(view).sel(),
                &selections
            );
            assert_eq!(restored.workspace.editor().text, expected.as_str());
            assert!(restored.workspace.tabs[restored.workspace.active].dirty());
            named(&mut restored, "undo", json!({}));
            assert_ne!(restored.workspace.editor().text, expected.as_str());
            named(&mut restored, "redo", json!({}));
            assert_eq!(restored.workspace.editor().text, expected.as_str());
            rewrite(&mut restored, &path, b"real external rewrite\n");
            assert!(
                restored.workspace.tabs[restored.workspace.active]
                    .external
                    .is_some()
            );
            assert_eq!(restored.workspace.editor().text, expected.as_str());
        }
    }
}
#[test]
fn r20_one_mixed_line_ending_change_is_a_disk_change() {
    for background in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.txt");
        let source = "one\r\ntwo\r\nthree\r\nfour\r\n";
        std::fs::write(&path, source).unwrap();
        let mut a = app("", "Plain Text", None);
        a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
        named(&mut a, "open_file", json!({"file":path}));
        set(
            &mut a,
            "large_file.load_bytes",
            json!(if background { 1 } else { 64 * 1024 * 1024 }),
        );
        input(&mut a, "local ");
        a.changed_state(Instant::now(), false);
        let before = a.workspace.editor().text.clone();
        rewrite(&mut a, &path, source.replacen("\r\n", "\n", 1).as_bytes());
        let tab = &a.workspace.tabs[a.workspace.active];
        let external = tab.external.as_ref().expect("one raw ending changed");
        assert_eq!(external.format.line_ending, tab.saved_format.line_ending);
        assert_eq!(external.text, "one\ntwo\nthree\nfour\n");
        assert_eq!(a.workspace.editor().text, before);
        assert_ne!(external.disk_content, tab.disk_content);
    }
}

#[test]
fn r20_main_format_shift_jis_session_uses_legacy_hint() {
    // Generated by git archive 385dcdd + main's Workspace::snapshot and serde
    // serializer; see fixtures/main-385dcdd-session.md. No wire keys were edited.
    let source = include_str!("fixtures/main-385dcdd-shift-jis-session.json");
    assert!(!source.contains("encoding_chosen"));
    let original: vel_session::Session = serde_json::from_str(source).unwrap();
    for background in [false, true] {
        for history in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("legacy.txt");
            std::fs::write(
                &path,
                model::sample("legacy", "日本語かな\n", "Japanese (Shift JIS)", false).bytes,
            )
            .unwrap();
            let mut wire = original.clone();
            // Relocate the document to the test fixture; leave the serialized
            // legacy History untouched. Restore supplies its decoded rope.
            wire.documents[0].path = Some(std::fs::canonicalize(&path).unwrap());
            if !history {
                wire.documents[0].history = None;
            }
            let stamp = wire.documents[0].disk.as_ref().unwrap();
            std::fs::File::open(&path)
                .unwrap()
                .set_modified(
                    std::time::SystemTime::UNIX_EPOCH
                        + Duration::from_nanos(stamp.modified_ns as u64),
                )
                .unwrap();
            let mut a = restore(&wire, "Western (Windows 1252)", background);
            assert_eq!(a.workspace.editor().text, "日本語かな\n");
            assert!(!a.workspace.tabs[a.workspace.active].dirty());
            assert!(!a.workspace.tabs[a.workspace.active].encoding_chosen);
            assert_eq!(
                a.workspace.tabs[a.workspace.active].format.encoding,
                "Shift_JIS"
            );
            rewrite(&mut a, &path, "naïve café ✓!\n".as_bytes());
            assert_eq!(a.workspace.editor().text, "naïve café ✓!\n");
        }
    }
}

#[test]
fn r20_saved_effective_codec_does_not_mask_a_new_pending_choice() {
    for background in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pending.txt");
        let mut a = app("", "Plain Text", None);
        chosen(&mut a, &path, None, "Western (Windows 1252)");
        named(&mut a, "save", json!({}));
        named(
            &mut a,
            "set_encoding",
            json!({"encoding":"Japanese (Shift JIS)"}),
        );
        let wire = session(&mut a);
        assert_eq!(wire.documents[0].encoding, "Shift_JIS");
        assert_eq!(
            wire.documents[0].effective_encoding.as_deref(),
            Some("UTF-8")
        );
        assert!(!wire.documents[0].saved_in_effective_encoding);
        let mut restored = restore(&wire, "Western (Windows 1252)", background);
        assert!(restored.workspace.tabs[restored.workspace.active].dirty());
        rewrite(&mut restored, &path, b"original text\n");
        assert!(
            restored.workspace.tabs[restored.workspace.active]
                .external
                .is_none()
        );
        let jis = model::sample(
            "new preference",
            "日本語かな\n",
            "Japanese (Shift JIS)",
            false,
        );
        rewrite(&mut restored, &path, &jis.bytes);
        let external = restored.workspace.tabs[restored.workspace.active]
            .external
            .as_ref()
            .unwrap();
        assert_eq!(external.text, "日本語かな\n");
        assert_eq!(external.format.encoding, "Shift_JIS");
        assert!(external.encoding_chosen);
    }
}

#[test]
fn r20_failed_background_restore_can_recover_the_original_revision() {
    for history in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.txt");
        let mut a = app("", "Plain Text", None);
        chosen(&mut a, &path, None, "Western (Windows 1252)");
        let original = std::fs::read(&path).unwrap();
        let mut wire = session(&mut a);
        if !history {
            wire.documents[0].history = None;
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut restored = restore(&wire, "UTF-8", true);
        assert!(restored.workspace.editor().read_only);
        assert_eq!(restored.workspace.editor().text, "");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        rewrite(&mut restored, &path, &original);
        assert_eq!(restored.workspace.editor().text, "original text\n");
        assert!(!restored.workspace.editor().read_only);
        assert!(!restored.workspace.tabs[restored.workspace.active].dirty());
    }
}
