use proptest::prelude::*;
use vel_buffer::{Buffer, Region, Selection, ops};
use vel_config::settings::EditorSettings;
use vel_fileio::*;
use vel_text::Text;

/// Hang guard: stops a wait that would otherwise never end. It is not a latency
/// bound; the test checks the condition it waits for.
const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

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
            display_name: None,
            bom,
            line_ending: LineEnding::CrLf,
        };
        save(&path, &text, &format, true).unwrap();
        let task = load_background(path.clone(), "windows-1252".into()).unwrap();
        let loaded = task.result.recv_timeout(HANG_GUARD).unwrap().unwrap();
        assert_eq!(loaded.text, text, "{format:?}");
        assert_eq!(loaded.format, format);
        assert_eq!(
            task.read.load(std::sync::atomic::Ordering::Relaxed),
            std::fs::metadata(&path).unwrap().len()
        );
    }
    std::fs::write(&path, b"\xff\xfea").unwrap();
    let loaded = load_background(path, "windows-1252".into())
        .unwrap()
        .result
        .recv_timeout(HANG_GUARD)
        .unwrap()
        .unwrap();
    // Fallback interprets every original byte, including the invalid BOM prefix.
    assert_eq!(loaded.text.to_string(), "ÿþa");
    assert_eq!(loaded.format.encoding, "windows-1252");
    assert!(!loaded.format.bom);
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
                    display_name: None,
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
    let loaded = task.result.recv_timeout(HANG_GUARD).unwrap().unwrap();
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
    let deadline = std::time::Instant::now() + HANG_GUARD;
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

#[cfg(unix)]
#[test]
fn new_files_get_the_umask_and_atomic_saves_keep_the_file_identity() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    // `File::create` applies the process umask to 0o666, which is what a new save must get.
    let reference = dir.path().join("reference");
    std::fs::File::create(&reference).unwrap();
    let expected = std::fs::metadata(&reference).unwrap().permissions().mode() & 0o777;
    for atomic in [false, true] {
        let path = dir.path().join(format!("new-{atomic}"));
        save(&path, &Text::from("x"), &Format::default(), atomic).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, expected, "atomic {atomic}");
    }
    // A plain file is replaced atomically: a new inode with the old permissions.
    let plain = dir.path().join("plain");
    std::fs::write(&plain, "a").unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o600)).unwrap();
    let inode = std::fs::metadata(&plain).unwrap().ino();
    save(&plain, &Text::from("b"), &Format::default(), true).unwrap();
    let metadata = std::fs::metadata(&plain).unwrap();
    assert_ne!(metadata.ino(), inode);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    // Atomic mode adapts to a journaled write if rename would break a hard link.
    let linked = dir.path().join("linked");
    let other = dir.path().join("other-name");
    std::fs::write(&linked, "a").unwrap();
    std::fs::hard_link(&linked, &other).unwrap();
    let before = std::fs::metadata(&linked).unwrap().ino();
    save(&linked, &Text::from("both"), &Format::default(), true).unwrap();
    assert_eq!(std::fs::metadata(&linked).unwrap().ino(), before);
    assert_eq!(std::fs::metadata(&other).unwrap().ino(), before);
    assert_eq!(std::fs::read(&linked).unwrap(), b"both");
    assert_eq!(std::fs::read(&other).unwrap(), b"both");
    // Extended attributes (Finder tags, quarantine) survive.
    let tagged = dir.path().join("tagged");
    std::fs::write(&tagged, "a").unwrap();
    if rustix::fs::setxattr(
        &tagged,
        "org.velocity.test",
        b"kept",
        rustix::fs::XattrFlags::empty(),
    )
    .is_ok()
    {
        save(&tagged, &Text::from("b"), &Format::default(), true).unwrap();
        let mut value = [0; 8];
        let len = rustix::fs::getxattr(&tagged, "org.velocity.test", &mut value[..]).unwrap();
        assert_eq!(&value[..len], b"kept");
        assert_eq!(std::fs::read(&tagged).unwrap(), b"b");
    }
    // The default writes in place after staging. Read-only files remain protected.
    let direct = dir.path().join("direct");
    std::fs::write(&direct, "a").unwrap();
    let inode = std::fs::metadata(&direct).unwrap().ino();
    save(&direct, &Text::from("b"), &Format::default(), false).unwrap();
    assert_eq!(std::fs::metadata(&direct).unwrap().ino(), inode);
    std::fs::set_permissions(&direct, std::fs::Permissions::from_mode(0o444)).unwrap();
    if std::fs::OpenOptions::new()
        .write(true)
        .open(&direct)
        .is_err()
    {
        for atomic in [false, true] {
            assert!(save(&direct, &Text::from("c"), &Format::default(), atomic).is_err());
            assert_eq!(std::fs::read(&direct).unwrap(), b"b");
        }
    }
    // Unrepresentable characters fail while staging, not while writing the document.
    let impossible = Format {
        encoding: "windows-1252".into(),
        ..Default::default()
    };
    std::fs::write(&plain, "kept").unwrap();
    assert!(save(&plain, &Text::from("🦀"), &impossible, false).is_err());
    assert_eq!(std::fs::read(&plain).unwrap(), b"kept");
}

#[test]
fn trim_all_and_the_system_line_ending() {
    assert_eq!(LineEnding::parse("system").unwrap(), LineEnding::system());
    let mut buffer = Buffer::new("tabs\t \nideographic\u{3000}\nnbsp\u{a0}\n");
    let id = buffer.new_view();
    let tracker = ChangeTracker::new(&buffer);
    let settings = EditorSettings {
        trim_trailing_white_space_on_save: vel_config::json!("all"),
        trim_only_modified_white_space: false,
        ..Default::default()
    };
    prepare_save(&mut buffer, id, &settings, &tracker);
    // Only spaces and tabs are trailing white space; other space characters are content.
    assert_eq!(
        buffer.text().to_string(),
        "tabs\nideographic\u{3000}\nnbsp\u{a0}\n"
    );
}

#[test]
fn default_trimming_preserves_untouched_lines() {
    let mut buffer = Buffer::new("untouched  \nchanged\n");
    let id = buffer.new_view();
    let tracker = ChangeTracker::new(&buffer);
    buffer.set_selection(id, Selection::from_regions([Region::caret(19)]));
    ops::insert(&mut buffer, id, "  ");
    let settings = EditorSettings {
        trim_trailing_white_space_on_save: vel_config::json!(true),
        ..Default::default()
    };
    assert!(settings.trim_only_modified_white_space);
    prepare_save(&mut buffer, id, &settings, &tracker);
    assert_eq!(buffer.text().to_string(), "untouched  \nchanged\n");
}

#[test]
fn rename_no_replace_refuses_existing_targets_but_allows_case_only_changes() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "a").unwrap();
    std::fs::write(&b, "b").unwrap();
    let error = rename_no_replace(&a, &b).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
    // A hard link to the same file under another name is still an existing target.
    let link = dir.path().join("link.txt");
    std::fs::hard_link(&a, &link).unwrap();
    assert!(rename_no_replace(&a, &link).is_err());
    assert!(a.exists() && link.exists());
    let c = dir.path().join("c.txt");
    rename_no_replace(&a, &c).unwrap();
    assert!(!a.exists());
    assert_eq!(std::fs::read_to_string(&c).unwrap(), "a");
    // Works on case-insensitive (default APFS) and case-sensitive volumes alike.
    let upper = dir.path().join("C.txt");
    if !upper.exists() {
        std::fs::hard_link(&c, &upper).unwrap();
        assert_eq!(
            rename_no_replace(&c, &upper).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        std::fs::remove_file(&upper).unwrap();
    }
    rename_no_replace(&c, &upper).unwrap();
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"C.txt".to_string()), "{names:?}");
    assert!(!names.contains(&"c.txt".to_string()), "{names:?}");
    assert_eq!(std::fs::read_to_string(&upper).unwrap(), "a");
}
