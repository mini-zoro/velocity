//! Shared native/headless mouse event routing; drawing stays in the application handler.
use super::*;

impl App {
    pub(super) fn handle_mouse_event(&mut self, w: Option<&WindowHandle>, event: MouseEvent) {
        // Every exit path releases capture, including overlays/status controls and
        // mouse bindings. Finishing a text move still happens in the normal route.
        let up = event.kind == MouseKind::Up;
        if event.kind == MouseKind::Down {
            self.end_mouse_gesture();
        }
        self.route_mouse_event(w, event);
        if up {
            self.end_mouse_gesture();
        }
    }
    fn end_mouse_gesture(&mut self) {
        self.view.end_pointer_gesture();
        self.drag_select = None;
        self.drag_text = None;
        self.text_drag_origin = None;
        self.text_drag_pressed = None;
        self.dragging_splitter = None;
    }
    fn route_mouse_event(&mut self, w: Option<&WindowHandle>, event: MouseEvent) {
        let captured = matches!(event.kind, MouseKind::Drag | MouseKind::Up)
            && (self.view.pointer_gesture_active()
                || self.drag_select.is_some()
                || self.drag_text.is_some());
        if event.kind == MouseKind::Down {
            self.accessibility_focus = None;
        }
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        if self.panels.overlay.is_some() && !captured {
            if event.kind == MouseKind::Move {
                self.panels.tree.pointer(event.position, None);
                if let Some(w) = w {
                    w.request_frame();
                }
                return;
            }
            let viewport = self.viewport();
            if let Err(error) = self.panels.mouse(
                &mut self.workspace,
                event.position,
                event.kind == MouseKind::Down,
                event.click_count >= 2,
                event.modifiers.shift(),
                viewport,
            ) {
                self.named_error(w, &error);
            }
            if let Some(invocation) = self.panels.pending_command.take() {
                self.run_named(w, &invocation.command, &invocation.args);
            }
            self.named_changed(w, event.kind == MouseKind::Down);
            return;
        }
        if event.is_context_click()
            && self
                .mouse_binding(event.button + 1, event.click_count, event.modifiers, true)
                .or_else(|| {
                    self.mouse_binding(event.button + 1, event.click_count, event.modifiers, false)
                })
                .is_none()
        {
            if let Some(w) = w {
                self.begin_context_menu(w, event.position);
            }
            return;
        }
        if event.kind == MouseKind::Down {
            self.pending_menu = None;
            self.panels.cancel_review_menu();
        }
        let local = Point {
            x: event.position.x - self.content.x,
            y: event.position.y - self.content.y,
        };
        if event.kind == MouseKind::Move {
            self.workspace.editor().html.borrow_mut().mouse_move(local);
        }
        if event.kind == MouseKind::Down {
            let target = self.workspace.editor().html.borrow().navigate(local);
            if let Some(popup) = &mut self.workspace.editor().html.borrow_mut().popup {
                popup.mouse_action();
            }
            if let Some(target) = target {
                let action = crate::html_ui::invocation(target);
                self.run_named(w, &action.command, &action.args);
                self.named_changed(w, false);
                return;
            }
        }

        if event.kind == MouseKind::Down {
            self.dragging_splitter =
                self.workspace
                    .workbench
                    .layout
                    .splitter(event.position, self.workbench_bounds, 4.);
            if self.dragging_splitter.is_some() {
                return;
            }
        }
        if let Some(splitter) = self.dragging_splitter {
            if event.kind == MouseKind::Drag {
                let bounds = self.workbench_bounds;
                let position = if splitter.vertical {
                    (event.position.x - bounds.x) / bounds.width
                } else {
                    (event.position.y - bounds.y) / bounds.height
                };
                self.workspace.workbench.layout.drag(splitter, position);
                self.named_changed(w, false);
                return;
            }
            if event.kind == MouseKind::Up {
                self.dragging_splitter = None;
                return;
            }
        }
        if event.kind == MouseKind::Down {
            let point = Point {
                x: event.position.x - self.content.x,
                y: event.position.y - self.content.y,
            };
            if self.workspace.editor_mut().completion.hit(point) {
                let _ = self.workspace.editor_mut().commit_completion();
                self.named_changed(w, true);
                return;
            }
            if self
                .panels
                .click_definition(&mut self.workspace, event.position)
                .unwrap_or(false)
            {
                self.sync_active_view();
                self.named_changed(w, true);
                return;
            }
        }
        if event.kind == MouseKind::Move {
            self.panels.review_hover(&self.workspace, event.position);
            let byte = self.view.hit_test(
                self.workspace.editor(),
                Point {
                    x: event.position.x - self.content.x,
                    y: event.position.y - self.content.y,
                },
            );
            self.panels.hover_at(&self.workspace, event.position, byte);
            let _ = self.view.pointer(
                self.workspace.editor_mut(),
                Point {
                    x: event.position.x - self.content.x,
                    y: event.position.y - self.content.y,
                },
                false,
                false,
                false,
            );
            self.panels.tree.pointer(event.position, None);
            if let Some(w) = w {
                w.request_frame();
            }
            return;
        }
        let viewport = self.viewport();
        self.panels.primary_click = event.modifiers.command();
        match if captured {
            Ok(false)
        } else {
            self.panels.mouse(
                &mut self.workspace,
                event.position,
                event.kind == MouseKind::Down,
                event.click_count >= 2,
                event.modifiers.shift(),
                viewport,
            )
        } {
            Ok(true) => {
                if !self.panels.review.items.is_empty() {
                    // The Git status control uses the same native tracking path.
                    let items = std::mem::take(&mut self.panels.review.items);
                    let entries: Vec<_> = items
                        .into_iter()
                        .map(|item| MenuEntry {
                            caption: item.caption,
                            command: Some(item.invocation.command),
                            args: item.invocation.args.to_string(),
                            children: vec![],
                        })
                        .collect();
                    if let Some(w) = w {
                        w.show_context_menu(&entries);
                    }
                }
                if let Some(invocation) = self.panels.pending_command.take() {
                    self.run_named(w, &invocation.command, &invocation.args);
                }
                if let Some(w) = w
                    && let Err(error) = self.replacement_confirmation(w)
                {
                    self.named_error(Some(w), &error);
                }
                // Opening a status-bar picker does not navigate the editor.
                self.named_changed(w, self.panels.overlay.is_none());
                return;
            }
            Ok(false) => {}
            Err(error) => {
                self.named_error(w, &error);
                return;
            }
        }
        if event.kind == MouseKind::Down
            && let Some((sheet, _)) = self
                .tab_hits
                .iter()
                .find(|(_, rect)| vel_ui::contains(*rect, event.position))
                .copied()
        {
            if let Some(index) = self.workspace.sheet_index(sheet) {
                if event.button == 2 {
                    // Middle click closes the tab under the pointer, like Sublime.
                    if self.can_close_mouse(w, &[index]) {
                        let _ = self.workspace.close(index);
                    }
                } else if self
                    .tab_close_hits
                    .iter()
                    .any(|(id, rect)| *id == sheet && vel_ui::contains(*rect, event.position))
                {
                    let mut indices = if self.workspace.workbench.group_of(sheet).is_some_and(|g| {
                        self.workspace.workbench.groups[g].selected.contains(&sheet)
                    }) {
                        let group = self.workspace.workbench.group_of(sheet).expect("group");
                        self.workspace.workbench.groups[group]
                            .visible()
                            .iter()
                            .filter_map(|s| self.workspace.sheet_index(*s))
                            .collect::<Vec<_>>()
                    } else {
                        vec![index]
                    };
                    if self.can_close_mouse(w, &indices) {
                        indices.sort_unstable();
                        for index in indices.into_iter().rev() {
                            let _ = self.workspace.close(index);
                        }
                    }
                } else {
                    self.workspace.tabs[self.workspace.active].scroll =
                        (self.view.scroll_x, self.view.scroll_y);
                    self.workspace.tabs[self.workspace.active].scroll_line_height =
                        self.view.line_height;
                    self.workspace.click_sheet(
                        index,
                        event.modifiers.command(),
                        event.modifiers.shift(),
                    );
                    self.panels.dragging_tab = Some(index);
                    if event.click_count >= 2 {
                        self.workspace.tabs[index].transient = false;
                    }
                }
                self.named_changed(w, false);
            }
            return;
        }
        if event.kind == MouseKind::Down
            && event.button == 0
            && event.click_count == 2
            && let Some(&(group, _, _)) = self
                .tab_bars
                .iter()
                .find(|(_, bar, _)| vel_ui::contains(*bar, event.position))
        {
            // A double click on the empty part of a tab bar opens a new file in that group.
            self.workspace.tabs[self.workspace.active].scroll =
                (self.view.scroll_x, self.view.scroll_y);
            self.workspace.workbench.active = group;
            self.run_named(w, "new_file", &json!({}));
            self.named_changed(w, false);
            return;
        }
        if event.kind == MouseKind::Drag
            && let Some(from) = self.panels.dragging_tab
        {
            if let Some(target) = self
                .group_bounds
                .iter()
                .position(|b| vel_ui::contains(*b, event.position))
            {
                if target != self.workspace.workbench.active {
                    let _ = self.workspace.workbench.move_selected(target);
                    let _ = self.workspace.focus_current_group();
                } else if let Some((sheet, _)) = self
                    .tab_hits
                    .iter()
                    .find(|(_, rect)| vel_ui::contains(*rect, event.position))
                {
                    let moved = self.workspace.tabs.get(from).map(|t| t.sheet);
                    let group = &mut self.workspace.workbench.groups[target];
                    if let Some(moved) = moved
                        && moved != *sheet
                        && let Some(to) = group.sheets.iter().position(|s| s == sheet)
                    {
                        group.sheets.retain(|s| *s != moved);
                        group.sheets.insert(to.min(group.sheets.len()), moved);
                    }
                }
                self.named_changed(w, false);
            }
            return;
        }
        if event.kind == MouseKind::Up {
            self.panels.dragging_tab = None;
        }
        if event.kind == MouseKind::Down
            && let Some((&sheet, _)) = self
                .sheet_bounds
                .iter()
                .find(|(_, rect)| vel_ui::contains(**rect, event.position))
            && let Some(index) = self.workspace.sheet_index(sheet)
            && index != self.workspace.active
        {
            self.workspace.tabs[self.workspace.active].scroll =
                (self.view.scroll_x, self.view.scroll_y);
            if let Some(group) = self.workspace.workbench.group_of(sheet) {
                self.workspace.workbench.active = group;
                self.workspace.workbench.groups[group].focused = Some(sheet);
            }
            if let Err(e) = self.workspace.prepare_sheet(index) {
                self.named_error(w, &e);
                return;
            }
            self.workspace.active = index;
            self.sync_active_view();
        }
        if event.kind == MouseKind::Up {
            self.drag_select = None;
        }
        if matches!(event.kind, MouseKind::Down | MouseKind::Up)
            && let Some((name, mut args)) = self.mouse_binding(
                event.button + 1,
                event.click_count,
                event.modifiers,
                event.kind == MouseKind::Down,
            )
        {
            vel_config::ui::merge_context_arg(
                args.as_object_mut().expect("binding arguments"),
                "event",
                &json!({"x":event.position.x,"y":event.position.y}),
            );
            self.run_named(w, &name, &args);
            if event.kind == MouseKind::Up {
                self.drag_select = None;
            }
            return;
        }
        // The middle button makes column selections, like ⌥ with the primary button.
        let middle = event.button == 2;
        if event.button != 0 && !middle {
            return;
        }
        let local = Point {
            x: event.position.x - self.content.x,
            y: event.position.y - self.content.y,
        };
        if !middle {
            match self.view.pointer_with_option(
                self.workspace.editor_mut(),
                local,
                event.kind == MouseKind::Down,
                event.kind == MouseKind::Drag,
                event.kind == MouseKind::Up,
                event.modifiers.option(),
            ) {
                Ok(true) => {
                    self.named_changed(w, false);
                    return;
                }
                Err(e) => {
                    self.named_error(w, &e);
                    return;
                }
                _ => {}
            }
        }
        let at = self.view.hit_test(self.workspace.editor(), local);
        if self.workspace.editor().binary.is_some() {
            // The hex view keeps byte-granular selection.
            match event.kind {
                MouseKind::Down => self
                    .workspace
                    .editor_mut()
                    .set_caret(at, event.modifiers.shift()),
                MouseKind::Drag => self.workspace.editor_mut().set_caret(at, true),
                MouseKind::Up | MouseKind::Move => {}
            }
            self.named_changed(w, false);
            return;
        }
        match event.kind {
            MouseKind::Down => {
                use crate::mouse_select::{Combine, DragSelect, Unit};
                let m = event.modifiers;
                // A click on the line numbers selects the line (fold buttons were handled above).
                let numbers =
                    !middle && local.x >= 0. && local.x < self.view.gutter(self.workspace.editor());
                let unit = if event.click_count >= 3 || numbers {
                    Unit::Line
                } else if event.click_count == 2 {
                    Unit::Word
                } else {
                    Unit::Char
                };
                let combine = match (m.command(), m.shift()) {
                    (true, true) => Combine::Subtract,
                    (true, false) => Combine::Add,
                    _ => Combine::Replace,
                };
                self.drag_select = None;
                self.drag_text = None;
                if unit == Unit::Char
                    && !middle
                    && !(m.command() || m.shift() || m.option() || m.control())
                {
                    self.text_drag_pressed = Some(Instant::now());
                    self.text_drag_origin = Some((local, at));
                    self.drag_text =
                        crate::mouse_select::DragText::begin(self.workspace.editor(), local, at);
                    if self.drag_text.is_some() {
                        return;
                    }
                }
                self.drag_select = Some(DragSelect::begin(
                    &self.view,
                    self.workspace.editor_mut(),
                    local,
                    at,
                    unit,
                    combine,
                    // A2 verification limit: the public references do not define anchors:
                    // https://www.sublimetext.com/docs/column_selection.html
                    // https://docs.sublimetext.io/reference/mouse_bindings.html
                    // Preserve the built-in rule until confirmed; explicit mousemap
                    // `extend` works with every unit in begin_bound_drag.
                    m.shift() && !m.command() && unit == Unit::Char,
                    middle || (m.option() && unit == Unit::Char),
                ));
            }
            MouseKind::Drag => {
                self.select_before_text_drag_delay(local, Instant::now());
                if let Some(drag) = self.drag_text.as_mut() {
                    drag.update(&mut self.view, self.workspace.editor(), local);
                } else if let Some(drag) = self.drag_select.as_mut() {
                    drag.update(&self.view, self.workspace.editor_mut(), local);
                } else {
                    return;
                }
            }
            MouseKind::Up => {
                self.drag_select = None;
                self.text_drag_origin = None;
                self.text_drag_pressed = None;
                if let Some(drag) = self.drag_text.take() {
                    drag.finish(
                        &mut self.view,
                        self.workspace.editor_mut(),
                        local,
                        event.modifiers.option(),
                    );
                }
            }
            MouseKind::Move => {}
        }
        self.named_changed(w, false);
    }
}

impl App {
    pub(super) fn handle_scroll_event(&mut self, w: Option<&WindowHandle>, event: ScrollEvent) {
        if let Some(w) = w {
            self.cancel_context(w);
        }
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        let scale = if event.precise {
            1.
        } else {
            self.view.line_height
        };
        if let Some(group) = self.group_bounds.iter().position(|r| {
            event.position.y >= r.y
                && event.position.y < r.y + crate::view::TAB_HEIGHT
                && vel_ui::contains(*r, event.position)
        }) {
            let max = self
                .tab_bars
                .iter()
                .find(|(g, _, _)| *g == group)
                .map_or(f64::INFINITY, |(_, _, max)| *max);
            let current = self.workspace.workbench.groups[group].tab_scroll.min(max);
            self.workspace.workbench.groups[group].tab_scroll =
                (current - (event.delta_x + event.delta_y) * scale).clamp(0., max);
            self.named_changed(w, false);
            return;
        }
        let local = Point {
            x: event.position.x - self.content.x,
            y: event.position.y - self.content.y,
        };
        if self.workspace.editor().html.borrow_mut().scroll(
            local,
            -event.delta_x * scale,
            -event.delta_y * scale,
        ) {
            self.named_changed(w, false);
            return;
        }
        if self.panels.scroll(event.position, -event.delta_y * scale) {
            self.named_changed(w, false);
            return;
        }
        if event.delta_y != 0. && vel_ui::contains(self.content, event.position) {
            let button = if event.delta_y > 0. {
                vel_config::ui::SCROLL_UP
            } else {
                vel_config::ui::SCROLL_DOWN
            };
            if let Some((name, args)) = self
                .mouse_binding(button, 1, event.modifiers, true)
                .or_else(|| self.mouse_binding(button, 1, event.modifiers, false))
            {
                self.run_named(w, &name, &args);
                return;
            }
        }
        if let Some((&sheet, _)) = self
            .sheet_bounds
            .iter()
            .find(|(_, rect)| vel_ui::contains(**rect, event.position))
            && sheet != self.workspace.tabs[self.workspace.active].sheet
            && let Some(index) = self.workspace.sheet_index(sheet)
        {
            let active = self.workspace.active;
            if self.workspace.prepare_sheet(index).is_ok()
                && let Some(view) = self.sheet_views.get_mut(&sheet)
            {
                view.scroll(
                    &self.workspace.tabs[index].editor,
                    -event.delta_x * scale,
                    -event.delta_y * scale,
                );
                self.workspace.tabs[index].scroll = (view.scroll_x, view.scroll_y);
            }
            let _ = self.workspace.prepare_sheet(active);
            self.named_changed(w, false);
            return;
        }
        self.view.scroll(
            self.workspace.editor(),
            -event.delta_x * scale,
            -event.delta_y * scale,
        );
        self.named_changed(w, false);
    }
}

impl App {
    fn can_close_mouse(&mut self, window: Option<&WindowHandle>, indices: &[usize]) -> bool {
        if let Some(window) = window {
            self.can_close_many(window, indices)
        } else {
            indices.iter().all(|i| !self.workspace.tabs[*i].dirty())
        }
    }
    fn mouse_binding(
        &mut self,
        button: usize,
        count: usize,
        modifiers: Modifiers,
        press: bool,
    ) -> Option<(String, Value)> {
        if self.workspace.mousemap.is_empty() {
            return None;
        }
        // A pointer press may just have moved focus out of a panel. Refresh the
        // same editor contexts used by key dispatch before testing mouse contexts.
        use crate::panels::Panel;
        let panel = self.panels.panel.map_or("", |panel| match panel {
            Panel::Find => "find",
            Panel::Replace => "replace",
            Panel::Incremental => "incremental_find",
            Panel::Files => "find_in_files",
            Panel::Input => "input",
            Panel::Output => "output.exec",
            Panel::Results => "output.find_results",
        });
        let editor = self.workspace.editor_mut();
        editor.ui_panel_visible = self.panels.panel.is_some();
        editor.ui_overlay_visible = self.panels.overlay.is_some();
        editor.ui_panel = panel;
        editor.ui_panel_focused = self.panels.focus.route().0 == vel_ui::Surface::Panel;
        self.workspace.mousemap.iter().rev().find_map(|binding| {
            let expected = [
                ("ctrl", modifiers.control()),
                ("alt", modifiers.option()),
                ("shift", modifiers.shift()),
                ("super", modifiers.command()),
            ];
            if binding.button != button
                || binding.count != count
                || expected
                    .iter()
                    .any(|(name, on)| *on != binding.modifiers.iter().any(|m| m == name))
                || !self.workspace.editor().context_matches(&binding.context)
            {
                return None;
            }
            let (name, args) = if press {
                (binding.press_command.as_ref()?, &binding.press_args)
            } else {
                (binding.command.as_ref()?, &binding.args)
            };
            Some((
                name.clone(),
                if args.is_object() {
                    args.clone()
                } else {
                    json!({})
                },
            ))
        })
    }
    pub(super) fn begin_bound_drag(&mut self, args: &Value) -> Result<(), String> {
        use crate::mouse_select::{Combine, DragSelect, Unit};
        let x = args["event"]["x"]
            .as_f64()
            .ok_or("drag_select needs mouse event x")?;
        let y = args["event"]["y"]
            .as_f64()
            .ok_or("drag_select needs mouse event y")?;
        let point = Point {
            x: x - self.content.x,
            y: y - self.content.y,
        };
        let unit = match args["by"].as_str().unwrap_or("characters") {
            "characters" => Unit::Char,
            "words" => Unit::Word,
            "lines" => Unit::Line,
            _ => return Err("drag_select by must be characters, words or lines".into()),
        };
        let combine = if args["subtractive"].as_bool().unwrap_or(false) {
            Combine::Subtract
        } else if args["additive"].as_bool().unwrap_or(false) {
            Combine::Add
        } else {
            Combine::Replace
        };
        let at = self.view.hit_test(self.workspace.editor(), point);
        self.drag_text = None;
        self.drag_select = Some(DragSelect::begin(
            &self.view,
            self.workspace.editor_mut(),
            point,
            at,
            unit,
            combine,
            args["extend"].as_bool().unwrap_or(false),
            args["column"].as_bool().unwrap_or(false),
        ));
        Ok(())
    }
}

impl App {
    pub(super) fn apply_view_action(&mut self, name: &str, args: &Value) -> Result<bool, String> {
        match name {
            "toggle_setting" => {
                let key = args["setting"]
                    .as_str()
                    .ok_or("toggle_setting needs setting")?;
                let value = self.workspace.editor().settings.bool(key, false);
                self.workspace
                    .editor_mut()
                    .settings
                    .set(Layer::View, key, json!(!value));
            }
            "set_setting" => {
                // https://www.sublimetext.com/docs/settings.html: per-view settings
                // are the highest ordinary preference layer.
                let key = args["setting"]
                    .as_str()
                    .ok_or("set_setting needs setting")?;
                let value = args.get("value").ok_or("set_setting needs value")?;
                self.workspace
                    .editor_mut()
                    .settings
                    .set(Layer::View, key, value.clone());
            }
            "drag_select" => self.begin_bound_drag(args)?,
            "scroll_lines" => {
                let amount = args["amount"].as_f64().unwrap_or(1.);
                self.view
                    .scroll(self.workspace.editor(), 0., -amount * self.view.line_height);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
