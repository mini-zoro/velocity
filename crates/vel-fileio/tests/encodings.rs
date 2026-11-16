use std::fs;
use vel_fileio::{self as io, LineEnding};
use vel_text::Text;
#[test]
fn display_names_work_for_defaults_and_roundtrip_saves_including_boms() {
    let dir = tempfile::tempdir().unwrap();
    for (index, (display, label)) in io::ENCODINGS.iter().enumerate() {
        assert_eq!(
            io::encoding(display).unwrap(),
            io::encoding(label).unwrap(),
            "{display}"
        );
        let settings = vel_config::settings::EditorSettings {
            default_encoding: display.to_string(),
            ..Default::default()
        };
        let format = io::default_format(&settings).unwrap();
        let path = dir.path().join(format!("{index}.txt"));
        io::save(&path, &Text::from("Ascii\n"), &format, false).unwrap();
        let loaded = io::load_with_encoding(&path, display).unwrap();
        assert_eq!(loaded.text.to_string(), "Ascii\n", "{display}");
        assert_eq!(loaded.format.encoding, format.encoding);
        assert_eq!(loaded.format.bom, format.bom);
    }
    assert!(
        io::encoding_format("UTF-16 LE with BOM", LineEnding::Lf)
            .unwrap()
            .bom
    );
    let path = dir.path().join("cyrillic.txt");
    let format = io::encoding_format("Cyrillic (Windows 1251)", LineEnding::CrLf).unwrap();
    io::save(&path, &Text::from("Привет\n"), &format, false).unwrap();
    assert_eq!(
        io::load(&path, "Cyrillic (Windows 1251)", |_| {})
            .unwrap()
            .text
            .to_string(),
        "Привет\n"
    );
}
#[test]
fn invalid_fallback_reports_once_and_invalid_bom_data_uses_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let name = format!("not-an-encoding-{}", dir.path().display());
    let (codec, warning) = io::fallback_encoding(&name);
    assert_eq!(codec.name(), "windows-1252");
    assert!(warning.unwrap().contains(&name));
    assert!(io::fallback_encoding(&name).1.is_none());
    for (index, bytes) in [
        b"\xef\xbb\xbf\x80".as_slice(),
        b"\xff\xfe\x80".as_slice(),
        b"\xfe\xff\x80".as_slice(),
    ]
    .iter()
    .enumerate()
    {
        let path = dir.path().join(format!("invalid-bom-{index}"));
        fs::write(&path, bytes).unwrap();
        for background in [false, true] {
            let loaded = if background {
                io::load_background(path.clone(), name.clone())
                    .unwrap()
                    .result
                    .recv()
                    .unwrap()
                    .unwrap()
            } else {
                io::load(&path, &name, |_| {}).unwrap()
            };
            let expected = encoding_rs::WINDOWS_1252
                .decode_without_bom_handling(bytes)
                .0;
            assert_eq!(loaded.text.to_string(), expected);
            let roundtrip = dir.path().join(format!("roundtrip-{index}-{background}"));
            io::save(&roundtrip, &loaded.text, &loaded.format, false).unwrap();
            assert_eq!(fs::read(roundtrip).unwrap(), *bytes);
            assert_eq!(loaded.format.encoding, "windows-1252");
            assert!(!loaded.format.bom);
        }
    }
}
#[cfg(unix)]
#[test]
fn unreadable_and_missing_background_loads_report_the_path() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private.txt");
    fs::write(&path, "private").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
    if fs::File::open(&path).is_err() {
        let error = io::load_background(path.clone(), "UTF-8".into())
            .unwrap()
            .result
            .recv()
            .unwrap()
            .unwrap_err();
        assert!(error.contains(&path.display().to_string()));
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(
        io::load_background(path.clone(), "UTF-8".into())
            .unwrap_err()
            .contains(&path.display().to_string())
    );
}
