//! Find/overlay transitions and status presentation shared by native and headless routes.
use super::*;
use std::time::Duration;

impl Panels {
    pub(super) fn panel_buttons(panel: Panel) -> &'static [(&'static str, &'static str)] {
        match panel {
            Panel::Find | Panel::Incremental => &[
                ("Find", "find_next"),
                ("Find Prev", "find_prev"),
                ("Find All", "find_all"),
            ],
            Panel::Replace => &[
                ("Find", "find_next"),
                ("Replace", "replace_next"),
                ("Replace All", "replace_all"),
            ],
            Panel::Files => &[
                ("Find", "find_in_files_start"),
                ("Replace", "replace_in_files"),
            ],
            _ => &[],
        }
    }
    pub(super) fn panel_message_clip(&self, panel: Panel) -> Rect {
        Rect {
            width: (self.panel_bounds.width
                - (Self::panel_buttons(panel).len() as f64 * 104. + 8.) * self.ui_scale)
                .max(0.),
            ..self.panel_bounds
        }
    }
    pub(crate) fn set_message(&mut self, message: String) {
        self.message = message;
        self.message_seen.clone_from(&self.message);
        self.message_until = Some(Instant::now() + Duration::from_secs(5));
    }
    /// Next native wake for feedback expiry or a bounded, slow hover-scope retry.
    pub fn message_deadline(&self) -> Option<Instant> {
        let hover = (!self.hover_term.is_empty()
            && !self.hover_requested
            && self.hover_since.elapsed() < Duration::from_secs(2))
        .then_some(self.hover_retry)
        .flatten();
        self.message_until.into_iter().chain(hover).min()
    }
    pub(super) fn expire_message(&mut self, now: Instant) -> bool {
        if self.message != self.message_seen {
            self.message_seen.clone_from(&self.message);
            self.message_until = (!self.message.is_empty()).then_some(now + Duration::from_secs(5));
        }
        if self.message_until.is_some_and(|until| now >= until) {
            self.message.clear();
            self.message_seen.clear();
            self.message_until = None;
            return true;
        }
        false
    }
    pub(super) fn cancel_incremental(&mut self, workspace: &mut Workspace) {
        let Some((origin, buffer, version)) = self.incremental_origin.take() else {
            return;
        };
        let _navigation = crate::editor::navigation::Guard::suppress(&workspace.jump_history);
        self.find_jump_origin = None;

        if let Some(index) = workspace
            .tabs
            .iter()
            .position(|t| t.sheet == origin.sheet && t.editor.buffer.id() == buffer)
        {
            workspace.select(index);
            let editor = workspace.editor_mut();
            let regions: Option<Vec<_>> = origin
                .selection
                .iter()
                .map(|r| editor.buffer.transform_region_from(r, version))
                .collect();
            if let Some(mut regions) = regions {
                let primary = regions.remove(origin.selection.primary_index());
                regions.push(primary);
                editor.set_selection(Selection::from_regions(regions));
            }
            workspace.tabs[index].scroll = origin.scroll;
            self.restore_scroll = Some(origin.scroll);
            self.preserve_scroll = true;
            self.reveal = false;
        }
        workspace
            .jump_history
            .borrow_mut()
            .reset_current(crate::intelligence::capture(workspace));
    }
    pub(super) fn preview_find(&mut self, workspace: &mut Workspace, origin: usize) {
        let _navigation = crate::editor::navigation::Guard::suppress(&workspace.jump_history);
        if self.in_selection {
            return;
        }
        let hit = self
            .highlights
            .iter()
            .find(|r| r.start >= origin)
            .or_else(|| self.options.wrap.then(|| self.highlights.first()).flatten())
            .cloned();
        if let Some(hit) = hit {
            workspace
                .editor_mut()
                .set_selection(Selection::from_regions([Region::from(hit)]));
            self.reveal = true;
        } else if !self.query.value().is_empty() {
            self.set_message(format!("Unable to find {}", self.query.value()));
        }
        workspace
            .jump_history
            .borrow_mut()
            .reset_current(crate::intelligence::capture(workspace));
    }
    /// A cancelled preview restores its exact viewport; skip the event's generic caret reveal.
    pub fn should_reveal(&mut self, requested: bool) -> bool {
        let preserve = std::mem::take(&mut self.preserve_scroll);
        requested && !preserve
    }
    pub(super) fn walk_history(
        &mut self,
        workspace: &mut Workspace,
        backwards: bool,
        viewport: Range<usize>,
    ) {
        let field = self.focus_input;
        if field > 1 {
            return;
        }
        let history = if field == 0 {
            &self.find_history
        } else {
            &self.replace_history
        };
        if history.is_empty() {
            return;
        }
        let cursor = self.history_cursor[field];
        if cursor.is_none() {
            if !backwards {
                return;
            }
            self.history_draft[field] = self.input().expect("focused field").value();
        }
        let next = if backwards {
            Some(cursor.map_or(history.len() - 1, |i| i.saturating_sub(1)))
        } else {
            cursor.and_then(|i| (i + 1 < history.len()).then_some(i + 1))
        };
        let value = next.map_or_else(|| self.history_draft[field].clone(), |i| history[i].clone());
        self.input_mut().expect("focused field").set(&value);
        self.input_changed(workspace, viewport);
        self.history_cursor[field] = next;
    }
    /// Makes builds the navigation source while any hidden file search continues.
    pub fn select_build_results(&mut self) {
        self.search_results_active = false;
    }
    /// Whether F4 can navigate the last search's match lines (context rows are skipped).
    pub fn has_search_results(&self) -> bool {
        self.result_view
            .rows
            .iter()
            .any(|r| matches!(r, files::Row::Line(line) if !line.highlights.is_empty()))
    }
    /// Walks search results through the same jump path used by Enter and the mouse.
    pub fn navigate_result(
        &mut self,
        workspace: &mut Workspace,
        forward: bool,
    ) -> Result<(), String> {
        let hits: Vec<_> = self
            .result_view
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                matches!(r, files::Row::Line(line) if !line.highlights.is_empty()).then_some(i)
            })
            .collect();
        let next = if forward {
            hits.iter()
                .find(|i| self.result_cursor.is_none_or(|cursor| **i > cursor))
                .or_else(|| hits.first())
        } else {
            hits.iter()
                .rev()
                .find(|i| self.result_cursor.is_none_or(|cursor| **i < cursor))
                .or_else(|| hits.last())
        };
        if let Some(&index) = next {
            self.result_selected = index;
            self.result_scroll.reveal(index);
            self.jump_result(workspace)?;
        }
        Ok(())
    }
    pub(super) fn overlay_footer(&self) -> &'static str {
        match self.overlay {
            Some(Overlay::Palette) => "Enter run · Esc cancel",
            Some(Overlay::Appearance) => "Arrows preview · Enter save · Esc cancel",
            Some(Overlay::Syntax | Overlay::Choices) => "Enter select · Esc cancel",
            Some(Overlay::ProjectSymbols | Overlay::Locations) => "Enter jump · Esc cancel",
            Some(Overlay::Goto) => "Enter open · Cmd/Ctrl+Enter split · Esc cancel",
            None => "",
        }
    }
    /// Whether the native command dispatcher is validating the accepted palette row.
    pub fn palette_accept_pending(&self, name: &str, args: &Value) -> bool {
        self.palette_accept
            .as_ref()
            .is_some_and(|i| i.command == name && &i.args == args)
    }
    /// Completes acceptance only after the app rechecks this row against the current context.
    pub fn finish_palette_accept(
        &mut self,
        workspace: &mut Workspace,
        name: &str,
        args: &Value,
        enabled: bool,
    ) {
        if !self.palette_accept_pending(name, args) {
            return;
        }
        let invocation = self.palette_accept.take().expect("pending palette row");
        if !enabled || self.overlay != Some(Overlay::Palette) {
            return;
        }
        self.uses += 1;
        self.palette_recent.insert(
            format!("{}:{}", invocation.command, invocation.args),
            self.uses,
        );
        self.overlay = None;
        if let Some(source) = self.preview.take()
            && let Some(tab) = workspace.tabs.iter_mut().find(|t| t.sheet == source.sheet)
        {
            tab.transient = source.transient;
        }
        self.prepared.clear();
        self.focus.show(Surface::Overlay, false);
    }
    /// Supplies the native app's `command_enabled` results without a second enablement policy.
    pub fn set_palette_enabled(&mut self, enabled: Vec<bool>) {
        if self.palette_enabled != enabled {
            self.palette_enabled = enabled;
            if self.overlay == Some(Overlay::Palette) {
                self.prepared.clear();
            }
        }
    }
    pub(super) fn palette_command_enabled(&self, workspace: &Workspace, index: usize) -> bool {
        self.palette_enabled.get(index).copied().unwrap_or_else(|| {
            let command = &workspace.palette[index].invocation;
            workspace.editor().registry.is_enabled(
                &command.command,
                &command.args,
                workspace.editor(),
            )
        })
    }
    pub(super) fn indentation_rect(&self) -> Rect {
        Rect {
            x: (self.size.width - 300. * self.ui_scale).max(0.),
            y: self.size.height - STATUS * self.ui_scale,
            width: 120. * self.ui_scale,
            height: STATUS * self.ui_scale,
        }
    }
    pub(super) fn indentation_choices(&mut self, workspace: &mut Workspace) {
        let mut items = Vec::new();
        for (caption, args) in [
            ("Indent Using Spaces".into(), json!({"spaces":true})),
            ("Indent Using Tabs".into(), json!({"spaces":false})),
            (
                "Convert Indentation to Spaces".into(),
                json!({"spaces":true,"convert":true}),
            ),
            (
                "Convert Indentation to Tabs".into(),
                json!({"spaces":false,"convert":true}),
            ),
        ]
        .into_iter()
        .chain(
            [1, 2, 3, 4, 5, 6, 7, 8]
                .into_iter()
                .map(|width| (format!("Tab Width: {width}"), json!({"width":width}))),
        ) {
            items.push(vel_config::ui::PaletteItem {
                caption,
                invocation: Invocation {
                    command: "set_indentation".into(),
                    args,
                },
            });
        }
        self.choices(workspace, items);
    }
    pub(super) fn set_indentation(
        &mut self,
        workspace: &mut Workspace,
        args: &Value,
    ) -> Result<(), String> {
        let editor = workspace.editor_mut();
        if let Some(spaces) = args["spaces"].as_bool() {
            editor.settings.set(
                vel_config::settings::Layer::View,
                "translate_tabs_to_spaces",
                json!(spaces),
            );
        }
        if let Some(width) = args["width"].as_u64().filter(|w| (1..=32).contains(w)) {
            editor
                .settings
                .set(vel_config::settings::Layer::View, "tab_size", json!(width));
        }
        editor.sync();
        if args["convert"].as_bool() == Some(true) && !editor.read_only {
            let tab = editor.preferences.tab_size.max(1);
            let spaces = editor.preferences.translate_tabs_to_spaces;
            let mut offset = 0;
            let mut edits = Vec::new();
            for line in editor.text.lines() {
                let line = line.to_string();
                let len = line
                    .bytes()
                    .take_while(|c| matches!(c, b' ' | b'\t'))
                    .count();
                let width = line[..len].bytes().fold(0, |col, c| {
                    if c == b'\t' {
                        col + tab - col % tab
                    } else {
                        col + 1
                    }
                });
                let replacement = if spaces {
                    " ".repeat(width)
                } else {
                    format!("{}{}", "\t".repeat(width / tab), " ".repeat(width % tab))
                };
                if line[..len] != replacement {
                    edits.push((offset..offset + len, replacement));
                }
                offset += line.len() + 1;
            }
            editor.apply_edits(edits, None, vel_buffer::EditKind::Other);
            editor.sync();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_text::Text;

    fn setup(source: &str) -> (Workspace, Panels) {
        let workspace = Workspace::new(Text::from(source), None, None, false).unwrap();
        let panels = Panels::new(&workspace, false);
        (workspace, panels)
    }
    fn action(p: &mut Panels, w: &mut Workspace, name: &str, args: Value) {
        let end = w.editor().text.len_bytes();
        assert!(p.action(w, name, &args, 0..end).unwrap(), "{name}");
    }
    fn key(p: &mut Panels, w: &mut Workspace, key: &str) {
        let end = w.editor().text.len_bytes();
        if let vel_config::keymap::Resolution::Command(invocation) =
            p.resolve_key(w, &Key::parse(key).unwrap(), 0..end).unwrap()
            && !p
                .action(w, &invocation.command, &invocation.args, 0..end)
                .unwrap()
        {
            w.editor_mut()
                .run(&invocation.command, &invocation.args)
                .unwrap();
        }
    }
    fn wait(p: &mut Panels, w: &mut Workspace, done: impl Fn(&Panels) -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !done(p) {
            let end = w.editor().text.len_bytes();
            p.poll(w, 0..end);
            assert!(Instant::now() < until, "panel worker did not finish");
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
    fn type_query(p: &mut Panels, w: &mut Workspace, text: &str, viewport: Range<usize>) {
        p.query.set(text);
        p.input_changed(w, viewport);
        wait(p, w, |p| p.all_matches_ready);
    }
    fn size() -> Size {
        Size {
            width: 1200.,
            height: 800.,
        }
    }

    #[test]
    fn early_enter_never_accepts_candidates_from_another_target() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("old-file.txt");
        std::fs::write(&file, "old file").unwrap();
        for target in ["@", "#word"] {
            let (mut w, mut p) = setup("word\nsecond line");
            p.file_paths = Arc::new(vec![file.clone()]);
            p.file_candidates = Arc::new(vec![fuzzy::Candidate::new(file.to_string_lossy())]);
            action(&mut p, &mut w, "show_overlay", json!({"text":"old-file"}));
            wait(&mut p, &mut w, |p| !p.quick_pending);
            assert_eq!(p.quick.items.len(), 1);
            key(&mut p, &mut w, "escape");
            assert_eq!(w.active, 0);
            action(&mut p, &mut w, "show_overlay", json!({"text":target}));
            assert!(p.quick.items.is_empty());
            key(&mut p, &mut w, "enter");
            assert_eq!(p.overlay, Some(Overlay::Goto), "{target}");
            assert_eq!(w.active, 0);
            assert_eq!(w.editor().caret, 0);
            assert!(w.tabs[w.active].path.is_none());
        }
    }
    #[test]
    fn target_switch_clears_rows_and_an_unavailable_symbol_source_is_not_ranked() {
        let (mut w, mut p) = setup(&"a".repeat(600));
        action(&mut p, &mut w, "show_overlay", json!({"text":"@"}));
        // A previously prepared symbol list must not survive a buffer/source change.
        p.set_local(vec![("previous".into(), 500)], "");
        wait(&mut p, &mut w, |p| !p.quick_pending);
        w.new_file().unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":"b".repeat(600)}))
            .unwrap();
        w.editor_mut().set_caret(0, false);
        p.query_quick(&mut w);
        assert!(!p.quick_ready);
        p.preview_current(&mut w).unwrap();
        assert_eq!(w.editor().caret, 0);
        key(&mut p, &mut w, "enter");
        assert_eq!(w.editor().caret, 0);
        assert_eq!(p.overlay, Some(Overlay::Goto));
        p.quick_input.set(":1");
        p.query_quick(&mut w);
        assert!(p.quick.items.is_empty());
        assert!(p.local_positions.is_empty());
        key(&mut p, &mut w, "enter");
        assert!(p.overlay.is_none());
    }
    #[test]
    fn every_new_palette_query_selects_its_best_hit_including_early_enter() {
        for early in [true, false] {
            let (mut w, mut p) = setup("text");
            w.palette = [("New File", "file"), ("New Window", "window")]
                .into_iter()
                .map(|(caption, item)| vel_config::ui::PaletteItem {
                    caption: caption.into(),
                    invocation: Invocation {
                        command: "new_file".into(),
                        args: json!({"item":item}),
                    },
                })
                .collect();
            action(
                &mut p,
                &mut w,
                "show_overlay",
                json!({"overlay":"command_palette","text":"n"}),
            );
            wait(&mut p, &mut w, |p| !p.quick_pending);
            p.quick.navigate(1);
            assert_eq!(p.quick.items[p.quick.selected].caption, "New Window");
            p.quick_input.set("ne");
            p.query_quick(&mut w);
            if !early {
                wait(&mut p, &mut w, |p| !p.quick_pending);
            }
            let invocation = p.accept(&mut w, false, 0..1).unwrap().unwrap();
            assert_eq!(invocation.args["item"], "file");
            p.finish_palette_accept(&mut w, &invocation.command, &invocation.args, true);
            assert!(p.overlay.is_none());
        }
    }
    #[test]
    fn query_selection_clipboard_and_cursor_commands_keep_the_current_match() {
        let (mut w, mut p) = setup("foo foo foo foo");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find","text":"foo"}),
        );
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        for _ in 0..3 {
            key(&mut p, &mut w, "enter");
        }
        assert_eq!(w.editor().selection(), 8..11);
        for command in [
            Command::Left(false),
            Command::Right(false),
            Command::SelectAll,
            Command::Copy,
        ] {
            assert!(p.command(&mut w, command, 0..15).unwrap().0);
            p.poll(&mut w, 0..15);
            assert_eq!(w.editor().selection(), 8..11, "{command:?}");
            assert!(p.pending_navigation.is_none());
            assert!(p.all_matches_ready);
        }
        p.query
            .run("move", &json!({"by":"characters","forward":false}))
            .unwrap();
        p.edited(&mut w, 0..15);
        assert_eq!(w.editor().selection(), 8..11);
        action(
            &mut p,
            &mut w,
            "toggle_find_option",
            json!({"option":"case_sensitive"}),
        );
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(w.editor().selection(), 0..3);
    }
    #[test]
    fn in_selection_typing_options_and_reopen_preserve_the_scope_and_primary() {
        let (mut w, mut p) = setup("foo\nbar\nfoo\nbaz");
        let original = Selection::from_regions([Region::new(8, 15), Region::new(0, 7)]);
        w.editor_mut().set_selection(original.clone());
        action(&mut p, &mut w, "show_panel", json!({"panel":"find"}));
        action(
            &mut p,
            &mut w,
            "toggle_find_option",
            json!({"option":"in_selection"}),
        );
        let scope = p.selection_scope.clone();
        type_query(&mut p, &mut w, "foo", 0..15);
        action(
            &mut p,
            &mut w,
            "toggle_find_option",
            json!({"option":"case_sensitive"}),
        );
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(w.editor().buffer.view(w.editor().id).sel(), &original);
        p.panel_focused = false;
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find","toggle":true}),
        );
        assert_eq!(p.selection_scope, scope);
        assert_eq!(w.editor().buffer.view(w.editor().id).sel(), &original);
    }
    #[test]
    fn selected_text_previews_from_the_primary_begin_in_either_direction() {
        for region in [Region::new(4, 7), Region::new(7, 4)] {
            let (mut w, mut p) = setup("foo foo foo");
            w.editor_mut()
                .set_selection(Selection::from_regions([Region::new(8, 11), region]));
            action(&mut p, &mut w, "show_panel", json!({"panel":"find"}));
            assert_eq!(p.query.value(), "foo");
            type_query(&mut p, &mut w, "fo", 0..11);
            assert_eq!(w.editor().selection(), 4..6);
            type_query(&mut p, &mut w, "foo", 0..11);
            assert_eq!(w.editor().selection(), 4..7);
        }
    }
    #[test]
    fn unchanged_invalid_regex_reports_again_after_feedback_expires() {
        let (mut w, mut p) = setup("foo foo");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"replace","text":"["}),
        );
        p.focus_input = 1; // Options still update while the Replace input has focus.
        action(
            &mut p,
            &mut w,
            "toggle_find_option",
            json!({"option":"regex"}),
        );
        let error = p.message.clone();
        assert!(error.contains("unclosed character class"));
        for command in [
            "find_next",
            "find_prev",
            "find_all",
            "replace_next",
            "replace_all",
        ] {
            p.expire_message(Instant::now() + Duration::from_secs(6));
            assert!(p.message.is_empty());
            action(&mut p, &mut w, command, json!({}));
            assert_eq!(p.message, error, "{command}");
            assert_eq!(w.editor().text.to_string(), "foo foo");
        }
        for command in ["enter", "shift+enter"] {
            p.expire_message(Instant::now() + Duration::from_secs(6));
            key(&mut p, &mut w, command);
            assert_eq!(p.message, error);
        }
        p.focus_input = 0;
        type_query(&mut p, &mut w, "foo", 0..7);
        assert!(p.find_error.is_none());
    }
    #[test]
    fn message_clip_stops_before_every_panels_first_button_at_each_scale() {
        let (mut w, mut p) = setup("text");
        for scale in [0.75, 1., 1.5, 2.] {
            p.ui_scale = scale;
            for name in ["find", "incremental_find", "replace", "find_in_files"] {
                action(&mut p, &mut w, "show_panel", json!({"panel":name}));
                p.layout(size());
                p.sync_widgets(&w);
                let message = p.panel_message_clip(p.panel.unwrap());
                let button = p.tree.nodes[&50].rect;
                assert!(message.x + message.width <= button.x, "{name} at {scale}");
                assert!(message.width > 0.);
            }
        }
    }
    #[test]
    fn file_search_survives_other_panels_and_build_source_switching() {
        let (mut w, mut p) = setup(&"foo\n".repeat(500));
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find_in_files","text":"foo"}),
        );
        p.where_input.set("<open files>");
        action(&mut p, &mut w, "find_in_files_start", json!({}));
        assert!(p.file_task.is_some());
        action(&mut p, &mut w, "show_panel", json!({"panel":"find"}));
        assert!(p.file_task.is_some());
        action(
            &mut p,
            &mut w,
            "show_input_panel",
            json!({"caption":"/","initial_text":""}),
        );
        assert!(p.file_task.is_some());
        action(&mut p, &mut w, "hide_panel", json!({}));
        assert!(p.file_task.is_some());
        p.select_build_results();
        assert!(p.file_task.is_some());
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!(p.result_status, "500 matches in 1 file");
        assert!(!p.search_results_active);
        p.select_build_results();
        assert!(p.file_complete);
        p.query.set("missing");
        action(&mut p, &mut w, "find_in_files_start", json!({}));
        assert!(p.search_results_active);
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!(p.result_status, "0 matches in 0 files");
        assert!(!p.has_search_results());
    }

    #[test]
    fn replace_enter_and_shift_enter_find_without_editing() {
        let (mut w, mut p) = setup("cat cat cat");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"replace","text":"cat"}),
        );
        p.replace.set("dog");
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        key(&mut p, &mut w, "enter");
        assert_eq!(w.editor().selection(), 0..3);
        key(&mut p, &mut w, "enter");
        assert_eq!(w.editor().selection(), 4..7);
        key(&mut p, &mut w, "shift+enter");
        assert_eq!(w.editor().selection(), 0..3);
        assert_eq!(w.editor().text.to_string(), "cat cat cat");
        key(&mut p, &mut w, "super+alt+e");
        assert_eq!(w.editor().text.to_string(), "dog cat cat");
        assert_eq!(p.panel, Some(Panel::Replace));
    }
    #[test]
    fn incremental_enter_commits_and_cancel_restores_selection_and_scroll() {
        let (mut w, mut p) = setup("cat\nx\ncat\ny\ncat");
        w.editor_mut().set_selection(Selection::from_regions([
            Region::new(6, 7),
            Region::caret(3),
        ]));
        let original = w.editor().buffer.view(w.editor().id).sel().clone();
        w.tabs[w.active].scroll = (12., 240.);
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"incremental_find"}),
        );
        type_query(&mut p, &mut w, "cat", 0..3);
        assert_eq!(w.editor().selection(), 6..9);
        assert!(p.reveal);
        assert!(p.find_history.is_empty());
        action(&mut p, &mut w, "hide_panel", json!({"cancel":true}));
        assert_eq!(
            w.editor()
                .buffer
                .view(w.editor().id)
                .sel()
                .iter()
                .collect::<Vec<_>>(),
            original.iter().collect::<Vec<_>>()
        );
        assert_eq!(p.restore_scroll, Some((12., 240.)));
        assert!(!p.reveal);
        assert!(!p.should_reveal(true));
        assert!(p.should_reveal(true));
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"incremental_find","text":"cat"}),
        );
        type_query(&mut p, &mut w, "c", 0..3);
        type_query(&mut p, &mut w, "cat", 0..3);
        assert_eq!(w.editor().selection(), 6..9);
        key(&mut p, &mut w, "enter");
        wait(&mut p, &mut w, |p| p.panel.is_none());
        assert_eq!(w.editor().selection(), 12..15);
        assert_eq!(p.find_history, ["cat"]);
    }
    #[test]
    fn incremental_escape_invalidates_pending_preview() {
        let (mut w, mut p) = setup("cat cat");
        w.editor_mut().set_caret(4, false);
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"incremental_find"}),
        );
        p.query.set("cat");
        p.input_changed(&mut w, 0..3);
        key(&mut p, &mut w, "escape");
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(w.editor().selection(), 4..4);
        assert!(p.panel.is_none());
    }
    #[test]
    fn typing_find_uses_opening_caret_wraps_and_requests_reveal() {
        for panel in ["find", "incremental_find"] {
            let (mut w, mut p) = setup("cat\ncat\ncat");
            w.editor_mut().set_caret(5, false);
            action(&mut p, &mut w, "show_panel", json!({"panel":panel}));
            type_query(&mut p, &mut w, "c", 0..3);
            assert_eq!(w.editor().selection(), 8..9);
            type_query(&mut p, &mut w, "cat", 0..3);
            assert_eq!(w.editor().selection(), 8..11);
            assert!(p.reveal);
            assert!(p.find_history.is_empty());
            action(&mut p, &mut w, "hide_panel", json!({}));
            w.editor_mut().set_caret(11, false);
            action(&mut p, &mut w, "show_panel", json!({"panel":panel}));
            type_query(&mut p, &mut w, "c", 8..11);
            type_query(&mut p, &mut w, "cat", 8..11);
            assert_eq!(w.editor().selection(), 0..3);
        }
    }
    #[test]
    fn reopen_selects_fields_and_editor_focus_shortcut_focuses_before_toggling() {
        let (mut w, mut p) = setup("cat cat");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find","text":"old"}),
        );
        p.panel_focused = false;
        key(&mut p, &mut w, "super+f");
        assert_eq!(p.panel, Some(Panel::Find));
        assert!(p.panel_focused);
        assert_eq!(
            p.query
                .buffer
                .view(p.query.view)
                .sel()
                .primary()
                .unwrap()
                .to_range(),
            0..3
        );
        p.text_input(
            &mut w,
            TextInput::Insert {
                text: "cat".into(),
                replacement: None,
            },
            0..7,
        );
        assert_eq!(p.query.value(), "cat");
        key(&mut p, &mut w, "super+f");
        assert!(p.panel.is_none());
        p.replace.set("replacement");
        key(&mut p, &mut w, "super+alt+f");
        assert_eq!(
            p.replace
                .buffer
                .view(p.replace.view)
                .sel()
                .primary()
                .unwrap()
                .to_range(),
            0..11
        );
    }
    #[test]
    fn histories_record_only_commits_and_arrows_restore_drafts() {
        let (mut w, mut p) = setup("cat cat");
        w.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "close_find_after_replace_all",
            json!(false),
        );
        action(&mut p, &mut w, "show_panel", json!({"panel":"replace"}));
        for query in ["c", "ca"] {
            type_query(&mut p, &mut w, query, 0..7);
            let mut window = vel_session::Window::default();
            p.capture_session(&mut window);
            assert!(window.find_history.is_empty());
        }
        p.replace.set("d");
        type_query(&mut p, &mut w, "cat", 0..7);
        action(&mut p, &mut w, "find_next", json!({}));
        assert_eq!(p.find_history, ["cat"]);
        assert!(p.replace_history.is_empty());
        p.replace.set("dog");
        action(&mut p, &mut w, "replace_all", json!({}));
        assert_eq!(p.replace_history, ["dog"]);
        p.find_history.insert(0, "older".into());
        p.query.set("draft");
        key(&mut p, &mut w, "up");
        assert_eq!(p.query.value(), "cat");
        key(&mut p, &mut w, "up");
        assert_eq!(p.query.value(), "older");
        key(&mut p, &mut w, "down");
        assert_eq!(p.query.value(), "cat");
        key(&mut p, &mut w, "down");
        assert_eq!(p.query.value(), "draft");
        key(&mut p, &mut w, "tab");
        p.replace.set("replacement draft");
        p.command(&mut w, Command::Up(false), 0..7).unwrap();
        assert_eq!(p.replace.value(), "dog");
        p.command(&mut w, Command::Down(false), 0..7).unwrap();
        assert_eq!(p.replace.value(), "replacement draft");
    }
    #[test]
    fn feedback_survives_poll_and_expires_without_regex_modal_errors() {
        let (mut w, mut p) = setup("cat cat");
        w.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "close_find_after_replace_all",
            json!(false),
        );
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"replace","text":"cat"}),
        );
        p.replace.set("dog");
        action(&mut p, &mut w, "replace_all", json!({}));
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(p.message, "Replaced 2 occurrences");
        action(&mut p, &mut w, "find_next", json!({}));
        assert_eq!(p.message, "Unable to find cat");
        p.options.regex = true;
        p.query.set("[");
        action(&mut p, &mut w, "replace_all", json!({}));
        assert!(!p.message.contains("(?m)"));
        assert!(p.message.contains("unclosed"));
        action(&mut p, &mut w, "find_in_files_start", json!({}));
        assert_eq!(p.panel, Some(Panel::Replace));
        assert!(p.file_task.is_none());
        assert!(p.expire_message(Instant::now() + Duration::from_secs(6)));
        assert!(p.message.is_empty());
    }
    #[test]
    fn replacing_the_final_match_retains_feedback_after_navigation_completes() {
        let (mut w, mut p) = setup("cat");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"replace","text":"cat"}),
        );
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        action(&mut p, &mut w, "find_next", json!({}));
        p.replace.set("dog");
        action(&mut p, &mut w, "replace_next", json!({}));
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(p.message, "Replaced 1 occurrence");
        assert_eq!(w.editor().text.to_string(), "dog");
    }
    #[test]
    fn panel_buttons_and_files_alt_enter_do_not_search_the_buffer() {
        let (mut w, mut p) = setup("cat cat");
        for (panel, labels) in [
            ("find", vec!["Find", "Find Prev", "Find All"]),
            ("replace", vec!["Find", "Replace", "Replace All"]),
            ("find_in_files", vec!["Find", "Replace"]),
        ] {
            action(
                &mut p,
                &mut w,
                "show_panel",
                json!({"panel":panel,"text":"cat"}),
            );
            p.layout(size());
            p.paint(&w, &mut Scene::default());
            assert_eq!(
                (50..50 + labels.len() as u64)
                    .map(|i| p.tree.nodes[&i].label.as_str())
                    .collect::<Vec<_>>(),
                labels
            );
        }
        let before = w.editor().buffer.view(w.editor().id).sel().clone();
        key(&mut p, &mut w, "alt+enter");
        assert_eq!(w.editor().buffer.view(w.editor().id).sel(), &before);
        assert_eq!(p.panel, Some(Panel::Files));
        let rect = p.tree.nodes[&50].rect;
        p.where_input.set("<current file>");
        p.mouse(
            &mut w,
            Point {
                x: rect.x + 4.,
                y: rect.y + 4.,
            },
            true,
            false,
            false,
            0..7,
        )
        .unwrap();
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!(p.result_status, "2 matches in 1 file");
    }
    #[test]
    fn file_progress_summary_navigation_and_close_cancel() {
        let (mut w, mut p) = setup("cat cat\ncontext\ncat");
        w.new_file().unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":"no match"}))
            .unwrap();
        w.select(0);
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find_in_files","text":"cat"}),
        );
        p.where_input.set("<open files>");
        key(&mut p, &mut w, "enter");
        assert_eq!(p.result_status, "Searching 0 files…");
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!(p.files_searched, 2);
        assert_eq!(p.result_status, "3 matches in 1 file");
        assert_eq!(p.message, p.result_status);
        assert!(p.has_search_results());
        p.navigate_result(&mut w, true).unwrap();
        assert_eq!(w.editor().caret, 0);
        p.navigate_result(&mut w, true).unwrap();
        assert_eq!(w.editor().caret, 16);
        p.navigate_result(&mut w, false).unwrap();
        assert_eq!(w.editor().caret, 0);
        action(&mut p, &mut w, "find_in_files_start", json!({}));
        key(&mut p, &mut w, "escape");
        assert!(p.file_task.is_none());
        assert!(!p.file_complete);
        assert!(p.message.contains("cancelled"));
        p.poll(&mut w, 0..19);
        assert!(p.panel.is_none());
    }
    #[test]
    fn current_file_scope_uses_the_active_unsaved_buffer() {
        let (mut w, mut p) = setup("cat");
        w.new_file().unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":"cat cat"}))
            .unwrap();
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find_in_files","text":"cat"}),
        );
        p.where_input.set("<current file>");
        key(&mut p, &mut w, "enter");
        wait(&mut p, &mut w, |p| p.file_complete);
        assert_eq!((p.file_matches, p.files_searched), (2, 1));
        p.navigate_result(&mut w, true).unwrap();
        assert_eq!(w.active, 1);
    }
    #[test]
    fn early_enter_opens_the_best_current_fuzzy_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let exact = dir.path().join("readme");
        let other = dir.path().join("really-distant-name");
        std::fs::write(&exact, "exact").unwrap();
        std::fs::write(&other, "other").unwrap();
        let (mut w, mut p) = setup("original");
        p.file_paths = Arc::new(vec![other.clone(), exact.clone()]);
        p.file_candidates = Arc::new(
            p.file_paths
                .iter()
                .map(|p| fuzzy::Candidate::new(p.to_string_lossy()))
                .collect(),
        );
        action(&mut p, &mut w, "show_overlay", json!({"text":"readme"}));
        assert!(p.quick_pending);
        key(&mut p, &mut w, "enter");
        assert!(p.overlay.is_none());
        assert_eq!(
            w.tabs[w.active].path.as_ref(),
            Some(&exact.canonicalize().unwrap())
        );
        assert!(!w.tabs[w.active].transient);
    }
    #[test]
    fn overlay_mouse_accepts_single_click_and_outside_click_restores_preview() {
        let (mut w, mut p) = setup("original");
        p.choices(
            &mut w,
            vec![vel_config::ui::PaletteItem {
                caption: "Indent Using Tabs".into(),
                invocation: Invocation {
                    command: "set_indentation".into(),
                    args: json!({"spaces":false}),
                },
            }],
        );
        wait(&mut p, &mut w, |p| !p.quick_pending);
        p.layout(size());
        p.mouse(
            &mut w,
            Point {
                x: p.quick_bounds.x + 20.,
                y: p.quick_bounds.y + 50.,
            },
            true,
            false,
            false,
            0..8,
        )
        .unwrap();
        assert!(p.overlay.is_none());
        assert_eq!(p.pending_command.take().unwrap().command, "set_indentation");
        w.editor_mut().set_caret(4, false);
        action(&mut p, &mut w, "show_overlay", json!({"text":":1:2"}));
        assert_eq!(w.editor().caret, 1);
        p.layout(size());
        p.mouse(&mut w, Point { x: 1., y: 1. }, true, false, false, 0..8)
            .unwrap();
        assert!(p.overlay.is_none());
        assert_eq!(w.editor().caret, 4);
    }
    #[test]
    fn overlay_footers_and_scaled_geometry_are_surface_specific() {
        let (w, mut p) = setup("text");
        p.message = "stale find message".into();
        for overlay in [
            Overlay::Goto,
            Overlay::Palette,
            Overlay::Syntax,
            Overlay::Choices,
            Overlay::ProjectSymbols,
            Overlay::Locations,
        ] {
            p.overlay = Some(overlay);
            assert!(!p.overlay_footer().contains("stale"));
            assert_eq!(
                p.overlay_footer().contains("split"),
                overlay == Overlay::Goto
            );
        }
        for scale in [0.75, 1., 1.5, 2.] {
            p.ui_scale = scale;
            p.layout(size());
            assert!((p.quick_bounds.x + p.quick_bounds.width / 2. - 600.).abs() < 0.001);
            p.side_rows = vec![SideRow {
                path: "/root/child".into(),
                folder: 0,
                depth: 1,
                directory: true,
            }];
            let point = Point {
                x: 10.,
                y: p.side_bounds.y + (w.tabs.len() + 2) as f64 * 24. * scale + 10. * scale,
            };
            assert_eq!(
                p.review_side_path(&w, point),
                Some((PathBuf::from("/root/child"), true))
            );
        }
    }
    #[test]
    fn indentation_status_click_switches_width_and_converts_in_one_undo() {
        let (mut w, mut p) = setup("\t  code\n  \tmore\n");
        p.layout(size());
        let rect = p.indentation_rect();
        p.mouse(
            &mut w,
            Point {
                x: rect.x + 5.,
                y: rect.y + 5.,
            },
            true,
            false,
            false,
            0..18,
        )
        .unwrap();
        assert_eq!(p.overlay, Some(Overlay::Choices));
        assert!(p.choice_items.iter().any(|i| i.caption == "Tab Width: 8"));
        action(
            &mut p,
            &mut w,
            "set_indentation",
            json!({"spaces":true,"width":4,"convert":true}),
        );
        assert_eq!(w.editor().text.to_string(), "      code\n    more\n");
        w.editor_mut().run("undo", &json!({})).unwrap();
        assert_eq!(w.editor().text.to_string(), "\t  code\n  \tmore\n");
        action(
            &mut p,
            &mut w,
            "set_indentation",
            json!({"spaces":false,"width":2,"convert":true}),
        );
        assert_eq!(w.editor().text.to_string(), "\t\tcode\n\t\tmore\n");
    }
    #[test]
    fn sidebar_focus_and_escape_preserve_visible_find_panel() {
        let (mut w, mut p) = setup("cat");
        action(
            &mut p,
            &mut w,
            "show_panel",
            json!({"panel":"find","text":"cat"}),
        );
        action(&mut p, &mut w, "focus_side_bar", json!({}));
        assert_eq!(p.panel, Some(Panel::Find));
        assert!(p.side_focused);
        assert!(!p.panel_focused);
        key(&mut p, &mut w, "escape");
        assert_eq!(p.panel, Some(Panel::Find));
        assert!(!p.side_focused);
        assert!(p.input().is_none());
        key(&mut p, &mut w, "super+f");
        assert!(p.panel_focused);
        assert_eq!(p.panel, Some(Panel::Find));
        p.layout(size());
        let point = Point {
            x: 10.,
            y: p.side_bounds.y + 30.,
        };
        p.mouse(&mut w, point, true, false, false, 0..3).unwrap();
        assert!(p.side_focused);
        assert!(!p.panel_focused);
        key(&mut p, &mut w, "escape");
        assert_eq!(p.panel, Some(Panel::Find));
    }
    #[test]
    fn slurp_and_find_under_use_the_primary_without_collapsing_other_carets() {
        let (mut w, mut p) = setup("first second first");
        w.editor_mut().set_selection(Selection::from_regions([
            Region::caret(10),
            Region::caret(5),
        ]));
        action(&mut p, &mut w, "slurp_find_string", json!({}));
        assert_eq!(p.query.value(), "first");
        assert_eq!(w.editor().regions().len(), 2);
        assert_eq!(w.editor().selection(), 0..5);
        action(&mut p, &mut w, "find_under", json!({}));
        wait(&mut p, &mut w, |p| p.all_matches_ready);
        assert_eq!(w.editor().selection(), 13..18);
        w.editor_mut().set_selection(Selection::from_regions([
            Region::caret(10),
            Region::caret(5),
        ]));
        w.editor_mut().run("find_all_under", &json!({})).unwrap();
        assert_eq!(
            w.editor()
                .regions()
                .iter()
                .map(|r| r.to_range())
                .collect::<Vec<_>>(),
            [0..5, 13..18]
        );
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    #[test]
    fn symbol_poll_keeps_worker_results_and_refreshes_after_an_edit() {
        let mut w = Workspace::new(
            vel_text::Text::from("fn alpha() {}\nfn beta() {}\n"),
            None,
            None,
            false,
        )
        .unwrap();
        w.set_file_type("Rust").unwrap();
        let mut p = Panels::new(&w, false);
        let until = Instant::now() + Duration::from_secs(10);
        while !w
            .editor()
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.ready())
        {
            w.poll();
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        p.action(&mut w, "show_overlay", &json!({"text":"@"}), 0..26)
            .unwrap();
        loop {
            p.poll(&mut w, 0..26);
            if !p.quick_pending && p.quick.items.len() == 2 {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        p.quick.navigate(1);
        let selected = p.quick.items[p.quick.selected].caption.clone();
        for _ in 0..25 {
            p.poll(&mut w, 0..26);
            assert!(!p.quick_pending);
            assert_eq!(p.quick.items[p.quick.selected].caption, selected);
        }
        let end = w.editor().text.len_bytes();
        w.editor_mut().set_caret(end, false);
        w.editor_mut()
            .run("insert", &json!({"characters":"fn gamma() {}\n"}))
            .unwrap();
        loop {
            w.poll();
            p.poll(&mut w, 0..40);
            if !p.quick_pending && p.quick.items.len() == 3 {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert_eq!(p.quick.items[p.quick.selected].caption, selected);
    }
    #[test]
    fn actual_status_scene_hides_optional_formats_and_uses_feedback() {
        let mut w = Workspace::new(vel_text::Text::from("你好"), None, None, false).unwrap();
        let mut p = Panels::new(&w, false);
        p.layout(Size {
            width: 1200.,
            height: 800.,
        });
        let text = |p: &mut Panels, w: &Workspace| {
            let mut scene = Scene::default();
            p.paint(w, &mut scene);
            scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .filter(|t| t.baseline >= 780.)
                .map(|t| t.text.clone())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let initial = text(&mut p, &w);
        assert!(initial.contains("Ln 1, Col 1"));
        assert!(!initial.contains("selected"));
        assert!(!initial.contains("UTF-8"));
        assert!(!initial.contains("Unix"));
        assert!(
            initial.contains(&vel_ui::status::indentation(
                w.editor().preferences.translate_tabs_to_spaces,
                w.editor().preferences.tab_size
            )),
            "{initial}"
        );
        w.editor_mut()
            .set_selection(Selection::from_regions([Region::new(0, 6)]));
        w.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "show_encoding",
            json!(true),
        );
        w.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "show_line_endings",
            json!(true),
        );
        let selected = text(&mut p, &w);
        assert!(selected.contains("2 characters selected"));
        assert!(selected.contains("UTF-8"));
        assert!(selected.contains(w.tabs[w.active].format.line_ending.name()));
        p.set_message("Unable to find missing".into());
        assert!(text(&mut p, &w).contains("Unable to find missing"));
    }
}
