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
    /// A section heading in a content list.
    Heading,
    /// Noninteractive status text.
    Status,
    /// Action button.
    Button,
    /// A checked/unchecked toggle.
    CheckBox,
    /// An on/off preference.
    Switch,
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
            Self::Heading => "AXHeading",
            Self::Status => "AXStaticText",
            Self::Button => "AXButton",
            Self::CheckBox => "AXCheckBox",
            Self::Switch => "AXSwitch",
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
    /// Zero-based line of the insertion point within the exposed value.
    pub insertion_line: usize,
    /// Visible intersection with this value window.
    pub visible: Utf16Range,
}
impl TextArea {
    /// Builds a bounded window around the primary selection/caret, including IME preedit.
    pub fn new(state: &TextInputState, visible: Utf16Range) -> Self {
        Self::windowed(state, visible, state.selected.end(), None)
    }
    /// Uses a page-aligned window with hysteresis, relative to the primary caret.
    pub fn windowed(
        state: &TextInputState,
        visible: Utf16Range,
        caret: usize,
        previous: Option<usize>,
    ) -> Self {
        let source = &state.text;
        let total = source.len_utf16();
        let caret = source.clip_utf16(caret.min(total));
        let page = TEXT_WINDOW / 4;
        let last = total.saturating_sub(TEXT_WINDOW).div_ceil(page) * page;
        let start = source.clip_utf16(
            previous
                .filter(|&start| {
                    start <= last
                        && (start == 0 || caret >= start + page)
                        && (start + TEXT_WINDOW >= total || caret <= start + 3 * page)
                })
                .unwrap_or_else(|| (caret.saturating_sub(2 * page) / page * page).min(last)),
        );
        let end = source.clip_utf16((start + TEXT_WINDOW).min(total));
        let bytes = source.utf16_to_byte(start)..source.utf16_to_byte(end);
        let first_line = source.byte_to_line(bytes.start);
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
        let insertion_line = text.byte_to_line(text.utf16_to_byte(caret.clamp(start, end) - start));
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
/// Coarse keyboard navigation stops. Arrow keys traverse controls inside each region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Region {
    /// Tab strip.
    Tabs,
    /// Editors and their splits.
    Editor,
    /// Side bar.
    Sidebar,
    /// Find, output, quick panels and popups.
    Panel,
    /// Status bar commands.
    Status,
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
    /// The region used by keyboard focus cycling.
    pub region: Option<Region>,
    /// State exposed by a checkbox.
    pub checked: Option<bool>,
    /// A change worth announcing to assistive clients.
    pub announcement: Option<String>,
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
            region: None,
            checked: None,
            announcement: None,
        }
    }
    /// Whether the control participates in linear keyboard navigation.
    pub fn focusable(&self) -> bool {
        self.enabled
            && (self.activation.is_some() || matches!(self.role, Role::TextArea | Role::TextField))
    }
}
/// Current hierarchy in logical navigation order. Built only for accessibility requests.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    /// Parent-before-child ordered elements.
    pub nodes: Vec<Node>,
}

#[derive(Default)]
pub(crate) struct TreeCache(std::cell::RefCell<Option<std::rc::Rc<Tree>>>);
impl TreeCache {
    pub(crate) fn invalidate(&self) {
        self.0.take();
    }
    pub(crate) fn get(&self, build: impl FnOnce() -> Option<Tree>) -> std::rc::Rc<Tree> {
        if let Some(tree) = self.0.borrow().as_ref().cloned() {
            return tree;
        }
        let Some(built) = build() else {
            return Default::default();
        };
        let built = std::rc::Rc::new(built);
        *self.0.borrow_mut() = Some(built.clone());
        built
    }
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
    /// Advances focus by region, preferring the selected control in each.
    pub fn next_focus(&self, current: Option<Id>, backwards: bool) -> Option<Id> {
        let mut regions: Vec<&Node> = Vec::new();
        for node in self.nodes.iter().filter(|n| n.focusable()) {
            if let Some(prior) = regions.iter_mut().find(|prior| same_region(prior, node)) {
                if node.focused || (!prior.focused && !prior.selected && node.selected) {
                    *prior = node;
                }
            } else {
                regions.push(node);
            }
        }
        let index = current
            .and_then(|id| self.node(id))
            .and_then(|node| regions.iter().position(|prior| same_region(prior, node)));
        advance(regions, index, backwards)
    }
    /// Moves to another control inside the focused keyboard region.
    pub fn next_in_region(&self, current: Id, backwards: bool) -> Option<Id> {
        let node = self.node(current)?;
        let order: Vec<_> = self
            .nodes
            .iter()
            .filter(|n| n.focusable() && same_region(n, node))
            .collect();
        let index = order.iter().position(|n| n.id == current);
        advance(order, index, backwards).filter(|id| *id != current)
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

fn same_region(a: &Node, b: &Node) -> bool {
    a.id == b.id || (a.region.is_some() && a.region == b.region)
}

fn advance(order: Vec<&Node>, index: Option<usize>, backwards: bool) -> Option<Id> {
    if order.is_empty() {
        return None;
    }
    let next = match (index, backwards) {
        (Some(i), true) => (i + order.len() - 1) % order.len(),
        (Some(i), false) => (i + 1) % order.len(),
        (None, true) => order.len() - 1,
        (None, false) => 0,
    };
    Some(order[next].id)
}

/// Remembers only the latest announced item in each visible semantic channel.
#[derive(Debug, Default)]
pub struct Announcements(std::collections::BTreeMap<Id, String>);
impl Announcements {
    /// Returns new or changed announcements and forgets dismissed controls.
    pub fn changes(&mut self, tree: &Tree) -> Vec<String> {
        let next: std::collections::BTreeMap<_, _> = tree
            .nodes
            .iter()
            .filter_map(|node| {
                node.announcement
                    .as_ref()
                    .map(|text| (node.parent.unwrap_or(node.id), text.clone()))
            })
            .collect();
        let changed = next
            .iter()
            .filter(|(id, text)| self.0.get(id) != Some(text))
            .map(|(_, text)| text.clone())
            .collect();
        self.0 = next;
        changed
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
    fn page_alignment_keeps_caret_and_line_coordinates_stable() {
        let text = Text::from("line\n".repeat(200_000).as_str());
        let offset = text.line_to_byte(100_000);
        let mut state = TextInputState {
            text,
            selected: Utf16Range::new(offset, 0),
            ..Default::default()
        };
        let first = TextArea::windowed(&state, state.selected, offset, None);
        state.selected.location += 1;
        let second = TextArea::windowed(
            &state,
            state.selected,
            offset + 1,
            Some(first.document_offset),
        );
        assert_eq!(first.document_offset % (TEXT_WINDOW / 4), 0);
        assert_eq!(second.document_offset, first.document_offset);
        assert_eq!(second.selected.location, first.selected.location + 1);
        let line = second.range_for_line(second.insertion_line).unwrap();
        assert!(line.location <= second.selected.location && line.end() > second.selected.location);
        let beyond = offset + TEXT_WINDOW;
        assert_ne!(
            TextArea::windowed(&state, state.selected, beyond, Some(first.document_offset))
                .document_offset,
            first.document_offset
        );
    }

    #[test]
    fn twenty_getters_share_one_tree_and_announcement_tracks_the_selected_row() {
        let cache = TreeCache::default();
        let calls = std::cell::Cell::new(0);
        let get = |caption: &str| {
            cache.get(|| {
                calls.set(calls.get() + 1);
                let mut node = Node::new(2, Some(1), Role::Row, caption, Rect::default());
                node.announcement = Some(caption.into());
                Some(Tree { nodes: vec![node] })
            })
        };
        let mut tracker = Announcements::default();
        assert_eq!(tracker.changes(&get("First, 1 of 2")), ["First, 1 of 2"]);
        for _ in 0..20 {
            assert!(tracker.changes(&get("First, 1 of 2")).is_empty());
        }
        assert_eq!(calls.get(), 1);
        cache.invalidate();
        assert_eq!(tracker.changes(&get("Second, 2 of 2")), ["Second, 2 of 2"]);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn five_regions_do_not_walk_every_tab_and_sidebar_row() {
        let mut nodes = Vec::new();
        for (region, count) in [
            (Region::Tabs, 5),
            (Region::Editor, 1),
            (Region::Sidebar, 50),
            (Region::Panel, 3),
            (Region::Status, 2),
        ] {
            for _ in 0..count {
                let mut n = Node::new(
                    nodes.len() as u64 + 1,
                    None,
                    Role::Button,
                    "",
                    Rect::default(),
                );
                n.region = Some(region);
                n.activation = Some((Point::default(), false));
                nodes.push(n);
            }
        }
        let tree = Tree { nodes };
        let mut focus = Some(6);
        for _ in 0..5 {
            focus = tree.next_focus(focus, false);
        }
        assert_eq!(focus, Some(6));
        assert_eq!(tree.next_in_region(8, false), Some(9));
    }
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
        let line = model.range_for_line(model.insertion_line).unwrap();
        assert!(line.location <= model.selected.location && line.end() > model.selected.location);
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
