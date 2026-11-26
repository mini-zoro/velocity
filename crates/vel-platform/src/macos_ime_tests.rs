use super::*;

#[test]
fn a_key_that_finishes_preedit_cannot_also_execute_a_responder_command() {
    for command in [
        Command::Newline,
        Command::Cancel,
        Command::Left(false),
        Command::DeleteBackward,
    ] {
        assert_eq!(
            CompositionKey::selector(command, None, false),
            Some(command)
        );
        assert_eq!(
            CompositionKey::selector(command, Some(CompositionKey::Other), false),
            None
        );
    }
    assert_eq!(
        CompositionKey::selector(Command::Newline, Some(CompositionKey::Other), true),
        Some(Command::Newline)
    );
    assert_eq!(
        CompositionKey::selector(Command::Cancel, Some(CompositionKey::Escape), true),
        Some(Command::Cancel)
    );
    for command in [
        Command::Down(false),
        Command::DeleteBackward,
        Command::Undo,
        Command::Paste,
    ] {
        assert_eq!(
            CompositionKey::selector(command, Some(CompositionKey::Other), true),
            None
        );
    }
}

#[test]
fn appkit_not_found_and_utf16_range_round_trip() {
    assert_eq!(from_range(to_range(None)), None);
    let range = Some(Utf16Range::new(7, 2));
    assert_eq!(from_range(to_range(range)), range);
}
