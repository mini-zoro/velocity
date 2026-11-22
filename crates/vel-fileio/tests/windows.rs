#![cfg(windows)]
use std::{fs, io::Write, os::windows::fs::OpenOptionsExt};
use vel_fileio::{
    Format, LineEnding,
    atomic::{Durability, Stage},
    rename_no_replace, save,
};
use vel_text::Text;
use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

#[test]
fn atomic_save_keeps_original_on_encoding_readonly_and_sharing_failures() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Revision.txt");
    fs::write(&path, "original").unwrap();
    let format = Format::default();
    let text = Text::from("one\ntwo\n");
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .unwrap();
    assert!(save(&path, &text, &format, true).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"original");
    drop(lock);
    let permissions = fs::metadata(&path).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&path, readonly).unwrap();
    assert!(save(&path, &text, &format, true).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"original");
    fs::set_permissions(&path, permissions).unwrap();
    let narrow = Format {
        encoding: "Western (Windows 1252)".into(),
        ..format.clone()
    };
    assert!(save(&path, &Text::from("\u{4f60}"), &narrow, true).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"original");
    save(&path, &text, &format, true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"one\r\ntwo\r\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn staged_create_and_rename_cannot_clobber_a_competing_name() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let from = dir.path().join("source");
    let mut stage = Stage::new(dir.path(), ".stage-", None).unwrap();
    stage.file().write_all(b"new").unwrap();
    stage.sync(Durability::Full).unwrap();
    fs::write(&target, b"competing").unwrap();
    fs::write(&from, b"source").unwrap();
    assert_eq!(
        stage.publish(&target, true).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        rename_no_replace(&from, &target).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&target).unwrap(), b"competing");
    assert_eq!(fs::read(&from).unwrap(), b"source");
}

#[test]
fn system_line_endings_apply_only_when_no_convention_is_detected() {
    assert_eq!(LineEnding::parse("system").unwrap(), LineEnding::CrLf);
    assert_eq!(Format::default().line_ending, LineEnding::CrLf);
    assert_eq!(LineEnding::parse("unix").unwrap(), LineEnding::Lf);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input");
    for (bytes, expected) in [
        (b"no break".as_slice(), LineEnding::CrLf),
        (b"has LF\n", LineEnding::Lf),
    ] {
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            vel_fileio::load_with_options(&path, &Default::default(), &mut |_| {})
                .unwrap()
                .format
                .line_ending,
            expected
        );
    }
}
