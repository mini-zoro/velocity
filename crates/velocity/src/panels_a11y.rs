//! Accessibility semantics for the retained panel widgets. Shares painted hit rectangles.
use super::*;
use std::fmt::Write as _;
use vel_platform::{
    TextInputState, Utf16Range,
    accessibility::{Node as AxNode, Region, Role, TextArea, Tree as AxTree},
};
use vel_text::Text;
const PANEL_BASE: u64 = 1 << 48;
pub(crate) fn ax_id(id: u64) -> u64 {
    PANEL_BASE + id
}
impl Panels {
    pub fn accessibility(&self, workspace: &Workspace, include_text: bool) -> AxTree {
        let mut out = AxTree::default();
        // Walk actual ancestry, in child order. Hidden rows have no accessible object.
        fn visit(
            p: &Panels,
            workspace: &Workspace,
            id: u64,
            parent: Option<u64>,
            out: &mut AxTree,
            include_text: bool,
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
                        10 => (
                            &p.quick_input,
                            match p.overlay {
                                Some(Overlay::Palette) => "Search commands",
                                Some(Overlay::Goto) => "Go to file, symbol, or line",
                                Some(Overlay::Syntax) => "Filter syntaxes",
                                Some(Overlay::Appearance) => "Filter appearances",
                                Some(Overlay::ProjectSymbols) => "Search project symbols",
                                Some(Overlay::Locations) => "Filter locations",
                                Some(Overlay::Choices) | None => "Filter choices",
                            },
                        ),
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
                    if include_text {
                        let state = input.state();
                        text = Some(TextArea::new(
                            &state,
                            Utf16Range::new(0, state.text.len_utf16()),
                        ));
                    }
                    (Role::TextField, label.into())
                }
                "quick_panel_row" => {
                    let label = p
                        .quick
                        .items
                        .iter()
                        .find(|item| (1 << 40) + item.value as u64 * 5 == id)
                        .map(|i| {
                            [
                                i.caption.as_str(),
                                i.annotation.as_str(),
                                i.details.as_str(),
                            ]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(", ")
                        })
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
                    (
                        Role::Row,
                        if tab.dirty() {
                            format!("{}, edited", tab.name())
                        } else {
                            tab.name()
                        },
                    )
                }
                "button" => (
                    if (30..36).contains(&id) {
                        Role::CheckBox
                    } else {
                        Role::Button
                    },
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
                        visit(p, workspace, *child, parent, out, include_text);
                    }
                    return;
                }
            };
            let mut node = AxNode::new(ax_id(id), parent, role, label, widget.rect);
            node.selected = widget.attributes.contains("selected");
            node.region = Some(match widget.class {
                "sidebar_container" | "side_bar" | "tree_row" | "open_file" => Region::Sidebar,
                "status_bar" => Region::Status,
                _ => Region::Panel,
            });
            if role == Role::CheckBox {
                node.checked = Some(
                    [
                        p.options.regex,
                        p.options.case_sensitive,
                        p.options.whole_word,
                        p.options.wrap,
                        p.in_selection,
                        p.options.preserve_case,
                    ][(id - 30) as usize],
                );
            }
            if widget.class == "quick_panel_row"
                && node.selected
                && let Some(index) = p
                    .quick
                    .items
                    .iter()
                    .position(|item| (1 << 40) + item.value as u64 * 5 == id)
            {
                node.announcement = Some(format!(
                    "{}, {} of {}",
                    node.label,
                    index + 1,
                    p.quick.items.len()
                ));
            }
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
            if matches!(
                role,
                Role::Row | Role::Button | Role::CheckBox | Role::TextField
            ) {
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
                visit(p, workspace, *child, parent, out, include_text);
            }
        }
        for root in &self.tree.roots {
            visit(self, workspace, *root, None, &mut out, include_text);
        }
        if self.overlay.is_none() && matches!(self.panel, Some(Panel::Output | Panel::Results)) {
            let results = self.panel == Some(Panel::Results);
            let title = if results {
                "Find Results"
            } else if self.output.starts_with("Indexing Status\n") {
                "Indexing Status"
            } else {
                "Output"
            };
            let mut node = AxNode::new(
                ax_id(89),
                Some(ax_id(2)),
                Role::TextArea,
                title,
                self.panel_bounds,
            );
            node.region = Some(Region::Panel);
            node.focused = self.panel_focused;
            if include_text {
                let (text, caret) = if results {
                    (self.accessible_results(), 0)
                } else {
                    let text = if let Some(editor) = &self.output_editor {
                        editor.text.clone()
                    } else {
                        let tail = self
                            .output
                            .len()
                            .saturating_sub(4 * vel_platform::accessibility::TEXT_WINDOW);
                        Text::from(&self.output[self.output.ceil_char_boundary(tail)..])
                    };
                    let caret = text.len_utf16();
                    (text, caret)
                };
                let state = TextInputState {
                    text,
                    selected: Utf16Range::new(caret, 0),
                    ..Default::default()
                };
                node.text = Some(TextArea::new(&state, Utf16Range::default()));
            }
            out.nodes.push(node);
        }
        if !self.message.is_empty() {
            let mut node = AxNode::new(
                ax_id(90),
                None,
                Role::Status,
                self.message.clone(),
                Rect::default(),
            );
            node.announcement = Some(self.message.clone());
            out.nodes.push(node);
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
            node.region = Some(Region::Status);
            out.nodes.push(node);
            let frame = self.indentation_rect();
            let editor = workspace.editor().settings.editor();
            let mut indent = AxNode::new(
                ax_id(98),
                None,
                Role::PopupButton,
                format!(
                    "Indentation: {}",
                    vel_ui::status::indentation(editor.translate_tabs_to_spaces, editor.tab_size)
                ),
                frame,
            );
            indent.activation = Some((
                Point {
                    x: frame.x + frame.width / 2.,
                    y: frame.y + frame.height / 2.,
                },
                false,
            ));
            indent.region = Some(Region::Status);
            out.nodes.push(indent);
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
                node.region = Some(Region::Panel);
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
    fn accessible_results(&self) -> Text {
        const ROWS: usize = 48;
        let rows = &self.result_view.rows;
        let first = self
            .result_selected
            .saturating_sub(ROWS / 2)
            .min(rows.len().saturating_sub(ROWS));
        let mut output = String::new();
        for row in rows.iter().skip(first).take(ROWS) {
            match row {
                files::Row::Header(path) => {
                    let _ = writeln!(output, "\n{}:", path.display());
                }
                files::Row::Gap => output.push_str("        …\n"),
                files::Row::Line(line) => {
                    let text =
                        &line.text[..line.text.floor_char_boundary(line.text.len().min(1024))];
                    let marker = if line.highlights.is_empty() { ' ' } else { ':' };
                    let _ = writeln!(output, "  {:>5}{marker} {text}", line.number);
                }
            }
        }
        Text::from(output.as_str())
    }
    pub(crate) fn accessibility_rect(&self, id: u64) -> Option<Rect> {
        let id = id.checked_sub(PANEL_BASE)?;
        match id {
            89 => (self.overlay.is_none()
                && matches!(self.panel, Some(Panel::Output | Panel::Results)))
            .then_some(self.panel_bounds),
            98 | 99 => {
                if !self.chrome || self.overlay.is_some() {
                    return None;
                }
                let syntax = self.status_item(super::details::StatusItem::Syntax)?;
                Some(if id == 99 {
                    syntax
                } else {
                    self.indentation_rect()
                })
            }
            1001..2000
                if !self.review.native_context && (id - 1001) < self.review.items.len() as u64 =>
            {
                let bounds = self.review.bounds;
                Some(Rect {
                    x: bounds.x + 4.,
                    y: bounds.y + 4. + (id - 1001) as f64 * 26.,
                    width: bounds.width - 8.,
                    height: 26.,
                })
            }
            2000..=4095 if self.overlay.is_none() => self
                .hover_html
                .as_ref()
                .filter(|p| p.visible)?
                .html
                .accessibility_nodes(ax_id(2000), Point::default())
                .into_iter()
                .find(|node| node.id == ax_id(id))
                .map(|node| node.frame),
            _ => self
                .tree
                .nodes
                .get(&id)
                .filter(|widget| {
                    widget.visible
                        && match id {
                            1 | 10 | 11 => self.overlay.is_some(),
                            2 | 20..=84 => self.overlay.is_none() && self.panel.is_some(),
                            _ if id >> 40 == 1 => self.overlay.is_some(),
                            _ if id >> 40 == 2 || id >> 40 == 3 => {
                                self.overlay.is_none() && self.side_visible
                            }
                            _ => true,
                        }
                })
                .map(|widget| widget.rect),
        }
    }
    pub fn accessibility_focus(&mut self, id: u64) {
        let id = id.saturating_sub(PANEL_BASE);
        self.side_focused = false;
        self.panel_focused = id == 89;
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
