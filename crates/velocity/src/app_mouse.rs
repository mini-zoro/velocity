//! Shared native/headless mouse event routing; drawing stays in the application handler.
use super::*;

impl App {
    pub(super) fn handle_mouse_event(&mut self, w: Option<&WindowHandle>, event: MouseEvent) {
        self.handle_mouse_event_with_focus(w, event, false);
    }
    pub(super) fn handle_mouse_event_with_focus(
        &mut self,
        w: Option<&WindowHandle>,
        event: MouseEvent,
        keep_focus: bool,
    ) {
        // Every exit path releases capture, including overlays/status controls and
        // mouse bindings. Finishing a text move still happens in the normal route.
        if matches!(event.kind, MouseKind::Move | MouseKind::Down) {
            self.reveal_typing_chrome(w);
        }
        let up = event.kind == MouseKind::Up;
        if event.kind == MouseKind::Down {
            self.end_mouse_gesture();
        }
        self.route_mouse_event(w, event, keep_focus);
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
    fn route_mouse_event(&mut self, w: Option<&WindowHandle>, event: MouseEvent, keep_focus: bool) {
        if self.settings_page_has_focus() {
            if event.kind == MouseKind::Down && event.button == 0 && !event.is_context_click() {
                self.settings_pointer(w, event.position, event.modifiers.shift(), Instant::now());
            } else if event.kind == MouseKind::Move
                && self
                    .workspace
                    .settings_page
                    .as_mut()
                    .unwrap()
                    .hover(event.position, Instant::now())
            {
                self.settings_redraw(w);
            }
            return;
        }
        let captured = matches!(event.kind, MouseKind::Drag | MouseKind::Up)
            && (self.view.pointer_gesture_active()
                || self.scrollbar_binding_button == Some(event.button)
                || self.drag_select.is_some()
                || self.drag_text.is_some());
        if event.kind == MouseKind::Down {
            self.workspace.jump_history.borrow_mut().mouse_down(
                event.position.x,
                event.position.y,
                event.button,
                event.click_count,
            );
            if !keep_focus && self.accessibility_focus.is_some() {
                self.leave_region_focus();
            }
        }
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        self.panels.primary_click = event.modifiers.command();
        self.panels.alternate_click = event.modifiers.option();
        if event.kind == MouseKind::Move {
            // Hover feedback for the theme-drawn tab close and tab-bar buttons.
            let close = self
                .tab_close_hits
                .iter()
                .find(|(_, rect)| vel_ui::contains(*rect, event.position))
                .map(|(sheet, _)| *sheet);
            let button = self
                .tab_button_hits
                .iter()
                .find(|(_, _, rect)| vel_ui::contains(*rect, event.position))
                .map(|(group, class, _)| (*group, *class));
            if close != self.hovered_close || button != self.hovered_tab_button {
                self.hovered_close = close;
                self.hovered_tab_button = button;
                if let Some(w) = w {
                    w.request_frame();
                }
            }
        }
        if self.panels.overlay.is_some() && !captured && !event.is_context_click() {
            if event.kind == MouseKind::Move {
                self.panels.tree.pointer(event.position, None);
                if let Some(w) = w {
                    w.request_frame();
                }
                return;
            }
            let viewport = self.viewport();
            let before = self.command_states();
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
            let moved = self.target_changed(&before);
            self.named_changed(w, moved);
            return;
        }
        if event.kind == MouseKind::Down
            && event.button == 0
            && self.panels.hit_find_options(event.position)
            && let Some(w) = w
        {
            self.show_find_options(w, event.position);
            return;
        }
        if self.status_switcher_mouse(w, &event) {
            return;
        }
        if event.is_context_click()
            && self
                .mouse_binding(
                    event.button + 1,
                    event.click_count,
                    event.modifiers,
                    true,
                    self.point_over_scrollbar(event.position),
                )
                .or_else(|| {
                    self.mouse_binding(
                        event.button + 1,
                        event.click_count,
                        event.modifiers,
                        false,
                        self.point_over_scrollbar(event.position),
                    )
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
                self.workspace.workbench.layout.drag_with_minimum(
                    splitter,
                    position,
                    bounds,
                    self.workspace.pane_minimum,
                );
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
            match self.workspace.editor_mut().completion.hit(point) {
                crate::completion::PopupHit::Row => {
                    let _ = self.workspace.editor_mut().commit_completion();
                    self.named_changed(w, true);
                    return;
                }
                // Padding and footer belong to the popup, not to the text below it.
                crate::completion::PopupHit::Chrome => return,
                crate::completion::PopupHit::Outside => {}
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
            // Definition hover belongs to the text area; the panels, side bar and status
            // bar drawn over or beside it must not open popups for text they cover.
            if vel_ui::contains(self.content, event.position) {
                let byte = self.view.hit_test(
                    self.workspace.editor(),
                    Point {
                        x: event.position.x - self.content.x,
                        y: event.position.y - self.content.y,
                    },
                );
                self.panels.hover_at(&self.workspace, event.position, byte);
            } else {
                self.panels.leave_hover(event.position);
            }
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
        if self.format_status_mouse(w, &event) {
            return;
        }
        let viewport = self.viewport();
        // Picker previews restore the tab's stored scroll; capture the live viewport first.
        let active = self.workspace.active;
        self.workspace.tabs[active].scroll = (self.view.scroll_x, self.view.scroll_y);
        self.workspace.tabs[active].scroll_line_height = self.view.line_height;
        let panel_before = self.command_states();
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
                            ..Default::default()
                        })
                        .collect();
                    if let Some(w) = w {
                        w.show_context_menu(&entries);
                    }
                }
                if let Some(invocation) = self.panels.pending_command.take() {
                    self.run_named(w, &invocation.command, &invocation.args);
                }
                if let Some(w) = w {
                    self.replacement_confirmation(w);
                }
                // Opening or accepting a settings/syntax picker does not navigate.
                let moved = self.target_changed(&panel_before);
                self.named_changed(w, moved);
                return;
            }
            Ok(false) => {}
            Err(error) => {
                self.named_error(w, &error);
                return;
            }
        }
        if self.tab_button_mouse(w, &event) {
            return;
        }
        if event.kind == MouseKind::Down
            && event.button == 0
            && event.click_count >= 2
            && let Some((at, sheet)) = self.tab_click
            && (at.x - event.position.x).abs() <= 4.
            && (at.y - event.position.y).abs() <= 4.
            && let Some(index) = self.workspace.sheet_index(sheet)
        {
            // The first click revealed this tab, which may have moved the strip: the double
            // click belongs to the tab it started on, not to whatever slid under the pointer.
            self.workspace.make_permanent(index);
            self.named_changed(w, false);
            return;
        }
        if event.kind == MouseKind::Down {
            self.tab_click = None;
        }
        if event.kind == MouseKind::Down
            && let Some((sheet, _)) = self
                .tab_hits
                .iter()
                .find(|(_, rect)| vel_ui::contains(*rect, event.position))
                .copied()
        {
            if let Some(index) = self.workspace.sheet_index(sheet) {
                if event.button == 2
                    || self
                        .tab_close_hits
                        .iter()
                        .any(|(id, rect)| *id == sheet && vel_ui::contains(*rect, event.position))
                {
                    self.run_named(w, "close_by_index", &json!({"_context_sheet":sheet}));
                } else {
                    self.workspace.tabs[self.workspace.active].scroll =
                        (self.view.scroll_x, self.view.scroll_y);
                    self.workspace.tabs[self.workspace.active].scroll_line_height =
                        self.view.line_height;
                    if event.modifiers.option() && !event.modifiers.shift() {
                        self.workspace.replace_focused_sheet(index);
                    } else {
                        self.workspace.click_sheet(
                            index,
                            event.modifiers.command(),
                            event.modifiers.shift(),
                        );
                    }
                    self.panels.dragging_tab = Some(index);
                    self.tab_click = Some((event.position, sheet));
                    if event.click_count >= 2 {
                        self.workspace.make_permanent(index);
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
        if self.workspace.tabs[self.workspace.active].find_results
            && event.kind == MouseKind::Down
            && event.button == 0
            && !event.modifiers.control()
            && !event.modifiers.shift()
            && !event.modifiers.option()
            && event.click_count == if event.modifiers.command() { 1 } else { 2 }
            && vel_ui::contains(self.content, event.position)
        {
            let point = Point {
                x: event.position.x - self.content.x,
                y: event.position.y - self.content.y,
            };
            let byte = self.view.hit_test(self.workspace.editor(), point);
            self.panels.focus_editor_for_context();
            match self.panels.jump_tab_result(&mut self.workspace, byte) {
                Ok(true) => {
                    self.named_changed(w, true);
                    return;
                }
                Err(error) => {
                    self.named_error(w, &error);
                    return;
                }
                Ok(false) => {}
            }
        }
        let over_scrollbar = self.point_over_scrollbar(event.position);
        let captured_scrollbar = self.scrollbar_binding_button == Some(event.button);
        let scrollbar = over_scrollbar || (event.kind != MouseKind::Down && captured_scrollbar);
        if event.kind == MouseKind::Down {
            self.scrollbar_binding_button = None;
            if scrollbar
                && self
                    .mouse_binding(
                        event.button + 1,
                        event.click_count,
                        event.modifiers,
                        false,
                        true,
                    )
                    .is_some()
            {
                self.scrollbar_binding_button = Some(event.button);
            }
        }
        if matches!(event.kind, MouseKind::Down | MouseKind::Up)
            && let Some((name, mut args)) = self.mouse_binding(
                event.button + 1,
                event.click_count,
                event.modifiers,
                event.kind == MouseKind::Down,
                scrollbar,
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
                self.scrollbar_binding_button = None;
            } else if scrollbar {
                self.scrollbar_binding_button = Some(event.button);
            }
            return;
        }
        if self.scrollbar_binding_button == Some(event.button) {
            if event.kind == MouseKind::Up {
                self.scrollbar_binding_button = None;
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
        self.reveal_typing_chrome(w);
        if let Some(w) = w {
            self.cancel_context(w);
        }
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        if self.settings_page_has_focus() {
            let page = self.workspace.settings_page.as_mut().unwrap();
            page.scroll_at(
                event.position,
                -event.delta_y * if event.precise { 1. } else { 23. },
            );
            self.settings_redraw(w);
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
            // https://docs.sublimetext.io/reference/settings.html#global-settings
            if !event.precise
                && event.delta_y != 0.
                && self
                    .workspace
                    .global_bool("mouse_wheel_switches_tabs", false)
            {
                let group = &self.workspace.workbench.groups[group];
                if let Some(at) = group
                    .sheets
                    .iter()
                    .position(|sheet| Some(*sheet) == group.focused)
                    && group.sheets.len() > 1
                {
                    let offset = if event.delta_y > 0. {
                        group.sheets.len() - 1
                    } else {
                        1
                    };
                    if let Some(tab) = self
                        .workspace
                        .sheet_index(group.sheets[(at + offset) % group.sheets.len()])
                    {
                        self.workspace.select(tab);
                        self.sync_active_view();
                        self.named_changed(w, true);
                    }
                }
                return;
            }
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
            let scrollbar = self.point_over_scrollbar(event.position);
            if let Some((name, args)) = self
                .mouse_binding(button, 1, event.modifiers, true, scrollbar)
                .or_else(|| self.mouse_binding(button, 1, event.modifiers, false, scrollbar))
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
    fn point_over_scrollbar(&self, point: Point) -> bool {
        self.view.hit_scrollbar(Point {
            x: point.x - self.content.x,
            y: point.y - self.content.y,
        })
    }

    fn mouse_binding(
        &mut self,
        button: usize,
        count: usize,
        modifiers: Modifiers,
        press: bool,
        scrollbar: bool,
    ) -> Option<(String, Value)> {
        let bindings = if scrollbar {
            &self.workspace.scrollbar_mousemap
        } else {
            &self.workspace.mousemap
        };
        if bindings.is_empty() {
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
        let bindings = if scrollbar {
            &self.workspace.scrollbar_mousemap
        } else {
            &self.workspace.mousemap
        };
        bindings.iter().rev().find_map(|binding| {
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
                let editor = self.workspace.editor();
                let value = if key == "word_wrap" {
                    crate::view::motion_layout_options(editor).width != 0
                } else {
                    editor.settings.bool(key, false)
                };
                if key == "save_all_on_build" {
                    self.workspace.set_user_preference(key, json!(!value))?;
                } else {
                    self.workspace
                        .editor_mut()
                        .set_view_setting(key, json!(!value));
                }
            }
            "set_setting" => {
                // https://www.sublimetext.com/docs/settings.html: per-view settings
                // are the highest ordinary preference layer.
                let key = args["setting"]
                    .as_str()
                    .ok_or("set_setting needs setting")?;
                let value = args.get("value").ok_or("set_setting needs value")?;
                if key == "save_all_on_build" {
                    self.workspace.set_user_preference(key, value.clone())?;
                } else {
                    self.workspace
                        .editor_mut()
                        .set_view_setting(key, value.clone());
                }
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
