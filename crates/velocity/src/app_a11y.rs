//! The native adapter queries this model only when an accessibility client requests it.
use super::*;
use vel_platform::accessibility::{Id, Node, Preferences, Request, Role, TextArea, Tree};
const EDITOR: Id = 100;
const TABS: Id = 101;
const COMPLETIONS: Id = 1 << 46;
impl App {
    pub(super) fn accessible_tree(&self) -> Tree {
        let mut tree = Tree::default();
        if self.panels.overlay.is_none() {
            if !self.tab_hits.is_empty() {
                tree.nodes.push(Node::new(
                    TABS,
                    None,
                    Role::TabGroup,
                    "Documents",
                    self.workbench_bounds,
                ));
                for (sheet, frame) in &self.tab_hits {
                    if let Some(tab) = self.workspace.tabs.iter().find(|t| t.sheet == *sheet) {
                        let mut node =
                            Node::new(1000 + sheet * 2, Some(TABS), Role::Tab, tab.label(), *frame);
                        node.selected = self
                            .workspace
                            .workbench
                            .groups
                            .iter()
                            .any(|g| g.selected.contains(sheet));
                        node.activation = Some((
                            Point {
                                x: frame.x + frame.width / 2.,
                                y: frame.y + frame.height / 2.,
                            },
                            false,
                        ));
                        tree.nodes.push(node);
                    }
                }
                for (sheet, frame) in &self.tab_close_hits {
                    if let Some(tab) = self.workspace.tabs.iter().find(|t| t.sheet == *sheet) {
                        let mut node = Node::new(
                            1001 + sheet * 2,
                            Some(TABS),
                            Role::Button,
                            format!("Close {}", tab.label()),
                            *frame,
                        );
                        node.activation = Some((
                            Point {
                                x: frame.x + frame.width / 2.,
                                y: frame.y + frame.height / 2.,
                            },
                            false,
                        ));
                        tree.nodes.push(node);
                    }
                }
            }
            let e = self.workspace.editor();
            let state = TextInputState {
                text: e.text.clone(),
                selected: e.selected_utf16(),
                marked: e.marked_utf16(),
                caret: self.view.rect_at(e, e.caret),
            };
            let mut model = TextArea::new(
                &state,
                Utf16Range::from_bytes(&e.text, self.view.viewport(e)),
            );
            model.insertion_line = e.text.byte_to_line(e.caret.min(e.text.len_bytes()));
            let mut node = Node::new(
                EDITOR,
                None,
                Role::TextArea,
                format!(
                    "Editor, {}",
                    self.workspace.tabs[self.workspace.active].label()
                ),
                self.content,
            );
            node.focused = self.panels.input().is_none() && !self.panels.side_focused;
            node.editable = !e.read_only
                && model.document_offset == 0
                && model.text.len_utf16() == e.text.len_utf16();
            node.help = format!(
                "Primary selection of {}. Cmd+F6 moves focus; Cmd+Shift+F6 moves backward. {}",
                e.regions().len(),
                if model.document_offset > 0 || model.text.len_utf16() < e.text.len_utf16() {
                    format!(
                        "Windowed document text; UTF-16 document offset {}, first document line {}. Move the caret to navigate beyond this window.",
                        model.document_offset,
                        model.first_line + 1
                    )
                } else {
                    String::new()
                }
            );
            if e.binary.is_some() {
                node.label = "Read-only hex document".into();
                node.help = "Hex rendering; text selection is unavailable".into();
                node.text = None;
            } else {
                node.text = Some(model);
            }
            tree.nodes.push(node);
        }
        let panels = self.panels.accessibility(&self.workspace);
        tree.nodes.extend(panels.nodes.into_iter().filter(|n| {
            self.panels.overlay.is_none()
                || n.id == crate::panels::ax_id(1)
                || n.parent == Some(crate::panels::ax_id(1))
                || n.parent == Some(crate::panels::ax_id(11))
        }));
        // Completion choices use the same visible rows and paint geometry.
        let completion = &self.workspace.editor().completion;
        if self.panels.overlay.is_none()
            && let Some(mut bounds) = completion.popup_rect()
        {
            bounds.x += self.content.x;
            bounds.y += self.content.y;
            tree.nodes.push(Node::new(
                COMPLETIONS,
                None,
                Role::List,
                "Completions",
                bounds,
            ));
            if let Some(list) = &completion.list {
                let count = (bounds.height / 24.).floor().max(1.) as usize;
                let first = completion.selected.saturating_sub(count.saturating_sub(1));
                for (row, item) in list.items.iter().skip(first).take(count).enumerate() {
                    let frame = Rect {
                        y: bounds.y + row as f64 * 24.,
                        height: 24.,
                        ..bounds
                    };
                    let mut node = Node::new(
                        COMPLETIONS + 1 + (first + row) as u64,
                        Some(COMPLETIONS),
                        Role::Row,
                        format!("{} {}", item.trigger, item.annotation),
                        frame,
                    );
                    node.selected = first + row == completion.selected;
                    node.activation = Some((
                        Point {
                            x: frame.x + frame.width / 2.,
                            y: frame.y + 12.,
                        },
                        false,
                    ));
                    tree.nodes.push(node);
                }
            }
        }
        if self.panels.overlay.is_none()
            && let Some(popup) = self
                .workspace
                .editor()
                .html
                .borrow()
                .popup
                .as_ref()
                .filter(|p| p.visible)
        {
            tree.nodes.extend(popup.html.accessibility_nodes(
                1 << 47,
                Point {
                    x: self.content.x,
                    y: self.content.y,
                },
            ));
        }
        if let Some(focus) = self.accessibility_focus
            && tree.node(focus).is_some()
        {
            for node in &mut tree.nodes {
                node.focused = node.id == focus;
            }
        }
        tree
    }
    pub(super) fn accessibility_action(&mut self, w: &WindowHandle, id: Id, request: Request) {
        let Some(node) = self.accessible_tree().node(id).cloned() else {
            return;
        };
        match request {
            Request::Press => {
                if let Some((point, double)) = node.activation {
                    for kind in [MouseKind::Down, MouseKind::Up] {
                        self.on_mouse(
                            w,
                            MouseEvent {
                                button: 0,
                                kind,
                                position: point,
                                click_count: if double { 2 } else { 1 },
                                modifiers: Modifiers::default(),
                                timestamp: 0.,
                            },
                        );
                    }
                }
            }
            Request::Focus => {
                self.accessibility_focus = Some(id);
                if id == EDITOR {
                    self.panels.accessibility_editor_focus();
                } else {
                    self.panels.accessibility_focus(id);
                }
                w.focus();
            }
            Request::Select(range) => {
                let Some(model) = node.text else {
                    return;
                };
                let range = model.document_range(range);
                self.accessibility_action(w, id, Request::Focus);
                if id == EDITOR {
                    let bytes = range.bytes(&self.workspace.editor().text);
                    self.workspace
                        .editor_mut()
                        .set_selection(vel_buffer::Selection::from_regions([
                            vel_buffer::Region::new(bytes.start, bytes.end),
                        ]));
                    self.changed(w, Instant::now(), true);
                } else if let Some(input) = self.panels.accessibility_input_mut(id) {
                    let bytes = range.bytes(input.buffer.text());
                    input.buffer.set_selection(
                        input.view,
                        vel_buffer::Selection::from_regions([vel_buffer::Region::new(
                            bytes.start,
                            bytes.end,
                        )]),
                    );
                    input.reveal();
                }
            }
            Request::SetValue(value) => {
                if !node.editable {
                    return;
                }
                let Some(model) = node.text else {
                    return;
                };
                let replacement =
                    Some(model.document_range(Utf16Range::new(0, model.text.len_utf16())));
                self.accessibility_action(w, id, Request::Focus);
                if id == EDITOR {
                    self.input(
                        w,
                        TextInput::Insert {
                            text: value,
                            replacement,
                        },
                        Instant::now(),
                    );
                } else {
                    if let Some(input) = self.panels.accessibility_input_mut(id) {
                        input.text_input(TextInput::Insert {
                            text: value,
                            replacement,
                        });
                    }
                    let viewport = self.view.viewport(self.workspace.editor());
                    self.panels.edited(&mut self.workspace, viewport);
                }
            }
        }
        w.invalidate_character_coordinates();
        w.request_frame();
    }
    pub(super) fn accessible_range_rect(&self, id: Id, range: Utf16Range) -> Rect {
        let Some(model) = self.accessible_tree().node(id).and_then(|n| n.text.clone()) else {
            return Rect::default();
        };
        let document = model.document_range(range);
        if let Some(input) = self.panels.accessibility_input(id) {
            return input.range_rect(document).0;
        }
        let e = self.workspace.editor();
        let bytes = document.bytes(&e.text);
        let mut rect = self.view.rect_at(e, bytes.start);
        if !bytes.is_empty() {
            let last_byte = e
                .text
                .char_to_byte(e.text.byte_to_char(bytes.end).saturating_sub(1));
            let first = e.layout.borrow().visual_at(&e.text, bytes.start).0;
            let last = e.layout.borrow().visual_at(&e.text, last_byte).0;
            let mut bounds: Option<Rect> = None;
            let mut previous = None;
            // The exposed value is bounded. Skip duplicate annotation rows, and
            // union actual text fragments rather than the whole viewport/gutter.
            for visual in first..=last.min(first.saturating_add(accessibility::TEXT_WINDOW)) {
                let (row, index) = e.layout.borrow().locate(visual);
                if previous == Some((row, index)) {
                    continue;
                }
                previous = Some((row, index));
                let segment = e.layout.borrow().segment(&e.text, row, index);
                let line = e.text.line_to_byte(row);
                let start = bytes.start.max(line + segment.bytes.start);
                let end = bytes.end.min(line + segment.bytes.end);
                if start > end {
                    continue;
                }
                let part = self
                    .view
                    .range_rect(e, Utf16Range::from_bytes(&e.text, start..end))
                    .0;
                bounds = Some(bounds.map_or(part, |old| {
                    let x = old.x.min(part.x);
                    let y = old.y.min(part.y);
                    Rect {
                        x,
                        y,
                        width: (old.x + old.width).max(part.x + part.width) - x,
                        height: (old.y + old.height).max(part.y + part.height) - y,
                    }
                }));
            }
            if let Some(bounds) = bounds {
                rect = bounds;
            }
        }
        rect.x += self.content.x;
        rect.y += self.content.y;
        rect
    }
    pub(super) fn accessibility_preferences(&mut self, preferences: Preferences) {
        self.accessibility_preferences = preferences;
        self.workspace
            .themes
            .ui
            .borrow_mut()
            .set_accessibility_preferences(preferences);
    }
    pub(super) fn next_accessibility_focus(&mut self, w: &WindowHandle, backwards: bool) {
        let tree = self.accessible_tree();
        if let Some(id) = tree.next_focus(
            self.accessibility_focus.or_else(|| tree.focused()),
            backwards,
        ) {
            self.accessibility_action(w, id, Request::Focus);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(text: &str) -> App {
        App::new(
            Options::default(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            Reporter::for_options(&Options::default(), Instant::now()).unwrap(),
            Workspace::new(vel_text::Text::from(text), None, None, false).unwrap(),
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
        )
    }
    #[test]
    fn editor_primary_selection_bounds_and_tabs_use_the_actual_layout() {
        let mut a = app("a😀é\n你好\nlast line");
        a.workspace
            .editor_mut()
            .set_selection(vel_buffer::Selection::from_regions([
                vel_buffer::Region::new(1, 5),
                vel_buffer::Region::caret(8),
            ]));
        a.scene();
        let tree = a.accessible_tree();
        let node = tree.node(EDITOR).unwrap();
        let text = node.text.as_ref().unwrap();
        assert_eq!(text.selected, a.workspace.editor().selected_utf16());
        assert!(node.help.contains("2"));
        assert!(tree.nodes.iter().any(|n| n.role == Role::Tab
            && n.label.contains("Untitled")
            && n.activation.is_some()));
        let range = Utf16Range::new(1, 2);
        let mut expected = a.view.range_rect(a.workspace.editor(), range).0;
        expected.x += a.content.x;
        expected.y += a.content.y;
        assert_eq!(a.accessible_range_rect(EDITOR, range), expected);
        let multiple = a.accessible_range_rect(EDITOR, Utf16Range::new(0, 10));
        assert!(multiple.height > expected.height);
        assert!(
            multiple.width < a.content.width / 2.,
            "bounds must hug the selected text, not the viewport"
        );
        assert!(tree.next_focus(tree.focused(), false).is_some());
    }
    #[test]
    fn modal_panel_rows_fields_buttons_status_and_syntax_have_roles_and_labels() {
        let mut a = app("hello");
        a.scene();
        let tree = a.accessible_tree();
        assert!(
            tree.nodes
                .iter()
                .any(|n| n.role == Role::Status && n.label.contains("Line 1"))
        );
        assert!(
            tree.nodes
                .iter()
                .any(|n| n.role == Role::PopupButton && n.label.contains("Syntax"))
        );
        a.panels
            .action(
                &mut a.workspace,
                "show_panel",
                &json!({"panel":"replace","text":"hello"}),
                0..10,
            )
            .unwrap();
        a.scene();
        let tree = a.accessible_tree();
        assert!(
            tree.nodes
                .iter()
                .any(|n| n.role == Role::TextField && n.label == "Replace")
        );
        assert!(
            tree.nodes
                .iter()
                .any(|n| n.role == Role::Button && n.label == "Replace All")
        );
        a.panels.choices(
            &mut a.workspace,
            vec![vel_config::ui::PaletteItem {
                caption: "Package Fixture — no plugin API".into(),
                invocation: vel_config::keymap::Invocation {
                    command: "package_list".into(),
                    args: json!({}),
                },
            }],
        );
        let deadline = Instant::now() + Duration::from_secs(120);
        while a.panels.ranking_pending() {
            a.panels.poll(&mut a.workspace, 0..10);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        a.scene();
        let tree = a.accessible_tree();
        assert!(tree.node(EDITOR).is_none());
        assert!(
            tree.nodes
                .iter()
                .any(|n| n.role == Role::Row && n.label.contains("no plugin API"))
        );
        assert!(
            tree.nodes
                .iter()
                .any(|n| n.focused && n.role == Role::TextField)
        );
    }
}
