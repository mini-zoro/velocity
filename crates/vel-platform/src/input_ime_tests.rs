use super::*;

#[test]
fn appkit_offsets_are_relative_only_for_set_marked_text() {
    let mut state = TextInputState {
        text: Text::from("ab𠮷e\u{301}z"),
        marked: Some(Utf16Range::new(2, 4)),
        ..Default::default()
    };
    let proposed = Utf16Range::new(1, 1);
    let replacement = state.marked_replacement(Some(proposed)).unwrap();
    assert_eq!(replacement, Utf16Range::new(3, 1));
    assert_eq!(
        state.substring(replacement),
        Some(("𠮷".into(), Utf16Range::new(2, 2)))
    );
    assert_eq!(
        state.marked_replacement(Some(Utf16Range::new(4, usize::MAX))),
        Some(Utf16Range::new(6, 0))
    );
    assert_eq!(state.marked_replacement(None), None);
    state.marked = None;
    assert_eq!(state.marked_replacement(Some(proposed)), None);
}

#[test]
fn explicit_edit_inside_or_crossing_preedit_keeps_unreplaced_text() {
    let visible = Text::from("a𠮷e\u{301}z");
    let mark = 1..8;
    let original = 1..5;
    for (range, expected) in [
        (Utf16Range::new(2, 1), (1..5, "你e\u{301}".to_owned(), 0)),
        (Utf16Range::new(3, 2), (1..5, "𠮷你".to_owned(), 2)),
        (Utf16Range::new(3, 0), (1..5, "𠮷你e\u{301}".to_owned(), 2)),
        (Utf16Range::new(3, 3), (1..6, "𠮷你".to_owned(), 2)),
        (Utf16Range::new(5, 1), (5..6, "你".to_owned(), 0)),
    ] {
        assert_eq!(
            replace_in_preedit(&visible, mark.clone(), original.clone(), range, "你".into()),
            expected
        );
    }
}

#[test]
fn right_edge_anchor_clips_without_moving_the_leading_edge() {
    let bounds = Rect {
        x: 20.,
        y: 30.,
        width: 100.,
        height: 20.,
    };
    let rect = Rect {
        x: 118.,
        y: 31.,
        width: 20.,
        height: 25.,
    }
    .ime_anchor_in(bounds);
    assert_eq!(
        rect,
        Rect {
            x: 118.,
            y: 31.,
            width: 2.,
            height: 19.
        }
    );
}
