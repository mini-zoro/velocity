//! Accessibility semantics for the retained panel widgets. Shares painted hit rectangles.
use super::*;
use vel_platform::{
    Utf16Range,
    accessibility::{Node as AxNode, Role, TextArea, Tree as AxTree},
};
const PANEL_BASE: u64 = 1 << 48;
pub(crate) fn ax_id(id: u64) -> u64 {
    PANEL_BASE + id
}
impl Panels {
    pub fn accessibility(&self, workspace: &Workspace) -> AxTree {
        let mut out = AxTree::default();
        // Walk actual ancestry, in child order. Hidden rows have no accessible object.
        fn visit(
            p: &Panels,
            workspace: &Workspace,
            id: u64,
            parent: Option<u64>,
            out: &mut AxTree,
        ) {
            let Some(widget) = p.tree.nodes.get(&id).filter(|n| n.visible) else {
                return;
            };
            let mut text = None;
            let (role, label) = match widget.class {
                "text_area" => return,
                "quick_panel" => (Role::Group, "Quick panel".into()),
                "scroll_area" if id == 11 => (Role::List, "Results".into()),
                "sidebar_container" | "side_bar" => {
                    (Role::Outline, "Project and open files".into())
                }
                "status_bar" => {
                    let e = workspace.editor();
                    let point = e.text.byte_to_point(e.caret.min(e.text.len_bytes()));
                    (
                        Role::Status,
                        format!(
                            "Line {}, column {}; {} selections; {}. {}",
                            point.row + 1,
                            point.column + 1,
                            e.regions().len(),
                            workspace.tabs[workspace.active].format.encoding,
                            p.message
                        ),
                    )
                }
                "text_input" => {
                    let (input, label) = match id {
                        10 => (&p.quick_input, "Filter choices"),
                        21 => (&p.replace, "Replace"),
                        22 => (&p.where_input, "Where"),
                        _ => (
                            &p.query,
                            if p.panel == Some(Panel::Input) {
                                p.input_caption.as_str()
                            } else {
                                "Find"
                            },
                        ),
                    };
                    let state = input.state();
                    text = Some(TextArea::new(
                        &state,
                        Utf16Range::new(0, state.text.len_utf16()),
                    ));
                    (Role::TextField, label.into())
                }
                "quick_panel_row" => {
                    let label = p
                        .quick
                        .items
                        .iter()
                        .find(|item| (1 << 40) + item.value as u64 * 5 == id)
                        .map(|i| format!("{} {} {} {}", i.caption, i.kind, i.annotation, i.details))
                        .unwrap_or_default();
                    (Role::Row, label)
                }
                "tree_row" => {
                    let index = id.saturating_sub(3 << 40) as usize;
                    let Some(row) = p.side_rows.get(index) else {
                        return;
                    };
                    (
                        Role::Row,
                        row.path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    )
                }
                "open_file" => {
                    let Some(tab) = workspace.tabs.get(id.saturating_sub(2 << 40) as usize) else {
                        return;
                    };
                    (Role::Row, tab.label())
                }
                "button" => (
                    Role::Button,
                    if (30..36).contains(&id) {
                        [
                            "Regular expression",
                            "Case sensitive",
                            "Whole word",
                            "Wrap search",
                            "In selection",
                            "Preserve case",
                        ][(id - 30) as usize]
                            .into()
                    } else {
                        widget.label.clone()
                    },
                ),
                class if class.starts_with("panel_control") => (
                    Role::Group,
                    if p.panel == Some(Panel::Input) {
                        p.input_caption.clone()
                    } else {
                        "Find and output panel".into()
                    },
                ),
                _ => {
                    for child in &widget.children {
                        visit(p, workspace, *child, parent, out);
                    }
                    return;
                }
            };
            let mut node = AxNode::new(ax_id(id), parent, role, label, widget.rect);
            node.selected = widget.attributes.contains("selected");
            node.expanded = widget
                .attributes
                .contains("expandable")
                .then(|| widget.attributes.contains("expanded"));
            if widget.class == "tree_row" {
                node.level = p
                    .side_rows
                    .get(id.saturating_sub(3 << 40) as usize)
                    .map_or(0, |r| r.depth);
            }
            node.text = text;
            node.editable = node.text.is_some();
            node.focused = match id {
                10 => p.overlay.is_some(),
                20..=22 => {
                    p.overlay.is_none() && p.panel_focused && p.focus_input as u64 == id - 20
                }
                _ => p.side_focused && node.selected,
            };
            if matches!(role, Role::Row | Role::Button | Role::TextField) {
                node.activation = Some((
                    Point {
                        x: widget.rect.x + widget.rect.width / 2.,
                        y: widget.rect.y + widget.rect.height / 2.,
                    },
                    role == Role::Row,
                ));
            }
            let parent = Some(node.id);
            out.nodes.push(node);
            for child in &widget.children {
                visit(p, workspace, *child, parent, out);
            }
        }
        for root in &self.tree.roots {
            visit(self, workspace, *root, None, &mut out);
        }
        // Syntax status popup has the exact hit area used by mouse dispatch (as painted).
        if self.chrome
            && self.overlay.is_none()
            && let Some(frame) = self.status_item(super::details::StatusItem::Syntax)
        {
            let mut node = AxNode::new(
                ax_id(99),
                None,
                Role::PopupButton,
                format!(
                    "Syntax: {}",
                    workspace
                        .editor()
                        .language
                        .borrow()
                        .as_ref()
                        .map_or("Plain Text", |l| l.name.as_str())
                ),
                frame,
            );
            node.activation = Some((
                Point {
                    x: frame.x + frame.width / 2.,
                    y: frame.y + frame.height / 2.,
                },
                false,
            ));
            out.nodes.push(node);
        }
        if let Some(popup) = self.hover_html.as_ref().filter(|p| p.visible) {
            out.nodes.extend(
                popup
                    .html
                    .accessibility_nodes(ax_id(2000), Point::default()),
            );
        }
        if !self.review.native_context && !self.review.items.is_empty() {
            let bounds = self.review.bounds;
            out.nodes.push(AxNode::new(
                ax_id(1000),
                None,
                Role::List,
                "Context menu",
                bounds,
            ));
            for (i, item) in self.review.items.iter().enumerate() {
                let frame = Rect {
                    x: bounds.x + 4.,
                    y: bounds.y + 4. + i as f64 * 26.,
                    width: bounds.width - 8.,
                    height: 26.,
                };
                let mut node = AxNode::new(
                    ax_id(1001 + i as u64),
                    Some(ax_id(1000)),
                    Role::Row,
                    item.caption.clone(),
                    frame,
                );
                node.selected = i == self.review.selected;
                node.focused = node.selected;
                node.activation = Some((
                    Point {
                        x: frame.x + frame.width / 2.,
                        y: frame.y + 13.,
                    },
                    false,
                ));
                out.nodes.push(node);
            }
            for node in &mut out.nodes {
                if node.parent != Some(ax_id(1000)) {
                    node.focused = false;
                }
            }
        }
        out
    }
    pub fn accessibility_focus(&mut self, id: u64) {
        let id = id.saturating_sub(PANEL_BASE);
        self.side_focused = false;
        if (20..=22).contains(&id) {
            self.panel_focused = true;
            self.focus_input = (id - 20) as usize;
            self.focus.show(Surface::Panel, true);
        }
    }
    pub fn accessibility_input(&self, id: u64) -> Option<&Input> {
        match id.checked_sub(PANEL_BASE)? {
            10 => Some(&self.quick_input),
            20 => Some(&self.query),
            21 => Some(&self.replace),
            22 => Some(&self.where_input),
            _ => None,
        }
    }
    pub fn accessibility_input_mut(&mut self, id: u64) -> Option<&mut Input> {
        match id.checked_sub(PANEL_BASE)? {
            10 => Some(&mut self.quick_input),
            20 => Some(&mut self.query),
            21 => Some(&mut self.replace),
            22 => Some(&mut self.where_input),
            _ => None,
        }
    }
    pub fn accessibility_editor_focus(&mut self) {
        self.panel_focused = false;
        self.side_focused = false;
        self.focus.show(Surface::Panel, false);
    }
}
