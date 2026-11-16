use vel_buffer::{
    Buffer, Region, Selection, ViewId,
    ops::{self, IdentCase, Permutation},
};

fn fixture(text: &str, regions: impl IntoIterator<Item = Region>) -> (Buffer, ViewId) {
    let mut b = Buffer::new(text);
    let id = b.new_view();
    b.set_selection(id, Selection::from_regions(regions));
    (b, id)
}
fn roundtrip(b: &mut Buffer, id: ViewId, original: &str, selection: &Selection, expected: &str) {
    assert_eq!(b.text().to_string(), expected);
    if original == expected {
        assert!(!b.undo(id));
        assert!(!b.is_dirty());
        return;
    }
    assert!(b.undo(id));
    assert_eq!(b.text().to_string(), original);
    assert_eq!(b.view(id).sel(), selection);
    assert!(b.redo(id));
    assert_eq!(b.text().to_string(), expected);
}
#[test]
fn line_permutations_deduplicate_stably_and_preserve_line_structure() {
    for (text, expected, regions) in [
        ("b\na\nb\n", "b\na\n", vec![Region::new(0, 6)]),
        ("b\na\nb", "b\na", vec![Region::new(5, 0)]),
        (
            "🦀\n🦀\n--\nx\nx",
            "🦀\n--\nx",
            vec![Region::new(0, 10), Region::new(13, 16)],
        ),
        ("\n\n", "\n", vec![Region::new(0, 2)]),
    ] {
        let (mut b, id) = fixture(text, regions);
        let before = b.view(id).sel().clone();
        ops::permute_lines(&mut b, id, Permutation::Unique);
        roundtrip(&mut b, id, text, &before, expected);
    }
    for text in ["", "x", "x\n", "a\nb\n"] {
        let (mut b, id) = fixture(text, [Region::new(0, text.len())]);
        ops::permute_lines(&mut b, id, Permutation::Reverse);
        assert_eq!(b.text().to_string().ends_with('\n'), text.ends_with('\n'));
    }
}
#[test]
fn shuffle_is_reproducible_and_preserves_the_multiset() {
    for seed in [0, 1, u64::MAX] {
        let text = "a\nb\nc\nd\ne\na\n";
        let mut results = Vec::new();
        for _ in 0..2 {
            let (mut b, id) = fixture(text, [Region::new(0, text.len())]);
            let before = b.view(id).sel().clone();
            ops::permute_lines(&mut b, id, Permutation::Shuffle(seed));
            let output = b.text().to_string();
            let mut words: Vec<_> = output.lines().collect();
            words.sort();
            assert_eq!(words, ["a", "a", "b", "c", "d", "e"]);
            roundtrip(&mut b, id, text, &before, &output);
            results.push(output);
        }
        assert_eq!(results[0], results[1]);
    }
}
#[test]
fn selected_strings_move_between_variable_length_slots_without_touching_gaps() {
    for (operation, expected) in [
        (Permutation::Reverse, "猫 | A | z"),
        (Permutation::Unique, "z | A | 猫"),
    ] {
        let text = "z | A | 猫";
        let (mut b, id) = fixture(
            text,
            [Region::new(0, 1), Region::new(5, 4), Region::new(8, 11)],
        );
        let before = b.view(id).sel().clone();
        ops::permute_selection(&mut b, id, operation);
        roundtrip(&mut b, id, text, &before, expected);
    }
    let text = "z | A | 猫";
    let (mut b, id) = fixture(
        text,
        [Region::new(0, 1), Region::new(4, 5), Region::new(8, 11)],
    );
    let before = b.view(id).sel().clone();
    ops::sort_selection(&mut b, id, false, false);
    roundtrip(&mut b, id, text, &before, "A | z | 猫");
    let text = "a+a+b";
    let (mut b, id) = fixture(
        text,
        [Region::new(0, 1), Region::new(2, 3), Region::new(4, 5)],
    );
    let before = b.view(id).sel().clone();
    ops::permute_selection(&mut b, id, Permutation::Unique);
    roundtrip(&mut b, id, text, &before, "a+b+");
}
#[test]
fn identifier_cases_cover_acronyms_unicode_and_multi_caret_overlap() {
    for (case, expected) in [
        (IdentCase::Snake, "http_server_foo_bar"),
        (IdentCase::Camel, "httpServerFooBar"),
        (IdentCase::Pascal, "HttpServerFooBar"),
        (IdentCase::Kebab, "http-server-foo-bar"),
        (IdentCase::UpperSnake, "HTTP_SERVER_FOO_BAR"),
    ] {
        let text = "HTTPServer_foo-bar";
        let (mut b, id) = fixture(text, [Region::new(text.len(), 0)]);
        let before = b.view(id).sel().clone();
        ops::convert_ident_case(&mut b, id, case);
        roundtrip(&mut b, id, text, &before, expected);
    }
    let text = "straße_猫";
    let (mut b, id) = fixture(text, [Region::caret(1), Region::caret(4)]);
    let before = b.view(id).sel().clone();
    ops::convert_ident_case(&mut b, id, IdentCase::UpperSnake);
    roundtrip(&mut b, id, text, &before, "STRASSE_猫");
    for (op, expected) in [
        (
            ops::title_case as fn(&mut Buffer, ViewId) -> _,
            "Élan\nStraße",
        ),
        (ops::swap_case, "éLAN\nSTRASSE"),
    ] {
        let text = "Élan\nstraße";
        let (mut b, id) = fixture(text, [Region::new(0, text.len())]);
        let before = b.view(id).sel().clone();
        op(&mut b, id);
        roundtrip(&mut b, id, text, &before, expected);
    }
}
#[test]
fn paragraphs_wrap_once_with_display_width_and_retain_blank_separators() {
    let text = "  one two\n  三四 five\n \nlast paragraph";
    let (mut b, id) = fixture(text, [Region::caret(3), Region::caret(12)]);
    let before = b.view(id).sel().clone();
    ops::wrap_lines(&mut b, id, 9);
    roundtrip(
        &mut b,
        id,
        text,
        &before,
        "  one two\n  三四\n  five\n \nlast paragraph",
    );
    b.set_selection(id, Selection::from(Region::new(10, 2)));
    ops::expand_selection_to_paragraph(&mut b, id);
    assert_eq!(b.view(id).sel().primary().unwrap(), Region::new(26, 0));
    let (mut b, id) = fixture("\nlongword\n", [Region::caret(2)]);
    ops::wrap_lines(&mut b, id, 1);
    assert_eq!(b.text().to_string(), "\nlongword\n");
}
#[test]
fn split_lines_keeps_reversed_unicode_blank_cr_and_eof_edges() {
    for (text, region, expected) in [
        (
            "猫\n\n🦀\n",
            Region::new(9, 0),
            vec![Region::new(3, 0), Region::caret(4), Region::new(9, 5)],
        ),
        ("a\r\nb", Region::new(0, 3), vec![Region::new(0, 2)]),
        ("a\n", Region::caret(2), vec![Region::caret(2)]),
        ("", Region::caret(0), vec![Region::caret(0)]),
    ] {
        let (mut b, id) = fixture(text, [region]);
        ops::split_selection_into_lines(&mut b, id);
        assert_eq!(b.view(id).sel().iter().collect::<Vec<_>>(), expected);
    }
}
#[test]
fn mark_exchange_and_bookmark_navigation_map_through_edits_and_stay_per_view() {
    let (mut b, id) = fixture("a\n猫\nc\nd", [Region::caret(2)]);
    let second = b.new_view();
    assert!(!ops::select_to_mark(&mut b, id));
    assert!(!ops::next_bookmark(&mut b, id, false));
    ops::set_mark(&mut b, id);
    ops::toggle_bookmark(&mut b, id);
    b.set_selection(id, Selection::from(Region::caret(8)));
    ops::toggle_bookmark(&mut b, id);
    assert!(!ops::select_bookmarks(&mut b, second));
    b.set_selection(id, Selection::from(Region::caret(0)));
    ops::insert(&mut b, id, "x\n");
    assert!(ops::next_bookmark(&mut b, id, false));
    assert_eq!(b.view(id).sel().primary().unwrap().b, 4);
    assert!(ops::prev_bookmark(&mut b, id, false));
    assert_eq!(b.view(id).sel().primary().unwrap().b, 10);
    assert!(ops::next_bookmark(&mut b, id, true));
    assert_eq!(b.view(id).sel().primary().unwrap(), Region::new(10, 4));
    b.set_selection(id, Selection::from(Region::caret(10)));
    assert!(ops::select_to_mark(&mut b, id));
    assert_eq!(b.view(id).sel().primary().unwrap(), Region::new(10, 4));
    assert!(ops::swap_with_mark(&mut b, id));
    assert_eq!(b.view(id).sel().primary().unwrap(), Region::caret(4));
    assert!(ops::swap_with_mark(&mut b, id));
    assert_eq!(b.view(id).sel().primary().unwrap(), Region::new(10, 4));
    ops::clear_bookmarks(&mut b, id);
    assert!(!ops::prev_bookmark(&mut b, id, false));
    b.set_selection(id, Selection::from(Region::caret(4)));
    ops::toggle_bookmark(&mut b, id);
    ops::toggle_bookmark(&mut b, id);
    assert!(!ops::select_bookmarks(&mut b, id));
}
#[test]
fn whitespace_alias_preserves_unselected_rows_and_undo() {
    let text = "a \t\r\n猫　\nother  ";
    let (mut b, id) = fixture(text, [Region::new(0, 12)]);
    let before = b.view(id).sel().clone();
    ops::trim_trailing_white_space(&mut b, id);
    roundtrip(&mut b, id, text, &before, "a\n猫\nother  ");
}

#[test]
fn append_complement_endpoint_split_and_rot13_roundtrip() {
    let (mut b, id) = fixture("ab猫c", [Region::new(0, 2)]);
    ops::invert_selection(&mut b, id);
    assert_eq!(b.view(id).sel().primary().unwrap(), Region::new(2, 6));
    ops::split_selection(&mut b, id);
    assert_eq!(
        b.view(id).sel().iter().collect::<Vec<_>>(),
        vec![Region::caret(2), Region::caret(6)]
    );
    let before = b.view(id).sel().clone();
    ops::append(&mut b, id, "!\n");
    roundtrip(&mut b, id, "ab猫c", &before, "ab猫c!\n");
    let (mut b, id) = fixture("aZ猫🦀", [Region::new(0, 9)]);
    let before = b.view(id).sel().clone();
    ops::rot13(&mut b, id);
    roundtrip(&mut b, id, "aZ猫🦀", &before, "nM猫🦀");
}

#[test]
fn mark_exchange_preserves_the_interactive_primary_region() {
    let (mut b, id) = fixture("abcdef", [Region::caret(5), Region::caret(1)]);
    let original = b.view(id).sel().clone();
    ops::set_mark(&mut b, id);
    b.set_selection(id, Selection::from(Region::caret(3)));
    assert!(ops::swap_with_mark(&mut b, id));
    assert_eq!(b.view(id).sel(), &original);
}

#[test]
fn identifier_conversion_at_whitespace_or_punctuation_keeps_text_intact() {
    let (mut b, id) = fixture("x \t-\ny", [Region::caret(1), Region::caret(3)]);
    ops::convert_ident_case(&mut b, id, IdentCase::Camel);
    assert_eq!(b.text().to_string(), "x \t-\ny");
}

proptest::proptest! {
    #[test]
    fn arbitrary_line_shuffles_preserve_lines_and_undo(lines in proptest::collection::vec("[a-z猫🦀]{0,12}",1..30),seed in proptest::prelude::any::<u64>(),terminal in proptest::prelude::any::<bool>()) {
        let mut text=lines.join("\n");if terminal{text.push('\n');}
        let(mut b,id)=fixture(&text,[Region::new(0,text.len())]);let before=b.view(id).sel().clone();
        ops::permute_lines(&mut b,id,Permutation::Shuffle(seed));let result=b.text().to_string();
        let strip=|s:&str| (if text.ends_with('\n') { s.strip_suffix('\n').expect("retained terminal LF") } else { s }).split('\n').map(str::to_owned).collect::<Vec<_>>();
        let mut expected=strip(&text);let mut actual=strip(&result);expected.sort();actual.sort();
        proptest::prop_assert_eq!(actual,expected);roundtrip(&mut b,id,&text,&before,&result);
    }
}
