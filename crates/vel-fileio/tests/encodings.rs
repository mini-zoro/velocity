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

#[test]
fn bomless_utf16_detection_and_roundtrip_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample");
    for codec in ["UTF-16LE", "UTF-16BE"] {
        for ending in [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr] {
            let text = Text::from("ASCII and 中文 😀\nsecond line\n");
            let format = io::Format {
                encoding: codec.into(),
                display_name: None,
                bom: false,
                line_ending: ending,
            };
            io::save(&path, &text, &format, false).unwrap();
            let bytes = fs::read(&path).unwrap();
            assert_ne!(
                io::content_kind(&path).unwrap(),
                vel_largefile::ContentKind::Binary
            );
            for background in [false, true] {
                let loaded = if background {
                    io::load_background(path.clone(), "windows-1252".into())
                        .unwrap()
                        .result
                        .recv()
                        .unwrap()
                        .unwrap()
                } else {
                    io::load(&path, "windows-1252", |_| {}).unwrap()
                };
                assert_eq!(loaded.text, text);
                assert_eq!(loaded.format, format);
                io::save(&path, &loaded.text, &loaded.format, false).unwrap();
                assert_eq!(fs::read(&path).unwrap(), bytes);
            }
        }
    }
    for bytes in [
        b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0".as_slice(),
        b"abc\0binary",
        b"\0\0\0\0",
        b"a\0\0\xd8",
        b"a\0b",
    ] {
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            io::content_kind(&path).unwrap(),
            vel_largefile::ContentKind::Binary,
            "{bytes:?}"
        );
        assert!(io::load(&path, "windows-1252", |_| {}).is_err());
    }
    let mut bounded = vec![b'a'; 8192];
    bounded.extend_from_slice(b"\0\0");
    assert_eq!(
        vel_largefile::detect_content(&bounded),
        vel_largefile::ContentKind::Utf8
    );
}

#[test]
fn preferred_encoding_precedes_detection_and_only_falls_back_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferred");
    let options = io::DecodeOptions {
        encoding: Some("windows-1252".into()),
        ..Default::default()
    };
    fs::write(&path, "é").unwrap();
    for background in [false, true] {
        let loaded = if background {
            io::load_background_with_options(path.clone(), options.clone())
                .unwrap()
                .result
                .recv()
                .unwrap()
                .unwrap()
        } else {
            io::load_with_options(&path, &options, |_| {}).unwrap()
        };
        assert_eq!(loaded.text.to_string(), "Ã©");
        assert_eq!(loaded.format.encoding, "windows-1252");
    }
    fs::write(&path, b"\x80").unwrap();
    let options = io::DecodeOptions {
        encoding: Some("UTF-8".into()),
        ..Default::default()
    };
    for background in [false, true] {
        let loaded = if background {
            io::load_background_with_options(path.clone(), options.clone())
                .unwrap()
                .result
                .recv()
                .unwrap()
                .unwrap()
        } else {
            io::load_with_options(&path, &options, |_| {}).unwrap()
        };
        assert_eq!(loaded.text.to_string(), "€");
        assert_eq!(loaded.format.encoding, "windows-1252");
    }
}

#[test]
fn no_line_break_uses_the_configured_convention_in_every_decoder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no-break");
    for codec in ["UTF-8", "UTF-16LE", "windows-1252"] {
        let format = io::Format {
            encoding: codec.into(),
            ..Default::default()
        };
        io::save(&path, &Text::from("abc"), &format, false).unwrap();
        for preferred in [None, Some(codec.to_owned())] {
            let options = io::DecodeOptions {
                encoding: preferred,
                default_line_ending: LineEnding::CrLf,
                ..Default::default()
            };
            let loaded = io::load_with_options(&path, &options, |_| {}).unwrap();
            assert_eq!(loaded.format.line_ending, LineEnding::CrLf);
            let loaded = io::load_background_with_options(path.clone(), options)
                .unwrap()
                .result
                .recv()
                .unwrap()
                .unwrap();
            assert_eq!(loaded.format.line_ending, LineEnding::CrLf);
        }
    }
}

#[test]
fn bomless_utf16_review_surrogate_endianness_and_controls_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("utf16-review");
    for text in [
        format!("{}😀 end\n", "a".repeat(4095)),
        "😀😃😄😁😆😅😂🤣😊🙂".repeat(400),
        "hello\x0c中文\tworld\r\n".into(),
    ] {
        for (codec, le) in [("UTF-16LE", true), ("UTF-16BE", false)] {
            let bytes: Vec<u8> = text
                .encode_utf16()
                .flat_map(|unit| {
                    if le {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    }
                })
                .collect();
            assert_eq!(vel_largefile::detect_utf16(&bytes), Some(codec));
            fs::write(&path, &bytes).unwrap();
            for background in [false, true] {
                let loaded = if background {
                    io::load_background(path.clone(), "windows-1252".into())
                        .unwrap()
                        .result
                        .recv()
                        .unwrap()
                        .unwrap()
                } else {
                    io::load(&path, "windows-1252", |_| {}).unwrap()
                };
                assert_eq!(loaded.format.encoding, codec);
                assert_eq!(loaded.text.to_string(), text.replace("\r\n", "\n"));
            }
        }
    }
    // Without ASCII or surrogate evidence, byte order of pure BMP CJK is ambiguous.
    for le in [false, true] {
        let bytes: Vec<u8> = "中文漢字"
            .repeat(100)
            .encode_utf16()
            .flat_map(|unit| {
                if le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect();
        assert_eq!(vel_largefile::detect_utf16(&bytes), None);
    }
}
