// Fixture generator only; uses this archive's unmodified production snapshot code.
#[test]
fn export_r20_main_format_shift_jis_session() {
    let path = std::path::PathBuf::from("main-format-shift-jis.txt");
    let text = Text::from("日本語かな\n");
    vel_fileio::save(&path, &text, &vel_fileio::Format { encoding: "Shift_JIS".into(), bom: false, line_ending: vel_fileio::LineEnding::Lf }, false).unwrap();
    let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
    w.open(FileLocation { path: std::fs::canonicalize(&path).unwrap(), line: 1, column: 1 }).unwrap();
    w.reopen_encoding("Japanese (Shift JIS)").unwrap();
    assert_eq!(w.editor().text.to_string(), "日本語かな\n");
    let (window, documents) = w.snapshot().unwrap();
    let session = vel_session::Session { documents, windows: vec![window], ..Default::default() };
    let output = std::env::var_os("R20_MAIN_SESSION_OUTPUT").unwrap();
    std::fs::write(output, serde_json::to_vec_pretty(&session).unwrap()).unwrap();
}
