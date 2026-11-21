//! Buffer-backed panel input with transient native composition and undo.
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_buffer::{Buffer, Region, Selection, ViewId};
use vel_commands::{Host, Registry};
use vel_config::{Value, settings::EditorSettings};
use vel_platform::{Command, Point, Rect, TextInput, TextInputState, Utf16Range};
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
    byte_limit: Option<usize>,
    /// UI scale shared by painting, hit testing, selection and native IME geometry.
    pub scale: f64,
    font: Option<vel_minihtml::TextStyle>,
    metrics: Option<std::rc::Rc<crate::minihtml::Metrics>>,
    margin: f64,
    /// Editable shared document.
    pub buffer: Buffer,
    /// Single input view.
    pub view: ViewId,
    /// Layout bounds for hit testing and native candidate geometry.
    pub bounds: Rect,
    /// Horizontal scroll in grid cells.
    pub scroll: usize,
    multiline: bool,
    first_row: usize,
    row_pitch: f64,
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
            byte_limit: None,
            scale: 1.,
            font: None,
            metrics: None,
            margin: 8.,
            buffer,
            view,
            bounds: Rect::default(),
            scroll: 0,
            multiline: false,
            first_row: 0,
            row_pitch: 18.,
            registry: Registry::standard(),
            clipboard: String::new(),
            composition: None,
        }
    }
    /// Creates an empty single-line input with a UTF-8 byte limit.
    pub fn bounded_line(limit: usize) -> Self {
        let mut input = Self::new("");
        input.byte_limit = Some(limit);
        input.buffer.set_history_limits(vel_buffer::HistoryLimits {
            entries: 32,
            bytes: limit * 4,
            ..Default::default()
        });
        input
    }
    fn available(&self, replacing: usize) -> usize {
        self.byte_limit.map_or(usize::MAX, |limit| {
            limit.saturating_sub(self.buffer.text().len_bytes() - replacing)
        })
    }
    fn selected_len(&self) -> usize {
        self.buffer
            .view(self.view)
            .sel()
            .primary()
            .expect("selection")
            .to_range()
            .len()
    }
    /// Enables hard line breaks for search and replacement fields only.
    pub fn set_multiline(&mut self, enabled: bool) {
        if self.multiline != enabled {
            self.multiline = enabled;
            self.first_row = 0;
            self.scroll = 0;
        }
    }
    /// Whether the field accepts hard line breaks.
    pub fn multiline(&self) -> bool {
        self.multiline
    }
    /// Natural height in hard rows, capped so a paste cannot fill the window.
    pub fn natural_rows(&self) -> usize {
        if self.multiline {
            self.preview_text().len_lines().min(4)
        } else {
            1
        }
    }
    /// Whether a vertical arrow can move within the field instead of browsing history.
    pub fn can_move_vertically(&self, forward: bool) -> bool {
        if !self.multiline {
            return false;
        }
        let text = self.buffer.text();
        let caret = self
            .buffer
            .view(self.view)
            .sel()
            .primary()
            .expect("selection")
            .b;
        let row = text.byte_to_line(caret);
        if forward {
            row + 1 < text.len_lines()
        } else {
            row > 0
        }
    }
    /// Row spacing after the panel's vertical size constraints.
    pub fn set_row_pitch(&mut self, pitch: f64) {
        self.row_pitch = pitch.max(1.);
    }
    /// Rows the field can show at once, how many hard rows it holds, and the first one shown
    /// (for a scroll indicator).
    pub fn shown_rows(&self) -> (usize, usize, usize) {
        (
            self.fitted_rows(),
            self.buffer.text().len_lines(),
            self.first_row,
        )
    }
    fn fitted_rows(&self) -> usize {
        ((self.bounds.height - self.row_pitch * (10. / 18.)) / self.row_pitch + 0.01)
            .floor()
            .max(1.) as usize
    }
    /// Visible hard rows, with their absolute byte starts and top positions.
    pub fn visible_rows<'a>(
        &'a self,
        text: &'a Text,
    ) -> impl Iterator<Item = (usize, f64, String)> + 'a {
        let first = self.first_row.min(text.len_lines() - 1);
        let end = first
            .saturating_add(self.fitted_rows())
            .min(text.len_lines());
        (first..end).map(move |row| {
            let (start, value) = Self::line(text, row);
            (
                start,
                self.bounds.y + (row - first) as f64 * self.row_pitch,
                value,
            )
        })
    }
    fn line(text: &Text, row: usize) -> (usize, String) {
        let start = text.line_to_byte(row);
        (
            start,
            text.slice(start..start + text.line_len_bytes(row))
                .to_string(),
        )
    }
    /// Shares the theme's actual metrics with painting, hit testing and native IME geometry.
    pub fn configure_theme(
        &mut self,
        style: &crate::theme::Style,
        metrics: std::rc::Rc<crate::minihtml::Metrics>,
    ) {
        self.font = style.family.as_ref().map(|family| vel_minihtml::TextStyle {
            font_family: family.clone(),
            font_size: style.size,
            font_weight: if style.font.bold { 700 } else { 400 },
            italic: style.font.italic,
            underline: false,
            color: vel_theme::Color::rgb(0, 0, 0),
        });
        self.metrics = Some(metrics);
        self.margin = f64::from(style.margin.left).max(4. * self.scale);
        self.reveal();
    }
    /// Horizontal position for a byte boundary in the displayed text, including scrolling.
    pub fn x_for_byte(&self, text: &str, byte: usize) -> f64 {
        let start = byte_at_column(text, self.scroll);
        let byte = byte.max(start).min(text.len());
        self.bounds.x + self.margin() + self.advance(&text[start..byte])
    }
    fn margin(&self) -> f64 {
        if self.font.is_some() {
            self.margin
        } else {
            8. * self.scale
        }
    }
    fn advance(&self, text: &str) -> f64 {
        if let (Some(font), Some(metrics)) = (&self.font, &self.metrics) {
            use vel_minihtml::FontMetrics;
            f64::from(metrics.measure(text, font).advance)
        } else {
            cells(text) as f64 * 7.2 * self.scale
        }
    }
    fn byte_at_x(&self, text: &str, x: f64) -> usize {
        let start = byte_at_column(text, self.scroll);
        let x = (x - self.bounds.x - self.margin()).max(0.);
        let mut previous = 0.;
        for (offset, g) in text[start..].grapheme_indices(true) {
            let end = start + offset + g.len();
            let advance = self.advance(&text[start..end]);
            if x <= (previous + advance) * 0.5 {
                return start + offset;
            }
            previous = advance;
        }
        text.len()
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
    /// Whether Right would have no text selection to collapse or caret movement left.
    pub fn caret_at_end(&self) -> bool {
        self.buffer
            .view(self.view)
            .sel()
            .primary()
            .is_some_and(|region| {
                region.a == region.b && region.b == self.buffer.text().len_bytes()
            })
    }
    /// Executes editing/history/clipboard commands through vel-commands.
    pub fn run(&mut self, name: &str, args: &Value) -> Result<(), String> {
        self.commit_composition();
        let limited;
        let args = if self.byte_limit.is_some()
            && name == "insert"
            && let Some(text) = args["characters"].as_str()
        {
            limited = serde_json::json!({"characters": &text[..text.floor_char_boundary(self.available(self.selected_len()).min(text.len()))]});
            &limited
        } else {
            args
        };
        let mut registry = self.registry.take_for_dispatch();
        let result = registry.run(name, args, self);
        self.registry = registry;
        result
    }
    /// Handles insertText/setMarkedText/unmarkText without publishing preedit into undo history.
    pub fn text_input(&mut self, input: TextInput) {
        match input {
            TextInput::Marked {
                mut text,
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
                if self.byte_limit.is_some() {
                    text.truncate(
                        text.floor_char_boundary(self.available(range.len()).min(text.len())),
                    );
                    text.retain(|c| !matches!(c, '\r' | '\n'));
                }
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
                let text = if self.multiline {
                    normalize_breaks(&text)
                } else {
                    text.replace(['\n', '\r'], "")
                };
                let _ = self.run("insert", &serde_json::json!({"characters":text}));
            }
            TextInput::Unmark => self.commit_composition(),
            TextInput::Discard => self.cancel_composition(),
        }
    }
    fn commit_composition(&mut self) {
        if let Some(c) = self.composition.take() {
            self.buffer
                .set_selection(self.view, Selection::from_regions([Region::from(c.range)]));
            let text = if self.multiline {
                normalize_breaks(&c.text)
            } else {
                c.text
            };
            let _ = self.run("insert", &serde_json::json!({"characters":text}));
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
            Up(e) | Down(e) if self.multiline => (
                "move",
                serde_json::json!({"by":"lines","forward":matches!(command,Down(_)),"extend":e}),
            ),
            Home(e) if self.multiline => ("move_to", serde_json::json!({"to":"bol","extend":e})),
            End(e) if self.multiline => ("move_to", serde_json::json!({"to":"eol","extend":e})),
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
    fn preview_text(&self) -> Text {
        let mut text = self.buffer.text().clone();
        if let Some(c) = &self.composition {
            text.replace(c.range.clone(), &c.text);
        }
        text
    }
    /// Native UTF-16 snapshot and candidate rectangle, including inline preedit.
    pub fn state(&self) -> TextInputState {
        let text = self.preview_text();
        let region = self
            .buffer
            .view(self.view)
            .sel()
            .primary()
            .expect("selection");
        let (selected, marked) = if let Some(c) = &self.composition {
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
        let caret = self.rect_for_bytes(&text, caret_byte..caret_byte);
        TextInputState {
            text,
            selected,
            marked,
            caret,
        }
    }
    /// Sets clipboard contents before an editing command.
    pub fn set_clipboard(&mut self, mut text: String) {
        if let Some(limit) = self.byte_limit {
            text.truncate(text.floor_char_boundary(limit.min(text.len())));
            text.shrink_to_fit();
        }
        self.clipboard = text;
    }
    /// Clipboard contents after copy/cut.
    pub fn clipboard(&self) -> &str {
        &self.clipboard
    }
    /// Clips horizontal scroll so native caret geometry stays in the visible input.
    pub fn reveal(&mut self) {
        let state = self.state();
        let mut byte = state.selected.bytes(&state.text).end;
        let value = if self.multiline {
            let row = state.text.byte_to_line(byte);
            let visible = self.fitted_rows();
            self.first_row = self
                .first_row
                .min(row)
                .max((row + 1).saturating_sub(visible));
            self.first_row = self
                .first_row
                .min(state.text.len_lines().saturating_sub(visible));
            let (start, value) = Self::line(&state.text, row);
            byte -= start;
            value
        } else {
            state.text.to_string()
        };
        let col = cells(&value[..byte]);
        if col < self.scroll {
            self.scroll = col;
        }
        while self.x_for_byte(&value, byte) > self.bounds.x + self.bounds.width - self.margin()
            && self.scroll < col
        {
            self.scroll += 1;
        }
        self.scroll = self::cells(&value[..byte_at_column(&value, self.scroll)]);
    }
    /// Places a caret from an input-local pointer column.
    pub fn click(&mut self, x: f64, extend: bool) {
        self.click_at(
            Point {
                x,
                y: self.bounds.y,
            },
            extend,
        );
    }
    /// Places a caret from a pointer, using both coordinates in multiline fields.
    pub fn click_at(&mut self, point: Point, extend: bool) {
        self.commit_composition();
        let byte = self.byte_at_point(self.buffer.text(), point);
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
        self.reveal();
    }
    /// Visible suffix at the current display-cell scroll position.
    pub fn visible_text(&self) -> String {
        let value = self.state().text.to_string();
        value[byte_at_column(&value, self.scroll)..].into()
    }
    /// Horizontally clipped suffix of one display row.
    pub fn visible_suffix<'a>(&self, row: &'a str) -> &'a str {
        &row[byte_at_column(row, self.scroll)..]
    }
    fn byte_at_point(&self, text: &Text, point: Point) -> usize {
        if self.multiline {
            let local_y = point.y - self.bounds.y - self.row_pitch * (5. / 18.);
            let row = self
                .first_row
                .saturating_add(
                    ((local_y / self.row_pitch).max(0.) as usize).min(self.fitted_rows() - 1),
                )
                .min(text.len_lines() - 1);
            let (start, value) = Self::line(text, row);
            start + self.byte_at_x(&value, point.x)
        } else {
            self.byte_at_x(&text.to_string(), point.x)
        }
    }
    /// Native point-to-UTF-16 lookup, including preedit and wide graphemes.
    pub fn utf16_at_x(&self, x: f64) -> usize {
        self.utf16_at_point(Point {
            x,
            y: self.bounds.y,
        })
    }
    /// Native point-to-UTF-16 lookup in either kind of input.
    pub fn utf16_at_point(&self, point: Point) -> usize {
        let state = self.state();
        state
            .text
            .byte_to_utf16(self.byte_at_point(&state.text, point))
    }
    /// Native range geometry in logical widget coordinates.
    pub fn range_rect(&self, range: Utf16Range) -> (Rect, Utf16Range) {
        let state = self.state();
        let mut bytes = range.bytes(&state.text);
        if self.multiline {
            let row = state.text.byte_to_line(bytes.start);
            bytes.end = bytes
                .end
                .min(state.text.line_to_byte(row) + state.text.line_len_bytes(row));
        }
        (
            self.rect_for_bytes(&state.text, bytes.clone()),
            Utf16Range::from_bytes(&state.text, bytes),
        )
    }
    fn rect_for_bytes(&self, text: &Text, bytes: std::ops::Range<usize>) -> Rect {
        let (offset, y, line, scale) = if self.multiline {
            let row = text.byte_to_line(bytes.start);
            let (start, line) = Self::line(text, row);
            (
                start,
                self.bounds.y + (row as f64 - self.first_row as f64) * self.row_pitch,
                line,
                self.row_pitch / 18.,
            )
        } else {
            (0, self.bounds.y, text.to_string(), self.scale)
        };
        let x = self.x_for_byte(&line, bytes.start - offset);
        Rect {
            x,
            y: y + 6. * scale,
            width: (self.x_for_byte(&line, bytes.end - offset) - x).max(1.),
            height: 18. * scale,
        }
    }
}
fn normalize_breaks(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
impl Host for Input {
    fn buffer(&mut self) -> (&mut Buffer, ViewId) {
        (&mut self.buffer, self.view)
    }
    fn settings(&self) -> EditorSettings {
        EditorSettings::default()
    }
    fn clipboard(&mut self) -> String {
        if self.multiline {
            normalize_breaks(&self.clipboard)
        } else {
            let end = self.clipboard.floor_char_boundary(
                self.available(self.selected_len())
                    .min(self.clipboard.len()),
            );
            self.clipboard[..end].replace(['\n', '\r'], " ")
        }
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
