//! Retained device-free widgets, focus routing and buffer-backed native text inputs.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod input;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    time::{Duration, Instant},
};
use vel_platform::{Point, Rect};
use vel_render::scene::{Layer, SolidRect, TextRun};

/// Stable widget identity, independent of a row's current position.
pub type Id = u64;
/// Keyboard capture priority. Pointer focus inside the chosen surface selects its input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Surface {
    /// Main document or side bar.
    #[default]
    View,
    /// Docked input/output panels.
    Panel,
    /// Quick panel/command palette above other surfaces.
    Overlay,
}
/// Focus router; a visible modal overlay always captures keys, even without a focused child.
#[derive(Clone, Debug, Default)]
pub struct Focus {
    focused: BTreeMap<Surface, Id>,
    visible: BTreeSet<Surface>,
}
impl Focus {
    /// Sets visibility. Closing a surface restores the retained focus of the lower surface.
    pub fn show(&mut self, surface: Surface, visible: bool) {
        if visible {
            self.visible.insert(surface);
        } else {
            self.visible.remove(&surface);
        }
    }
    /// Focuses one input/control in a surface.
    pub fn set(&mut self, surface: Surface, id: Id) {
        self.focused.insert(surface, id);
    }
    /// Returns the highest visible surface and its focused child.
    pub fn route(&self) -> (Surface, Option<Id>) {
        let surface = self.visible.last().copied().unwrap_or(Surface::View);
        (surface, self.focused.get(&surface).copied())
    }
}
/// Container layout. Fixed child extents come from `Node::extent`; other children share space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// Children arranged horizontally.
    Row,
    /// Children arranged vertically.
    Column,
    /// Children overlap; later siblings hit/paint on top.
    #[default]
    Stack,
}
/// Retained widget state. Classes/attributes are the future .sublime-theme selector seam.
#[derive(Clone, Debug)]
pub struct Node {
    /// Theme class, documented in the crate README.
    pub class: &'static str,
    /// Dynamic theme attributes: hover/pressed/selected/dirty/focused/transient.
    pub attributes: BTreeSet<&'static str>,
    /// Laid-out logical bounds.
    pub rect: Rect,
    /// Descendant layout.
    pub layout: Layout,
    /// Fixed extent along the parent's main axis; None shares remaining space.
    pub extent: Option<f64>,
    /// Stable child identities in painter order.
    pub children: Vec<Id>,
    /// Label for simple controls.
    pub label: String,
    /// Receives pointer events.
    pub interactive: bool,
    /// Whether the node participates in painting/layout/hit testing.
    pub visible: bool,
}
impl Node {
    /// Creates an empty visible widget with no children.
    pub fn new(class: &'static str) -> Self {
        Self {
            class,
            attributes: BTreeSet::new(),
            rect: Rect::default(),
            layout: Layout::Stack,
            extent: None,
            children: Vec::new(),
            label: String::new(),
            interactive: true,
            visible: true,
        }
    }
}
/// Retained scene tree and pointer state. No device/font initialization occurs here.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    /// Stable nodes. Callers update properties in place across frames.
    pub nodes: BTreeMap<Id, Node>,
    /// Root identities in back-to-front order.
    pub roots: Vec<Id>,
    /// Focus capture state.
    pub focus: Focus,
    hovered: Option<Id>,
    pressed: Option<Id>,
}
/// Inclusive logical point hit test.
pub fn contains(rect: Rect, point: Point) -> bool {
    point.x >= rect.x
        && point.y >= rect.y
        && point.x < rect.x + rect.width
        && point.y < rect.y + rect.height
}
impl Tree {
    /// Lays out one retained subtree inside a logical rectangle.
    pub fn layout(&mut self, id: Id, bounds: Rect) {
        let Some(node) = self.nodes.get_mut(&id) else {
            return;
        };
        node.rect = bounds;
        let layout = node.layout;
        let children = node.children.clone();
        let visible: Vec<_> = children
            .into_iter()
            .filter(|id| self.nodes.get(id).is_some_and(|n| n.visible))
            .collect();
        let main = if layout == Layout::Row {
            bounds.width
        } else {
            bounds.height
        };
        let fixed: f64 = visible.iter().filter_map(|id| self.nodes[id].extent).sum();
        let flexible = visible
            .iter()
            .filter(|id| self.nodes[id].extent.is_none())
            .count()
            .max(1);
        let share = (main - fixed).max(0.) / flexible as f64;
        let mut offset = 0.;
        for child in visible {
            let extent = self.nodes[&child].extent.unwrap_or(share);
            let rect = match layout {
                Layout::Row => Rect {
                    x: bounds.x + offset,
                    width: extent,
                    ..bounds
                },
                Layout::Column => Rect {
                    y: bounds.y + offset,
                    height: extent,
                    ..bounds
                },
                Layout::Stack => bounds,
            };
            self.layout(child, rect);
            offset += extent;
        }
    }
    /// Topmost visible, interactive descendant at a point, clipped by ancestors.
    pub fn hit_test(&self, point: Point) -> Option<Id> {
        fn hit(tree: &Tree, id: Id, point: Point) -> Option<Id> {
            let node = tree.nodes.get(&id)?;
            if !node.visible || !contains(node.rect, point) {
                return None;
            }
            node.children
                .iter()
                .rev()
                .find_map(|child| hit(tree, *child, point))
                .or_else(|| node.interactive.then_some(id))
        }
        self.roots.iter().rev().find_map(|id| hit(self, *id, point))
    }
    /// Updates hover/pressed attributes. Returns a clicked widget only on matching release.
    pub fn pointer(&mut self, point: Point, pressed: Option<bool>) -> Option<Id> {
        let hovered = self.hit_test(point);
        if let Some(old) = self.hovered.and_then(|id| self.nodes.get_mut(&id)) {
            old.attributes.remove("hover");
        }
        self.hovered = hovered;
        if let Some(node) = hovered.and_then(|id| self.nodes.get_mut(&id)) {
            node.attributes.insert("hover");
        }
        match pressed {
            Some(true) => {
                self.pressed = hovered;
                if let Some(node) = hovered.and_then(|id| self.nodes.get_mut(&id)) {
                    node.attributes.insert("pressed");
                }
                None
            }
            Some(false) => {
                let old = self.pressed.take();
                if let Some(node) = old.and_then(|id| self.nodes.get_mut(&id)) {
                    node.attributes.remove("pressed");
                }
                old.filter(|id| Some(*id) == hovered)
            }
            None => None,
        }
    }
}
/// Bounded virtual-list scroll state, shared by quick panels, side bars and output views.
#[derive(Clone, Debug, Default)]
pub struct Scroll {
    /// Vertical logical offset.
    pub offset: f64,
    /// Number of rows.
    pub rows: usize,
    /// Row extent in points.
    pub row_height: f64,
    /// Visible extent in points.
    pub viewport: f64,
}
impl Scroll {
    /// Clamps scrolling to the current content extent.
    pub fn scroll(&mut self, delta: f64) {
        self.offset = (self.offset + delta).clamp(
            0.,
            (self.rows as f64 * self.row_height - self.viewport).max(0.),
        );
    }
    /// Visible rows plus one overscan row. Allocation/painting stays independent of total rows.
    pub fn visible(&self) -> Range<usize> {
        let first = (self.offset / self.row_height.max(1.)).floor() as usize;
        first.min(self.rows)
            ..(first + (self.viewport / self.row_height.max(1.)).ceil() as usize + 1).min(self.rows)
    }
    /// Reveals a selected row after keyboard navigation.
    pub fn reveal(&mut self, row: usize) {
        let top = row as f64 * self.row_height;
        if top < self.offset {
            self.offset = top;
        }
        if top + self.row_height > self.offset + self.viewport {
            self.offset = top + self.row_height - self.viewport;
        }
        self.scroll(0.);
    }
}
/// Virtualized quick-panel row with independent caption, badge, annotation and details.
#[derive(Clone, Debug, Default)]
pub struct Item {
    /// Caption.
    pub caption: String,
    /// Kind badge, e.g. file/command/symbol.
    pub kind: String,
    /// Right-hand hint, e.g. a key binding.
    pub annotation: String,
    /// Second-line path/help.
    pub details: String,
    /// Highlighted UTF-8 scalar offsets in the caption.
    pub highlights: Vec<usize>,
    /// Stable application payload.
    pub value: usize,
}
/// Overlay list selection; application handles returned preview payloads as transient sheets.
#[derive(Clone, Debug, Default)]
pub struct QuickPanel {
    /// Currently ranked rows (normally the top 100).
    pub items: Vec<Item>,
    /// Selected row.
    pub selected: usize,
    /// Virtualized list viewport.
    pub scroll: Scroll,
}
impl QuickPanel {
    /// Replaces ranking and returns the new live-preview payload.
    pub fn set_items(&mut self, items: Vec<Item>) -> Option<usize> {
        self.items = items;
        self.selected = 0;
        self.scroll.offset = 0.;
        self.scroll.rows = self.items.len();
        self.current()
    }
    /// Selected payload, if any.
    pub fn current(&self) -> Option<usize> {
        self.items.get(self.selected).map(|i| i.value)
    }
    /// Wraps navigation, reveals selection and returns a live-preview payload.
    pub fn navigate(&mut self, delta: isize) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        self.selected =
            (self.selected as isize + delta).rem_euclid(self.items.len() as isize) as usize;
        self.scroll.reveal(self.selected);
        self.current()
    }
    /// Paints visible rows only, including matched-character highlights and metadata.
    pub fn paint(&self, layer: &mut Layer, bounds: Rect) {
        for row in self.scroll.visible() {
            let item = &self.items[row];
            let y = bounds.y + row as f64 * self.scroll.row_height - self.scroll.offset;
            let rect = Rect {
                y,
                height: self.scroll.row_height,
                ..bounds
            };
            if row == self.selected {
                fill(layer, rect, bounds, [0.19, 0.27, 0.37, 1.]);
            }
            label(
                layer,
                &item.caption,
                rect.x + 12.,
                y + 18.,
                bounds,
                [0.87, 0.89, 0.94, 1.],
            );
            for (i, c) in item.caption.char_indices() {
                if item.highlights.contains(&i) {
                    label(
                        layer,
                        &c.to_string(),
                        rect.x + 12. + input::cells(&item.caption[..i]) as f64 * 7.2,
                        y + 18.,
                        bounds,
                        [0.45, 0.8, 1., 1.],
                    );
                }
            }
            label(
                layer,
                &item.details,
                rect.x + 12.,
                y + 35.,
                bounds,
                [0.5, 0.57, 0.65, 1.],
            );
            let hint = format!("{}  {}", item.kind, item.annotation);
            label(
                layer,
                &hint,
                (bounds.x + bounds.width - input::cells(&hint) as f64 * 7.2 - 12.)
                    .max(bounds.x + 12.),
                y + 35.,
                bounds,
                [0.67, 0.7, 0.77, 1.],
            );
        }
    }
}
/// Adds a clipped solid primitive without initializing a renderer.
pub fn fill(layer: &mut Layer, rect: Rect, clip: Rect, color: [f32; 4]) {
    layer.rects.push(SolidRect {
        rect,
        clip,
        color,
        radius: 0.,
    });
}
/// Adds one clipped UI text run (12pt grid).
pub fn label(layer: &mut Layer, text: &str, x: f64, baseline: f64, clip: Rect, color: [f32; 4]) {
    layer.text.push(TextRun {
        text: text.replace(['\n', '\r', '\t'], " "),
        x: x as f32,
        baseline: baseline as f32,
        size: 12.,
        color,
        clip,
    });
}
/// Optional animation hook: clients request frames only until a finite deadline.
#[derive(Clone, Copy, Debug)]
pub struct Animation {
    start: Instant,
    duration: Duration,
}
impl Animation {
    /// Starts a bounded animation.
    pub fn new(duration: Duration) -> Self {
        Self {
            start: Instant::now(),
            duration,
        }
    }
    /// Progress and whether another frame is needed. Zero-duration animations complete immediately.
    pub fn tick(&self, now: Instant) -> (f32, bool) {
        let value = (now.saturating_duration_since(self.start).as_secs_f32()
            / self.duration.as_secs_f32().max(f32::EPSILON))
        .min(1.);
        (value, value < 1.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_routing_layout_hit_order_and_virtualization() {
        let mut focus = Focus::default();
        focus.set(Surface::View, 1);
        focus.show(Surface::Panel, true);
        focus.set(Surface::Panel, 2);
        focus.show(Surface::Overlay, true);
        assert_eq!(focus.route(), (Surface::Overlay, None));
        focus.set(Surface::Overlay, 3);
        assert_eq!(focus.route().1, Some(3));
        focus.show(Surface::Overlay, false);
        assert_eq!(focus.route(), (Surface::Panel, Some(2)));
        focus.show(Surface::Panel, false);
        assert_eq!(focus.route().1, Some(1));
        let mut tree = Tree::default();
        let mut root = Node::new("panel");
        root.layout = Layout::Row;
        root.children = vec![2, 3];
        tree.nodes.insert(1, root);
        let mut left = Node::new("button");
        left.extent = Some(40.);
        tree.nodes.insert(2, left);
        tree.nodes.insert(3, Node::new("input"));
        tree.roots.push(1);
        tree.layout(
            1,
            Rect {
                x: 0.,
                y: 0.,
                width: 100.,
                height: 50.,
            },
        );
        assert_eq!(tree.nodes[&3].rect.width, 60.);
        assert_eq!(tree.hit_test(Point { x: 50., y: 2. }), Some(3));
        tree.pointer(Point { x: 50., y: 2. }, Some(true));
        assert_eq!(tree.pointer(Point { x: 50., y: 2. }, Some(false)), Some(3));
        let mut scroll = Scroll {
            rows: 100_000,
            row_height: 40.,
            viewport: 400.,
            ..Scroll::default()
        };
        scroll.reveal(99_999);
        assert!(scroll.visible().len() <= 12);
        assert_eq!(scroll.visible().end, 100_000);
    }
}
