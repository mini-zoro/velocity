//! Headless contracts through the real editor/tab/scene routes.
use crate::{cli::FileLocation, editor::Editor, view::View, workspace::Workspace};
use std::{
    io::{Seek, SeekFrom, Write},
    time::{Duration, Instant},
};
use vel_config::{json, settings::Layer};
use vel_platform::{Point, Size, TextInput, Utf16Range};
use vel_text::Text;

fn view() -> View {
    View::new(
        Size {
            width: 1200.,
            height: 800.,
        },
        8.,
        12.,
    )
}
fn workspace() -> Workspace {
    Workspace::new(Text::new(), None, None, false).unwrap()
}
fn until(w: &mut Workspace, ready: impl Fn(&Workspace) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready(w) {
        w.poll();
        assert!(Instant::now() < deadline, "{:?}", w.messages);
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
#[test]
fn prefix_scene_is_read_only_save_safe_and_close_cancels_its_generation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.log");
    let mut file = std::fs::File::create(&path).unwrap();
    let block = b"a log line\n".repeat(100_000);
    for _ in 0..128 {
        file.write_all(&block).unwrap();
    }
    drop(file);
    let mut w = workspace();
    w.open(FileLocation {
        path: path.clone(),
        line: 1,
        column: 1,
    })
    .unwrap();
    let cancellation = w.tabs[w.active].loading.as_ref().unwrap().cancellation();
    assert!(
        !w.editor().features.indexing && !w.editor().features.diff_against_disk,
        "known source size applies before the first prefix"
    );
    until(&mut w, |w| w.editor().text.len_bytes() > 0);
    let tab = &w.tabs[w.active];
    assert!(
        tab.loading.is_some(),
        "fixture must still be loading when prefix is published"
    );
    assert!(tab.editor.read_only);
    assert!(!tab.editor.features.indexing && !tab.editor.features.diff_against_disk);
    assert!(tab.progress().unwrap().1 > tab.editor.text.len_bytes() as u64);
    let prefix = tab.editor.text.clone();
    let mut v = view();
    v.scroll(w.editor(), 0., 400.);
    assert!(!v.scene(w.editor()).layers.is_empty());
    w.editor_mut().text_input(TextInput::Insert {
        text: "lost?".into(),
        replacement: None,
    });
    assert_eq!(w.editor().text, prefix);
    assert!(
        w.save(w.active, Some(dir.path().join("unsafe-save")))
            .is_err()
    );
    assert!(!dir.path().join("unsafe-save").exists());
    let (_, documents) = w.snapshot().unwrap();
    assert!(documents.iter().all(|d| d.history.is_none()));
    w.close(w.active).unwrap();
    assert!(cancellation.is_cancelled());
    assert_eq!(w.editor().text.len_bytes(), 0);
    // Late results belong solely to the dropped task, never the new tab.
    w.poll();
    assert_eq!(w.editor().text.len_bytes(), 0);
}
#[test]
fn late_encoding_fallback_discards_prefix_and_preserves_format_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("Packages/User");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Preferences.sublime-settings"),
        "{\"large_file.load_bytes\":1,\"ensure_newline_at_eof_on_save\":false}",
    )
    .unwrap();
    let path = dir.path().join("fallback.txt");
    let mut bytes = b"hello\r\n".repeat(600_000);
    bytes.push(0x80);
    std::fs::write(&path, &bytes).unwrap();
    let mut w = Workspace::new(Text::new(), Some(dir.path().into()), None, false).unwrap();
    w.open(FileLocation {
        path: path.clone(),
        line: 1,
        column: 1,
    })
    .unwrap();
    until(&mut w, |w| w.tabs[w.active].loading.is_none());
    assert_eq!(
        w.editor()
            .text
            .slice(w.editor().text.len_bytes() - 3..w.editor().text.len_bytes())
            .to_string(),
        "€"
    );
    assert_eq!(w.tabs[w.active].format.encoding, "windows-1252");
    assert_eq!(
        w.tabs[w.active].format.line_ending,
        vel_fileio::LineEnding::CrLf
    );
    assert!(!w.editor().read_only);
    let out = dir.path().join("roundtrip.txt");
    w.save(w.active, Some(out.clone())).unwrap();
    assert_eq!(std::fs::read(out).unwrap(), bytes);
}
#[test]
fn indexed_long_line_geometry_selection_motion_and_ime_share_columns_after_edits() {
    let prefix = "界\t👩‍👩‍👧‍👦e\u{301} ".repeat(8000);
    let mut e = Editor::new(Text::from(
        format!("{prefix}target next\nfollowing").as_str(),
    ));
    e.sync();
    let mut v = view();
    e.set_caret(prefix.len(), false);
    crate::large_bench::wait_index(&e, &mut v, 0).unwrap();
    v.reveal(&e);
    let r = v.rect_at(&e, e.caret);
    assert!(r.x >= v.gutter(&e) && r.x < v.size.width);
    assert_eq!(
        v.hit_test(
            &e,
            Point {
                x: r.x,
                y: r.y + 1.
            }
        ),
        e.caret
    );
    let model = vel_largefile::LongLineIndex::new(e.text.clone(), 0, e.preferences.tab_size);
    assert_eq!(
        e.layout.borrow().column(&e.text, e.caret),
        model.byte_to_column(e.caret)
    );
    e.run(
        "move",
        &json!({"by":"word_ends","forward":true,"extend":true}),
    )
    .unwrap();
    assert_eq!(e.selection(), prefix.len()..prefix.len() + 6);
    e.text_input(TextInput::Insert {
        text: "你好".into(),
        replacement: None,
    });
    assert!(
        e.layout.borrow().long.ready(0),
        "ordinary line-local edit updates the existing index"
    );
    v.reveal(&e);
    let before = e.text.clone();
    e.text_input(TextInput::Marked {
        text: "🦀".into(),
        selected: Utf16Range {
            location: 2,
            length: 0,
        },
        replacement: None,
    });
    v.scene(&e);
    let marked = e.marked_utf16().unwrap();
    let (rect, actual) = v.range_rect(&e, marked);
    assert_eq!(actual, marked);
    assert!((rect.width - v.cell * 2.).abs() < 0.001);
    assert!(e.layout.borrow().long.ready(0));
    let marked_bytes = e.marked.clone().unwrap();
    let replacement = Utf16Range::from_bytes(&e.text, marked_bytes.start - 3..marked_bytes.end);
    e.text_input(TextInput::Marked {
        text: "再".into(),
        selected: Utf16Range {
            location: 1,
            length: 0,
        },
        replacement: Some(replacement),
    });
    v.scene(&e);
    crate::large_bench::wait_index(&e, &mut v, 0).unwrap();
    let (rect, _) = v.range_rect(&e, e.marked_utf16().unwrap());
    assert!((rect.width - v.cell * 2.).abs() < 0.001);
    e.cancel_composition();
    assert_eq!(e.text, before);
    v.scene(&e);
    crate::large_bench::wait_index(&e, &mut v, 0).unwrap();
    e.command(vel_platform::Command::End(false));
    assert_eq!(e.caret, crate::layout::body_len(&e.text, 0));
    // Rebuild paths never use an index from a different snapshot.
    e.text_input(TextInput::Insert {
        text: "\n".into(),
        replacement: None,
    });
    v.scene(&e);
    assert!(!e.layout.borrow().long.ready(0));
}
#[test]
fn document_policy_and_explicit_feature_override_are_per_view() {
    let mut e = Editor::new(Text::from("x".repeat(32_000).as_str()));
    e.sync();
    assert_eq!(
        e.features.highlighting,
        vel_largefile::HighlightMode::Viewport
    );
    assert!(
        !e.features.word_wrap
            && !e.features.indexing
            && !e.features.spell_check
            && !e.features.diff_against_disk
    );
    e.attach_language(
        crate::language::Assets::bundled().unwrap(),
        "Rust",
        std::sync::Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
    )
    .unwrap();
    assert_eq!(e.language.borrow().as_ref().unwrap().policy, e.features);
    e.run("large_file_enable_feature", &json!({"feature":"word_wrap"}))
        .unwrap();
    assert!(e.features.word_wrap);
    let other = Editor::new(e.text.clone());
    assert!(!other.features.word_wrap);
    e.run("large_file_enable_feature", &json!({"feature":"indexing"}))
        .unwrap();
    assert!(e.features.indexing);
    assert_eq!(
        e.language.borrow().as_ref().unwrap().policy.highlighting,
        vel_largefile::HighlightMode::Full
    );
    e.settings
        .set(Layer::View, "large_file.viewport_bytes", json!(1));
    e.sync();
    assert!(!e.features.diff_against_disk);
}

#[test]
fn long_line_multi_caret_motion_deletion_and_lf_crossing_preserve_selections() {
    use vel_buffer::{Region, Selection};
    let mut e = Editor::new(Text::from(format!("{}\nend", "a".repeat(40_000)).as_str()));
    let mut v = view();
    crate::large_bench::wait_index(&e, &mut v, 0).unwrap();
    e.set_selection(Selection::from_regions([
        Region::caret(10_000),
        Region::caret(20_000),
    ]));
    e.command(vel_platform::Command::Right(true));
    assert_eq!(
        e.regions(),
        vec![Region::new(10_000, 10_001), Region::new(20_000, 20_001)]
    );
    e.command(vel_platform::Command::DeleteBackward);
    assert_eq!(e.regions().len(), 2);
    assert_eq!(e.text.len_bytes(), 40_002);
    e.set_caret(39_998, false);
    e.command(vel_platform::Command::Right(false));
    assert_eq!(e.caret, 39_999);
    e.command(vel_platform::Command::Left(false));
    assert_eq!(e.caret, 39_998);
}

#[test]
fn hot_exit_restores_binary_and_progressive_tabs_through_the_same_loader() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hex.bin");
    std::fs::write(&path, b"a\0b\xff").unwrap();
    let mut w = workspace();
    w.open(FileLocation {
        path,
        line: 1,
        column: 1,
    })
    .unwrap();
    until(&mut w, |w| w.editor().binary.is_some());
    let (window, docs) = w.snapshot().unwrap();
    let mut restored = workspace();
    restored.restore_window(&window, &docs).unwrap();
    until(&mut restored, |w| w.editor().binary.is_some());
    assert!(restored.editor().read_only);
    assert_eq!(
        restored
            .editor()
            .binary
            .as_ref()
            .unwrap()
            .view
            .row(0)
            .unwrap()
            .bytes,
        b"a\0b\xff"
    );
    let mut panels = crate::panels::Panels::new(&restored, true);
    assert!(
        panels
            .action(&mut restored, "goto_line", &json!({}), 0..0)
            .unwrap()
    );
    assert!(panels.input().is_some());
    panels.layout(Size {
        width: 1200.,
        height: 800.,
    });
    let mut scene = vel_render::scene::Scene::default();
    panels.paint(&restored, &mut scene);
    assert!(!scene.layers.is_empty());
    let path = dir.path().join("reload.log");
    std::fs::write(&path, b"one line\n".repeat(300_000)).unwrap();
    w.settings
        .set(Layer::View, "large_file.load_bytes", json!(1));
    w.open(FileLocation {
        path,
        line: 1,
        column: 1,
    })
    .unwrap();
    let (window, docs) = w.snapshot().unwrap();
    let mut restored = workspace();
    restored
        .settings
        .set(Layer::View, "large_file.load_bytes", json!(1));
    restored.restore_window(&window, &docs).unwrap();
    assert!(restored.tabs[restored.active].loading.is_some());
    assert!(restored.editor().read_only);
    until(&mut restored, |w| w.tabs[w.active].loading.is_none());
    assert!(!restored.editor().read_only);
    assert_eq!(restored.editor().text.len_bytes(), 2_700_000);
}
#[test]
fn sparse_highlighting_repairs_following_lines_without_parsing_on_the_scene_thread() {
    let mut e = Editor::new(Text::from(
        format!("/*{}\n*/ fn after() {{}}\n", "x".repeat(32_000)).as_str(),
    ));
    e.attach_language(
        crate::language::Assets::bundled().unwrap(),
        "Rust",
        std::sync::Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
    )
    .unwrap();
    let mut v = view();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        v.scene(&e);
        if e.language
            .borrow()
            .as_ref()
            .unwrap()
            .styled
            .get(&1)
            .is_some_and(|l| l.exact)
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert!(e.language.borrow().as_ref().unwrap().analysis.is_none());
    assert!(
        e.language
            .borrow()
            .as_ref()
            .unwrap()
            .scopes_ready_at(&e.text, e.text.line_to_byte(1) + 3)
    );
}
#[test]
fn binary_tab_reads_4gb_tail_copies_and_survives_external_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binary.bin");
    let mut f = std::fs::File::create(&path).unwrap();
    #[cfg(windows)]
    crate::binary::make_sparse(&f);
    f.set_len(4_000_000_003).unwrap();
    f.seek(SeekFrom::End(-3)).unwrap();
    f.write_all(b"END").unwrap();
    drop(f);
    let mut w = workspace();
    w.open(FileLocation {
        path: path.clone(),
        line: 1,
        column: 1,
    })
    .unwrap();
    until(&mut w, |w| w.editor().binary.is_some());
    assert!(w.editor().read_only && w.tabs[w.active].label().contains("[hex]"));
    assert_eq!(
        w.editor()
            .binary
            .as_ref()
            .unwrap()
            .view
            .row(125_000_000)
            .unwrap()
            .bytes,
        &[0; 16]
    );
    w.editor_mut()
        .run("goto_hex_offset", &json!({"text":"0xee6b2800"}))
        .unwrap();
    assert_eq!(w.editor().caret, 4_000_000_000);
    for _ in 0..3 {
        w.editor_mut().command(vel_platform::Command::Right(true));
    }
    assert_eq!(w.editor().selection(), 4_000_000_000..4_000_000_003);
    w.editor_mut().run("copy_hex", &json!({})).unwrap();
    assert_eq!(w.editor().clipboard, "45 4e 44");
    w.editor_mut().run("copy_hex_text", &json!({})).unwrap();
    assert_eq!(w.editor().clipboard, "END");
    assert!(w.save(w.active, None).is_err());
    let mut v = view();
    v.reveal(w.editor());
    let r = v.rect_at(w.editor(), 4_000_000_000);
    assert_eq!(
        v.hit_test(
            w.editor(),
            Point {
                x: r.x,
                y: r.y + 1.
            }
        ),
        4_000_000_000
    );
    assert!(!v.scene(w.editor()).layers.is_empty());
    w.editor_mut()
        .run("insert", &json!({"characters":"bad"}))
        .unwrap();
    assert_eq!(
        w.editor().binary.as_ref().unwrap().view.len_bytes(),
        4_000_000_003
    );
    std::fs::write(path, b"changed").unwrap();
    assert_eq!(
        w.editor()
            .binary
            .as_ref()
            .unwrap()
            .view
            .row(250_000_000)
            .unwrap()
            .bytes,
        b"END"
    );
}
