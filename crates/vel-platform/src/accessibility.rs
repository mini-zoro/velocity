//! Device-free accessibility tree and bounded, scalar-aligned UTF-16 text queries.
//! The native bridge converts logical rectangles to screen points, never pixels.
use crate::{Point, Rect, TextInputState, Utf16Range};
use vel_text::Text;
/// Maximum UTF-16 code units returned by one text-area value query.
pub const TEXT_WINDOW: usize = 65_536;
/// Stable identity of an accessible element in a window.
pub type Id = u64;
/// Public AppKit accessibility roles used by the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Container with ordered children.
    Group,
    /// Editable or read-only multiline document.
    TextArea,
    /// Single line query, replacement, URL or other input.
    TextField,
    /// Document tabs.
    TabGroup,
    /// One document tab (AppKit represents tabs as radio buttons).
    Tab,
    /// Project tree.
    Outline,
    /// Project entry or quick-panel result.
    Row,
    /// Quick-panel results or completion suggestions.
    List,
    /// Noninteractive status text.
    Status,
    /// Action button.
    Button,
    /// Opens a choice list.
    PopupButton,
    /// Navigates from a visible minihtml link.
    Link,
}
impl Role {
    /// The corresponding public AX role string.
    pub const fn native(self) -> &'static str {
        match self {
            Self::Group => "AXGroup",
            Self::TextArea => "AXTextArea",
            Self::TextField => "AXTextField",
            Self::TabGroup => "AXTabGroup",
            Self::Tab => "AXRadioButton",
            Self::Outline => "AXOutline",
            Self::Row => "AXRow",
            Self::List => "AXList",
            Self::Status => "AXStaticText",
            Self::Button => "AXButton",
            Self::PopupButton => "AXPopUpButton",
            Self::Link => "AXLink",
        }
    }
}
/// A bounded value with range coordinates relative to that value, as required by AX.
/// `document_offset` translates these ranges back to the editor's UTF-16 coordinates.
#[derive(Clone, Debug)]
pub struct TextArea {
    /// Bounded immutable text, materialized only in response to an AX query.
    pub text: Text,
    /// Document UTF-16 offset corresponding to index zero in `text`.
    pub document_offset: usize,
    /// Zero-based document line containing index zero in `text`.
    pub first_line: usize,
    /// Primary selection clipped to this value window.
    pub selected: Utf16Range,
    /// Zero-based document line of the insertion point, even in a windowed value.
    pub insertion_line: usize,
    /// Visible intersection with this value window.
    pub visible: Utf16Range,
}
impl TextArea {
    /// Builds a bounded window around the primary selection/caret, including IME preedit.
    pub fn new(state: &TextInputState, visible: Utf16Range) -> Self {
        let source = &state.text;
        let total = source.len_utf16();
        let caret = state.selected.location.min(total);
        let start = source.clip_utf16(
            caret
                .saturating_sub(TEXT_WINDOW / 2)
                .min(total.saturating_sub(TEXT_WINDOW)),
        );
        let end = source.clip_utf16((start + TEXT_WINDOW).min(total));
        let bytes = source.utf16_to_byte(start)..source.utf16_to_byte(end);
        let first_line = source.byte_to_line(bytes.start);
        let insertion_line = source
            .byte_to_line(source.utf16_to_byte(source.clip_utf16(state.selected.end().min(total))));
        let text = if start == 0 && end == total {
            source.clone()
        } else {
            Text::from(source.slice(bytes).to_string())
        };
        let local = |range: Utf16Range| {
            let a = range.location.clamp(start, end) - start;
            let b = range.end().clamp(start, end) - start;
            Utf16Range::from_bytes(&text, Utf16Range::new(a, b.saturating_sub(a)).bytes(&text))
        };
        let selected = local(state.selected);
        let visible = local(visible);
        Self {
            text,
            document_offset: start,
            first_line,
            selected,
            insertion_line,
            visible,
        }
    }
    /// Returns the complete bounded accessibility value.
    pub fn value(&self) -> String {
        self.text.to_string()
    }
    /// Clips an untrusted relative range to complete Unicode scalars.
    pub fn clip(&self, range: Utf16Range) -> Utf16Range {
        Utf16Range::from_bytes(&self.text, range.bytes(&self.text))
    }
    /// Translates a window-relative range into a document-relative range.
    pub fn document_range(&self, range: Utf16Range) -> Utf16Range {
        let range = self.clip(range);
        Utf16Range::new(self.document_offset + range.location, range.length)
    }
    /// AXStringForRange; out-of-bounds start returns no string.
    pub fn string(&self, range: Utf16Range) -> Option<String> {
        (range.location <= self.text.len_utf16())
            .then(|| self.text.slice(range.bytes(&self.text)).to_string())
    }
    /// AXLineForIndex, zero-based within the exposed value window.
    pub fn line(&self, index: usize) -> Option<usize> {
        (index <= self.text.len_utf16()).then(|| {
            self.text
                .byte_to_line(self.text.utf16_to_byte(self.text.clip_utf16(index)))
        })
    }
    /// AXRangeForLine, including the terminating newline, or None for an absent line.
    pub fn range_for_line(&self, line: usize) -> Option<Utf16Range> {
        (line < self.text.len_lines()).then(|| {
            let start = self.text.line_to_byte(line);
            let end = if line + 1 < self.text.len_lines() {
                self.text.line_to_byte(line + 1)
            } else {
                self.text.len_bytes()
            };
            Utf16Range::from_bytes(&self.text, start..end)
        })
    }
}
/// One accessible control; children are determined by their `parent` identity.
#[derive(Clone, Debug)]
pub struct Node {
    /// Stable identity.
    pub id: Id,
    /// Parent identity, or None for a root child of the native content view.
    pub parent: Option<Id>,
    /// Public semantic role.
    pub role: Role,
    /// Spoken name.
    pub label: String,
    /// Usage instructions or window offset explanation.
    pub help: String,
    /// Logical top-left coordinates in the content view.
    pub frame: Rect,
    /// Current selected state.
    pub selected: bool,
    /// Current keyboard focus.
    pub focused: bool,
    /// Whether actions are available.
    pub enabled: bool,
    /// Whether the control accepts text edits.
    pub editable: bool,
    /// Tree expansion state; None for leaves.
    pub expanded: Option<bool>,
    /// Zero-based tree indentation.
    pub level: usize,
    /// Activation point and whether activation requires a double click.
    pub activation: Option<(Point, bool)>,
    /// Text data, only for text roles.
    pub text: Option<TextArea>,
}
impl Node {
    /// Creates an enabled, unselected control.
    pub fn new(
        id: Id,
        parent: Option<Id>,
        role: Role,
        label: impl Into<String>,
        frame: Rect,
    ) -> Self {
        Self {
            id,
            parent,
            role,
            label: label.into(),
            help: String::new(),
            frame,
            selected: false,
            focused: false,
            enabled: true,
            editable: false,
            expanded: None,
            level: 0,
            activation: None,
            text: None,
        }
    }
    /// Whether the control participates in linear keyboard navigation.
    pub fn focusable(&self) -> bool {
        self.enabled && (self.activation.is_some() || self.text.is_some())
    }
}
/// Current hierarchy in logical navigation order. Built only for accessibility requests.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    /// Parent-before-child ordered elements.
    pub nodes: Vec<Node>,
}
impl Tree {
    /// Looks up a currently visible identity.
    pub fn node(&self, id: Id) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    /// Current keyboard target.
    pub fn focused(&self) -> Option<Id> {
        self.nodes.iter().find(|n| n.focused).map(|n| n.id)
    }
    /// Advances focus through active controls, wrapping in both directions.
    pub fn next_focus(&self, current: Option<Id>, backwards: bool) -> Option<Id> {
        let order = self
            .nodes
            .iter()
            .filter(|n| n.focusable())
            .map(|n| n.id)
            .collect::<Vec<_>>();
        if order.is_empty() {
            return None;
        }
        let index = current.and_then(|id| order.iter().position(|n| *n == id));
        let next = match (index, backwards) {
            (Some(i), true) => (i + order.len() - 1) % order.len(),
            (Some(i), false) => (i + 1) % order.len(),
            (None, true) => order.len() - 1,
            (None, false) => 0,
        };
        Some(order[next])
    }
    /// Returns the deepest visible element at a logical point.
    pub fn hit(&self, point: Point) -> Option<Id> {
        self.nodes
            .iter()
            .rev()
            .find(|n| {
                point.x >= n.frame.x
                    && point.y >= n.frame.y
                    && point.x < n.frame.x + n.frame.width
                    && point.y < n.frame.y + n.frame.height
            })
            .map(|n| n.id)
    }
}
/// Native accessibility actions, delivered on the same queue as keyboard/mouse events.
#[derive(Clone, Debug)]
pub enum Request {
    /// Activate a control.
    Press,
    /// Move keyboard focus to a control.
    Focus,
    /// Set the primary selection in value-relative UTF-16 coordinates.
    Select(Utf16Range),
    /// Replace a text control's entire exposed value.
    SetValue(String),
}
/// System display preferences; queried and observed by the native bridge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Preferences {
    /// Disable theme transitions and texture animation.
    pub reduce_motion: bool,
    /// Strengthen control borders and selection contrast.
    pub increase_contrast: bool,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_surrogates_combining_crlf_empty_lines_and_invalid_ranges() {
        let state = TextInputState {
            text: Text::from("a😀e\u{301}\r\n你好\n"),
            selected: Utf16Range::new(1, 2),
            ..Default::default()
        };
        let model = TextArea::new(&state, Utf16Range::new(0, 99));
        assert_eq!(model.string(Utf16Range::new(2, 1)).as_deref(), Some("😀"));
        assert_eq!(model.range_for_line(0), Some(Utf16Range::new(0, 7)));
        assert_eq!(model.line(7), Some(1));
        assert_eq!(model.range_for_line(2), Some(Utf16Range::new(10, 0)));
        assert_eq!(model.range_for_line(3), None);
        assert_eq!(model.string(Utf16Range::new(usize::MAX, usize::MAX)), None);
        assert_eq!(
            model.clip(Utf16Range::new(usize::MAX, usize::MAX)),
            Utf16Range::new(10, 0)
        );
        assert_eq!(
            TextArea::new(&TextInputState::default(), Utf16Range::default()).range_for_line(0),
            Some(Utf16Range::default())
        );
    }
    #[test]
    fn huge_value_is_bounded_and_selected_ranges_round_trip() {
        let text = Text::from("😀line\n".repeat(100_000));
        let at = text.line_to_byte(90_000);
        let selected = Utf16Range::from_bytes(&text, at..at + 4);
        let model = TextArea::new(
            &TextInputState {
                text,
                selected,
                ..Default::default()
            },
            selected,
        );
        assert!(model.text.len_utf16() <= TEXT_WINDOW);
        assert!(model.document_offset > 0);
        assert_eq!(model.insertion_line, 90_000);
        assert_eq!(model.document_range(model.selected), selected);
        assert_eq!(model.string(model.selected).as_deref(), Some("😀"));
    }
    #[test]
    fn roles_focus_and_logical_bounds_are_independent_of_retina() {
        let bounds = Rect {
            x: 10.,
            y: 20.,
            width: 100.,
            height: 30.,
        };
        let mut tab = Node::new(1, None, Role::Tab, "file.rs", bounds);
        tab.activation = Some((Point { x: 15., y: 25. }, false));
        let status = Node::new(2, None, Role::Status, "Line 2", bounds);
        let tree = Tree {
            nodes: vec![tab, status],
        };
        assert_eq!(tree.next_focus(None, false), Some(1));
        assert_eq!(tree.next_focus(Some(1), true), Some(1));
        assert_eq!(tree.hit(Point { x: 11., y: 21. }), Some(2));
        assert_eq!(Role::TextArea.native(), "AXTextArea");
        assert_eq!(Role::Outline.native(), "AXOutline");
        assert_eq!(tree.nodes[0].frame, bounds);
    }
}
