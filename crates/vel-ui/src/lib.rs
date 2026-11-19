//! Retained device-free widgets, focus routing and buffer-backed native text inputs.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod images;
pub mod input;
pub mod minihtml;
pub mod status;
pub mod theme;
pub mod workbench;
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
/// Retained widget state. Classes and attributes supply the live .sublime-theme selector path.
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
    style_attributes: Vec<&'static str>,
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
            style_attributes: Vec::new(),
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
    parents: BTreeMap<Id, Option<Id>>,
}
/// Inclusive logical point hit test.
pub fn contains(rect: Rect, point: Point) -> bool {
    point.x >= rect.x
        && point.y >= rect.y
        && point.x < rect.x + rect.width
        && point.y < rect.y + rect.height
}
impl Tree {
    /// Refreshes retained ancestry after changing roots or child lists. Stable links reuse storage.
    pub fn index_ancestry(&mut self) {
        for node in self.nodes.values_mut() {
            if !node.style_attributes.iter().eq(node.attributes.iter()) {
                node.style_attributes.clear();
                node.style_attributes
                    .extend(node.attributes.iter().copied());
            }
        }
        fn index(
            nodes: &BTreeMap<Id, Node>,
            parents: &mut BTreeMap<Id, Option<Id>>,
            id: Id,
            parent: Option<Id>,
            depth: usize,
        ) {
            if depth >= 256 {
                return;
            }
            parents.insert(id, parent);
            if let Some(node) = nodes.get(&id) {
                for child in &node.children {
                    index(nodes, parents, *child, Some(id), depth + 1);
                }
            }
        }
        self.parents.retain(|id, _| self.nodes.contains_key(id));
        for root in &self.roots {
            index(&self.nodes, &mut self.parents, *root, None, 0);
        }
    }

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
    /// A row to reveal once the viewport can show a whole row (set while it cannot).
    pub pending_reveal: Option<usize>,
}
impl Scroll {
    /// Clamps scrolling to the current content extent.
    pub fn scroll(&mut self, delta: f64) {
        if delta != 0. {
            self.pending_reveal = None;
        }
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
    /// Reveals a selected row after keyboard navigation. Before the list has been laid out
    /// (a viewport smaller than one row) only the top edge is honoured and the rest of the
    /// reveal waits for [`Scroll::set_viewport`]; revealing against a stale 1-point viewport
    /// would scroll the row under whatever sits above the list.
    pub fn reveal(&mut self, row: usize) {
        let top = row as f64 * self.row_height;
        if top < self.offset {
            self.offset = top;
        }
        if self.viewport < self.row_height {
            self.pending_reveal = Some(row);
            self.scroll(0.);
            return;
        }
        self.pending_reveal = None;
        if top + self.row_height > self.offset + self.viewport {
            self.offset = top + self.row_height - self.viewport;
        }
        self.scroll(0.);
    }
    /// Applies a laid-out viewport extent, completing a deferred reveal, and clamps.
    pub fn set_viewport(&mut self, viewport: f64) {
        self.viewport = viewport;
        match self.pending_reveal {
            Some(row) if viewport >= self.row_height => self.reveal(row),
            _ => self.scroll(0.),
        }
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
        let selected = self.items.get(self.selected).and_then(|old| {
            items.iter().position(|item| {
                item.caption == old.caption
                    && item.details == old.details
                    && item.annotation == old.annotation
                    && item.kind == old.kind
            })
        });
        self.items = items;
        self.selected = selected.unwrap_or(0);
        if selected.is_none() {
            self.scroll.offset = 0.;
        }
        self.scroll.rows = self.items.len();
        self.scroll.reveal(self.selected);
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
    /// Paints visible rows with the original default theme, retaining a thread-local resolver.
    pub fn paint(&self, layer: &mut Layer, bounds: Rect) {
        thread_local! {static THEME: std::cell::RefCell<theme::ThemeUi>=std::cell::RefCell::new(theme::ThemeUi::default());}
        THEME.with(|theme| {
            self.paint_with_theme(
                layer,
                bounds,
                &mut theme.borrow_mut(),
                &vel_config::settings::Settings::default(),
            )
        });
    }
    /// Paints through the caller's shared resolver, applying actual selected-row ancestor state.
    pub fn paint_with_theme(
        &self,
        layer: &mut Layer,
        bounds: Rect,
        theme: &mut theme::ThemeUi,
        settings: &vel_config::settings::Settings,
    ) {
        use vel_theme::{Element, ElementPath};
        for row in self.scroll.visible() {
            let item = &self.items[row];
            let rect = Rect {
                y: bounds.y + row as f64 * self.scroll.row_height - self.scroll.offset,
                height: self.scroll.row_height,
                ..bounds
            };
            let attrs = if row == self.selected {
                &["selected"][..]
            } else {
                &[]
            };
            let path = [
                Element::new("window", &[]),
                Element::new("quick_panel", &[]),
                Element::new("quick_panel_row", attrs),
            ];
            let style = theme.style(row as u64, &ElementPath(&path), settings);
            theme.surface(layer, &style, rect, bounds);
            let label_path = [
                path[0],
                path[1],
                path[2],
                Element::new("quick_panel_label", &[]),
            ];
            let caption = theme.style(0x1000 + row as u64, &ElementPath(&label_path), settings);
            theme.text_fit_highlighted(
                layer,
                &caption,
                &item.caption,
                &item.highlights,
                rect.x + 12.,
                rect.y + 18.,
                rect.width - 24.,
                minihtml::intersection(rect, bounds),
            );
            let detail_path = [
                path[0],
                path[1],
                path[2],
                Element::new("quick_panel_detail_label", &[]),
            ];
            let detail = theme.style(0x2000 + row as u64, &ElementPath(&detail_path), settings);
            theme.text(
                layer,
                &detail,
                &item.details,
                rect.x + 12.,
                rect.y + 35.,
                bounds,
            );
            let hint_path = [
                path[0],
                path[1],
                path[2],
                Element::new("quick_panel_label hint", &[]),
            ];
            let hint = theme.style(0x3000 + row as u64, &ElementPath(&hint_path), settings);
            let text = format!("{}  {}", item.kind, item.annotation);
            theme.text(
                layer,
                &hint,
                &text,
                (rect.x + rect.width - theme.advance(&hint, &text) - 12.).max(rect.x + 12.),
                rect.y + 35.,
                bounds,
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
        family: None,
        style: Default::default(),
        text: text.replace(['\n', '\r', '\t'], " "),
        x: x as f32,
        baseline: baseline as f32,
        size: 12.,
        color,
        highlight: None,
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

impl Tree {
    /// Resolves a retained widget and optional semantic child against its actual ancestor states.
    /// Ancestor/attribute scratch storage is stack-backed; cache hits allocate nothing.
    pub fn style(
        &self,
        id: Id,
        child: Option<&str>,
        theme: &mut theme::ThemeUi,
        settings: &vel_config::settings::Settings,
    ) -> theme::Style {
        self.style_child(id, child, &[], theme, settings)
    }
    /// Like [`Tree::style`], with dynamic attributes on the semantic child (for example
    /// a Git badge's status or an input's validity).
    pub fn style_child(
        &self,
        id: Id,
        child: Option<&str>,
        child_attributes: &[&str],
        theme: &mut theme::ThemeUi,
        settings: &vel_config::settings::Settings,
    ) -> theme::Style {
        let paint_id = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            child.hash(&mut h);
            child_attributes.hash(&mut h);
            id.wrapping_mul(0x9e3779b97f4a7c15) ^ h.finish()
        };
        fn find(
            tree: &Tree,
            root: Id,
            target: Id,
            ids: &mut [Id; 8],
            depth: usize,
        ) -> Option<usize> {
            if depth >= ids.len() {
                return None;
            }
            let node = tree.nodes.get(&root)?;
            ids[depth] = root;
            if root == target {
                return Some(depth + 1);
            }
            node.children
                .iter()
                .find_map(|id| find(tree, *id, target, ids, depth + 1))
        }
        let mut ids = [0; 8];
        let indexed = self.parents.get(&id).and_then(|_| {
            let mut current = Some(id);
            let mut count = 0;
            while let Some(id) = current {
                if count == ids.len() {
                    return None;
                }
                ids[count] = id;
                count += 1;
                current = *self.parents.get(&id)?;
            }
            ids[..count].reverse();
            Some(count)
        });
        // Normal chrome fits the stack buffer. Preserve complete ancestry for deeper
        // retained extension trees instead of silently styling only the leaf.
        if indexed.is_none() && self.parents.contains_key(&id) {
            let mut chain = Vec::new();
            let mut current = Some(id);
            while let Some(id) = current {
                if chain.len() >= 256 || chain.contains(&id) {
                    break;
                }
                chain.push(id);
                current = self.parents.get(&id).copied().flatten();
            }
            let mut path = vec![vel_theme::Element::new("window", &[])];
            for id in chain.iter().rev() {
                let node = &self.nodes[id];
                path.push(vel_theme::Element::new(node.class, &node.style_attributes));
            }
            if let Some(child) = child {
                path.push(vel_theme::Element::new(child, child_attributes));
            }
            return theme.style(paint_id, &vel_theme::ElementPath(&path), settings);
        }
        let n = indexed
            .or_else(|| {
                self.roots
                    .iter()
                    .find_map(|root| find(self, *root, id, &mut ids, 0))
            })
            .unwrap_or_else(|| {
                ids[0] = id;
                1
            });
        if indexed.is_some() {
            let mut path = [vel_theme::Element::new("window", &[]); 10];
            for i in 0..n {
                let node = &self.nodes[&ids[i]];
                path[i + 1] = vel_theme::Element::new(node.class, &node.style_attributes);
            }
            let count = if let Some(child) = child {
                path[n + 1] = vel_theme::Element::new(child, child_attributes);
                n + 2
            } else {
                n + 1
            };
            return theme.style(paint_id, &vel_theme::ElementPath(&path[..count]), settings);
        }
        let mut attrs = [[""; 12]; 8];
        let mut lengths = [0; 8];
        for i in 0..n {
            if let Some(node) = self.nodes.get(&ids[i]) {
                for (j, attr) in node.attributes.iter().take(12).enumerate() {
                    attrs[i][j] = attr;
                    lengths[i] += 1;
                }
            }
        }
        let mut path = [vel_theme::Element::new("window", &[]); 10];
        for i in 0..n {
            path[i + 1] = vel_theme::Element::new(
                self.nodes.get(&ids[i]).map_or("label_control", |n| n.class),
                &attrs[i][..lengths[i]],
            );
        }
        let count = if let Some(child) = child {
            path[n + 1] = vel_theme::Element::new(child, child_attributes);
            n + 2
        } else {
            n + 1
        };
        theme.style(paint_id, &vel_theme::ElementPath(&path[..count]), settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn theme_paths_preserve_deep_retained_ancestors() {
        let mut tree = Tree::default();
        for id in 1..=12 {
            let mut node = Node::new(if id == 1 {
                "panel_control"
            } else if id == 12 {
                "button_control"
            } else {
                "container"
            });
            if id < 12 {
                node.children.push(id + 1);
            }
            if id == 1 {
                node.attributes.insert("hover");
            }
            tree.nodes.insert(id, node);
        }
        tree.roots.push(1);
        tree.index_ancestry();
        let palette = vel_theme::Palette::default();
        let mut painter = theme::ThemeUi::new(std::sync::Arc::new(vel_theme::Theme::parse(r##"{"rules":[{"class":"button_control","parents":[{"class":"panel_control","attributes":["hover"]}],"fg":"#123456"}]}"##, &palette).unwrap()), palette);
        assert_eq!(
            tree.style(12, None, &mut painter, &Default::default())
                .foreground,
            [18. / 255., 52. / 255., 86. / 255., 1.]
        );
    }
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
        // Before layout (1-point viewport) a reveal never scrolls the row out of view; the
        // first real viewport completes it.
        let mut scroll = Scroll {
            rows: 50,
            row_height: 32.,
            viewport: 1.,
            ..Scroll::default()
        };
        scroll.reveal(0);
        assert_eq!(scroll.offset, 0.);
        scroll.reveal(20);
        assert_eq!(scroll.offset, 0.);
        scroll.set_viewport(320.);
        assert_eq!(scroll.offset, 21. * 32. - 320.);
        assert_eq!(scroll.pending_reveal, None);
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    #[test]
    fn quick_panel_refresh_retains_selection_or_falls_back_to_first() {
        let item = |caption: &str, value| Item {
            caption: caption.into(),
            value,
            ..Default::default()
        };
        let mut panel = QuickPanel::default();
        panel.set_items(vec![item("one", 1), item("two", 2), item("three", 3)]);
        panel.navigate(1);
        assert_eq!(
            panel.set_items(vec![item("three", 30), item("two", 20), item("one", 10)]),
            Some(20)
        );
        assert_eq!(panel.selected, 1);
        assert_eq!(panel.set_items(vec![item("three", 30)]), Some(30));
        assert_eq!(panel.selected, 0);
        assert_eq!(panel.set_items(vec![]), None);
    }
}
