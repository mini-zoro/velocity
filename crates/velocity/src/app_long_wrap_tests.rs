//! App scene coverage for multi-MiB wrapped editor lines.
use super::*;

#[test]
fn ordinary_fold_button_repaints_use_the_existing_line_text() {
    let mut a = app(
        "fn alpha() {\n    hello();\n}\nfn beta() {\n    world();\n}\n",
        "Rust",
        None,
    );
    set(&mut a, "fade_fold_buttons", json!(false));
    set(&mut a, "word_wrap", json!(false));
    until(&mut a, |a| {
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .ready()
    });
    named(&mut a, "fold_by_level", json!({"level":1}));
    assert!(!a.workspace.editor().folded().is_empty());
    key(&mut a, "super+right");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let gutter = a.view.gutter(a.workspace.editor());
        let scene = a.scene();
        if scene.layers.iter().flat_map(|l| &l.surfaces).any(|s| {
            matches!(s, vel_render::scene::Surface::Image(image) if image.rect.x >= gutter - 18. && image.rect.x < gutter)
        }) {
            break;
        }
        a.workspace.themes.ui.borrow().metrics.poll_images();
        assert!(Instant::now() < deadline, "fold button did not paint");
        std::thread::park_timeout(Duration::from_millis(2));
    }
    assert_eq!(a.view.fold_header_rope_lookups, 0);
}

#[test]
fn wrapped_four_mib_repaints_row_slices_and_matches_fresh_layout_after_typing() {
    let profile = tempfile::tempdir().unwrap();
    let file = profile.path().canonicalize().unwrap().join("min.json");
    let source = format!("[{}0]", "123456789012345,".repeat((4 << 20) / 16));
    std::fs::write(&file, &source).unwrap();
    let mut a = app("", "Plain Text", Some(profile.path()));
    a.window_size = vel_platform::Size {
        width: 900.,
        height: 600.,
    };
    set(&mut a, "index_files", json!(false));
    open_ready(&mut a, &file);
    named(&mut a, "toggle_setting", json!({"setting":"word_wrap"}));
    let _ = a.scene();
    assert!(a.workspace.editor().features.word_wrap);
    assert!(a.workspace.editor().layout.borrow().total() > 1000);
    let renders = |scene: vel_render::scene::Scene| {
        scene
            .layers
            .into_iter()
            .flat_map(|l| l.text)
            .map(|r| r.text)
            .collect::<Vec<_>>()
    };
    for at in [0, source.len() / 2, source.len()] {
        a.workspace.editor_mut().set_caret(at, false);
        a.view.reveal(a.workspace.editor());
        let before = a.view.wrapped_row_bytes;
        let _ = a.scene();
        assert!(a.view.wrapped_row_bytes - before < 128 * 1024);
        let before = a.view.wrapped_row_bytes;
        let start = Instant::now();
        input(&mut a, "7");
        a.changed_state(Instant::now(), true);
        let actual = renders(a.scene());
        if !cfg!(debug_assertions) {
            assert!(
                start.elapsed() < Duration::from_millis(600),
                "typing at {at}: {:?}",
                start.elapsed()
            );
        }
        assert_eq!(a.workspace.editor().text.byte_at(at), b'7');
        assert!((1..128 * 1024).contains(&(a.view.wrapped_row_bytes - before)));
        let e = a.workspace.editor();
        let revision = e.layout.borrow().geometry_revision;
        let cached = a.view.wrapped_row_bytes;
        a.view.poison_wrapped_rows();
        assert_ne!(
            actual,
            renders(a.scene()),
            "oracle must see poisoned text at {at}"
        );
        assert_eq!(
            a.view.wrapped_row_bytes, cached,
            "poison must hit the cache"
        );
        a.view.clear_wrapped_rows();
        a.workspace.editor().layout.borrow_mut().invalidate();
        assert_eq!(
            actual,
            renders(a.scene()),
            "paint mismatch after typing at {at}"
        );
        assert!(a.workspace.editor().layout.borrow().geometry_revision > revision);
        assert!(
            a.view.wrapped_row_bytes > cached,
            "fresh paint measured no rows"
        );
        key(&mut a, "super+z");
    }
}

#[test]
fn setting_tab_size_reflows_a_cached_long_wrapped_line_on_the_next_frame() {
    let mut a = app(&"a\tbb\tccc\t".repeat(2000), "Plain Text", None);
    set(&mut a, "word_wrap", json!(true));
    set(&mut a, "wrap_width", json!(31));
    set(&mut a, "tab_size", json!(4));
    let _ = a.scene();
    let revision = a.workspace.editor().layout.borrow().geometry_revision;
    let old = a.workspace.editor().layout.borrow().total();
    let measured = a.view.wrapped_row_bytes;
    set(&mut a, "tab_size", json!(3));
    let _ = a.scene();
    let e = a.workspace.editor();
    assert_eq!(e.preferences.tab_size, 3);
    let layout = e.layout.borrow();
    assert!(layout.geometry_revision > revision);
    assert_ne!(layout.total(), old);
    assert!(a.view.wrapped_row_bytes > measured);
    let mut fresh = crate::layout::Layout::default();
    fresh.configure(
        &e.text,
        (e.buffer.id(), e.buffer.change_id()),
        crate::view::motion_layout_options(e),
        &[],
    );
    assert_eq!(layout.total(), fresh.total());
    for visual in [0, 42, fresh.total() - 1] {
        let byte = fresh.offset(&e.text, visual, 9.);
        assert_eq!(layout.offset(&e.text, visual, 9.), byte);
        assert_eq!(layout.column(&e.text, byte), fresh.column(&e.text, byte));
    }
}

#[test]
fn folded_tail_uses_projected_invisible_columns_for_paint_and_hit_testing() {
    let tail = "q\u{7}r\u{200b}s";
    let source = format!("fn a() {{\n    work();\n    }}{tail}\n");
    let mut a = app(&source, "Rust", None);
    let mut model = app(tail, "Rust", None);
    for app in [&mut a, &mut model] {
        for (name, value) in [
            ("draw_control_characters", json!(true)),
            ("draw_unicode_white_space", json!("all")),
            ("word_wrap", json!(false)),
        ] {
            set(app, name, value);
        }
        until(app, |app| {
            app.workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .ready()
        });
    }
    let start = source.find(tail).unwrap();
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(8, start)]));
    named(&mut a, "fold", json!({}));
    assert!(!a.workspace.editor().folded().is_empty());
    let scene = a.scene();
    let rendered: Vec<_> = scene
        .layers
        .iter()
        .flat_map(|l| &l.text)
        .map(|r| r.text.as_str())
        .collect();
    assert!(
        rendered.contains(&"<0x07>") && rendered.contains(&"<0x200b>"),
        "{rendered:?}"
    );
    assert!(rendered.iter().all(|s| !s.contains(['\u{7}', '\u{200b}'])));
    let _ = model.scene();
    let e = a.workspace.editor();
    let l = e.layout.borrow();
    let reference = model.workspace.editor().layout.borrow();
    let visual = l.visual_at(&e.text, start).0;
    for (i, _) in tail.char_indices() {
        let col = reference.column(&model.workspace.editor().text, i);
        assert_eq!(l.column(&e.text, start + i), col, "column at {i}");
        assert_eq!(
            l.offset(&e.text, visual, col as f64),
            start + i,
            "hit at {i}"
        );
    }
}

#[test]
fn wrapped_fold_tail_tabs_chips_and_clicks_use_projected_columns() {
    let tail = "q\u{7}\tr\u{200b}s\tz; more words\twith a few more words at the end";
    let source = format!("fn a() {{\n    work();\n    }}{tail}\n");
    let mut a = app(&source, "Rust", None);
    for (name, value) in [
        ("draw_control_characters", json!(true)),
        ("draw_unicode_white_space", json!("all")),
        ("word_wrap", json!(true)),
        ("wrap_width", json!(24)),
        ("tab_size", json!(4)),
    ] {
        set(&mut a, name, value);
    }
    until(&mut a, |a| {
        a.workspace
            .editor()
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .ready()
    });
    let start = source.find(tail).unwrap();
    a.workspace
        .editor_mut()
        .set_selection(Selection::from_regions([Region::new(8, start)]));
    named(&mut a, "fold", json!({}));
    assert!(!a.workspace.editor().folded().is_empty());
    let scene = a.scene();
    let rendered: Vec<_> = scene
        .layers
        .iter()
        .flat_map(|l| &l.text)
        .map(|r| r.text.as_str())
        .collect();
    assert!(
        rendered.contains(&"<0x07>") && rendered.contains(&"<0x200b>"),
        "{rendered:?}"
    );
    let e = a.workspace.editor();
    let layout = e.layout.borrow();
    let row = e.text.byte_to_line(start);
    let line_start = e.text.line_to_byte(row);
    let (first_visual, _) = layout.visual_at(&e.text, start);
    let first = layout.segment(&e.text, row, 0);
    assert_eq!(line_start + first.bytes.start, start);
    assert!(layout.total() > first_visual + 1);
    // 'q' and the control byte consume two source columns; the tab
    // advances from column 2 to 4, regardless of the hidden "    }".
    let visual = layout.visual_at(&e.text, start + 2).0;
    assert_eq!(visual, layout.visual_at(&e.text, start + 3).0);
    let tab_col = layout.column(&e.text, start + 2);
    let next_col = layout.column(&e.text, start + 3);
    assert_eq!(next_col - tab_col, 2);
    assert_eq!(layout.offset(&e.text, visual, next_col as f64), start + 3);
}
