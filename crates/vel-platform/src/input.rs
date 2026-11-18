use crate::{Point, Rect};
use std::{ops::Range, time::Instant};
use vel_text::Text;

/// A document-relative range in UTF-16 code units. `None` represents NSNotFound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Utf16Range {
    /// Starting code-unit offset.
    pub location: usize,
    /// Number of code units.
    pub length: usize,
}
impl Utf16Range {
    /// Constructs a range; clipping happens against the current text snapshot.
    pub const fn new(location: usize, length: usize) -> Self {
        Self { location, length }
    }
    /// Saturating end, so untrusted native ranges cannot overflow.
    pub fn end(self) -> usize {
        self.location.saturating_add(self.length)
    }
    /// Clips to valid scalar boundaries via vel-text. A split surrogate expands
    /// at the right edge, so replacing a nonempty range never leaves half a scalar.
    pub fn bytes(self, text: &Text) -> Range<usize> {
        let start = text.clip_utf16(self.location);
        let end = text.clip_utf16(self.end());
        let end = if self.length > 0 && end < self.end().min(text.len_utf16()) {
            end + 2
        } else {
            end
        };
        text.utf16_to_byte(start)..text.utf16_to_byte(end.min(text.len_utf16()))
    }
    /// Creates a UTF-16 range from valid byte boundaries.
    pub fn from_bytes(text: &Text, bytes: Range<usize>) -> Self {
        let start = text.byte_to_utf16(bytes.start);
        Self::new(start, text.byte_to_utf16(bytes.end) - start)
    }
}
/// Snapshot queried synchronously by NSTextInputClient, including marked text.
#[derive(Clone, Debug, Default)]
pub struct TextInputState {
    /// Visible document, including transient marked text.
    pub text: Text,
    /// Selection in document UTF-16 units.
    pub selected: Utf16Range,
    /// Inline preedit in document UTF-16 units, or no marked range.
    pub marked: Option<Utf16Range>,
    /// Caret in top-left view points, before native screen conversion.
    pub caret: Rect,
}
impl TextInputState {
    /// Returns only the intersecting substring and its actual scalar-aligned range.
    pub fn substring(&self, proposed: Utf16Range) -> Option<(String, Utf16Range)> {
        if proposed.location > self.text.len_utf16() {
            return None;
        }
        let bytes = proposed.bytes(&self.text);
        let actual = Utf16Range::from_bytes(&self.text, bytes.clone());
        Some((self.text.slice(bytes).to_string(), actual))
    }
}
/// Native text input. Explicit replacement ranges always win over current selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextInput {
    /// Commit text, even when no composition preceded it (picker/dead keys).
    Insert {
        /// Committed UTF-8.
        text: String,
        /// Native document UTF-16 replacement, or marked/selected default.
        replacement: Option<Utf16Range>,
    },
    /// Replace preedit; selection is relative to the new preedit, in UTF-16.
    Marked {
        /// New preedit.
        text: String,
        /// Relative selection in the preedit.
        selected: Utf16Range,
        /// Optional explicit document replacement.
        replacement: Option<Utf16Range>,
    },
    /// Commit the current marked characters without inserting them again.
    Unmark,
}
/// Platform-neutral key state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// Key down.
    Down,
    /// Key up.
    Up,
    /// Modifier transition.
    FlagsChanged,
}
/// Raw macOS modifier bits, including device-dependent left/right flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(pub u64);
impl Modifiers {
    /// Either shift key.
    pub fn shift(self) -> bool {
        self.0 & (1 << 17) != 0
    }
    /// Either command key.
    pub fn command(self) -> bool {
        self.0 & (1 << 20) != 0
    }
    /// Either option key.
    pub fn option(self) -> bool {
        self.0 & (1 << 19) != 0
    }
    /// Either control key.
    pub fn control(self) -> bool {
        self.0 & (1 << 18) != 0
    }
    /// Left/right shift, control, option, command, in that order.
    pub fn sides(self) -> [bool; 8] {
        [0x2, 0x4, 0x1, 0x2000, 0x20, 0x40, 0x8, 0x10].map(|b| self.0 & b != 0)
    }
}
/// Physical keyboard event, observed after the IME had first refusal.
#[derive(Clone, Debug)]
pub struct KeyEvent {
    /// Hardware keycode, independent of keyboard layout.
    pub physical_code: u16,
    /// Layout-translated characters.
    pub characters: String,
    /// Characters with command/option modifiers removed by AppKit.
    pub characters_ignoring_modifiers: String,
    /// Both independent and left/right device flags.
    pub modifiers: Modifiers,
    /// Native repeat flag.
    pub repeat: bool,
    /// Native seconds since system startup.
    pub timestamp: f64,
    /// Receipt before interpretKeyEvents.
    pub received: Instant,
    /// Down, up, or flags transition.
    pub state: KeyState,
}
/// Mouse input kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    /// Pointer movement without a pressed button (widget hover).
    Move,
    /// Button press.
    Down,
    /// Drag with button pressed.
    Drag,
    /// Button release.
    Up,
}
/// Button event in logical view coordinates.
#[derive(Clone, Copy, Debug)]
pub struct MouseEvent {
    /// Native button number (zero is primary).
    pub button: usize,
    /// Press, drag, or release.
    pub kind: MouseKind,
    /// Position in logical points.
    pub position: Point,
    /// Native multi-click count.
    pub click_count: usize,
    /// Current modifiers.
    pub modifiers: Modifiers,
    /// Native timestamp in seconds.
    pub timestamp: f64,
}
impl MouseEvent {
    /// A secondary-button (including trackpad) or Control-primary-button press.
    /// Use this before primary-click caret/selection handling.
    pub fn is_context_click(self) -> bool {
        self.kind == MouseKind::Down
            && (self.button == 1 || self.button == 0 && self.modifiers.control())
    }
}
/// Trackpad or wheel scroll with native phase bitmasks.
#[derive(Clone, Copy, Debug)]
pub struct ScrollEvent {
    /// Pointer position in logical view coordinates, for scroll-container routing.
    pub position: Point,
    /// Horizontal delta (points for precise devices, lines otherwise).
    pub delta_x: f64,
    /// Vertical delta (native sign).
    pub delta_y: f64,
    /// Whether deltas have point precision.
    pub precise: bool,
    /// NSEventPhase bitmask for the physical gesture.
    pub phase: u64,
    /// NSEventPhase bitmask for inertial motion.
    pub momentum_phase: u64,
    /// Current modifiers.
    pub modifiers: Modifiers,
    /// Native timestamp.
    pub timestamp: f64,
}
/// Editor commands produced by standard NSTextInputClient selectors and menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Remove the preceding grapheme/selection.
    DeleteBackward,
    /// Remove following grapheme/selection.
    DeleteForward,
    /// Decompose the preceding grapheme, then remove its last component.
    DeleteDecomposingBackward,
    /// Remove a word; true deletes forwards.
    DeleteWord(bool),
    /// Kill text before the caret on the current visual line.
    DeleteToLineStart,
    /// Kill the remaining text on the hard line.
    DeleteToParagraphEnd,
    /// Insert LF.
    Newline,
    /// Insert LF without auto-indentation.
    LiteralNewline,
    /// Insert a tab.
    Tab,
    /// Remove one level of indentation.
    Backtab,
    /// Move one grapheme left/right; bool extends selection.
    Left(bool),
    /// Move right.
    Right(bool),
    /// Move up.
    Up(bool),
    /// Move down.
    Down(bool),
    /// Move to line start.
    Home(bool),
    /// Move to line end.
    End(bool),
    /// Move to the start of a hard line.
    ParagraphStart(bool),
    /// Move to the end of a hard line.
    ParagraphEnd(bool),
    /// Extend backward/forward by a hard line boundary.
    SelectParagraphBoundary(bool),
    /// Move one word left.
    WordLeft(bool),
    /// Move one word right.
    WordRight(bool),
    /// Move to document start.
    DocumentStart(bool),
    /// Move to document end.
    DocumentEnd(bool),
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Scroll without moving the selection; true scrolls down.
    ScrollPage(bool),
    /// Extend the selection by a page; true moves down.
    SelectPage(bool),
    /// Center the primary selection.
    CenterSelection,
    /// Restore killed text.
    Yank,
    /// Transpose adjacent characters.
    Transpose,
    /// Save the selection as the mark.
    SetMark,
    /// Select through the mark.
    SelectToMark,
    /// Exchange the selection and the mark.
    SwapWithMark,
    /// Kill text through the mark.
    DeleteToMark,
    /// Uppercase the current word.
    UppercaseWord,
    /// Lowercase the current word.
    LowercaseWord,
    /// Capitalize the current word.
    CapitalizeWord,
    /// Expand to the current word.
    SelectWord,
    /// Expand to the current line.
    SelectLine,
    /// Expand to the current paragraph.
    SelectParagraph,
    /// Cancel current composition.
    Cancel,
    /// Undo the last edit.
    Undo,
    /// Redo the last undone edit.
    Redo,
    /// Copy selected text.
    Copy,
    /// Cut selected text.
    Cut,
    /// Paste text.
    Paste,
    /// Select the whole document.
    SelectAll,
}
impl Command {
    /// Maps AppKit's standard selector spelling; unknown selectors are ignored.
    pub fn from_selector(selector: &str) -> Option<Self> {
        // https://developer.apple.com/documentation/appkit/nsstandardkeybindingresponding
        Some(match selector {
            "deleteBackward:" => Self::DeleteBackward,
            "deleteForward:" => Self::DeleteForward,
            "deleteBackwardByDecomposingPreviousCharacter:" => Self::DeleteDecomposingBackward,
            "deleteWordBackward:" => Self::DeleteWord(false),
            "deleteWordForward:" => Self::DeleteWord(true),
            "deleteToBeginningOfLine:" => Self::DeleteToLineStart,
            "deleteToEndOfParagraph:" => Self::DeleteToParagraphEnd,
            "insertNewline:" | "insertLineBreak:" => Self::Newline,
            "insertNewlineIgnoringFieldEditor:" => Self::LiteralNewline,
            "insertTab:" => Self::Tab,
            "insertBacktab:" => Self::Backtab,
            "moveLeft:" | "moveBackward:" => Self::Left(false),
            "moveRight:" | "moveForward:" => Self::Right(false),
            "moveLeftAndModifySelection:" | "moveBackwardAndModifySelection:" => Self::Left(true),
            "moveRightAndModifySelection:" | "moveForwardAndModifySelection:" => Self::Right(true),
            "moveUp:" => Self::Up(false),
            "moveUpAndModifySelection:" => Self::Up(true),
            "moveDown:" => Self::Down(false),
            "moveDownAndModifySelection:" => Self::Down(true),
            // NSTextView's lines are layout rows; its paragraphs are LF-delimited.
            "moveToBeginningOfLine:" => Self::Home(false),
            "moveToBeginningOfLineAndModifySelection:" => Self::Home(true),
            "moveToEndOfLine:" => Self::End(false),
            "moveToEndOfLineAndModifySelection:" => Self::End(true),
            "moveToBeginningOfParagraph:" => Self::ParagraphStart(false),
            "moveToBeginningOfParagraphAndModifySelection:" => Self::ParagraphStart(true),
            "moveToEndOfParagraph:" => Self::ParagraphEnd(false),
            "moveToEndOfParagraphAndModifySelection:" => Self::ParagraphEnd(true),
            "moveParagraphBackwardAndModifySelection:" => Self::SelectParagraphBoundary(false),
            "moveParagraphForwardAndModifySelection:" => Self::SelectParagraphBoundary(true),
            "moveWordLeft:" | "moveWordBackward:" => Self::WordLeft(false),
            "moveWordRight:" | "moveWordForward:" => Self::WordRight(false),
            "moveWordLeftAndModifySelection:" | "moveWordBackwardAndModifySelection:" => {
                Self::WordLeft(true)
            }
            "moveWordRightAndModifySelection:" | "moveWordForwardAndModifySelection:" => {
                Self::WordRight(true)
            }
            "moveToBeginningOfDocument:" => Self::DocumentStart(false),
            "moveToBeginningOfDocumentAndModifySelection:" => Self::DocumentStart(true),
            "moveToEndOfDocument:" => Self::DocumentEnd(false),
            "moveToEndOfDocumentAndModifySelection:" => Self::DocumentEnd(true),
            "pageUp:" => Self::PageUp,
            "pageDown:" => Self::PageDown,
            "scrollPageUp:" => Self::ScrollPage(false),
            "scrollPageDown:" => Self::ScrollPage(true),
            "pageUpAndModifySelection:" => Self::SelectPage(false),
            "pageDownAndModifySelection:" => Self::SelectPage(true),
            "centerSelectionInVisibleArea:" => Self::CenterSelection,
            "yank:" => Self::Yank,
            "transpose:" => Self::Transpose,
            "setMark:" => Self::SetMark,
            "selectToMark:" => Self::SelectToMark,
            "swapWithMark:" => Self::SwapWithMark,
            "deleteToMark:" => Self::DeleteToMark,
            "uppercaseWord:" => Self::UppercaseWord,
            "lowercaseWord:" => Self::LowercaseWord,
            "capitalizeWord:" => Self::CapitalizeWord,
            "selectWord:" => Self::SelectWord,
            "selectLine:" => Self::SelectLine,
            "selectParagraph:" => Self::SelectParagraph,
            "cancelOperation:" => Self::Cancel,
            "undo:" => Self::Undo,
            "redo:" => Self::Redo,
            "copy:" => Self::Copy,
            "cut:" => Self::Cut,
            "paste:" => Self::Paste,
            "selectAll:" => Self::SelectAll,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_expand_surrogates_clip_overflow_and_preserve_actual_range() {
        let state = TextInputState {
            text: Text::from("a🦀你é"),
            ..Default::default()
        };
        assert_eq!(
            state.substring(Utf16Range::new(2, 1)),
            Some(("🦀".into(), Utf16Range::new(1, 2)))
        );
        assert_eq!(
            state.substring(Utf16Range::new(4, usize::MAX)).unwrap().0,
            "é"
        );
        assert!(state.substring(Utf16Range::new(99, 1)).is_none());
        assert_eq!(Utf16Range::new(2, 0).bytes(&state.text), 1..1);
    }
    #[test]
    fn standard_selectors_and_side_flags() {
        assert_eq!(
            Command::from_selector("moveLeftAndModifySelection:"),
            Some(Command::Left(true))
        );
        assert!(Command::from_selector("unrecognized:").is_none());
        let flags = Modifiers((1 << 17) | 0x4);
        assert!(flags.shift());
        assert!(flags.sides()[1]);
        assert!(!flags.sides()[0]);
    }
}
