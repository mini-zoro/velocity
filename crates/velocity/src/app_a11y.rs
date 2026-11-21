//! The native adapter queries this model only when an accessibility client requests it.
use super::*;
use vel_platform::accessibility::{Id, Node, Preferences, Region, Request, Role, TextArea, Tree};
const EDITOR: Id = 100;
const TABS: Id = 101;
const COMPLETIONS: Id = 1 << 46;
const OTHER_EDITOR: Id = 1 << 45;

fn tab_label(workspace: &crate::workspace::Workspace, tab: &crate::workspace::Tab) -> String {
    let name = tab.ui_name(&workspace.ui);
    if tab.dirty() {
        workspace
            .translations
            .tr_fmt("{name}, edited", &[("name", name)])
    } else {
        name
    }
}
impl App {
    pub(super) fn accessible_tree(&self) -> Tree {
        self.accessibility_model(true)
    }
    fn accessibility_model(&self, include_text: bool) -> Tree {
        if self.settings_page_has_focus() {
            return self
                .workspace
                .settings_page
                .as_ref()
                .unwrap()
                .accessibility(&self.workspace.translations, include_text);
        }
        let mut tree = Tree::default();
        let ws = &self.workspace;
        let tr = &ws.translations;
        if self.panels.overlay.is_none() {
            if !self.tab_hits.is_empty() {
                tree.nodes.push(Node::new(
                    TABS,
                    None,
                    Role::TabGroup,
                    tr.tr("Documents"),
                    self.workbench_bounds,
                ));
                for (sheet, frame) in &self.tab_hits {
                    if let Some(tab) = self.workspace.tabs.iter().find(|t| t.sheet == *sheet) {
                        let mut node = Node::new(
                            1000 + sheet * 2,
                            Some(TABS),
                            Role::Tab,
                            tab_label(ws, tab),
                            *frame,
                        );
                        node.region = Some(Region::Tabs);
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
                            tr.tr_fmt("Close {name}", &[("name", tab_label(ws, tab))]),
                            *frame,
                        );
                        node.region = Some(Region::Tabs);
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
            let active = &self.workspace.tabs[self.workspace.active];
            let mut node = Node::new(
                EDITOR,
                None,
                Role::TextArea,
                tr.tr_fmt("Editor, {name}", &[("name", tab_label(ws, active))]),
                self.content,
            );
            node.region = Some(Region::Editor);
            node.focused = self.panels.input().is_none() && !self.panels.side_focused;
            if include_text && e.binary.is_none() {
                let state = TextInputState {
                    text: e.text.clone(),
                    selected: e.selected_utf16(),
                    marked: e.marked_utf16(),
                    caret: self.view.rect_at(e, e.caret),
                };
                let caret = e.caret.min(e.text.len_bytes());
                let previous = self
                    .accessibility_windows
                    .borrow()
                    .get(&active.sheet)
                    .copied();
                let model = TextArea::windowed(
                    &state,
                    Utf16Range::from_bytes(&e.text, self.view.viewport(e)),
                    Utf16Range::from_bytes(&e.text, caret..caret).location,
                    previous,
                );
                if previous != Some(model.document_offset) {
                    let mut windows = self.accessibility_windows.borrow_mut();
                    // Only keep anchors for open sheets.
                    windows.retain(|id, _| self.workspace.tabs.iter().any(|tab| tab.sheet == *id));
                    windows.insert(active.sheet, model.document_offset);
                }
                node.editable = !e.read_only
                    && model.document_offset == 0
                    && model.text.len_utf16() == e.text.len_utf16();
                node.help = tr.tr_fmt(
                    "Primary selection of {count}. Cmd+F6 moves focus; Cmd+Shift+F6 moves backward.",
                    &[("count", e.regions().len().to_string())],
                );
                if model.document_offset > 0 || model.text.len_utf16() < e.text.len_utf16() {
                    node.help.push(' ');
                    node.help.push_str(&tr.tr_fmt(
                        "Windowed document text; UTF-16 document offset {offset}, first document line {line}. Move the caret to navigate beyond this window.",
                        &[("offset", model.document_offset.to_string()), ("line", (model.first_line + 1).to_string())],
                    ));
                }
                node.text = Some(model);
            } else if e.binary.is_some() {
                node.label = tr.tr("Read-only hex document").into_owned();
                node.help = tr
                    .tr("Hex rendering; text selection is unavailable")
                    .into_owned();
            }
            tree.nodes.push(node);
            for (&sheet, &frame) in &self.sheet_bounds {
                if sheet == active.sheet {
                    continue;
                }
                let Some(tab) = self.workspace.tabs.iter().find(|t| t.sheet == sheet) else {
                    continue;
                };
                if tab.lazy.is_some() || tab.editor.binary.is_some() {
                    continue;
                }
                let editor = &tab.editor;
                let mut node = Node::new(
                    OTHER_EDITOR + sheet,
                    None,
                    Role::TextArea,
                    tr.tr_fmt("Editor, {name}", &[("name", tab_label(ws, tab))]),
                    frame,
                );
                node.region = Some(Region::Editor);
                if include_text {
                    let visible = self
                        .sheet_views
                        .get(&sheet)
                        .map_or_else(Utf16Range::default, |view| {
                            Utf16Range::from_bytes(&editor.text, view.viewport(editor))
                        });
                    node.text = Some(TextArea::new(
                        &TextInputState {
                            text: editor.text.clone(),
                            selected: editor.selected_utf16(),
                            ..Default::default()
                        },
                        visible,
                    ));
                }
                tree.nodes.push(node);
            }
        }
        let panels = self.panels.accessibility(&self.workspace, include_text);
        tree.nodes.extend(panels.nodes.into_iter().filter(|n| {
            self.panels.overlay.is_none()
                || n.id == crate::panels::ax_id(1)
                || n.parent == Some(crate::panels::ax_id(1))
                || n.parent == Some(crate::panels::ax_id(11))
                || n.id == crate::panels::ax_id(90)
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
                tr.tr("Completions"),
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
                    node.region = Some(Region::Panel);
                    if node.selected {
                        node.announcement = Some(tr.tr_fmt(
                            "{name}, {index} of {count}",
                            &[
                                ("name", node.label.clone()),
                                ("index", (first + row + 1).to_string()),
                                ("count", list.items.len().to_string()),
                            ],
                        ));
                    }
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
        if self.settings_page_has_focus() {
            if !node.enabled {
                return;
            }
            if matches!(request, Request::Press) {
                self.press_accessible(Some(w), &node);
                return;
            }
            let Some((point, _)) = node.activation else {
                return;
            };
            let mut page = self.workspace.settings_page.take().unwrap();
            page.ax_focus(&mut self.workspace, point);
            if let Some(model) = node.text {
                match request {
                    Request::Select(range) if node.editable => {
                        if let Some(input) = page.input_mut() {
                            let bytes = model.document_range(range).bytes(input.buffer.text());
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
                    Request::SetValue(text) if node.editable => {
                        page.text_input(
                            &self.workspace.translations,
                            TextInput::Insert {
                                text,
                                replacement: Some(
                                    model
                                        .document_range(Utf16Range::new(0, model.text.len_utf16())),
                                ),
                            },
                            Instant::now(),
                        );
                    }
                    _ => {}
                }
            }
            self.workspace.settings_page = Some(page);
            self.settings_redraw(Some(w));
            return;
        }
        match request {
            Request::Press => {
                self.press_accessible(Some(w), &node);
            }
            Request::Focus => {
                self.set_region_focus(Some(w), id);
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
        let document = range;
        if self.settings_page_has_focus() {
            return self
                .workspace
                .settings_page
                .as_ref()
                .unwrap()
                .ax_text_input(id)
                .map_or(Rect::default(), |input| input.range_rect(document).0);
        }
        if let Some(input) = self.panels.accessibility_input(id) {
            return input.range_rect(document).0;
        }
        if id != EDITOR {
            if let Some(sheet) = id.checked_sub(OTHER_EDITOR)
                && let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.sheet == sheet)
                && let Some(view) = self.sheet_views.get(&sheet)
                && let Some(bounds) = self.sheet_bounds.get(&sheet)
            {
                let mut rect = view.range_rect(&tab.editor, document).0;
                rect.x += bounds.x;
                rect.y += bounds.y;
                return rect;
            }
            return Rect::default();
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
    pub(super) fn next_accessibility_focus(&mut self, w: Option<&WindowHandle>, backwards: bool) {
        self.validate_region_focus();
        let mut tree = self.accessibility_model(false);
        tree.nodes.retain(|node| {
            node.region.is_none()
                || node.region == Some(Region::Editor)
                || self.focus_rect(node.id).is_some()
        });
        if let Some(id) = tree.next_focus(
            self.accessibility_focus.or_else(|| tree.focused()),
            backwards,
        ) {
            self.set_region_focus(w, id);
        }
    }
    fn press_accessible(&mut self, w: Option<&WindowHandle>, node: &Node) {
        if let Some((point, double)) = node.activation {
            let sheet = self.workspace.tabs[self.workspace.active].sheet;
            let overlay = self.panels.overlay;
            let panel = self.panels.panel;
            for kind in [MouseKind::Down, MouseKind::Up] {
                self.handle_mouse_event_with_focus(
                    w,
                    MouseEvent {
                        button: 0,
                        kind,
                        position: point,
                        click_count: if double { 2 } else { 1 },
                        modifiers: Modifiers::default(),
                        timestamp: 0.,
                    },
                    true,
                );
            }
            if self.workspace.tabs[self.workspace.active].sheet != sheet
                || self.panels.overlay != overlay
                || self.panels.panel != panel
            {
                // The activation navigated to a different surface. Preserve
                // the input focus assigned by the action itself.
                self.accessibility_focus = None;
            } else {
                self.validate_region_focus();
            }
        }
    }
    pub(super) fn set_region_focus(&mut self, window: Option<&WindowHandle>, id: Id) {
        if id == EDITOR || (OTHER_EDITOR..COMPLETIONS).contains(&id) {
            if let Some(index) = id
                .checked_sub(OTHER_EDITOR)
                .and_then(|sheet| self.workspace.sheet_index(sheet))
            {
                self.workspace.select(index);
            }
            self.leave_region_focus();
        } else {
            self.workspace.editor_mut().unmark();
            self.workspace.editor_mut().keymap.cancel();
            self.accessibility_focus = Some(id);
            self.panels.accessibility_focus(id);
        }
        if let Some(w) = window {
            w.focus();
            w.request_frame();
        }
    }
    pub(super) fn leave_region_focus(&mut self) {
        self.accessibility_focus = None;
        self.panels.accessibility_editor_focus();
    }
    pub(super) fn validate_region_focus(&mut self) {
        if self
            .accessibility_focus
            .is_some_and(|id| self.focus_rect(id).is_none())
        {
            self.leave_region_focus();
        }
    }
    pub(super) fn region_blocks_text(&mut self) -> bool {
        self.validate_region_focus();
        self.accessibility_focus
            .is_some_and(|id| self.panels.accessibility_input(id).is_none())
    }
    pub(super) fn dispatch_region_key(
        &mut self,
        window: Option<&WindowHandle>,
        key: &vel_config::keymap::Key,
    ) -> bool {
        self.validate_region_focus();
        let Some(id) = self.accessibility_focus else {
            return false;
        };
        if key.modifiers == 0 && key.name == "escape" {
            self.set_region_focus(window, EDITOR);
            return true;
        }
        if self.panels.accessibility_input(id).is_some() || key.modifiers & (1 | 8) != 0 {
            return false;
        }
        if key.modifiers == 0 {
            match key.name.as_str() {
                "enter" | "keypad_enter" | "space" => {
                    if let Some(node) = self.accessibility_model(false).node(id) {
                        self.press_accessible(window, node);
                    }
                }
                "left" | "right" | "up" | "down" => {
                    if let Some(next) = self
                        .accessibility_model(false)
                        .next_in_region(id, matches!(key.name.as_str(), "left" | "up"))
                    {
                        self.set_region_focus(window, next);
                    }
                }
                _ => (),
            }
        }
        true
    }
    fn focus_rect(&self, id: Id) -> Option<Rect> {
        if (1000..OTHER_EDITOR).contains(&id) {
            if self.panels.overlay.is_some()
                || !self.tabs_visible()
                || self.workspace.sheet_index((id - 1000) / 2).is_none()
            {
                return None;
            }
            let close = id % 2 == 1;
            let hits = if close {
                &self.tab_close_hits
            } else {
                &self.tab_hits
            };
            return hits
                .iter()
                .find(|(sheet, _)| *sheet == (id - 1000) / 2)
                .map(|(_, rect)| *rect);
        }
        if ((1 << 47)..(1 << 48)).contains(&id) && self.panels.overlay.is_none() {
            let html = self.workspace.editor().html.borrow();
            let popup = html.popup.as_ref().filter(|p| p.visible)?;
            return popup
                .html
                .accessibility_nodes(
                    1 << 47,
                    Point {
                        x: self.content.x,
                        y: self.content.y,
                    },
                )
                .into_iter()
                .find(|node| node.id == id)
                .map(|node| node.frame);
        }
        self.panels.accessibility_rect(id).or_else(|| {
            if self.panels.overlay.is_some() {
                return None;
            }
            let mut frame = self.workspace.editor().completion.popup_rect()?;
            let row = id.checked_sub(COMPLETIONS + 1)? as usize;
            if row
                >= self
                    .workspace
                    .editor()
                    .completion
                    .list
                    .as_ref()?
                    .items
                    .len()
            {
                return None;
            }
            let count = (frame.height / 24.).floor().max(1.) as usize;
            let first = self
                .workspace
                .editor()
                .completion
                .selected
                .saturating_sub(count.saturating_sub(1));
            (first..first + count).contains(&row).then(|| {
                frame.x += self.content.x;
                frame.y += self.content.y + (row - first) as f64 * 24.;
                frame.height = 24.;
                frame
            })
        })
    }
    pub(super) fn paint_accessibility_focus(&self, scene: &mut vel_render::scene::Scene) {
        let Some(rect) = self.accessibility_focus.and_then(|id| self.focus_rect(id)) else {
            return;
        };
        use vel_render::scene::{BorderEdge, CssBox, Layer, Surface};
        // The ring marks the region focus (Cmd/Ctrl+F6) in the color scheme's accent, at the
        // panel's own UI scale: a fixed blue would clash with every other theme colour.
        let accent = self
            .workspace
            .themes
            .ui
            .borrow()
            .palette
            .color("var(--accent)")
            .map_or([0.34, 0.64, 1., 1.], vel_ui::theme::rgba);
        // Inside the region's own bounds, so a ring on a tab neither covers its neighbour's
        // divider nor the strip's edge.
        let edge = BorderEdge {
            width: 1.5 * self.panels.ui_scale as f32,
            color: accent,
        };
        let inset = 1.5 * self.panels.ui_scale;
        let rect = Rect {
            x: rect.x + inset,
            y: rect.y + inset,
            width: (rect.width - inset * 2.).max(1.),
            height: (rect.height - inset * 2.).max(1.),
        };
        let mut layer = Layer::default();
        layer.surfaces.push(Surface::CssBox(CssBox {
            rect,
            clip: Rect {
                x: 0.,
                y: 0.,
                width: self.window_size.width,
                height: self.window_size.height,
            },
            background: [0.; 4],
            radii: [0.; 4],
            borders: [edge; 4],
        }));
        scene.layers.push(layer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The region focus ring is painted only while region focus is on, in the scheme's
    /// accent colour.
    #[test]
    fn region_focus_ring_only_exists_while_the_region_has_focus() {
        let mut app = app("selector\n");
        app.panels.side_visible = true;
        app.scene();
        let ring = |app: &mut App| {
            let scene = app.scene();
            scene
                .layers
                .iter()
                .flat_map(|l| &l.surfaces)
                .find_map(|s| match s {
                    vel_render::scene::Surface::CssBox(b)
                        if b.borders.iter().any(|e| e.width > 0.) =>
                    {
                        Some(b.borders[0].color)
                    }
                    _ => None,
                })
        };
        assert_eq!(ring(&mut app), None);
        let tab = app.tab_hits[0].0;
        app.set_region_focus(None, 1000 + tab * 2);
        let accent = app
            .workspace
            .themes
            .ui
            .borrow()
            .palette
            .color("var(--accent)")
            .map(vel_ui::theme::rgba)
            .expect("accent");
        assert_eq!(ring(&mut app), Some(accent));
        app.set_region_focus(None, EDITOR);
        assert_eq!(ring(&mut app), None);
    }
    #[test]
    fn find_toggle_exposes_checkbox_and_output_exposes_read_only_text() {
        let mut app = app("editor");
        app.panels.panel = Some(crate::panels::Panel::Find);
        app.panels.options.regex = true;
        app.scene();
        let tree = app.accessible_tree();
        let regex = tree.node(crate::panels::ax_id(30)).unwrap();
        assert_eq!(regex.role, Role::CheckBox);
        assert_eq!(regex.checked, Some(true));
        app.panels.panel = Some(crate::panels::Panel::Output);
        app.panels.output = "build output\n".into();
        app.scene();
        let tree = app.accessible_tree();
        let output = tree.node(crate::panels::ax_id(89)).unwrap();
        assert_eq!(output.role, Role::TextArea);
        assert!(!output.editable);
        assert_eq!(output.text.as_ref().unwrap().value(), "build output\n");
    }
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
    fn keyboard_region_focus_blocks_text_and_exits_on_escape_or_cycle() {
        let mut a = app("start");
        a.scene();
        let dispatch = |a: &mut App, key: &str| {
            a.dispatch_key_binding(
                None,
                &vel_config::keymap::Key::parse(key).unwrap(),
                Instant::now(),
                a.viewport(),
            )
        };
        for _ in 0..8 {
            assert!(dispatch(&mut a, "super+f6"));
            if a.region_blocks_text() {
                break;
            }
        }
        assert!(a.region_blocks_text());
        let id = a.accessibility_focus.unwrap();
        assert!(a.focus_rect(id).is_some());
        let mut scene = vel_render::scene::Scene::default();
        a.paint_accessibility_focus(&mut scene);
        assert_eq!(scene.layers.len(), 1);
        assert!(
            a.accessibility_windows.borrow().is_empty(),
            "paint built a document AX value"
        );
        assert!(dispatch(&mut a, "j"));
        a.handle_text_input(
            None,
            TextInput::Insert {
                text: "j ".into(),
                replacement: None,
            },
        );
        a.run_named(None, "insert", &json!({"characters": "blocked"}));
        assert_eq!(a.workspace.editor().text, "start");
        assert!(dispatch(&mut a, "escape"));
        assert!(a.accessibility_focus.is_none());
        a.paint_accessibility_focus(&mut scene);
        assert_eq!(scene.layers.len(), 1, "paint added an editor ring");
        a.handle_text_input(
            None,
            TextInput::Insert {
                text: "ok ".into(),
                replacement: None,
            },
        );
        assert_eq!(a.workspace.editor().text, "ok start");
        let mut returned = false;
        for _ in 0..8 {
            assert!(dispatch(&mut a, "super+f6"));
            if a.accessibility_focus.is_none() {
                returned = true;
                break;
            }
        }
        assert!(returned);
    }

    fn dispatch(a: &mut App, key: &str) -> bool {
        a.dispatch_key_binding(
            None,
            &vel_config::keymap::Key::parse(key).unwrap(),
            Instant::now(),
            a.viewport(),
        )
    }

    fn type_in(a: &mut App, text: &str) {
        a.handle_text_input(
            None,
            TextInput::Insert {
                text: text.into(),
                replacement: None,
            },
        );
    }

    #[test]
    fn hidden_and_closed_regions_restore_editor_input_without_a_frame() {
        let mut a = app("");
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("document.txt");
        std::fs::write(&target, "").unwrap();
        a.open_received_paths(std::slice::from_ref(&target))
            .unwrap();
        if !a.panels.side_visible {
            a.run_named(None, "toggle_side_bar", &json!({}));
        }
        a.scene();
        let sidebar = a
            .accessibility_model(false)
            .nodes
            .into_iter()
            .find(|node| node.region == Some(Region::Sidebar) && node.activation.is_some())
            .unwrap()
            .id;
        a.set_region_focus(None, sidebar);
        assert!(dispatch(&mut a, "super+k"));
        assert!(dispatch(&mut a, "super+b"));
        assert!(!a.panels.side_visible);
        type_in(&mut a, "after sidebar");
        assert_eq!(a.workspace.editor().text, "after sidebar");
        a.run_named(None, "save", &json!({}));

        a.run_named(None, "new_file", &json!({}));
        a.scene();
        let tab = 1000 + 2 * a.workspace.tabs[a.workspace.active].sheet;
        a.set_region_focus(None, tab);
        assert!(dispatch(&mut a, "super+w"));
        assert!(a.accessibility_focus.is_none());
        assert!(dispatch(&mut a, "super+f"));
        a.scene();
        let checkbox = crate::panels::ax_id(30);
        a.set_region_focus(None, checkbox);
        a.run_named(None, "hide_panel", &json!({}));
        type_in(&mut a, "ok");
        assert!(a.workspace.editor().text.to_string().contains("ok"));
        assert!(a.accessibility_focus.is_none());
    }

    #[test]
    fn activation_keeps_focus_until_navigation_and_file_open_clears_it() {
        let mut a = app("document");
        assert!(dispatch(&mut a, "super+f"));
        a.scene();
        let checkbox = crate::panels::ax_id(30);
        a.set_region_focus(None, checkbox);
        let regex = a.panels.options.regex;
        assert!(dispatch(&mut a, "space"));
        assert_ne!(a.panels.options.regex, regex);
        assert_eq!(a.accessibility_focus, Some(checkbox));
        assert!(dispatch(&mut a, "enter"));
        assert_eq!(a.panels.options.regex, regex);
        assert_eq!(a.accessibility_focus, Some(checkbox));

        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("opened.txt");
        std::fs::write(&target, "second").unwrap();
        a.open_received_paths(&[target]).unwrap();
        assert!(a.accessibility_focus.is_none());
        type_in(&mut a, "new ");
        assert_eq!(a.workspace.editor().text, "new second");
    }

    #[test]
    fn palette_accessibility_commands_keep_the_focus_they_just_set() {
        let mut a = app("document");
        let palette = a.workspace.palette.clone();
        for command in ["focus_next_control", "focus_previous_control"] {
            a.workspace.palette = palette
                .iter()
                .filter(|item| item.invocation.command == command)
                .cloned()
                .collect();
            assert_eq!(a.workspace.palette.len(), 1);
            a.scene();
            a.run_named(None, "show_overlay", &json!({"overlay":"command_palette"}));
            a.scene();
            assert!(dispatch(&mut a, "enter"));
            assert!(a.panels.overlay.is_none());
            let id = a
                .accessibility_focus
                .expect("palette command focused a region");
            assert!(a.focus_rect(id).is_some());
        }
    }

    #[test]
    fn output_accessibility_ends_at_the_tail_and_results_window_contains_the_selected_row() {
        let mut a = app("editor");
        a.panels.panel = Some(crate::panels::Panel::Output);
        a.panels.output = format!("HEAD ONLY{}TAIL 😀", "猫".repeat(110_000));
        let value = |a: &App| {
            a.accessible_tree()
                .node(crate::panels::ax_id(89))
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .value()
        };
        let output = value(&a);
        assert!(output.ends_with("TAIL 😀") && !output.contains("HEAD ONLY"));
        assert!(output.len() <= 256 * 1024);
        a.panels.panel = Some(crate::panels::Panel::Results);
        for row in 0..5000 {
            a.panels.result_view.push(vel_search::files::Line {
                path: std::sync::Arc::from(std::path::Path::new("fixture.txt")),
                number: row + 1,
                text: format!("sample {row}"),
                highlights: vec![],
                column_offset: 0,
                match_count: 0,
            });
        }
        a.panels.result_selected = 2501;
        let results = value(&a);
        assert!(results.contains("sample 2500") && !results.contains("sample 4999"));
        assert!(results.lines().count() <= 48);
    }
    #[test]
    fn cjk_accessibility_range_uses_measured_cell_extents() {
        use vel_render::typography::WideAdvances;
        for (text, cells) in [("中文", 3.), ("A中", 2.5), ("👩‍💻🦀", 4.5)] {
            let mut a = app(text);
            a.view.set_wide_advances(WideAdvances::new([
                1.5, 1.5, 1.5, 1.5, 1.5, 2.25, 2.25, 1.5,
            ]));
            a.scene();
            let range = Utf16Range::new(0, text.encode_utf16().count());
            let r = a.accessible_range_rect(EDITOR, range);
            assert_eq!(
                vel_platform::AppHandler::text_input_rect(&a, range),
                (r, range)
            );
            assert!(
                (r.width - cells * a.view.cell).abs() < 0.02,
                "{text}: {r:?}"
            );
            let mut begin = a.view.rect_at(a.workspace.editor(), 0);
            begin.x += a.content.x;
            begin.y += a.content.y;
            assert_eq!((r.x, r.y), (begin.x, begin.y));
        }
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
        assert!(
            tree.nodes
                .iter()
                .any(|n| n.role == Role::Tab && n.label.contains("a😀é") && n.activation.is_some())
        );
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
