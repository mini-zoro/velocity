//! Context menus, dictionary choices, spelling navigation and Git tool actions.
use crate::{panels::Panels, workspace::Workspace};
use std::path::PathBuf;
use vel_config::{Value, json, keymap::Invocation, settings::Layer, ui::PaletteItem};
use vel_platform::{Point, Rect};
use vel_ui::contains;
#[derive(Debug, Default)]
pub struct ReviewUi {
    /// Native tracking owns menu painting, keyboard input and accessibility.
    pub native_context: bool,
    pub reveal: bool,
    pub items: Vec<PaletteItem>,
    pub point: Point,
    pub bounds: Rect,
    pub selected: usize,
    context_identity: Option<(u64, u64)>,
    pending_diff: Option<(u64, u64, String, Value)>,
    suggestion: Option<(u64, u64, vel_spell::Misspelling)>,
    navigation: Option<(u64, u64, usize, bool, Option<u64>)>,
    pub tooltip: Option<(String, Point, std::time::Instant)>,
}
fn item(caption: &str, command: &str, args: Value) -> PaletteItem {
    PaletteItem {
        caption: caption.into(),
        invocation: Invocation {
            command: command.into(),
            args,
        },
    }
}
impl Panels {
    pub fn review_action(
        &mut self,
        workspace: &mut Workspace,
        name: &str,
        args: &Value,
    ) -> Result<bool, String> {
        match name {
            "wait_for_diff_command" => {
                self.review.pending_diff = Some((
                    workspace.tabs[workspace.active].sheet,
                    workspace.editor().revision,
                    args["command"]
                        .as_str()
                        .ok_or("missing diff command")?
                        .into(),
                    args["args"].clone(),
                ));
            }
            "navigate_misspelling" => {
                let editor = workspace.editor_mut();
                editor.settings.set(Layer::View, "spell_check", json!(true));
                editor.sync();
                self.review.navigation = Some((
                    workspace.tabs[workspace.active].sheet,
                    workspace.editor().revision,
                    workspace.editor().caret,
                    args["forward"] != false,
                    None,
                ));
            }
            "persist_spelling_words" => {
                let key = args["key"].as_str().ok_or("spelling setting key missing")?;
                if !matches!(key, "added_words" | "ignored_words") {
                    return Err("invalid spelling setting".into());
                }
                let value = args["words"].clone();
                workspace.settings.set(Layer::User, key, value.clone());
                for tab in &mut workspace.tabs {
                    tab.editor.settings.set(Layer::User, key, value.clone());
                    tab.editor.sync();
                }
                if let Some(root) = workspace.data_path() {
                    let root = root.to_owned();
                    let key = key.to_owned();
                    let result = crate::release::enqueue(move || {
                        let path = root.join("Packages/User/Preferences.sublime-settings");
                        let source = match std::fs::read_to_string(&path) {
                            Ok(s) => s,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
                            Err(e) => return Err(e.to_string()),
                        };
                        let document = vel_config::json::Document::parse(&source)?;
                        let encoded = document.set(&key, &value)?;
                        vel_session::atomic_write(&path, |file| {
                            use std::io::Write;
                            file.write_all(encoded.as_bytes())
                                .map_err(|e| e.to_string())
                        })?;
                        Ok(())
                    })?;
                    workspace.track_settings_write(result);
                }
            }
            "show_dictionary" => {
                let mut items = vec![item(
                    "English (US)",
                    "set_dictionary",
                    json!({"dictionary":crate::annotations::ENGLISH}),
                )];
                if let Some(root) = workspace.data_path() {
                    let packages = root.join("Packages");
                    if packages.exists() {
                        let dictionaries = vel_spell::SpellChecker::package_dictionaries(&packages)
                            .map_err(|e| e.to_string())?;
                        items.extend(dictionaries.into_iter().filter_map(|path| {
                            path.strip_prefix(&packages).ok().map(|p| {
                                let resource = format!("Packages/{}", p.to_string_lossy());
                                item(&resource, "set_dictionary", json!({"dictionary":resource}))
                            })
                        }));
                    }
                }
                self.choices(workspace, items);
            }
            "set_dictionary" => {
                let dictionary = args["dictionary"].as_str().ok_or("missing dictionary")?;
                workspace
                    .editor_mut()
                    .settings
                    .set(Layer::View, "dictionary", json!(dictionary));
                workspace.editor_mut().sync();
            }
            "git_open_repository"
            | "git_file_history"
            | "git_folder_history"
            | "git_line_history"
            | "git_blame_file" => {
                let tool = workspace.editor().settings.string("sublime_merge_path", "");
                if tool.is_empty() {
                    return Ok(true);
                }
                let path = args["path"]
                    .as_str()
                    .map(PathBuf::from)
                    .or_else(|| workspace.tabs[workspace.active].path.clone());
                let Some(path) = path else {
                    return Ok(true);
                };
                let Some(repo) = workspace.git.snapshot(&path) else {
                    return Ok(true);
                };
                let mut command = std::process::Command::new(tool);
                command.current_dir(&repo.root);
                match name {
                    "git_open_repository" => {
                        command.arg(&repo.root);
                    }
                    "git_blame_file" | "git_line_history" => {
                        command.arg("blame").arg(&path);
                        if name == "git_line_history" {
                            command.arg(args["line"].as_u64().map_or_else(
                                || {
                                    (workspace
                                        .editor()
                                        .text
                                        .byte_to_line(workspace.editor().caret)
                                        + 1)
                                    .to_string()
                                },
                                |line| line.to_string(),
                            ));
                        }
                    }
                    _ => {
                        command.arg("log").arg(&path);
                    }
                }
                command.spawn().map_err(|e| e.to_string())?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn git_items(
        &self,
        workspace: &Workspace,
        path: &std::path::Path,
        folder: bool,
    ) -> Vec<PaletteItem> {
        if workspace
            .editor()
            .settings
            .string("sublime_merge_path", "")
            .is_empty()
            || workspace.git.snapshot(path).is_none()
        {
            return vec![];
        }
        let mut items = vec![item(
            "Open Git Repository…",
            "git_open_repository",
            json!({"path":path}),
        )];
        items.push(item(
            if folder {
                "Folder History…"
            } else {
                "File History…"
            },
            if folder {
                "git_folder_history"
            } else {
                "git_file_history"
            },
            json!({"path":path}),
        ));
        if !folder {
            items.push(item(
                "Line History…",
                "git_line_history",
                json!({"path":path}),
            ));
            items.push(item("Blame File…", "git_blame_file", json!({"path":path})));
        }
        items
    }
    pub fn review_context(
        &mut self,
        workspace: &mut Workspace,
        point: Point,
        byte: usize,
        side: Option<(PathBuf, bool)>,
    ) {
        self.review.items.clear();
        self.review.context_identity = Some((
            workspace.tabs[workspace.active].sheet,
            workspace.editor().revision,
        ));
        self.review.suggestion = None;
        self.review.point = point;
        self.review.selected = 0;
        if let Some((path, folder)) = side {
            self.review.items = self.git_items(workspace, &path, folder);
            return;
        }
        // Preserve all selections when the pointer is inside any one of them. In
        // particular, invoking Copy from a context menu must not collapse the range.
        if !crate::context_menu::right_click_preserves_selection(
            &workspace.editor().regions(),
            byte,
        ) {
            workspace.editor_mut().set_caret(byte, false);
        }
        let editor = workspace.editor();
        let row = editor.text.byte_to_line(byte);
        let hunk_args = json!({"_context_byte":byte,"_context_version":editor.revision});
        let annotations = editor.annotations.borrow();
        if let Some(p) = annotations
            .current
            .as_ref()
            .filter(|p| p.identity.1.0 == editor.revision)
        {
            if let Some(h) = p.hunks.iter().find(|h| {
                h.new.start.min(editor.text.len_lines() - 1) <= row
                    && row < h.new.end.max(h.new.start + 1)
            }) {
                self.review.items.push(item(
                    if annotations.inline.contains(&h.new.start) {
                        "Hide Diff Hunk"
                    } else {
                        "Show Diff Hunk"
                    },
                    "toggle_inline_diff",
                    hunk_args.clone(),
                ));
                self.review.items.push(item(
                    "Show Only This Diff Hunk",
                    "toggle_inline_diff",
                    json!({"show_only":true,"_context_byte":byte,"_context_version":editor.revision}),
                ));
                self.review
                    .items
                    .push(item("Revert Diff Hunk", "revert_hunk", hunk_args));
            }
            if let Some(error) = p
                .spelling
                .iter()
                .find(|m| m.range.start <= byte && byte <= m.range.end)
                .cloned()
            {
                self.review.items.push(item(
                    "Add to Dictionary",
                    "add_word",
                    json!({"word":error.word}),
                ));
                self.review.items.push(item(
                    "Ignore Word",
                    "ignore_word",
                    json!({"word":error.word}),
                ));
                self.review.suggestion = Some((
                    workspace.tabs[workspace.active].sheet,
                    editor.revision,
                    error,
                ));
            }
        }
        drop(annotations);
        if let Some((_, _, error)) = &self.review.suggestion {
            editor.annotations.borrow_mut().suggest(error.word.clone());
        }
        if let Some(path) = &workspace.tabs[workspace.active].path {
            self.review
                .items
                .extend(self.git_items(workspace, path, false));
        }
        for item in &mut self.review.items {
            if item.invocation.command == "git_line_history" {
                item.invocation.args["line"] = json!(row + 1);
            }
        }
    }
    pub fn review_poll(&mut self, workspace: &mut Workspace) -> bool {
        let mut changed = false;
        let identity = (
            workspace.tabs[workspace.active].sheet,
            workspace.editor().revision,
        );
        if self.review.context_identity.is_some_and(|i| i != identity) {
            self.review.items.clear();
            self.review.context_identity = None;
            self.review.suggestion = None;
            changed = true;
        }
        if let Some((sheet, version, command, args)) = self.review.pending_diff.clone() {
            if (sheet, version) != identity {
                self.review.pending_diff = None;
            } else if workspace
                .editor()
                .annotations
                .borrow()
                .current
                .as_ref()
                .is_some_and(|p| p.identity.1.0 == version)
            {
                self.pending_command = Some(Invocation { command, args });
                self.review.pending_diff = None;
                changed = true;
            }
        }
        if let Some((sheet, version, error)) = self.review.suggestion.clone() {
            if workspace.tabs[workspace.active].sheet != sheet
                || workspace.editor().revision != version
            {
                self.review.items.clear();
                self.review.suggestion = None;
            } else {
                let suggestions = workspace
                    .editor()
                    .annotations
                    .borrow()
                    .current
                    .as_ref()
                    .and_then(|p| p.suggestions.clone());
                if let Some((word, suggestions)) = suggestions.filter(|(w, _)| *w == error.word) {
                    let mut items: Vec<_> = suggestions.into_iter().map(|s| item(&s, "replace_misspelling", json!({"start":error.range.start,"end":error.range.end,"word":word,"replacement":s,"version":version,"buffer_id":workspace.editor().buffer.id().0}))).collect();
                    items.append(&mut self.review.items);
                    self.review.items = items;
                    self.review.suggestion = None;
                    changed = true;
                }
            }
        }
        if let Some((sheet, version, caret, forward, token)) = self.review.navigation {
            let editor = workspace.editor_mut();
            if editor.revision != version
                || editor.caret != caret
                || workspace.tabs[workspace.active].sheet != sheet
            {
                self.review.navigation = None;
            } else if let Some(token) = token {
                let result = workspace
                    .editor()
                    .annotations
                    .borrow()
                    .current
                    .as_ref()
                    .and_then(|p| p.navigation.clone())
                    .filter(|(id, _)| *id == token);
                if let Some((_, error)) = result {
                    if let Some(error) = error {
                        workspace.editor_mut().select_misspelling(error.range);
                        self.review.reveal = true;
                    }
                    self.review.navigation = None;
                    changed = true;
                }
            } else {
                let scopes = workspace
                    .editor()
                    .language
                    .borrow()
                    .as_ref()
                    .and_then(|l| l.scopes.clone())
                    .filter(|s| s.version.0 == version);
                if let Some(scopes) = scopes {
                    let token = workspace
                        .editor()
                        .annotations
                        .borrow_mut()
                        .navigate(scopes, caret, forward);
                    self.review.navigation = Some((sheet, version, caret, forward, Some(token)));
                    changed = true;
                }
            }
        }
        changed
    }
    /// Whether the word under the context pointer still has an outstanding suggestion job.
    pub fn review_suggestions_pending(&self) -> bool {
        self.review.suggestion.is_some()
    }
    /// Discards menu choices while retaining independent diff/navigation jobs.
    pub fn cancel_review_menu(&mut self) {
        self.review.items.clear();
        self.review.context_identity = None;
        self.review.suggestion = None;
        self.review.tooltip = None;
    }
    pub fn review_paint(
        &mut self,
        workspace: &Workspace,
        size: vel_platform::Size,
        scene: &mut vel_render::scene::Scene,
    ) {
        let mut layer = vel_render::scene::Layer::default();
        let mut theme = workspace.themes.ui.borrow_mut();
        let settings = &workspace.editor().settings;
        if !self.review.native_context && !self.review.items.is_empty() {
            let height = self.review.items.len() as f64 * 26. + 8.;
            let rect = Rect {
                x: self.review.point.x.min((size.width - 300.).max(0.)),
                y: self.review.point.y.min((size.height - height).max(0.)),
                width: 300.,
                height,
            };
            self.review.bounds = rect;
            let popup = theme.style(
                0xF000,
                &vel_theme::ElementPath(&[
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("popup_control", &[]),
                ]),
                settings,
            );
            theme.surface(&mut layer, &popup, rect, rect);
            for (i, item) in self.review.items.iter().enumerate() {
                let row = Rect {
                    x: rect.x + 4.,
                    y: rect.y + 4. + i as f64 * 26.,
                    width: rect.width - 8.,
                    height: 26.,
                };
                let path = [
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("popup_control", &[]),
                    vel_theme::Element::new(
                        "quick_panel_row",
                        if i == self.review.selected {
                            &["selected"]
                        } else {
                            &[]
                        },
                    ),
                ];
                let style =
                    theme.style(0xF100 + i as u64, &vel_theme::ElementPath(&path), settings);
                theme.surface(&mut layer, &style, row, rect);
                theme.text(
                    &mut layer,
                    &style,
                    &item.caption,
                    row.x + 6.,
                    row.y + 18.,
                    rect,
                );
            }
        } else if let Some((text, point, since)) = &self.review.tooltip
            && since.elapsed() >= std::time::Duration::from_millis(350)
        {
            let rect = Rect {
                x: point.x.min((size.width - 260.).max(0.)),
                y: (point.y + 18.).min(size.height - 28.),
                width: 260.,
                height: 26.,
            };
            let style = theme.style(
                0xF200,
                &vel_theme::ElementPath(&[
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("tool_tip_control", &[]),
                ]),
                settings,
            );
            theme.surface(&mut layer, &style, rect, rect);
            theme.text(&mut layer, &style, text, rect.x + 8., rect.y + 18., rect);
        }
        if !layer.surfaces.is_empty() || !layer.text.is_empty() {
            scene.layers.push(layer);
        }
    }
    pub fn review_mouse(&mut self, point: Point, down: bool) -> Option<Option<Invocation>> {
        if self.review.native_context || self.review.items.is_empty() {
            return None;
        }
        if !down {
            return Some(None);
        }
        let invocation = if contains(self.review.bounds, point) {
            self.review
                .items
                .get(((point.y - self.review.bounds.y - 4.).max(0.) / 26.) as usize)
                .map(|i| i.invocation.clone())
        } else {
            None
        };
        self.review.items.clear();
        self.review.suggestion = None;
        Some(invocation)
    }
    pub fn review_key(&mut self, key: &vel_config::keymap::Key) -> Option<Option<Invocation>> {
        if self.review.native_context || self.review.items.is_empty() {
            return None;
        }
        let result = match key.name.as_str() {
            "escape" => {
                self.review.suggestion = None;
                self.review.items.clear();
                None
            }
            "up" => {
                self.review.selected =
                    (self.review.selected + self.review.items.len() - 1) % self.review.items.len();
                None
            }
            "down" => {
                self.review.selected = (self.review.selected + 1) % self.review.items.len();
                None
            }
            "enter" => {
                self.review.suggestion = None;
                let i = self.review.items[self.review.selected].invocation.clone();
                self.review.items.clear();
                Some(i)
            }
            _ => None,
        };
        Some(result)
    }
    pub fn review_busy(&self) -> bool {
        self.review.pending_diff.is_some()
            || self.review.navigation.is_some()
            || self.review.suggestion.is_some()
            || self
                .review
                .tooltip
                .as_ref()
                .is_some_and(|(_, _, t)| t.elapsed() < std::time::Duration::from_millis(400))
    }
}
