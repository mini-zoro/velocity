use vel_buffer::{Buffer, ChangeId, Edit, EditKind, MonospaceLayout, Region, Selection, ops};

fn fixture(text: &str, regions: &[Region]) -> (Buffer, vel_buffer::ViewId) {
    let mut b = Buffer::new(text);
    let id = b.new_view();
    b.set_selection(id, Selection::from_regions(regions.iter().copied()));
    (b, id)
}

#[test]
fn noops_preserve_saved_point_redo_regions_and_change_log() {
    for operation in [
        ops::upper_case,
        ops::unindent,
        ops::trim_trailing_whitespace,
    ] {
        let (mut b, id) = fixture("UPPER", &[Region::new(0, 5)]);
        ops::insert(&mut b, id, "OTHER");
        b.undo(id);
        b.mark_saved();
        let version = b.change_id();
        let entries = b.history_entries();
        operation(&mut b, id);
        assert_eq!(b.change_id(), version);
        assert_eq!(b.history_entries(), entries);
        assert!(!b.is_dirty());
        assert!(b.redo(id));
        assert_eq!(b.text().to_string(), "OTHER");
    }
    let (mut b, id) = fixture("ABC", &[Region::caret(0)]);
    assert_eq!(
        ops::delete_left(&mut b, id, ops::DeleteBy::Char),
        ChangeId(0)
    );
    assert!(!b.is_dirty());
    assert!(!b.undo(id));
    b.add_regions(id, "probe", vec![Region::new(0, 2)], false);
    b.transact(
        id,
        EditKind::Other,
        &[
            Edit {
                range: 0..2,
                text: "AB",
            },
            Edit {
                range: 2..3,
                text: "D",
            },
        ],
        Selection::from(Region::caret(3)),
    );
    assert_eq!(
        b.view(id).get_regions("probe").unwrap().regions,
        [Region::new(0, 2)]
    );
    assert_eq!(
        b.changes_since(ChangeId(0)).unwrap()[0].1.changes().len(),
        1
    );
}

#[test]
fn transpose_probe_table_and_circulation_keep_unicode_and_direction() {
    for (source, at, expected, caret) in [
        ("abcd", 2, "acbd", 3),
        ("abcd", 4, "abdc", 4),
        ("abcd", 0, "bacd", 1),
        ("é猫x", 2, "猫éx", 5),
        ("a\u{301}猫x", 3, "猫a\u{301}x", 6),
        ("", 0, "", 0),
    ] {
        let (mut b, id) = fixture(source, &[Region::caret(at)]);
        ops::transpose(&mut b, id);
        assert_eq!(b.text().to_string(), expected, "{source:?} at {at}");
        assert_eq!(b.view(id).sel().primary().unwrap().b, caret);
    }
    let (mut b, id) = fixture(
        "one 猫 three",
        &[Region::new(3, 0), Region::new(4, 7), Region::new(8, 13)],
    );
    ops::transpose(&mut b, id);
    assert_eq!(b.text().to_string(), "three one 猫");
    let regions: Vec<_> = b.view(id).sel().iter().collect();
    assert_eq!(
        regions,
        [Region::new(5, 0), Region::new(6, 9), Region::new(10, 13)]
    );
    assert!(b.undo(id));
    assert_eq!(b.text().to_string(), "one 猫 three");
}

#[test]
fn select_lines_probe_table_maps_both_ends_and_goals() {
    for (r, forward, expected) in [
        (Region::new(1, 3), true, Region::new(6, 8)),
        (Region::new(3, 1), true, Region::new(8, 6)),
        (Region::new(6, 8), false, Region::new(1, 3)),
        (Region::caret(2), true, Region::caret(7)),
    ] {
        let (mut b, id) = fixture("abcd\nabcd\nabcd", &[r]);
        ops::select_lines(&mut b, id, forward, &MonospaceLayout::default());
        assert!(
            b.view(id)
                .sel()
                .iter()
                .any(|r| r.a == expected.a && r.b == expected.b)
        );
        assert_eq!(b.view(id).sel().len(), 2);
    }
}

#[test]
fn permutations_without_selection_cover_all_lines_and_terminal_lf() {
    for terminal in ["", "\n"] {
        for operation in [
            ops::Permutation::Reverse,
            ops::Permutation::Unique,
            ops::Permutation::Shuffle(12),
        ] {
            let source = format!("a\nb\na\nc{terminal}");
            let (mut b, id) = fixture(&source, &[Region::caret(2)]);
            ops::permute_lines(&mut b, id, operation);
            let result = b.text().to_string();
            match operation {
                ops::Permutation::Reverse => assert_eq!(result, format!("c\na\nb\na{terminal}")),
                ops::Permutation::Unique => assert_eq!(result, format!("a\nb\nc{terminal}")),
                ops::Permutation::Shuffle(_) => {
                    let mut lines: Vec<_> = result.lines().collect();
                    lines.sort();
                    assert_eq!(lines, ["a", "a", "b", "c"]);
                    assert_eq!(result.ends_with('\n'), !terminal.is_empty());
                }
            }
        }
    }
}

#[test]
fn wrapping_prefix_probe_table_keeps_comments_and_code_separate() {
    for prefix in ["  // ", "\t# ", "-- ", "/// ", ";; "] {
        let source = format!("{prefix}one two three four\n{prefix}five six\n\ncode\n");
        let (mut b, id) = fixture(&source, &[Region::caret(prefix.len())]);
        ops::wrap_lines_with_prefixes(&mut b, id, 14, &[prefix.trim().to_owned()]);
        let result = b.text().to_string();
        let (comment, tail) = result.split_once("\n\n").unwrap();
        assert!(
            comment.lines().all(|line| line.starts_with(prefix)),
            "{result:?}"
        );
        assert_eq!(tail, "code\n");
        let words = comment
            .lines()
            .map(|line| &line[prefix.len()..])
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(words, "one two three four five six");
        assert!(b.undo(id));
        assert_eq!(b.text().to_string(), source);
    }
}

#[test]
fn join_preserves_interior_selection_and_bracket_spacing() {
    let (mut b, id) = fixture("alpha\n beta", &[Region::new(2, 9)]);
    ops::join_lines(&mut b, id);
    assert_eq!(b.text().to_string(), "alpha beta");
    assert_eq!(b.view(id).sel().primary(), Some(Region::new(2, 8)));
    let (mut b, id) = fixture("foo(\n)", &[Region::caret(3)]);
    ops::join_lines(&mut b, id);
    assert_eq!(b.text().to_string(), "foo()");
}

/// Sublime Text 4215, observed as a black box (2026-11-22): Select All + Join Lines on these
/// files, and ⌘L ⌘L + Join Lines (a selection ending at the first column of line 3).
#[test]
fn join_lines_matches_sublime_black_box() {
    let source = "call(\n    a,\n    b\n)\nx = [\n    1\n]\n{\n    key\n}\nfoo  (\n  bar  )\nend\n";
    let (mut b, id) = fixture(source, &[Region::new(0, source.len())]);
    ops::join_lines(&mut b, id);
    assert_eq!(
        b.text().to_string(),
        "call(a, b) x = [1] {key} foo  (bar  ) end"
    );
    let source = "x = (\n\n    1\n)\nfoo(\n    )\n[ 1,\n  2 ]\n";
    let (mut b, id) = fixture(source, &[Region::new(0, source.len())]);
    ops::join_lines(&mut b, id);
    assert_eq!(b.text().to_string(), "x = (1) foo() [ 1, 2 ]");
    let (mut b, id) = fixture("a\nb\nc\nd\n", &[Region::new(0, 4)]);
    ops::join_lines(&mut b, id);
    assert_eq!(b.text().to_string(), "a b c\nd\n");
}

#[test]
fn bookmark_regions_and_marks_follow_edits_navigation_and_kills() {
    let (mut b, id) = fixture("aa 猫 bb\ncc", &[Region::new(7, 3)]);
    ops::toggle_bookmark(&mut b, id);
    ops::set_mark(&mut b, id);
    b.set_selection(id, Selection::from(Region::caret(0)));
    ops::insert(&mut b, id, "x\n");
    assert_eq!(
        b.view(id).get_regions(ops::BOOKMARKS).unwrap().regions,
        [Region::new(9, 5)]
    );
    ops::next_bookmark(&mut b, id, false);
    assert_eq!(b.view(id).sel().primary(), Some(Region::new(9, 5)));
    ops::select_all_bookmarks(&mut b, id);
    ops::toggle_bookmark(&mut b, id);
    assert!(!ops::next_bookmark(&mut b, id, false));
    b.set_selection(id, Selection::from(Region::caret(2)));
    let killed = ops::delete_to_mark(&mut b, id);
    assert_eq!(killed, ["aa "]);
    ops::yank(&mut b, id, &killed);
    assert_eq!(b.text().to_string(), "x\naa 猫 bb\ncc");
    ops::clear_named_regions(&mut b, id, "mark");
    assert!(!ops::select_to_mark(&mut b, id));
}

#[test]
fn mapped_bookmarks_and_marks_are_stably_sorted_after_replacement() {
    let (mut b, id) = fixture("a\nb\nc\nd\n", &[Region::caret(2), Region::caret(4)]);
    ops::toggle_bookmark(&mut b, id);
    ops::set_mark(&mut b, id);
    b.set_selection(id, Selection::from(Region::new(2, 5)));
    ops::insert(&mut b, id, "X");
    for key in [ops::BOOKMARKS, ops::MARK] {
        assert_eq!(
            b.view(id).get_regions(key).unwrap().regions,
            [Region::caret(2), Region::caret(3)]
        );
    }
    b.set_selection(id, Selection::from(Region::caret(0)));
    ops::next_bookmark(&mut b, id, false);
    assert_eq!(b.view(id).sel().primary(), Some(Region::caret(2)));
    b.set_selection(id, Selection::from(Region::caret(b.text().len_bytes())));
    ops::prev_bookmark(&mut b, id, false);
    assert_eq!(b.view(id).sel().primary(), Some(Region::caret(3)));
    b.set_selection(
        id,
        Selection::from_regions([Region::caret(0), Region::caret(5)]),
    );
    ops::select_to_mark(&mut b, id);
    assert_eq!(
        b.view(id).sel().iter().collect::<Vec<_>>(),
        [Region::new(0, 2), Region::new(5, 3)]
    );

    b.add_regions(
        id,
        "equal-start-order",
        vec![Region::new(2, 3), Region::new(2, 4)],
        false,
    );
    b.set_selection(id, Selection::from(Region::caret(0)));
    ops::insert(&mut b, id, "!");
    assert_eq!(
        b.view(id).get_regions("equal-start-order").unwrap().regions,
        [Region::new(3, 4), Region::new(3, 5)]
    );
}

#[test]
fn delete_to_mark_skips_empty_regions_without_losing_other_carets() {
    let (mut b, id) = fixture("hello", &[Region::caret(3)]);
    ops::set_mark(&mut b, id);
    let before = b.change_id();
    assert!(ops::delete_to_mark(&mut b, id).is_empty());
    assert_eq!(b.text(), "hello");
    assert_eq!(b.change_id(), before);
    assert!(!b.is_dirty());
    assert!(!b.undo(id));
    assert_eq!(b.view(id).sel().primary(), Some(Region::caret(3)));

    let (mut b, id) = fixture("hello world", &[Region::caret(3), Region::caret(9)]);
    ops::set_mark(&mut b, id);
    b.set_selection(
        id,
        Selection::from_regions([Region::caret(3), Region::caret(6)]),
    );
    assert_eq!(ops::delete_to_mark(&mut b, id), ["", "wor"]);
    assert_eq!(b.text(), "hello ld");
    assert_eq!(
        b.view(id).sel().iter().collect::<Vec<_>>(),
        [Region::caret(3), Region::caret(6)]
    );
    assert!(b.undo(id));
    assert_eq!(b.text(), "hello world");
}

#[test]
fn bookmark_toggle_removes_line_touches_and_overlapping_regions() {
    let (mut b, id) = fixture("alpha beta\nsecond line\nthird\n", &[Region::caret(2)]);
    ops::toggle_bookmark(&mut b, id);
    b.set_selection(id, Selection::from(Region::caret(8)));
    ops::toggle_bookmark(&mut b, id);
    assert!(
        b.view(id)
            .get_regions(ops::BOOKMARKS)
            .unwrap()
            .regions
            .is_empty()
    );
    b.set_selection(id, Selection::from(Region::new(18, 3)));
    ops::toggle_bookmark(&mut b, id);
    b.set_selection(id, Selection::from(Region::caret(20)));
    ops::toggle_bookmark(&mut b, id);
    assert!(
        b.view(id)
            .get_regions(ops::BOOKMARKS)
            .unwrap()
            .regions
            .is_empty()
    );
    b.set_selection(id, Selection::from(Region::new(8, 2)));
    ops::toggle_bookmark(&mut b, id);
    b.set_selection(id, Selection::from(Region::new(6, 11)));
    ops::toggle_bookmark(&mut b, id);
    assert!(
        b.view(id)
            .get_regions(ops::BOOKMARKS)
            .unwrap()
            .regions
            .is_empty()
    );
    b.set_selection(
        id,
        Selection::from_regions([Region::caret(2), Region::caret(8)]),
    );
    ops::toggle_bookmark(&mut b, id);
    assert_eq!(
        b.view(id)
            .get_regions(ops::BOOKMARKS)
            .unwrap()
            .regions
            .len(),
        1
    );
    b.set_selection(id, Selection::from(Region::caret(b.text().len_bytes())));
    ops::toggle_bookmark(&mut b, id);
    ops::toggle_bookmark(&mut b, id);
    assert_eq!(
        b.view(id).get_regions(ops::BOOKMARKS).unwrap().regions,
        [Region::caret(2)]
    );
    b.set_selection(id, Selection::from(Region::caret(0)));
    ops::insert(&mut b, id, "moved ");
    b.set_selection(id, Selection::from(Region::caret(0)));
    ops::toggle_bookmark(&mut b, id);
    assert!(
        b.view(id)
            .get_regions(ops::BOOKMARKS)
            .unwrap()
            .regions
            .is_empty()
    );
}
