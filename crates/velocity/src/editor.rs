//! Demo single-selection editor. This deliberately has no vel-buffer dependency.
use std::{collections::VecDeque, ops::Range};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_platform::{Command, TextInput, Utf16Range};
use vel_text::Text;

#[derive(Clone, Debug)]
struct Snapshot {
    text: Text,
    anchor: usize,
    caret: usize,
}
#[derive(Debug)]
pub struct Editor {
    pub text: Text,
    pub anchor: usize,
    pub caret: usize,
    pub marked: Option<Range<usize>>,
    origin: Option<Snapshot>,
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    preferred_cell: Option<usize>,
    pub revision: u64,
}
impl Editor {
    pub fn new(text: Text) -> Self {
        Self {
            text,
            anchor: 0,
            caret: 0,
            marked: None,
            origin: None,
            undo: VecDeque::new(),
            redo: Vec::new(),
            preferred_cell: None,
            revision: 0,
        }
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            anchor: self.anchor,
            caret: self.caret,
        }
    }
    fn restore(&mut self, s: Snapshot) {
        self.text = s.text;
        self.anchor = s.anchor;
        self.caret = s.caret;
        self.marked = None;
        self.origin = None;
        self.preferred_cell = None;
        self.revision += 1;
    }
    fn remember(&mut self, s: Snapshot) {
        if self.undo.len() == 128 {
            self.undo.pop_front();
        }
        self.undo.push_back(s);
        self.redo.clear();
    }
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }
    pub fn selected_text(&self) -> String {
        self.text.slice(self.selection()).to_string()
    }
    pub fn selected_utf16(&self) -> Utf16Range {
        Utf16Range::from_bytes(&self.text, self.selection())
    }
    pub fn marked_utf16(&self) -> Option<Utf16Range> {
        self.marked
            .clone()
            .map(|r| Utf16Range::from_bytes(&self.text, r))
    }
    fn replacement(&self, explicit: Option<Utf16Range>) -> Range<usize> {
        explicit
            .map(|r| r.bytes(&self.text))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection())
    }
    pub fn text_input(&mut self, input: TextInput) {
        match input {
            TextInput::Insert { text, replacement } => {
                let range = self.replacement(replacement);
                let before = self.origin.take().unwrap_or_else(|| self.snapshot());
                self.text.replace(range.clone(), &text);
                self.caret = range.start + text.len();
                self.anchor = self.caret;
                self.marked = None;
                if before.text != self.text {
                    self.remember(before);
                }
                self.revision += 1;
                self.preferred_cell = None;
            }
            TextInput::Marked {
                text,
                selected,
                replacement,
            } => {
                if text.is_empty() {
                    if self.origin.is_some() {
                        self.cancel_composition();
                    } else if replacement.is_some() {
                        self.text_input(TextInput::Insert { text, replacement });
                    }
                    return;
                }
                let range = self.replacement(replacement);
                if self.origin.is_none() {
                    self.origin = Some(self.snapshot());
                }
                self.text.replace(range.clone(), &text);
                let relative = selected.bytes(&Text::from(text.as_str()));
                self.anchor = range.start + relative.start;
                self.caret = range.start + relative.end;
                self.marked = Some(range.start..range.start + text.len());
                self.revision += 1;
                self.preferred_cell = None;
            }
            TextInput::Unmark => self.unmark(),
        }
    }
    pub fn unmark(&mut self) {
        self.marked = None;
        if let Some(before) = self.origin.take()
            && before.text != self.text
        {
            self.remember(before);
        }
        self.revision += 1;
    }
    pub fn cancel_composition(&mut self) {
        if let Some(origin) = self.origin.take() {
            self.restore(origin);
        } else {
            self.marked = None;
        }
    }
    pub fn set_caret(&mut self, at: usize, extend: bool) {
        self.unmark();
        self.caret = self.text.floor_char_boundary(at);
        if !extend {
            self.anchor = self.caret;
        }
        self.preferred_cell = None;
    }
    fn move_to(&mut self, at: usize, extend: bool) {
        self.caret = at;
        if !extend {
            self.anchor = at;
        }
    }
    fn insert(&mut self, s: &str) {
        self.text_input(TextInput::Insert {
            text: s.into(),
            replacement: None,
        });
    }
    fn word_boundary(&self, forward: bool) -> usize {
        let mut at = self.caret;
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if forward {
            while at < self.text.len_bytes() && !is_word(self.text.char_at(at)) {
                at = self.text.next_grapheme_boundary(at);
            }
            while at < self.text.len_bytes() && is_word(self.text.char_at(at)) {
                at = self.text.next_grapheme_boundary(at);
            }
        } else {
            while at > 0 {
                let prev = self.text.prev_grapheme_boundary(at);
                if is_word(self.text.char_at(prev)) {
                    break;
                }
                at = prev;
            }
            while at > 0 {
                let prev = self.text.prev_grapheme_boundary(at);
                if !is_word(self.text.char_at(prev)) {
                    break;
                }
                at = prev;
            }
        }
        at
    }
    pub fn select_word(&mut self, at: usize) {
        self.set_caret(at, false);
        let start = self.word_boundary(false);
        let end = self.word_boundary(true);
        self.anchor = start;
        self.caret = end;
    }
    pub fn select_line(&mut self, at: usize) {
        self.unmark();
        let row = self.text.byte_to_line(self.text.floor_char_boundary(at));
        self.anchor = self.text.line_to_byte(row);
        self.caret = if row + 1 < self.text.len_lines() {
            self.text.line_to_byte(row + 1)
        } else {
            self.text.len_bytes()
        };
    }
    pub fn command(&mut self, command: Command) {
        use Command::*;
        if command == Cancel {
            self.cancel_composition();
            return;
        }
        // IME consumes composition keys first. A delivered selector operates on
        // the current preedit selection; navigation ends the composition once.
        if self.marked.is_some() {
            self.unmark();
        }
        match command {
            DeleteBackward | DeleteForward => {
                if self.selection().is_empty() {
                    if command == DeleteBackward {
                        self.anchor = self.text.prev_grapheme_boundary(self.caret);
                    } else {
                        self.anchor = self.text.next_grapheme_boundary(self.caret);
                    }
                }
                if !self.selection().is_empty() {
                    self.insert("");
                }
            }
            Newline => self.insert("\n"),
            Tab => self.insert("\t"),
            Left(extend) | Right(extend) => {
                let selection = self.selection();
                let at = if !extend && !selection.is_empty() {
                    if matches!(command, Left(_)) {
                        selection.start
                    } else {
                        selection.end
                    }
                } else if matches!(command, Left(_)) {
                    self.text.prev_grapheme_boundary(self.caret)
                } else {
                    self.text.next_grapheme_boundary(self.caret)
                };
                self.move_to(at, extend);
                self.preferred_cell = None;
            }
            Up(extend) | Down(extend) => {
                let row = self.text.byte_to_line(self.caret);
                let start = self.text.line_to_byte(row);
                let col = self
                    .preferred_cell
                    .unwrap_or_else(|| cells(&self.text.slice(start..self.caret).to_string()));
                let target = if matches!(command, Up(_)) {
                    row.saturating_sub(1)
                } else {
                    (row + 1).min(self.text.len_lines() - 1)
                };
                let line = self.text.line(target).to_string();
                let at = self.text.line_to_byte(target) + byte_at_cell(&line, col);
                self.move_to(at, extend);
                self.preferred_cell = Some(col);
            }
            Home(extend) | End(extend) => {
                let row = self.text.byte_to_line(self.caret);
                let at = self.text.line_to_byte(row)
                    + if matches!(command, End(_)) {
                        self.text.line(row).len_bytes()
                    } else {
                        0
                    };
                self.move_to(at, extend);
                self.preferred_cell = None;
            }
            WordLeft(extend) | WordRight(extend) => {
                let at = self.word_boundary(matches!(command, WordRight(_)));
                self.move_to(at, extend);
                self.preferred_cell = None;
            }
            DocumentStart(extend) => self.move_to(0, extend),
            DocumentEnd(extend) => self.move_to(self.text.len_bytes(), extend),
            SelectAll => {
                self.anchor = 0;
                self.caret = self.text.len_bytes();
            }
            Undo => {
                if let Some(previous) = self.undo.pop_back() {
                    let current = self.snapshot();
                    self.redo.push(current);
                    self.restore(previous);
                }
            }
            Redo => {
                if let Some(next) = self.redo.pop() {
                    let current = self.snapshot();
                    self.undo.push_back(current);
                    self.restore(next);
                }
            }
            Copy | Cut | Paste | PageUp | PageDown | Cancel => {}
        }
    }
}
/// Visual monospace columns: four-column tab stops and Unicode grapheme widths.
pub fn cells(text: &str) -> usize {
    let mut col = 0;
    for g in text.graphemes(true) {
        col += if g == "\t" {
            4 - col % 4
        } else {
            g.width().max(1)
        };
    }
    col
}
pub fn byte_at_cell(text: &str, target: usize) -> usize {
    let mut col = 0;
    for (i, g) in text.grapheme_indices(true) {
        let width = if g == "\t" {
            4 - col % 4
        } else {
            g.width().max(1)
        };
        if target < col + width {
            return if target - col < width.div_ceil(2) {
                i
            } else {
                i + g.len()
            };
        }
        col += width;
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn insert(e: &mut Editor, s: &str, r: Option<Utf16Range>) {
        e.text_input(TextInput::Insert {
            text: s.into(),
            replacement: r,
        });
    }
    fn mark(e: &mut Editor, s: &str, selection: Utf16Range, r: Option<Utf16Range>) {
        e.text_input(TextInput::Marked {
            text: s.into(),
            selected: selection,
            replacement: r,
        });
    }
    #[test]
    fn pinyin_commit_replaces_selection_once_and_undo_is_atomic() {
        let mut e = Editor::new(Text::from("a🦀z"));
        e.anchor = 1;
        e.caret = 5;
        mark(&mut e, "nihao", Utf16Range::new(5, 0), None);
        assert_eq!(e.text.to_string(), "anihaoz");
        assert_eq!(e.marked_utf16(), Some(Utf16Range::new(1, 5)));
        mark(&mut e, "你好", Utf16Range::new(2, 0), None);
        insert(&mut e, "你好", None);
        assert_eq!(e.text.to_string(), "a你好z");
        assert!(e.marked.is_none());
        e.command(Command::Undo);
        assert_eq!(e.text.to_string(), "a🦀z");
        e.command(Command::Redo);
        assert_eq!(e.text.to_string(), "a你好z");
    }
    #[test]
    fn replacement_ranges_reconversion_picker_and_split_surrogates() {
        let mut e = Editor::new(Text::from("a🦀日本z"));
        insert(&mut e, "😀", Some(Utf16Range::new(2, 1)));
        assert_eq!(e.text.to_string(), "a😀日本z");
        mark(
            &mut e,
            "にほん",
            Utf16Range::new(0, 3),
            Some(Utf16Range::new(3, 2)),
        );
        assert_eq!(e.selected_utf16(), Utf16Range::new(3, 3));
        insert(&mut e, "日本語", Some(Utf16Range::new(3, 3)));
        assert_eq!(e.text.to_string(), "a😀日本語z");
    }
    #[test]
    fn cancel_empty_preedit_unmark_and_commit_without_preedit() {
        let mut e = Editor::new(Text::from("old"));
        e.command(Command::SelectAll);
        mark(&mut e, "abc", Utf16Range::new(3, 0), None);
        e.command(Command::Cancel);
        assert_eq!(e.text.to_string(), "old");
        mark(&mut e, "é", Utf16Range::new(1, 0), None);
        e.text_input(TextInput::Unmark);
        assert_eq!(e.text.to_string(), "é");
        e.command(Command::Undo);
        assert_eq!(e.text.to_string(), "old");
        mark(&mut e, "x", Utf16Range::new(1, 0), None);
        mark(&mut e, "", Utf16Range::default(), None);
        assert_eq!(e.text.to_string(), "old");
        insert(&mut e, "🦀", None);
        assert_eq!(e.text.to_string(), "🦀");
    }
    #[test]
    fn extended_grapheme_deletion_and_column_movement() {
        let mut e = Editor::new(Text::from("a👩‍💻é\n你\nabc"));
        e.command(Command::End(false));
        e.command(Command::DeleteBackward);
        assert_eq!(e.text.line(0).to_string(), "a👩‍💻");
        e.command(Command::DeleteBackward);
        assert_eq!(e.text.line(0).to_string(), "a");
        e.command(Command::Down(false));
        assert_eq!(e.caret, e.text.line_to_byte(1) + 3);
        e.command(Command::Down(false));
        assert_eq!(e.caret, e.text.line_to_byte(2) + 1);
    }
    #[test]
    fn tabs_and_double_width_hit_testing() {
        assert_eq!(cells("a\t你👩‍💻é"), 9);
        assert_eq!(byte_at_cell("你x", 0), 0);
        assert_eq!(byte_at_cell("你x", 1), 3);
        assert_eq!(byte_at_cell("éx", 1), 3);
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    #[test]
    fn unicode_edits_match_flat_reference_over_repeated_transactions() {
        let mut editor = Editor::new(Text::new());
        let mut model = String::new();
        let mut caret = 0;
        let mut state = 0x76656c6f63697479u64;
        for _ in 0..2000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            match state % 7 {
                0..=3 => {
                    let choices = ["x", "你", "🦀", "é", "\n", "👩‍💻"];
                    let text = choices[(state >> 32) as usize % choices.len()];
                    editor.text_input(TextInput::Insert {
                        text: text.into(),
                        replacement: None,
                    });
                    model.insert_str(caret, text);
                    caret += text.len();
                }
                4 => {
                    editor.command(Command::DeleteBackward);
                    if caret > 0 {
                        let previous = model[..caret].grapheme_indices(true).next_back().unwrap().0;
                        model.replace_range(previous..caret, "");
                        caret = previous;
                    }
                }
                5 => {
                    editor.command(Command::Left(false));
                    if caret > 0 {
                        caret = model[..caret].grapheme_indices(true).next_back().unwrap().0;
                    }
                }
                _ => {
                    editor.command(Command::Right(false));
                    if caret < model.len() {
                        caret += model[caret..].graphemes(true).next().unwrap().len();
                    }
                }
            }
            assert_eq!(editor.text.to_string(), model);
            assert_eq!(editor.caret, caret);
            assert!(model.is_char_boundary(caret));
        }
    }
    #[test]
    fn empty_marked_text_with_an_explicit_range_can_delete_without_preedit() {
        let mut editor = Editor::new(Text::from("a🦀z"));
        editor.text_input(TextInput::Marked {
            text: String::new(),
            selected: Utf16Range::default(),
            replacement: Some(Utf16Range::new(1, 2)),
        });
        assert_eq!(editor.text.to_string(), "az");
        editor.command(Command::Undo);
        assert_eq!(editor.text.to_string(), "a🦀z");
    }
}
