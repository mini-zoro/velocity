use super::*;

fn mark(editor: &mut Editor, text: &str, selected: Utf16Range, replacement: Option<Utf16Range>) {
    editor.text_input(TextInput::Marked {
        text: text.into(),
        selected,
        replacement,
    });
}

#[test]
fn partial_mark_and_accent_insert_keep_surrounding_text() {
    let mut e = Editor::new(Text::from("a-zz"));
    e.set_selection(Selection::from_regions([Region::new(1, 2)]));
    mark(&mut e, "𠮷ne\u{301}", Utf16Range::new(2, 1), None);
    mark(
        &mut e,
        "你",
        Utf16Range::new(0, 1),
        Some(Utf16Range::new(3, 1)),
    );
    assert_eq!(e.text.to_string(), "a𠮷你e\u{301}zz");
    assert_eq!(e.selected_utf16(), Utf16Range::new(3, 1));
    assert_eq!(e.buffer.text().to_string(), "a-zz");
    e.text_input(TextInput::Insert {
        text: "x".into(),
        replacement: Some(Utf16Range::new(3, 1)),
    });
    assert_eq!(e.text.to_string(), "a𠮷xe\u{301}zz");
    e.command(Command::Undo);
    assert_eq!(e.text.to_string(), "a-zz");
    e.text_input(TextInput::Insert {
        text: "é".into(),
        replacement: Some(Utf16Range::new(0, 1)),
    });
    assert_eq!(e.text.to_string(), "é-zz");
    e.command(Command::Undo);
    assert_eq!(e.text.to_string(), "a-zz");
}

#[test]
fn partial_primary_replacement_updates_every_preedit() {
    let original = "abcdefgh\nx!";
    let mut e = Editor::new(Text::from(original));
    let selected = Selection::from_regions([Region::new(0, 8), Region::new(9, 10)]);
    e.set_selection(selected.clone());
    let version = e.buffer.change_id();
    mark(&mut e, "𠮷n", Utf16Range::new(2, 1), None);
    assert_eq!(e.text.to_string(), "𠮷n\n𠮷n!");
    assert_eq!(e.marked_utf16(), Some(Utf16Range::new(4, 3)));
    // The shorter first preview shifts the primary mark left. Address only
    // the low surrogate of the primary emoji; the replacement takes it whole.
    mark(
        &mut e,
        "e\u{301}",
        Utf16Range::new(0, 2),
        Some(Utf16Range::new(5, 1)),
    );
    assert_eq!(e.text.to_string(), "e\u{301}n\ne\u{301}n!");
    assert_eq!(e.marked_ranges(), &[0..4, 5..9]);
    assert_eq!(e.marked_utf16(), Some(Utf16Range::new(4, 3)));
    assert_eq!(e.selected_utf16(), Utf16Range::new(4, 2));
    assert_eq!(e.buffer.change_id(), version);
    e.text_input(TextInput::Insert {
        text: "好".into(),
        replacement: Some(Utf16Range::new(6, 1)),
    });
    e.text_input(TextInput::Unmark);
    assert!(e.marked_ranges().is_empty());
    assert_eq!(e.text.to_string(), "e\u{301}好\ne\u{301}好!");
    e.command(Command::Undo);
    assert_eq!(e.text.to_string(), original);
    assert_eq!(e.buffer.view(e.id).sel(), &selected);
}

#[test]
fn read_only_ignores_preedit_and_commit_without_losing_selection() {
    let mut e = Editor::new(Text::from("read-only"));
    e.set_selection(Selection::from_regions([Region::new(0, 4)]));
    e.read_only = true;
    let version = e.buffer.change_id();
    for text in ["n", "ni", ""] {
        mark(&mut e, text, Utf16Range::default(), None);
    }
    e.text_input(TextInput::Insert {
        text: "你".into(),
        replacement: None,
    });
    e.text_input(TextInput::Unmark);
    assert_eq!(e.text.to_string(), "read-only");
    assert_eq!(e.buffer.change_id(), version);
    assert_eq!(e.selection(), 0..4);
    assert!(e.marked.is_none());
}

#[test]
fn vintage_insert_commits_atomically_and_normal_mode_never_marks() {
    let mut e = Editor::new(Text::from("text"));
    e.settings.set(Layer::User, "ignored_packages", json!([]));
    e.configure_vintage();
    if !e.command_mode() {
        assert!(e.key(&Key::parse("escape").unwrap()).unwrap());
    }
    assert!(e.command_mode());
    mark(&mut e, "n", Utf16Range::new(1, 0), None);
    assert!(e.marked.is_none());
    assert!(e.key(&Key::parse("i").unwrap()).unwrap());
    assert!(!e.command_mode());
    mark(&mut e, "ni", Utf16Range::new(1, 1), None);
    assert!(!e.key(&Key::parse("escape").unwrap()).unwrap());
    e.command(Command::Cancel);
    assert!(!e.command_mode());
    assert_eq!(e.text.to_string(), "text");
    mark(&mut e, "ni", Utf16Range::new(2, 0), None);
    e.text_input(TextInput::Insert {
        text: "你".into(),
        replacement: None,
    });
    assert_eq!(e.text.to_string(), "你text");
    e.command(Command::Undo);
    assert_eq!(e.text.to_string(), "text");
}

#[test]
fn long_line_preedit_geometry_and_atomic_commit() {
    let source = Text::from("a".repeat(crate::large::LONG_LINE + 64).as_str());
    let mut e = Editor::new(source.clone());
    e.settings.set(Layer::View, "word_wrap", json!(false));
    e.sync();
    e.set_caret(source.len_bytes() - 10, false);
    let mut view = crate::view::View::new(
        vel_platform::Size {
            width: 480.,
            height: 180.,
        },
        8.,
        12.,
    );
    crate::large_bench::wait_index(&e, &mut view, 0).unwrap();
    let version = e.buffer.change_id();
    for (text, selection) in [
        ("n", Utf16Range::new(1, 0)),
        ("𠮷ni", Utf16Range::new(2, 2)),
    ] {
        mark(&mut e, text, selection, None);
        view.reveal(&e);
        let marked = e.marked_utf16().unwrap();
        let (r, actual) = view.range_rect(&e, marked);
        assert_eq!(actual, marked);
        assert!(r.x > 0. && r.x + r.width <= view.size.width);
        let scene = view.scene(&e);
        assert!(scene.layers[0].rects.iter().any(|band| {
            band.rect.height == 1.
                && (band.rect.y - (r.y + view.line_height - 2.)).abs() < 0.01
                && band.rect.x < r.x + r.width
                && band.rect.x + band.rect.width > r.x
        }));
        assert_eq!(e.buffer.change_id(), version);
    }
    e.text_input(TextInput::Insert {
        text: "你".into(),
        replacement: None,
    });
    e.text_input(TextInput::Unmark);
    assert_eq!(e.text.len_bytes(), source.len_bytes() + "你".len());
    e.command(Command::Undo);
    assert_eq!(e.text, source);
}
