use vel_buffer::{
    Buffer, ChangeId, Edit, EditKind, HistoryLimits, MonospaceLayout, Region, Selection, Settings,
    VerticalLayout, ViewId, ops::*,
};
fn setup(text: &str, regions: &[(usize, usize)]) -> (Buffer, ViewId) {
    let mut b = Buffer::new(text);
    let v = b.new_view();
    b.set_selection(
        v,
        Selection::from_regions(regions.iter().map(|&(a, z)| Region::new(a, z))),
    );
    (b, v)
}
fn selected(b: &Buffer, v: ViewId) -> Vec<(usize, usize)> {
    b.view(v).sel().iter().map(|r| (r.a, r.b)).collect()
}
fn expect(b: &Buffer, v: ViewId, text: &str, regions: &[(usize, usize)]) {
    assert_eq!(b.text().to_string(), text);
    assert_eq!(selected(b, v), regions);
}

#[test]
fn insert_replaces_unicode_multicursors_and_paste_distributes() {
    let (mut b, v) = setup("a中b🦀c", &[(1, 4), (5, 9)]);
    insert(&mut b, v, "é");
    expect(&b, v, "aébéc", &[(3, 3), (6, 6)]);
    assert!(b.undo(v));
    paste(&mut b, v, "X\nY");
    expect(&b, v, "aXbYc", &[(2, 2), (4, 4)]);
    assert!(b.undo(v));
    paste(&mut b, v, "X\nY\n");
    expect(&b, v, "aX\nY\nbX\nY\nc", &[(5, 5), (10, 10)]);
}
#[test]
fn normalized_selection_and_primary_direction() {
    let mut s = Selection::new();
    s.add(Region::new(20, 10));
    s.add(Region::new(2, 6));
    assert_eq!(s.primary(), Some(Region::new(2, 6)));
    s.add(Region::new(12, 5));
    assert_eq!(s.iter().collect::<Vec<_>>(), vec![Region::new(20, 2)]);
    s.add(Region::new(30, 25));
    assert_eq!(s.primary(), Some(Region::new(30, 25)));
    s.subtract(Region::new(8, 15));
    assert_eq!(
        s.iter().map(|r| (r.a, r.b)).collect::<Vec<_>>(),
        vec![(8, 2), (20, 15), (30, 25)]
    );
    s.clear();
    assert!(s.is_empty());
    s.normalize();
    assert_eq!(s.primary(), Some(Region::caret(0)));
}
#[test]
fn regions_methods_and_lines() {
    let r = Region::new(8, 2);
    assert_eq!(r.begin(), 2);
    assert_eq!(r.end(), 8);
    assert_eq!(r.size(), 6);
    assert!(r.contains(8));
    assert!(r.contains(Region::new(7, 3)));
    assert_eq!(r.to_range(), 2..8);
    assert_eq!(r.cover(Region::new(10, 12)), Region::new(2, 12));
    assert_eq!(r.intersection(Region::new(5, 10)), Region::new(5, 8));
    assert!(!r.intersects(Region::new(8, 10)));
    assert!(r.intersects(Region::caret(8)));
    assert_eq!(r.intersection(Region::new(20, 22)), Region::caret(0));
    let (b, v) = setup("ab\ncd\n", &[(0, 1), (3, 5), (6, 6)]);
    assert_eq!(b.view(v).sel().regions_on_line(b.text(), 1).count(), 1);
}
#[test]
fn delete_zwj_and_combining_graphemes() {
    let text = "a👩‍👩‍👧‍👦e\u{301}中";
    let emoji_end = 1 + "👩‍👩‍👧‍👦".len();
    let combining_end = emoji_end + "e\u{301}".len();
    let (mut b, v) = setup(
        text,
        &[
            (emoji_end, emoji_end),
            (combining_end + 3, combining_end + 3),
        ],
    );
    delete_left(&mut b, v, DeleteBy::Char);
    expect(&b, v, "ae\u{301}", &[(1, 1), (4, 4)]);
    b.undo(v);
    b.set_selection(
        v,
        Selection::from_regions([Region::caret(1), Region::caret(emoji_end)]),
    );
    delete_right(&mut b, v, DeleteBy::Char);
    expect(&b, v, "a中", &[(1, 1)]);
}
#[test]
fn leading_spaces_backspace_to_tab_stops() {
    let (mut b, v) = setup("      x\n  y", &[(6, 6), (10, 10)]);
    delete_left(&mut b, v, DeleteBy::Char);
    expect(&b, v, "    x\ny", &[(4, 4), (6, 6)]);
    b.undo(v);
    b.set_settings(
        v,
        Settings {
            translate_tabs_to_spaces: false,
            ..Settings::default()
        },
    );
    delete_left(&mut b, v, DeleteBy::Char);
    expect(&b, v, "     x\n y", &[(5, 5), (8, 8)]);
}
#[test]
fn delete_words_forward_backward_and_newlines() {
    let (mut b, v) = setup("foo bar\nzip", &[(3, 3), (8, 8)]);
    delete_word(&mut b, v, true);
    expect(&b, v, "foobar\n", &[(3, 3), (7, 7)]);
    b.undo(v);
    delete_word(&mut b, v, false);
    expect(&b, v, " barzip", &[(0, 0), (4, 4)]);
}
#[test]
fn horizontal_move_collapses_and_extends() {
    let (mut b, v) = setup("ab中d", &[(5, 1)]);
    move_by(&mut b, v, MoveUnit::Characters, true, false, None);
    assert_eq!(selected(&b, v), [(5, 5)]);
    move_by(&mut b, v, MoveUnit::Characters, false, true, None);
    assert_eq!(selected(&b, v), [(5, 2)]);
    move_by(&mut b, v, MoveUnit::Characters, false, false, None);
    assert_eq!(selected(&b, v), [(2, 2)]);
}
#[test]
fn words_subwords_ends_and_stops() {
    let (mut b, v) = setup("fooBar_baz  qux\nend", &[(0, 0)]);
    for p in [3, 6, 7, 10] {
        move_by(&mut b, v, MoveUnit::Subwords, true, false, None);
        assert_eq!(selected(&b, v), [(p, p)]);
    }
    move_by(&mut b, v, MoveUnit::WordEnds, true, false, None);
    assert_eq!(selected(&b, v), [(15, 15)]);
    move_by(&mut b, v, MoveUnit::Words, true, false, None);
    assert_eq!(selected(&b, v), [(16, 16)]);
    move_by(&mut b, v, MoveUnit::Words, false, false, None);
    assert_eq!(selected(&b, v), [(15, 15)]);
    move_by(
        &mut b,
        v,
        MoveUnit::Stops(Stops {
            word_start: true,
            newline: true,
            ..Stops::default()
        }),
        false,
        false,
        None,
    );
    assert_eq!(selected(&b, v), [(12, 12)]);
    b.set_selection(v, Selection::from(Region::caret(10)));
    move_by(&mut b, v, MoveUnit::SubwordEnds, true, false, None);
    assert_eq!(selected(&b, v), [(15, 15)]);
}
#[test]
fn vertical_keeps_goal_through_short_line_tabs_cjk() {
    let (mut b, v) = setup("\t中ab\nx\n\t中cd\nlast", &[(6, 6)]);
    let layout = MonospaceLayout {
        tab_size: 4,
        page_lines: 2,
    };
    assert_eq!(layout.x_of(b.text(), 6), 8.0);
    move_by(&mut b, v, MoveUnit::Lines, true, false, Some(&layout));
    assert_eq!(selected(&b, v), [(8, 8)]);
    assert_eq!(b.view(v).sel().primary().unwrap().xpos, Some(8.0));
    move_by(&mut b, v, MoveUnit::Lines, true, false, Some(&layout));
    assert_eq!(selected(&b, v), [(15, 15)]);
    move_by(&mut b, v, MoveUnit::Pages, false, true, Some(&layout));
    assert_eq!(selected(&b, v), [(15, 6)]);
    move_by(&mut b, v, MoveUnit::Characters, false, false, None);
    assert_eq!(b.view(v).sel().primary().unwrap().xpos, None);
}
#[test]
fn absolute_moves_hard_bol_and_literal_cr() {
    let (mut b, v) = setup("  中x\r\nlast", &[(6, 6)]);
    // Bol is the smart line start: indentation first, then column zero.
    move_to(&mut b, v, MoveTo::Bol, false);
    assert_eq!(selected(&b, v), [(2, 2)]);
    move_to(&mut b, v, MoveTo::Bol, false);
    assert_eq!(selected(&b, v), [(0, 0)]);
    move_to(&mut b, v, MoveTo::Eol, true);
    assert_eq!(selected(&b, v), [(0, 7)]);
    move_to(&mut b, v, MoveTo::HardEol, false);
    assert_eq!(selected(&b, v), [(7, 7)]);
    move_to(&mut b, v, MoveTo::HardBol, false);
    assert_eq!(selected(&b, v), [(0, 0)]);
    move_to(&mut b, v, MoveTo::Eof, false);
    assert_eq!(selected(&b, v), [(12, 12)]);
    move_to(&mut b, v, MoveTo::Bof, true);
    assert_eq!(selected(&b, v), [(12, 0)]);
}
#[test]
fn expand_word_line_indentation_and_split() {
    let (mut b, v) = setup("root\n  foo bar\n  zip\nend", &[(8, 8), (18, 18)]);
    expand_selection(&mut b, v, ExpandTo::Word);
    assert_eq!(selected(&b, v), [(7, 10), (17, 20)]);
    expand_selection(&mut b, v, ExpandTo::Line);
    assert_eq!(selected(&b, v), [(5, 21)]);
    split_selection_into_lines(&mut b, v);
    assert_eq!(selected(&b, v), [(5, 14), (15, 20)]);
    b.set_selection(v, Selection::from(Region::caret(8)));
    expand_selection(&mut b, v, ExpandTo::Indentation);
    assert_eq!(selected(&b, v), [(5, 21)]);
    single_selection(&mut b, v);
    assert_eq!(b.view(v).sel().len(), 1);
}
#[test]
fn primary_survives_document_order_and_single_selection() {
    let (mut b, v) = setup("abcdef", &[(5, 5), (1, 1)]);
    single_selection(&mut b, v);
    assert_eq!(selected(&b, v), [(1, 1)]);
}
#[test]
fn split_selection_backward_empty_and_unterminated() {
    let (mut b, v) = setup("ab\n\n中", &[(7, 0)]);
    split_selection_into_lines(&mut b, v);
    assert_eq!(selected(&b, v), [(2, 0), (3, 3), (7, 4)]);
}
#[test]
fn column_add_handles_tabs_and_wide_characters() {
    let (mut b, v) = setup("\t中\nabcdef\n\t中", &[(4, 4)]);
    let layout = MonospaceLayout::default();
    select_lines(&mut b, v, true, &layout);
    assert_eq!(selected(&b, v), [(4, 4), (11, 11)]);
    select_lines(&mut b, v, true, &layout);
    assert_eq!(selected(&b, v), [(4, 4), (11, 11), (16, 16)]);
}
#[test]
fn find_incremental_wrap_skip_whole_word_and_case() {
    let (mut b, v) = setup("foo foobar Foo foo foo", &[(0, 0)]);
    assert!(find_under_expand(&mut b, v));
    assert_eq!(selected(&b, v), [(0, 3)]);
    assert!(find_under_expand(&mut b, v));
    assert_eq!(selected(&b, v), [(0, 3), (15, 18)]);
    assert!(find_under_expand(&mut b, v));
    assert!(!find_under_expand(&mut b, v));
    assert_eq!(b.view(v).sel().len(), 3);
    b.set_selection(v, Selection::from(Region::new(15, 18)));
    assert!(find_under_expand(&mut b, v));
    assert!(find_under_expand(&mut b, v));
    assert_eq!(b.view(v).sel().len(), 3);
}
#[test]
fn find_all_selected_literals_and_caret_words() {
    let (mut b, v) = setup("中 中x 中\n中", &[(0, 0)]);
    find_all_under(&mut b, v);
    assert_eq!(b.view(v).sel().len(), 3);
    b.set_selection(v, Selection::from(Region::new(0, 3)));
    find_all_under(&mut b, v);
    assert_eq!(b.view(v).sel().len(), 4);
}
#[test]
fn find_across_chunks_and_after_edit() {
    let text = format!("{}needle{}needle", "x".repeat(4093), "_".repeat(5000));
    let (mut b, v) = setup(&text, &[(4093, 4099)]);
    assert!(find_under_expand(&mut b, v));
    assert_eq!(b.view(v).sel().len(), 2);
    insert(&mut b, v, "X");
    assert!(b.undo(v));
    assert_eq!(b.view(v).sel().len(), 2);
}
#[test]
fn indent_unindent_trim_multiselect_lines_once() {
    let (mut b, v) = setup("a \r\n 中\t\nlast", &[(0, 1), (2, 3), (5, 8)]);
    indent(&mut b, v);
    assert_eq!(b.text().to_string(), "    a \r\n     中\t\nlast");
    unindent(&mut b, v);
    assert_eq!(b.text().to_string(), "a \r\n 中\t\nlast");
    trim_trailing_whitespace(&mut b, v);
    assert_eq!(b.text().to_string(), "a\n 中\nlast");
}
#[test]
fn tabs_indent_and_unindent() {
    let (mut b, v) = setup("a\nb", &[(0, 3)]);
    b.set_settings(
        v,
        Settings {
            translate_tabs_to_spaces: false,
            ..Settings::default()
        },
    );
    indent(&mut b, v);
    assert_eq!(b.text().to_string(), "\ta\n\tb");
    unindent(&mut b, v);
    assert_eq!(b.text().to_string(), "a\nb");
}
#[test]
fn join_lines_multiselection_and_blank_line() {
    let (mut b, v) = setup("a  \n  b\n c\nd", &[(0, 7), (8, 10)]);
    join_lines(&mut b, v);
    assert_eq!(b.text().to_string(), "a b\n c d");
    let (mut b, v) = setup("a\n \nb", &[(0, 5)]);
    join_lines(&mut b, v);
    assert_eq!(b.text().to_string(), "a  b");
}
#[test]
fn duplicate_selected_text_or_caret_lines() {
    let (mut b, v) = setup("a\nb", &[(0, 1), (2, 3)]);
    duplicate_line(&mut b, v);
    expect(&b, v, "aa\nbb", &[(1, 2), (4, 5)]);
    assert!(b.undo(v));
    expect(&b, v, "a\nb", &[(0, 1), (2, 3)]);
    // Carets duplicate their line once and move onto the copy; the last line gains an LF.
    let (mut b, v) = setup("ab\ncd", &[(1, 1), (2, 2), (4, 4)]);
    duplicate_line(&mut b, v);
    expect(&b, v, "ab\nab\ncd\ncd", &[(4, 4), (5, 5), (10, 10)]);
}
#[test]
fn caret_words_prefer_the_word_on_the_left_at_its_end() {
    // ⌘D at the end of a word selects it, before a space, a newline or the end of the text.
    for (text, at, word) in [
        ("foo bar", 3, (0, 3)),
        ("foo\nbar", 3, (0, 3)),
        ("foo", 3, (0, 3)),
        ("foo bar", 4, (4, 7)),
        ("foo.bar", 3, (0, 3)),
    ] {
        let (mut b, v) = setup(text, &[(at, at)]);
        assert!(find_under_expand(&mut b, v));
        assert_eq!(selected(&b, v), [word], "{text:?} at {at}");
    }
    // Several carets each take their own word.
    let (mut b, v) = setup("one two\nthree four\n", &[(1, 1), (11, 11)]);
    assert!(find_under_expand(&mut b, v));
    assert_eq!(selected(&b, v), [(0, 3), (8, 13)]);
    // The pointer variant keeps the run under the pointer.
    let (b, v) = setup("foo bar", &[(0, 0)]);
    assert_eq!(word_region(&b, v, 3), Region::new(3, 4));
}
#[test]
fn repeated_line_expansion_takes_the_next_line() {
    let (mut b, v) = setup("one\ntwo\nthree", &[(1, 1)]);
    expand_selection(&mut b, v, ExpandTo::Line);
    assert_eq!(selected(&b, v), [(0, 4)]);
    expand_selection(&mut b, v, ExpandTo::Line);
    assert_eq!(selected(&b, v), [(0, 8)]);
    expand_selection(&mut b, v, ExpandTo::Line);
    assert_eq!(selected(&b, v), [(0, 13)]);
    expand_selection(&mut b, v, ExpandTo::Line);
    assert_eq!(selected(&b, v), [(0, 13)]);
}
#[test]
fn sorting_without_a_selection_sorts_the_whole_buffer() {
    let (mut b, v) = setup("c\nB\na\n", &[(0, 0)]);
    sort_lines(&mut b, v);
    assert_eq!(b.text().to_string(), "B\na\nc\n");
    let (mut b, v) = setup("c\nB\na", &[(0, 0)]);
    sort_lines_folded(&mut b, v);
    assert_eq!(b.text().to_string(), "a\nB\nc");
    reverse_lines(&mut b, v);
    assert_eq!(b.text().to_string(), "c\nB\na");
}
#[test]
fn swap_blocks_and_preserve_final_newline_and_column() {
    let (mut b, v) = setup("a\n中\nc", &[(2, 2)]);
    swap_line_up(&mut b, v);
    expect(&b, v, "中\na\nc", &[(0, 0)]);
    swap_line_down(&mut b, v);
    expect(&b, v, "a\n中\nc", &[(2, 2)]);
    swap_line_down(&mut b, v);
    expect(&b, v, "a\nc\n中", &[(4, 4)]);
    swap_line_down(&mut b, v);
    assert_eq!(b.text().to_string(), "a\nc\n中");
}
#[test]
fn sort_reverse_multiple_blocks_unicode_no_final_lf() {
    let (mut b, v) = setup("b\na\nkeep\n中\n世", &[(0, 4), (9, 16)]);
    sort_lines(&mut b, v);
    assert_eq!(b.text().to_string(), "a\nb\nkeep\n世\n中");
    reverse_lines(&mut b, v);
    assert_eq!(b.text().to_string(), "b\na\nkeep\n中\n世");
}
#[test]
fn sort_reverse_terminal_empty_line() {
    let (mut b, v) = setup("b\na\n", &[(0, 4)]);
    sort_lines(&mut b, v);
    assert_eq!(b.text().to_string(), "a\nb\n");
    reverse_lines(&mut b, v);
    assert_eq!(b.text().to_string(), "b\na\n");
}
#[test]
fn unicode_case_length_changes_and_title_swap() {
    let (mut b, v) = setup("straße 世界\nheLLo WORLD", &[(0, 7), (15, 26)]);
    upper_case(&mut b, v);
    assert_eq!(b.text().to_string(), "STRASSE 世界\nHELLO WORLD");
    b.undo(v);
    lower_case(&mut b, v);
    assert_eq!(b.text().to_string(), "straße 世界\nhello world");
    b.undo(v);
    title_case(&mut b, v);
    assert_eq!(b.text().to_string(), "Straße 世界\nHello World");
    b.undo(v);
    swap_case(&mut b, v);
    assert_eq!(b.text().to_string(), "STRASSE 世界\nHEllO world");
}
#[test]
fn transpose_unicode_graphemes_multicursor_and_selection() {
    let (mut b, v) = setup("e\u{301}中 ab", &[(3, 3), (8, 8)]);
    transpose(&mut b, v);
    assert_eq!(b.text().to_string(), "中e\u{301} ba");
    b.undo(v);
    b.set_selection(v, Selection::from(Region::new(0, 6)));
    transpose(&mut b, v);
    assert_eq!(b.text().to_string(), "e\u{301}中 ab");
}
#[test]
fn shared_views_annotations_and_undo_saved_state() {
    let (mut b, a) = setup("abcdef", &[(2, 2)]);
    let z = b.new_view();
    b.set_selection(z, Selection::from(Region::caret(5)));
    b.add_regions(
        z,
        "r",
        vec![Region::new(2, 5), Region::new(5, 2), Region::caret(2)],
        false,
    );
    let old = b.change_id();
    insert(&mut b, a, "X");
    assert!(b.is_dirty());
    assert_eq!(selected(&b, z), [(6, 6)]);
    assert_eq!(
        b.view(z).get_regions("r").unwrap().regions,
        vec![Region::new(3, 6), Region::new(6, 3), Region::caret(3)]
    );
    assert_eq!(
        b.transform_region_from(Region::new(2, 5), old),
        Some(Region::new(3, 6))
    );
    b.mark_saved();
    insert(&mut b, a, "Y");
    assert!(b.is_dirty());
    b.undo(z);
    assert!(!b.is_dirty());
    b.undo(z);
    assert!(b.is_dirty());
    b.redo(z);
    assert!(!b.is_dirty());
    assert!(b.erase_regions(z, "r"));
    assert!(b.close_view(a));
    assert_eq!(b.views().count(), 1);
}
#[test]
fn typing_group_word_space_word_and_selection_break() {
    let (mut b, v) = setup("", &[(0, 0)]);
    for s in ["f", "o", "o", " ", "b", "a", "r"] {
        insert(&mut b, v, s);
    }
    b.undo(v);
    assert_eq!(b.text().to_string(), "foo ");
    b.undo(v);
    assert!(b.text().is_empty());
    b.redo(v);
    b.redo(v);
    b.set_selection(v, Selection::from(Region::caret(0)));
    insert(&mut b, v, "X");
    b.undo(v);
    assert_eq!(b.text().to_string(), "foo bar");
    assert_eq!(selected(&b, v), [(0, 0)]);
}
#[test]
fn soft_undo_each_typing_transaction_and_redo_invalidation() {
    let (mut b, v) = setup("", &[(0, 0)]);
    insert(&mut b, v, "a");
    insert(&mut b, v, "b");
    assert!(b.soft_undo(v));
    assert_eq!(b.text().to_string(), "a");
    assert!(b.soft_undo(v));
    assert_eq!(b.text().to_string(), "");
    assert!(b.soft_redo(v));
    assert!(b.soft_redo(v));
    assert_eq!(b.text().to_string(), "ab");
    b.undo(v);
    insert(&mut b, v, "x");
    assert!(!b.redo(v));
    assert!(!b.soft_redo(v));
}
#[test]
fn soft_history_keeps_at_least_256_and_byte_bound_drops() {
    let (mut b, v) = setup("ab", &[(0, 0)]);
    b.set_history_limits(HistoryLimits {
        selection_entries: 1,
        ..HistoryLimits::default()
    });
    for i in 0..300 {
        b.set_selection(v, Selection::from(Region::caret(i % 2)));
    }
    let mut n = 0;
    while b.soft_undo(v) {
        n += 1;
    }
    assert_eq!(n, 256);
    b.set_history_limits(HistoryLimits {
        bytes: 1,
        ..HistoryLimits::default()
    });
    insert(&mut b, v, "x");
    assert_eq!(b.history_entries(), 0);
    assert_eq!(b.history_retained_bytes(), 0);
    assert!(!b.undo(v));
    assert!(!b.soft_undo(v));
}
#[test]
fn invalid_transaction_is_atomic() {
    let (mut b, v) = setup("中a", &[(0, 0)]);
    let before = b.change_id();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.transact(
            v,
            EditKind::Other,
            &[Edit {
                range: 1..2,
                text: "x",
            }],
            Selection::default(),
        )
    }));
    assert!(failed.is_err());
    expect(&b, v, "中a", &[(0, 0)]);
    assert_eq!(b.change_id(), before);
    assert_eq!(b.history_entries(), 0);
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.transact(
            v,
            EditKind::Other,
            &[Edit {
                range: 0..3,
                text: "x",
            }],
            Selection::from(Region::caret(99)),
        )
    }));
    assert!(failed.is_err());
    assert_eq!(b.change_id(), before);
    assert_eq!(b.text().to_string(), "中a");
}

#[test]
fn dense_unicode_selection_validation_is_atomic() {
    let source = "中x\n".repeat(256);
    let (mut b, v) = setup(&source, &[(0, 0)]);
    let before = b.change_id();
    // The final endpoint splits the CJK scalar; all earlier endpoints are valid.
    let invalid = Selection::from_regions(
        (0..256).map(|row| Region::caret(row * 5 + if row == 255 { 1 } else { 0 })),
    );
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.transact(v, EditKind::Other, &[], invalid);
    }));
    assert!(failed.is_err());
    assert_eq!(b.change_id(), before);
    assert_eq!(b.text().to_string(), source);
    assert_eq!(b.history_entries(), 0);
    let edits = (0..256)
        .map(|row| Edit {
            range: row * 5..row * 5,
            text: "z",
        })
        .collect::<Vec<_>>();
    let invalid = Selection::from_regions(
        (0..256).map(|row| Region::caret(row * 6 + if row == 255 { 2 } else { 1 })),
    );
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.transact(v, EditKind::InsertChars, &edits, invalid);
    }));
    assert!(failed.is_err());
    assert_eq!(b.change_id(), before);
    assert_eq!(b.text().to_string(), source);
    assert_eq!(b.history_entries(), 0);
    b.set_selection(
        v,
        Selection::from_regions((0..256).map(|row| Region::new(row * 5 + 3, row * 5))),
    );
    assert_eq!(b.view(v).sel().len(), 256);
}
#[test]
fn change_ids_exclude_noops_and_include_undo_redo() {
    let (mut b, v) = setup("", &[(0, 0)]);
    assert_eq!(insert(&mut b, v, ""), ChangeId(0));
    assert!(!b.undo(v));
    assert_eq!(insert(&mut b, v, "x"), ChangeId(1));
    assert!(b.undo(v));
    assert_eq!(b.change_id(), ChangeId(2));
    assert!(b.redo(v));
    assert_eq!(b.change_id(), ChangeId(3));
}
#[test]
fn empty_buffer_operations_stay_valid() {
    let (mut b, v) = setup("", &[(0, 0)]);
    delete_left(&mut b, v, DeleteBy::Char);
    delete_right(&mut b, v, DeleteBy::Char);
    delete_word(&mut b, v, true);
    move_by(&mut b, v, MoveUnit::Words, true, false, None);
    for target in [
        MoveTo::Bol,
        MoveTo::Eol,
        MoveTo::HardBol,
        MoveTo::HardEol,
        MoveTo::Bof,
        MoveTo::Eof,
    ] {
        move_to(&mut b, v, target, false);
    }
    for to in [ExpandTo::Word, ExpandTo::Line, ExpandTo::Indentation] {
        expand_selection(&mut b, v, to);
    }
    split_selection_into_lines(&mut b, v);
    single_selection(&mut b, v);
    find_all_under(&mut b, v);
    assert!(!find_under_expand(&mut b, v));
    join_lines(&mut b, v);
    swap_line_up(&mut b, v);
    swap_line_down(&mut b, v);
    sort_lines(&mut b, v);
    reverse_lines(&mut b, v);
    upper_case(&mut b, v);
    lower_case(&mut b, v);
    title_case(&mut b, v);
    swap_case(&mut b, v);
    transpose(&mut b, v);
    trim_trailing_whitespace(&mut b, v);
    assert!(b.text().is_empty());
    assert_eq!(selected(&b, v), [(0, 0)]);
    indent(&mut b, v);
    unindent(&mut b, v);
    assert!(b.text().is_empty());
}
#[test]
fn primary_mapping_when_earlier_regions_collapse() {
    use vel_buffer::{Change, ChangeSet};
    let s = Selection::from_regions([Region::caret(2), Region::caret(8), Region::caret(4)]);
    assert_eq!(s.primary_index(), 1);
    let mapped = ChangeSet::new(vec![Change {
        old_range: 1..6,
        new_len: 0,
    }])
    .transform_selection(&s);
    assert_eq!(mapped.len(), 2);
    assert_eq!(mapped.primary(), Some(Region::caret(1)));
}
#[test]
fn typing_whitespace_before_first_word_groups_and_mark_saved_breaks() {
    let (mut b, v) = setup("", &[(0, 0)]);
    for s in [" ", " ", "a", "b", " "] {
        insert(&mut b, v, s);
    }
    b.undo(v);
    assert!(b.text().is_empty());
    b.redo(v);
    b.mark_saved();
    insert(&mut b, v, "c");
    b.undo(v);
    assert!(!b.is_dirty());
    assert_eq!(b.text().to_string(), "  ab ");
}
#[test]
fn stale_soft_branch_never_resurrects_discarded_text() {
    let (mut b, v) = setup("", &[(0, 0)]);
    insert(&mut b, v, "old");
    b.undo(v);
    insert(&mut b, v, "new");
    assert!(b.soft_undo(v));
    assert!(b.text().is_empty());
    assert!(!b.soft_undo(v));
    if b.redo(v) {
        assert_eq!(b.text().to_string(), "new");
    }
}
#[test]
fn merged_vertical_carets_keep_interactive_primary_goal() {
    let (mut b, v) = setup("abcd\nx\nabcd", &[(3, 3), (1, 1)]);
    let layout = MonospaceLayout::default();
    move_by(&mut b, v, MoveUnit::Lines, true, false, Some(&layout));
    assert_eq!(b.view(v).sel().len(), 1);
    assert_eq!(b.view(v).sel().primary().unwrap().xpos, Some(1.0));
    move_by(&mut b, v, MoveUnit::Lines, true, false, Some(&layout));
    assert_eq!(selected(&b, v), [(8, 8)]);
}
#[test]
fn line_prefix_transforms_preserve_caret_columns() {
    let (mut b, v) = setup("abcd\nefgh", &[(2, 2), (7, 7)]);
    indent(&mut b, v);
    expect(&b, v, "    abcd\n    efgh", &[(6, 6), (15, 15)]);
    unindent(&mut b, v);
    expect(&b, v, "abcd\nefgh", &[(2, 2), (7, 7)]);
}
#[test]
fn swap_whole_line_selection_ending_at_bol() {
    let (mut b, v) = setup("a\nb\nc", &[(0, 2)]);
    swap_line_down(&mut b, v);
    expect(&b, v, "b\na\nc", &[(2, 4)]);
    swap_line_up(&mut b, v);
    expect(&b, v, "a\nb\nc", &[(0, 2)]);
}
#[test]
fn leading_mixed_tabs_and_spaces_delete_to_visual_tab_stop() {
    let (mut b, v) = setup("\t  x\n \t  y", &[(3, 3), (9, 9)]);
    delete_left(&mut b, v, DeleteBy::Char);
    expect(&b, v, "\tx\n \ty", &[(1, 1), (5, 5)]);
}
#[test]
fn typing_over_selection_groups_with_following_characters() {
    let (mut b, v) = setup("old", &[(0, 3)]);
    insert(&mut b, v, "n");
    insert(&mut b, v, "e");
    insert(&mut b, v, "w");
    assert_eq!(b.text().to_string(), "new");
    b.undo(v);
    expect(&b, v, "old", &[(0, 3)]);
    b.redo(v);
    expect(&b, v, "new", &[(3, 3)]);
}
#[test]
fn subtract_caret_ignores_goal_and_line_membership_excludes_next_bol() {
    let mut s = Selection::from_regions([
        Region {
            a: 3,
            b: 3,
            xpos: Some(4.0),
        },
        Region::caret(8),
    ]);
    s.subtract(Region::caret(3));
    assert_eq!(s.iter().collect::<Vec<_>>(), vec![Region::caret(8)]);
    let (b, v) = setup("ab\ncd", &[(0, 3)]);
    assert_eq!(b.view(v).sel().regions_on_line(b.text(), 0).count(), 1);
    assert_eq!(b.view(v).sel().regions_on_line(b.text(), 1).count(), 0);
}
