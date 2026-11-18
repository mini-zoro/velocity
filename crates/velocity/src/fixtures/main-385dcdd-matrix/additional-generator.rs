// verify3: main-format session generator; production snapshot code unmodified.
#[test]
fn r20_revision_export_main_format_sessions() {
    let Some(out) = std::env::var_os("R20_REVISION_MAIN_OUT") else { return };
    let out = std::path::PathBuf::from(out);
    let enc = |text: &str, encoding: &str, bom: bool| -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        vel_fileio::save(
            &p,
            &Text::from(text),
            &vel_fileio::Format { encoding: encoding.into(), bom, line_ending: vel_fileio::LineEnding::Lf },
            false,
        )
        .unwrap();
        std::fs::read(&p).unwrap()
    };
    let cases: Vec<(&str, Vec<u8>, Option<&str>, Option<&str>)> = vec![
        ("utf8_clean", "main naïve ✓ café\n".as_bytes().to_vec(), None, None),
        ("cp1252_clean", b"caf\xe9 cr\xe8me br\xfbl\xe9e\n".to_vec(), None, None),
        ("sjis_clean", enc("日本語かな\n", "Shift_JIS", false), Some("Japanese (Shift JIS)"), None),
        ("gbk_clean", enc("中文简体字\n", "GBK", false), Some("Chinese Simplified (GBK)"), None),
        ("utf16le_clean", enc("hello utf16 text\n", "UTF-16LE", false), None, None),
        ("utf16le_bom_clean", enc("hello bom ✓\n", "UTF-16LE", true), None, None),
        ("cp1252_chosen_clean", b"na\xefve caf\xe9\n".to_vec(), Some("Western (Windows 1252)"), None),
        ("sjis_dirty", enc("日本語かな\n", "Shift_JIS", false), Some("Japanese (Shift JIS)"), Some("編集")),
        ("utf8_dirty", "main naïve ✓ café\n".as_bytes().to_vec(), None, Some(" edited")),
        ("cp1252_dirty", b"caf\xe9 cr\xe8me br\xfbl\xe9e\n".to_vec(), None, Some(" edited")),
        ("utf16le_bom_dirty", enc("hello bom ✓\n", "UTF-16LE", true), None, Some(" edited")),
        ("utf8_bom_dirty", enc("main naïve ✓ café\n", "UTF-8", true), None, Some(" edited")),
    ];
    for (name, bytes, reopen, edit) in cases {
        let dir = out.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.txt");
        std::fs::write(&path, &bytes).unwrap();
        let path = std::fs::canonicalize(&path).unwrap();
        let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
        w.open(FileLocation { path: path.clone(), line: 1, column: 1 }).unwrap();
        if let Some(e) = reopen {
            w.reopen_encoding(e).unwrap();
        }
        if let Some(e) = edit {
            let len = w.editor().text.len_bytes();
            w.editor_mut().set_caret(len, false);
            w.editor_mut().run("insert", &json!({"characters": e})).unwrap();
        }
        std::fs::write(dir.join("expected.txt"), w.editor().text.to_string()).unwrap();
        std::fs::write(dir.join("encoding.txt"), &w.tabs[w.active].format.encoding).unwrap();
        let (window, documents) = w.snapshot().unwrap();
        let session = vel_session::Session { documents, windows: vec![window], ..Default::default() };
        std::fs::write(dir.join("session.json"), serde_json::to_vec_pretty(&session).unwrap()).unwrap();
    }
}
