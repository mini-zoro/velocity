use super::*;
use vel_complete::{CompletionFormat, CompletionItem, CompletionList};
use vel_platform::{Command, TextInput, Utf16Range};
use vel_text::Text;

#[test]
fn preedit_hides_completion_and_return_does_not_accept_it() {
    let mut editor = Editor::new(Text::from("al"));
    editor.set_caret(2, false);
    editor.completion.list = Some(CompletionList {
        view_id: editor.id.0,
        version: editor.buffer.change_id(),
        generation: 0,
        replace_ranges: std::iter::once(0..2).collect(),
        items: vec![CompletionItem {
            trigger: "alpha".into(),
            annotation: String::new(),
            completion: "alpha".into(),
            completion_format: CompletionFormat::Text,
            kind: Default::default(),
            details: String::new(),
        }],
        commit_on_tab: false,
        cycle: false,
        cancelled: false,
    });
    assert!(editor.completion.visible());
    for text in ["n", "ni"] {
        editor.text_input(TextInput::Marked {
            text: text.into(),
            selected: Utf16Range::new(0, 1),
            replacement: None,
        });
        assert!(!editor.completion.visible());
        editor.command(Command::Down(false));
        assert_eq!(editor.buffer.text().to_string(), "al");
    }
    editor.command(Command::Newline);
    editor.text_input(TextInput::Unmark);
    assert_eq!(editor.buffer.text().to_string(), "alni");
    editor.command(Command::Undo);
    assert_eq!(editor.buffer.text().to_string(), "al");
}
