//! Single-line buffer-backed panel input with transient native composition and undo.
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_buffer::{Buffer, Region, Selection, ViewId};
use vel_commands::{Host, Registry};
use vel_config::{Value, settings::EditorSettings};
use vel_platform::{Command, Rect, TextInput, TextInputState, Utf16Range};
use vel_text::Text;

/// Monospace display cells, keeping combining sequences and emoji graphemes together.
pub fn cells(text: &str) -> usize {
    text.graphemes(true)
        .map(|g| UnicodeWidthStr::width(g).max(1))
        .sum()
}
fn byte_at_column(text: &str, column: usize) -> usize {
    let mut cell = 0;
    for (byte, g) in text.grapheme_indices(true) {
        let width = UnicodeWidthStr::width(g).max(1);
        if column < cell + width {
            return if column - cell < width.div_ceil(2) {
                byte
            } else {
                byte + g.len()
            };
        }
        cell += width;
    }
    text.len()
}

#[derive(Clone, Debug)]
struct Composition {
    range: std::ops::Range<usize>,
    text: String,
    selected: Utf16Range,
}
/// A widget input. Committed edits use the same registry and buffer transactions as document edits.
#[derive(Debug)]
pub struct Input {
    /// Editable shared document.
    pub buffer: Buffer,
    /// Single input view.
    pub view: ViewId,
    /// Layout bounds for hit testing and native candidate geometry.
    pub bounds: Rect,
    /// Horizontal scroll in grid cells.
    pub scroll: usize,
    registry: Registry,
    clipboard: String,
    composition: Option<Composition>,
}
impl Default for Input {
    fn default() -> Self {
        Self::new("")
    }
}
impl Input {
    /// Creates an input with the caret at its end.
    pub fn new(value: &str) -> Self {
        let mut buffer = Buffer::new(value);
        let view = buffer.new_view();
        buffer.set_selection(
            view,
            Selection::from_regions([Region::caret(buffer.text().len_bytes())]),
        );
        Self {
            buffer,
            view,
            bounds: Rect::default(),
            scroll: 0,
            registry: Registry::standard(),
            clipboard: String::new(),
            composition: None,
        }
    }
    /// Replaces the value as one undoable operation and selects it for immediate typing.
    pub fn set(&mut self, value: &str) {
        self.composition = None;
        self.run("select_all", &serde_json::json!({}))
            .expect("select all");
        self.run("insert", &serde_json::json!({"characters":value}))
            .expect("insert");
        self.run("select_all", &serde_json::json!({}))
            .expect("select all");
    }
    /// Committed text, normally a small input value.
    pub fn value(&self) -> String {
        self.buffer.text().to_string()
    }
    /// Whether AppKit is composing; navigation/accept must defer to the input method.
    pub fn composing(&self) -> bool {
        self.composition.is_some()
    }
    /// Executes editing/history/clipboard commands through vel-commands.
    pub fn run(&mut self, name: &str, args: &Value) -> Result<(), String> {
        self.commit_composition();
        let mut registry = std::mem::take(&mut self.registry);
        let result = registry.run(name, args, self);
        self.registry = registry;
        result
    }
    /// Handles insertText/setMarkedText/unmarkText without publishing preedit into undo history.
    pub fn text_input(&mut self, input: TextInput) {
        match input {
            TextInput::Marked {
                text,
                selected,
                replacement,
            } => {
                if text.is_empty() {
                    self.composition = None;
                    return;
                }
                let state = self.state();
                let range = replacement
                    .map(|r| r.bytes(&state.text))
                    .unwrap_or_else(|| {
                        self.composition.as_ref().map_or_else(
                            || {
                                self.buffer
                                    .view(self.view)
                                    .sel()
                                    .primary()
                                    .expect("selection")
                                    .to_range()
                            },
                            |c| c.range.clone(),
                        )
                    });
                // Native replacement coordinates refer to the preview. Clamp to the original selected span.
                let range = if let Some(c) = &self.composition {
                    let end = c.range.start + c.text.len();
                    let map = |p: usize| {
                        if p <= c.range.start {
                            p
                        } else if p >= end {
                            c.range.end + p - end
                        } else {
                            c.range.start
                        }
                    };
                    if replacement.is_some() {
                        map(range.start)..if range.end > c.range.start && range.end < end {
                            c.range.end
                        } else {
                            map(range.end)
                        }
                    } else {
                        c.range.clone()
                    }
                } else {
                    range
                };
                self.composition = Some(Composition {
                    range,
                    text,
                    selected,
                });
            }
            TextInput::Insert { text, replacement } => {
                let state = self.state();
                let composition = self.composition.take();
                let range = replacement
                    .map(|r| {
                        let range = r.bytes(&state.text);
                        if let Some(c) = &composition {
                            let end = c.range.start + c.text.len();
                            let map = |p: usize, right: bool| {
                                if p <= c.range.start {
                                    p
                                } else if p >= end {
                                    c.range.end + p - end
                                } else if right {
                                    c.range.end
                                } else {
                                    c.range.start
                                }
                            };
                            map(range.start, false)..map(range.end, true)
                        } else {
                            range
                        }
                    })
                    .or_else(|| composition.map(|c| c.range));
                if let Some(range) = range {
                    self.buffer
                        .set_selection(self.view, Selection::from_regions([Region::from(range)]));
                }
                let _ = self.run(
                    "insert",
                    &serde_json::json!({"characters":text.replace(['\n', '\r'], "")}),
                );
            }
            TextInput::Unmark => self.commit_composition(),
        }
    }
    fn commit_composition(&mut self) {
        if let Some(c) = self.composition.take() {
            self.buffer
                .set_selection(self.view, Selection::from_regions([Region::from(c.range)]));
            let _ = self.run("insert", &serde_json::json!({"characters":c.text}));
        }
    }
    /// Discards preedit only; Escape's panel-close behavior belongs to the focus router.
    pub fn cancel_composition(&mut self) {
        self.composition = None;
    }
    /// Routes native editing selectors. Newline/Tab acceptance belongs to the panel controller.
    pub fn command(&mut self, command: Command) {
        use Command::*;
        let (name, args) = match command {
            Cancel => {
                self.cancel_composition();
                return;
            }
            DeleteBackward => ("left_delete", serde_json::json!({})),
            DeleteForward => ("right_delete", serde_json::json!({})),
            Left(e) | Right(e) => (
                "move",
                serde_json::json!({"by":"characters","forward":matches!(command,Right(_)),"extend":e}),
            ),
            WordLeft(e) | WordRight(e) => (
                "move",
                serde_json::json!({"by":"word_ends","forward":matches!(command,WordRight(_)),"extend":e}),
            ),
            Home(e) | DocumentStart(e) => ("move_to", serde_json::json!({"to":"bof","extend":e})),
            End(e) | DocumentEnd(e) => ("move_to", serde_json::json!({"to":"eof","extend":e})),
            SelectAll => ("select_all", serde_json::json!({})),
            Undo => ("undo", serde_json::json!({})),
            Redo => ("redo", serde_json::json!({})),
            Copy => ("copy", serde_json::json!({})),
            Cut => ("cut", serde_json::json!({})),
            Paste => ("paste", serde_json::json!({})),
            _ => return,
        };
        let _ = self.run(name, &args);
    }
    /// Native UTF-16 snapshot and candidate rectangle, including inline preedit.
    pub fn state(&self) -> TextInputState {
        let mut text = self.buffer.text().clone();
        let region = self
            .buffer
            .view(self.view)
            .sel()
            .primary()
            .expect("selection");
        let (selected, marked) = if let Some(c) = &self.composition {
            text.replace(c.range.clone(), &c.text);
            let relative = c.selected.bytes(&Text::from(c.text.as_str()));
            (
                Utf16Range::from_bytes(
                    &text,
                    c.range.start + relative.start..c.range.start + relative.end,
                ),
                Some(Utf16Range::from_bytes(
                    &text,
                    c.range.start..c.range.start + c.text.len(),
                )),
            )
        } else {
            (Utf16Range::from_bytes(&text, region.to_range()), None)
        };
        let caret_byte = selected.bytes(&text).end;
        let caret = Rect {
            x: self.bounds.x
                + 8.
                + cells(&text.slice(0..caret_byte).to_string()).saturating_sub(self.scroll) as f64
                    * 7.2,
            y: self.bounds.y + 6.,
            width: 1.,
            height: 18.,
        };
        TextInputState {
            text,
            selected,
            marked,
            caret,
        }
    }
    /// Sets clipboard contents before an editing command.
    pub fn set_clipboard(&mut self, text: String) {
        self.clipboard = text;
    }
    /// Clipboard contents after copy/cut.
    pub fn clipboard(&self) -> &str {
        &self.clipboard
    }
    /// Clips horizontal scroll so native caret geometry stays in the visible input.
    pub fn reveal(&mut self) {
        let state = self.state();
        let col = cells(
            &state
                .text
                .slice(0..state.selected.bytes(&state.text).end)
                .to_string(),
        );
        let cells = ((self.bounds.width - 20.) / 7.2).max(1.) as usize;
        if col < self.scroll {
            self.scroll = col;
        }
        if col > self.scroll + cells {
            self.scroll = col - cells;
        }
        let value = state.text.to_string();
        self.scroll = self::cells(&value[..byte_at_column(&value, self.scroll)]);
    }
    /// Places a caret from an input-local pointer column.
    pub fn click(&mut self, x: f64, extend: bool) {
        self.commit_composition();
        let col = ((x - self.bounds.x - 8.) / 7.2).round().max(0.) as usize + self.scroll;
        let byte = byte_at_column(&self.value(), col);
        let old = self
            .buffer
            .view(self.view)
            .sel()
            .primary()
            .expect("selection");
        self.buffer.set_selection(
            self.view,
            Selection::from_regions([Region::new(if extend { old.a } else { byte }, byte)]),
        );
    }
    /// Visible suffix at the current display-cell scroll position.
    pub fn visible_text(&self) -> String {
        let value = self.state().text.to_string();
        value[byte_at_column(&value, self.scroll)..].into()
    }
    /// Native point-to-UTF-16 lookup, including preedit and wide graphemes.
    pub fn utf16_at_x(&self, x: f64) -> usize {
        let state = self.state();
        let column = ((x - self.bounds.x - 8.) / 7.2).round().max(0.) as usize + self.scroll;
        state
            .text
            .byte_to_utf16(byte_at_column(&state.text.to_string(), column))
    }
    /// Native range geometry in logical widget coordinates.
    pub fn range_rect(&self, range: Utf16Range) -> (Rect, Utf16Range) {
        let state = self.state();
        let bytes = range.bytes(&state.text);
        let start = cells(&state.text.slice(0..bytes.start).to_string());
        let length = cells(&state.text.slice(bytes.clone()).to_string());
        (
            Rect {
                x: self.bounds.x + 8. + start.saturating_sub(self.scroll) as f64 * 7.2,
                y: self.bounds.y + 6.,
                width: (length as f64 * 7.2).max(1.),
                height: 18.,
            },
            Utf16Range::from_bytes(&state.text, bytes),
        )
    }
}
impl Host for Input {
    fn buffer(&mut self) -> (&mut Buffer, ViewId) {
        (&mut self.buffer, self.view)
    }
    fn settings(&self) -> EditorSettings {
        EditorSettings::default()
    }
    fn clipboard(&mut self) -> String {
        self.clipboard.replace(['\n', '\r'], " ")
    }
    fn set_clipboard(&mut self, text: &str) {
        self.clipboard = text.into();
    }
    fn action(&mut self, _: &str, _: &Value) -> Result<(), String> {
        Err("input accepts text commands only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_ime_replacement_and_undo_are_isolated_from_document() {
        let mut input = Input::new("a😀z");
        input
            .buffer
            .set_selection(input.view, Selection::from_regions([Region::new(1, 5)]));
        input.text_input(TextInput::Marked {
            text: "ni".into(),
            selected: Utf16Range::new(2, 0),
            replacement: None,
        });
        assert_eq!(input.value(), "a😀z");
        assert_eq!(input.state().text.to_string(), "aniz");
        input.text_input(TextInput::Insert {
            text: "你".into(),
            replacement: None,
        });
        assert_eq!(input.value(), "a你z");
        input.command(Command::Undo);
        assert_eq!(input.value(), "a😀z");
    }
    #[test]
    fn wide_graphemes_preedit_and_candidate_geometry_use_display_cells() {
        let mut input = Input::new("你é😀");
        input.bounds = Rect {
            x: 100.,
            y: 200.,
            width: 200.,
            height: 30.,
        };
        assert_eq!(cells(&input.value()), 5);
        assert_eq!(input.state().caret.x, 144.);
        input.click(108. + 2. * 7.2, false);
        assert_eq!(input.buffer.view(input.view).sel().primary().unwrap().b, 3);
        input.text_input(TextInput::Marked {
            text: "你好".into(),
            selected: Utf16Range::new(usize::MAX, 50),
            replacement: None,
        });
        let state = input.state();
        assert!(state.selected.end() <= state.text.len_utf16());
        input.cancel_composition();
        input.command(Command::End(false));
        assert_eq!(input.utf16_at_x(144.), input.buffer.text().len_utf16());
    }
}
