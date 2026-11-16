use proptest::prelude::*;
use vel_buffer::{Buffer, Region, Selection, ops};
use vel_config::settings::EditorSettings;
use vel_fileio::*;
use vel_text::Text;

#[test]
fn progressive_decoder_matches_full_decoder_at_boms_cr_seams_and_encoding_fallbacks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stream");
    for (encoding, bom) in [
        ("UTF-8", false),
        ("UTF-8", true),
        ("UTF-16LE", true),
        ("UTF-16BE", true),
        ("windows-1252", false),
    ] {
        let text = Text::from(format!("{}\n€\nend", "a".repeat(128 * 1024 - 1)).as_str());
        let format = Format {
            encoding: encoding.into(),
            bom,
            line_ending: LineEnding::CrLf,
        };
        save(&path, &text, &format, true).unwrap();
        let task = load_background(path.clone(), "windows-1252".into()).unwrap();
        let loaded = task
            .result
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.text, text, "{format:?}");
        assert_eq!(loaded.format, format);
        assert_eq!(
            task.read.load(std::sync::atomic::Ordering::Relaxed),
            std::fs::metadata(&path).unwrap().len()
        );
    }
    std::fs::write(&path, b"\xff\xfea").unwrap();
    assert!(
        load_background(path, "windows-1252".into())
            .unwrap()
            .result
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .is_err()
    );
}
#[test]
fn encodings_boms_and_endings_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.txt");
    for enc in ["UTF-8", "UTF-16LE", "UTF-16BE", "windows-1252"] {
        for bom in [false, true] {
            if bom && enc == "windows-1252" || !bom && enc.starts_with("UTF-16") {
                continue;
            }
            for ending in [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr] {
                let format = Format {
                    encoding: encoding(enc).unwrap().name().into(),
                    bom,
                    line_ending: ending,
                };
                let text = Text::from(if enc == "windows-1252" {
                    "café €\nnext\n"
                } else {
                    "你好 🦀 é\nnext\n"
                });
                save(&path, &text, &format, true).unwrap();
                let loaded = load(&path, "Western (Windows 1252)", |_| {}).unwrap();
                assert_eq!(loaded.text, text, "{format:?}");
                assert_eq!(loaded.format, format);
            }
        }
    }
    std::fs::write(&path, b"a\r\nb\rc\r\nd\ne").unwrap();
    let loaded = load(&path, "windows-1252", |_| {}).unwrap();
    assert_eq!(loaded.text.to_string(), "a\nb\nc\nd\ne");
    assert_eq!(loaded.format.line_ending, LineEnding::CrLf);
    std::fs::write(&path, b"abc\0binary").unwrap();
    assert!(
        load(&path, "windows-1252", |_| {})
            .unwrap_err()
            .contains("binary")
    );
}
proptest! {
    #[test]
    fn utf8_atomic_roundtrip(text in "[^\\x00\\r]{0,4096}"){
        prop_assume!(!text.starts_with('\u{feff}')); // Leading BOM bytes are encoding metadata on load.
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("text");let source=Text::from(text.as_str());save(&path,&source,&Format::default(),true).unwrap();prop_assert_eq!(load(&path,"windows-1252",|_|{}).unwrap().text,source);
    }
}
#[test]
fn atomic_save_permissions_failure_and_direct_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("target");
    std::fs::write(&path, b"original").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    let impossible = Format {
        encoding: "windows-1252".into(),
        ..Default::default()
    };
    assert!(save(&path, &Text::from("🦀"), &impossible, true).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    save(&path, &Text::from("new\n"), &Format::default(), true).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
    save(&path, &Text::from("direct"), &Format::default(), false).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"direct");
}
#[test]
fn cleanup_tracks_modified_lines_caret_exclusions_and_undo() {
    let mut b = Buffer::new("old  \nedit  \ncaret  ");
    let id = b.new_view();
    let tracker = ChangeTracker::new(&b);
    b.set_selection(id, Selection::from_regions([Region::caret(9)]));
    ops::insert(&mut b, id, "x");
    b.set_selection(
        id,
        Selection::from_regions([Region::caret(b.text().len_bytes())]),
    );
    let settings = EditorSettings {
        trim_trailing_white_space_on_save: vel_config::json!("not_on_caret"),
        trim_only_modified_white_space: true,
        ensure_newline_at_eof_on_save: true,
        ..Default::default()
    };
    let original = b.text().clone();
    prepare_save(&mut b, id, &settings, &tracker);
    assert_eq!(b.text().to_string(), "old  \nedixt\ncaret  \n");
    assert!(b.undo(id));
    assert_eq!(b.text(), &original);
    assert!(b.redo(id));
}
#[test]
fn streaming_seams_and_background_loading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seams");
    let text = format!("{}🦀\nnext\n", "a".repeat(128 * 1024 - 3));
    let format = Format {
        line_ending: LineEnding::CrLf,
        ..Default::default()
    };
    save(&path, &Text::from(text.as_str()), &format, true).unwrap();
    let task = load_background(path, "windows-1252".into()).unwrap();
    let loaded = task
        .result
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(loaded.text.to_string(), text);
    assert_eq!(loaded.format, format);
}

#[test]
fn shifted_unchanged_lines_keep_their_trailing_spaces() {
    let mut buffer = Buffer::new("original  \nnext  \n");
    let id = buffer.new_view();
    let tracker = ChangeTracker::new(&buffer);
    ops::insert(&mut buffer, id, "new  \n");
    assert_eq!(
        tracker.modified_lines(&buffer),
        vec![true, false, false, false]
    );
    let settings = EditorSettings {
        trim_trailing_white_space_on_save: vel_config::json!(true),
        trim_only_modified_white_space: true,
        ..Default::default()
    };
    prepare_save(&mut buffer, id, &settings, &tracker);
    assert_eq!(buffer.text().to_string(), "new\noriginal  \nnext  \n");
}

#[test]
fn watcher_observes_atomic_replacement_with_metadata_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watched.txt");
    std::fs::write(&path, "old").unwrap();
    let mut watcher = Watcher::new().unwrap();
    watcher.watch(&path).unwrap();
    save(&path, &Text::from("changed"), &Format::default(), true).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if watcher.changed().into_iter().any(|event| {
            event
                .unwrap()
                .iter()
                .any(|p| p.file_name() == path.file_name())
        }) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("file watcher did not observe atomic replacement");
}
