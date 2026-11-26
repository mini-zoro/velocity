//! Inline editing belongs to a sheet, not to its document buffer or the tab index.
use super::*;
use crate::context_menu as files;
use vel_platform::accessibility::Id;
use vel_render::scene::SolidRect;
use vel_ui::{input::Input, theme};

pub(super) const RENAME_TAB: &str = "rename_tab";
pub(super) const RENAME_FIELD: Id = 102;

/// What Return commits.
#[derive(Debug)]
pub(super) enum RenameTarget {
    /// A file on a local volume: renamed on disk, every view follows.
    File(PathBuf),
    /// An untitled sheet: names the tab (Sublime's `set_name`), which is also the
    /// default name when it is first saved. Nothing is written to disk.
    Untitled { original: String },
}

#[derive(Debug)]
pub(super) struct TabRename {
    pub sheet: u64,
    pub input: Input,
    /// The tab: clicks inside it stay in the field.
    pub bounds: Rect,
    /// The rounded field itself, right of the file icon; input methods anchor to it.
    pub field: Rect,
    target: RenameTarget,
    selecting: bool,
}

impl TabRename {
    pub fn range_rect(&self, range: Utf16Range) -> (Rect, Utf16Range) {
        let (mut rect, actual) = self.input.range_rect(range);
        let field = self.field;
        let right = (field.x + field.width - 1.).max(field.x);
        rect.x = rect.x.clamp(field.x, right);
        rect.width = rect.width.min(right + 1. - rect.x);
        // AppKit anchors candidate windows below the returned rectangle: the field's bottom.
        rect.y = field.y;
        rect.height = field.height;
        (rect, actual)
    }

    /// A rounded field like Finder's and Xcode's rename fields: it starts after the file
    /// icon, keeps a few points from the tab's edges, and has a hairline border, a text
    /// background and the accent focus ring shared with the Settings window.
    pub fn paint(
        &mut self,
        layer: &mut SceneLayer,
        theme: &theme::ThemeUi,
        label: &theme::Style,
        (tab, origin): (Rect, Point),
        dark: bool,
        scale: f64,
    ) {
        self.bounds = tab;
        self.input.scale = scale;
        let inset = 4. * scale;
        let pad = 5. * scale;
        let right = tab.x + tab.width - inset;
        let left = (origin.x - pad).clamp(tab.x + inset, (right - 16. * scale).max(tab.x + inset));
        let height = (f64::from(label.size) + 10. * scale).min(tab.height - 2. * inset);
        // Centered on the label's text, so the name does not move when editing starts.
        let middle = origin.y - f64::from(label.size) * 0.34;
        let top = (middle - height / 2.).clamp(tab.y + inset, tab.y + tab.height - inset - height);
        let field = Rect {
            x: left,
            y: top,
            width: (right - left).max(0.),
            height: height.max(0.),
        };
        self.field = field;
        self.input.bounds = field;
        let mut style = label.clone();
        style.margin.left = (origin.x - left).max(0.) as f32;
        style.case = None; // Edit the actual filename, not a theme's display transform.
        self.input.configure_theme(&style, theme.metrics.clone());
        let colors = crate::field_chrome::FieldColors::new(
            theme.scheme_background(),
            style.foreground,
            crate::field_chrome::accent(dark),
            dark,
        );
        crate::field_chrome::focus_ring(
            layer,
            field,
            tab,
            colors.ring,
            crate::field_chrome::RADIUS,
        );
        crate::field_chrome::face(layer, field, tab, &colors);
        let state = self.input.state();
        let text = state.text.to_string();
        let x = |byte| self.input.x_for_byte(&text, byte);
        let selection = state.selected.bytes(&state.text);
        let text_clip = Rect {
            x: field.x + 2.,
            y: field.y + 1.,
            width: (field.width - 4.).max(0.),
            height: (field.height - 2.).max(0.),
        };
        let (a, b) = (x(selection.start), x(selection.end));
        let line = Rect {
            y: field.y + 3. * scale,
            height: (field.height - 6. * scale).max(0.),
            ..text_clip
        };
        layer.rects.push(SolidRect {
            rect: Rect {
                x: a,
                width: if selection.is_empty() {
                    1.5_f64.max(1. / f64::from(theme.density))
                } else {
                    b - a
                },
                ..line
            },
            clip: text_clip,
            color: if selection.is_empty() {
                colors.caret
            } else {
                colors.selection
            },
            radius: if selection.is_empty() { 0. } else { 2. },
        });
        theme.text(
            layer,
            &style,
            self.input.visible_suffix(&text),
            x(0),
            origin.y,
            text_clip,
        );
        if let Some(marked) = state.marked {
            let marked = marked.bytes(&state.text);
            crate::panels::paint_marked_underline(
                layer,
                text_clip,
                [x(marked.start), x(marked.end)],
                (origin.y + 2. * scale).min(field.y + field.height - 2.),
                f64::from(theme.density),
                style.foreground,
            );
        }
    }
}

impl App {
    pub(super) fn tab_rename_target(&self, index: usize) -> Option<RenameTarget> {
        let tab = self.workspace.tabs.get(index)?;
        if tab.transient
            || tab.goto_preview
            || tab.find_results
            || tab.deleted
            || tab.load_failed
            || tab.editor.read_only
            || tab.binary_document
            || tab.editor.binary.is_some()
            || tab.loading.is_some()
            || tab.binary_loading.is_some()
            || tab.editor.settings.get("velocity_settings_label").is_some()
            || tab
                .editor
                .settings
                .get("velocity_html_sheet_title")
                .is_some()
        {
            return None;
        }
        let Some(path) = tab.path.as_deref() else {
            return Some(RenameTarget::Untitled {
                original: tab.ui_name(&self.workspace.ui),
            });
        };
        let name = path.file_name()?.to_str()?;
        (path.is_absolute()
            && !name.contains(['\r', '\n'])
            && path.is_file()
            && files::local_volume(path))
        .then(|| RenameTarget::File(path.to_owned()))
    }

    pub(super) fn double_click_tab(
        &mut self,
        window: Option<&WindowHandle>,
        index: usize,
        clicks: usize,
    ) {
        if self.workspace.tabs[index].transient {
            self.workspace.make_permanent(index);
            self.named_changed(window, false);
        } else if clicks == 2 {
            self.start_tab_rename(window, index);
        }
    }

    pub(super) fn start_tab_rename(&mut self, window: Option<&WindowHandle>, index: usize) {
        let Some(target) = self.tab_rename_target(index) else {
            return;
        };
        let sheet = self.workspace.tabs[index].sheet;
        let Some(bounds) = self
            .tab_hits
            .iter()
            .find(|(id, _)| *id == sheet)
            .map(|hit| hit.1)
        else {
            return;
        };
        self.workspace.activate_sheet(index);
        self.leave_region_focus();
        self.panels.focus_editor_for_context();
        self.cancel_tab_drag(window);
        self.tab_click = None;
        self.hovered_close = None;
        self.workspace.editor_mut().completion.hide();
        let (name, selected) = match &target {
            RenameTarget::File(from) => {
                let name = from
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("checked name");
                let stem = Path::new(name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name);
                (name, stem.len())
            }
            // A first-line name such as a URL has no extension to keep: select it all.
            RenameTarget::Untitled { original } => (original.as_str(), original.len()),
        };
        let mut input = Input::new(name);
        input.bounds = bounds;
        input.buffer.set_selection(
            input.view,
            vel_buffer::Selection::from_regions([vel_buffer::Region::from(0..selected)]),
        );
        self.tab_close_hits.retain(|(id, _)| *id != sheet);
        self.tab_rename = Some(Box::new(TabRename {
            sheet,
            input,
            bounds,
            field: bounds,
            target,
            selecting: false,
        }));
        self.named_changed(window, false);
    }

    /// Returns false on a preflight/filesystem error: leave focus in the field.
    pub(super) fn finish_tab_rename(&mut self, window: Option<&WindowHandle>) -> bool {
        let Some(mut rename) = self.tab_rename.take() else {
            return true;
        };
        rename.input.text_input(TextInput::Unmark);
        let name = rename.input.value();
        let from = match &rename.target {
            RenameTarget::File(from) => from,
            RenameTarget::Untitled { original } => {
                // A pasted line break or control character cannot be part of a tab name.
                let name: String = name
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                let name = name.trim();
                if name != original
                    && let Some(index) = self.workspace.sheet_index(rename.sheet)
                {
                    // Empty returns the tab to its automatic first-line name.
                    self.workspace.tabs[index].suggested_name =
                        (!name.is_empty()).then(|| name.to_owned());
                }
                self.named_changed(window, false);
                return true;
            }
        };
        let result = if from.file_name().is_some_and(|old| old == name.as_str()) {
            Ok(from.clone())
        } else {
            files::prepare_rename(&self.workspace, std::slice::from_ref(from))
                .and_then(|()| files::rename_entry(from, &name))
        };
        match result {
            Ok(to) => {
                if *from != to
                    && let Err(error) = self.apply_file_rename(from, &to)
                {
                    self.named_error(window, &error);
                }
                self.named_changed(window, false);
                true
            }
            Err(error) => {
                rename.selecting = false;
                rename.input.command(Command::SelectAll);
                self.tab_rename = Some(rename);
                self.named_error(window, &error);
                self.redraw_tab_rename(window);
                false
            }
        }
    }

    pub(super) fn redraw_tab_rename(&self, window: Option<&WindowHandle>) {
        if let Some(window) = window {
            window.invalidate_character_coordinates();
            window.request_frame();
        }
    }

    pub(super) fn tab_rename_command(&mut self, window: Option<&WindowHandle>, command: Command) {
        if self.ime_command(window, command) {
            return;
        }
        let Some(rename) = self.tab_rename.as_mut() else {
            return;
        };
        match command {
            Command::Newline if !rename.input.composing() => {
                self.finish_tab_rename(window);
            }
            Command::Cancel => {
                self.tab_rename = None;
                self.named_changed(window, false);
            }
            Command::Newline | Command::Tab | Command::Backtab => {}
            command => {
                if command == Command::Paste
                    && let Some(w) = window
                {
                    rename
                        .input
                        .set_clipboard(w.clipboard_text().unwrap_or_default());
                }
                rename.input.command(match command {
                    Command::ParagraphStart(extend) => Command::Home(extend),
                    Command::ParagraphEnd(extend) => Command::End(extend),
                    other => other,
                });
                if matches!(command, Command::Copy | Command::Cut)
                    && let Some(w) = window
                {
                    w.set_clipboard_text(rename.input.clipboard());
                }
                self.redraw_tab_rename(window);
            }
        }
    }

    pub(super) fn tab_rename_key(
        &mut self,
        window: Option<&WindowHandle>,
        key: &vel_config::keymap::Key,
    ) -> bool {
        use Command::*;
        let command = match (key.name.as_str(), key.modifiers) {
            ("enter" | "keypad_enter", 0)
                if !self.tab_rename.as_ref().unwrap().input.composing() =>
            {
                Newline
            }
            ("escape", 0) if !self.tab_rename.as_ref().unwrap().input.composing() => Cancel,
            ("a", 8) => SelectAll,
            ("c", 8) => Copy,
            ("x", 8) => Cut,
            ("v", 8) => Paste,
            ("z", 8) => Undo,
            ("z", 12) => Redo,
            ("left", 8 | 12) => Home(key.modifiers == 12),
            ("right", 8 | 12) => End(key.modifiers == 12),
            // No editor bindings, but let AppKit interpret ordinary text and selectors.
            _ => return key.modifiers & 8 != 0,
        };
        self.tab_rename_command(window, command);
        true
    }

    /// True if the pointer was consumed by the field (or its failed commit).
    pub(super) fn tab_rename_mouse(
        &mut self,
        window: Option<&WindowHandle>,
        event: &MouseEvent,
    ) -> bool {
        let Some(rename) = self.tab_rename.as_mut() else {
            return false;
        };
        match event.kind {
            MouseKind::Down if vel_ui::contains(rename.bounds, event.position) => {
                if event.button == 0 && !event.is_context_click() {
                    rename
                        .input
                        .click_at(event.position, event.modifiers.shift());
                    rename.selecting = true;
                    self.redraw_tab_rename(window);
                }
                true
            }
            MouseKind::Down => !self.finish_tab_rename(window),
            MouseKind::Drag if rename.selecting => {
                rename.input.click_at(event.position, true);
                self.redraw_tab_rename(window);
                true
            }
            MouseKind::Up => std::mem::take(&mut rename.selecting),
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "app_tab_rename_tests.rs"]
mod tests;
