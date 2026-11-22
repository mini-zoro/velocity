//! Inline editing belongs to a sheet, not to its document buffer or the tab index.
use super::*;
use crate::context_menu as files;
use vel_platform::accessibility::Id;
use vel_render::scene::{SolidRect, Surface};
use vel_ui::{input::Input, theme};

pub(super) const RENAME_TAB: &str = "rename_tab";
pub(super) const RENAME_FIELD: Id = 102;

#[derive(Debug)]
pub(super) struct TabRename {
    pub sheet: u64,
    pub input: Input,
    pub bounds: Rect,
    from: PathBuf,
    selecting: bool,
}

impl TabRename {
    pub fn range_rect(&self, range: Utf16Range) -> (Rect, Utf16Range) {
        let (mut rect, actual) = self.input.range_rect(range);
        let right = (self.bounds.x + self.bounds.width - 1.).max(self.bounds.x);
        rect.x = rect.x.clamp(self.bounds.x, right);
        rect.width = rect.width.min(right + 1. - rect.x);
        // AppKit anchors candidate windows below the returned rectangle.
        rect.y = self.bounds.y;
        rect.height = self.bounds.height;
        (rect, actual)
    }

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
        let left =
            (origin.x - 4. * scale).clamp(tab.x, (tab.x + tab.width - 12. * scale).max(tab.x));
        let field = Rect {
            x: left,
            width: tab.x + tab.width - left,
            ..tab
        };
        self.input.bounds = field;
        let mut style = label.clone();
        style.margin.left = (4. * scale) as f32;
        style.case = None; // Edit the actual filename, not a theme's display transform.
        self.input.configure_theme(&style, theme.metrics.clone());
        let state = self.input.state();
        let text = state.text.to_string();
        let x = |byte| self.input.x_for_byte(&text, byte);
        let selection = state.selected.bytes(&state.text);
        let y = (origin.y - f64::from(style.size) - 1.).max(tab.y + 1.);
        let bottom = (origin.y + 4. * scale).min(tab.y + tab.height);
        let body = Rect {
            y,
            height: (bottom - y).max(0.),
            ..tab
        };
        let mut border = style.foreground;
        border[3] *= 0.65;
        layer.rects.push(SolidRect::outline(body, tab, border));
        layer.surfaces.push(Surface::Fill(SolidRect {
            rect: body,
            clip: tab,
            color: if dark {
                [0.12, 0.13, 0.15, 1.]
            } else {
                [1., 1., 1., 1.]
            },
            radius: 0.,
        }));
        let text_clip = Rect {
            y: y + 1.,
            height: (bottom - y - 2.).max(0.),
            width: (field.width - 1.).max(0.),
            ..field
        };
        let (a, b) = (x(selection.start), x(selection.end));
        layer.rects.push(SolidRect {
            rect: Rect {
                x: a,
                width: if selection.is_empty() { 1. } else { b - a },
                ..text_clip
            },
            clip: text_clip,
            color: if selection.is_empty() {
                style.foreground
            } else {
                [0.25, 0.48, 0.85, 0.42]
            },
            radius: 0.,
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
                (origin.y + 2. * scale).min(bottom - 2.),
                f64::from(theme.density),
                style.foreground,
            );
        }
    }
}

impl App {
    pub(super) fn tab_rename_path(&self, index: usize) -> Option<&Path> {
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
        let path = tab.path.as_deref()?;
        let name = path.file_name()?.to_str()?;
        (path.is_absolute()
            && !name.contains(['\r', '\n'])
            && path.is_file()
            && files::local_volume(path))
        .then_some(path)
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
        let Some(from) = self.tab_rename_path(index).map(Path::to_owned) else {
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
        self.panels.dragging_tab = None;
        self.tab_click = None;
        self.hovered_close = None;
        self.workspace.editor_mut().completion.hide();
        let name = from
            .file_name()
            .and_then(|name| name.to_str())
            .expect("checked name");
        let stem = Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(name);
        let mut input = Input::new(name);
        input.bounds = bounds;
        input.buffer.set_selection(
            input.view,
            vel_buffer::Selection::from_regions([vel_buffer::Region::from(0..stem.len())]),
        );
        self.tab_close_hits.retain(|(id, _)| *id != sheet);
        self.tab_rename = Some(Box::new(TabRename {
            sheet,
            input,
            bounds,
            from,
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
        let from = &rename.from;
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
