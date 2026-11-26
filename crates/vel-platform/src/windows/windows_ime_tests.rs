use super::*;

#[test]
fn cursor_and_target_clauses_are_utf16() {
    let text = "𠮷e\u{301}";
    let target = ATTR_TARGET_CONVERTED as u8;
    assert_eq!(
        composition_selection(text, 4, &[target, target, 0, 0]),
        Utf16Range::new(0, 2)
    );
    assert_eq!(
        composition_selection(text, 3, &[target, 0, 0]),
        Utf16Range::new(3, 0)
    );
    assert_eq!(
        composition_selection(text, usize::MAX, &[]),
        Utf16Range::new(4, 0)
    );
}

#[test]
fn result_precedes_the_next_clause_in_the_same_message() {
    let mut ime = Composition::default();
    ime.start();
    ime.update(None, Some(("ni".into(), Utf16Range::new(2, 0))), false);
    let events = ime.update(
        Some("你".into()),
        Some(("hao".into(), Utf16Range::new(1, 2))),
        false,
    );
    assert_eq!(
        events,
        [
            TextInput::Insert {
                text: "你".into(),
                replacement: None
            },
            TextInput::Marked {
                text: "hao".into(),
                selected: Utf16Range::new(1, 2),
                replacement: None
            }
        ]
    );
    assert_eq!(ime.end(), Some(TextInput::Discard));
}
