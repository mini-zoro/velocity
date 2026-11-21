//! Click dispatch for the format portions of the painted status text.
use super::*;
use vel_minihtml::FontMetrics;

pub(crate) fn selection_column(editor: &crate::editor::Editor, row: usize) -> usize {
    let text = &editor.text;
    let caret = editor.caret;
    match editor
        .settings
        .string("selection_description_column_type", "display")
    {
        "character" | "characters" => {
            text.byte_to_char(caret) - text.byte_to_char(text.line_to_byte(row)) + 1
        }
        "utf16" => text.byte_to_utf16(caret) - text.byte_to_utf16(text.line_to_byte(row)) + 1,
        "byte" => caret - text.line_to_byte(row) + 1,
        _ => editor.status_column.borrow_mut().column(
            text,
            (editor.buffer.id(), editor.buffer.change_id()),
            caret,
            editor.preferences.tab_size,
            crate::large::LONG_LINE,
        ),
    }
}

pub(crate) fn selection_sum(editor: &crate::editor::Editor) -> Option<String> {
    let regions = editor.regions();
    if regions.len() < 2 || editor.binary.is_some() {
        return None;
    }
    let mut integer = Some(0i128);
    let mut sum = 0.0;
    for region in regions.iter() {
        // Bound work per region in the status paint path, including nonnumeric selections.
        if region.empty() || region.end() - region.begin() > 128 {
            return None;
        }
        let source = editor.text.slice(region.to_range()).to_string();
        let number = source.trim();
        integer = integer.and_then(|total| total.checked_add(number.parse().ok()?));
        sum += number.parse::<f64>().ok()?;
        if !sum.is_finite() {
            return None;
        }
    }
    Some(integer.map_or_else(
        || {
            // Display useful precision without binary round-off tails (e.g. 0.1 + 0.2).
            format!("{sum:.14e}")
                .parse::<f64>()
                .unwrap_or(sum)
                .to_string()
        },
        |value| value.to_string(),
    ))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FormatItem {
    Encoding,
    LineEndings,
}
impl App {
    pub(super) fn status_switcher_mouse(
        &mut self,
        window: Option<&WindowHandle>,
        event: &MouseEvent,
    ) -> bool {
        if event.kind != MouseKind::Down
            || !self
                .panels
                .status_switcher_rect(&self.workspace)
                .is_some_and(|r| vel_ui::contains(r, event.position))
        {
            return false;
        }
        if event.is_context_click() {
            let entries = self.status_switcher_menu();
            if let Some(window) = window {
                window.show_context_menu(&entries);
            }
            #[cfg(test)]
            {
                self.last_status_switcher_menu = entries;
            }
        } else if event.button == 0 {
            self.run_named(window, "toggle_side_bar", &json!({}));
        } else {
            return false;
        }
        true
    }

    fn status_switcher_menu(&self) -> Vec<MenuEntry> {
        let tr = &self.workspace.translations;
        let item = |caption: std::borrow::Cow<'_, str>, command: &str, args: Value| MenuEntry {
            caption: caption.into_owned(),
            command: Some(command.into()),
            args: args.to_string(),
            ..Default::default()
        };
        let mut items = vec![
            item(tr.tr("Console"), "show_console", json!({})),
            item(
                tr.tr("Build Results"),
                "show_panel",
                json!({"panel":"output.exec"}),
            ),
            item(
                tr.tr("Find Results"),
                "show_panel",
                json!({"panel":"find_results"}),
            ),
        ];
        if self.panels.panel.is_some() {
            items.push(item(tr.tr("Hide Panel"), "hide_panel", json!({})));
        }
        items
    }

    pub(super) fn capture_format_hits(&mut self, scene: &vel_render::scene::Scene) {
        self.format_hits.clear();
        let editor = self.workspace.editor();
        // The status text shows the format fields unless a message replaces them; the find
        // panels show their message themselves.
        if !self.panels.status_visible()
            || (!self.panels.message.is_empty() && !self.panels.message_in_find_panel())
        {
            return;
        }
        let tab = &self.workspace.tabs[self.workspace.active];
        let fields = [
            (
                FormatItem::Encoding,
                editor.settings.bool("show_encoding", true),
                tab.format.display_encoding(),
            ),
            (
                FormatItem::LineEndings,
                editor.settings.bool("show_line_endings", true),
                tab.format.line_ending.display_name(),
            ),
        ];
        if !fields.iter().any(|(_, show, _)| *show) {
            return;
        }
        let bottom = self.window_size.height;
        let top = bottom - crate::panels::STATUS * self.panels.ui_scale;
        let metrics = self.workspace.themes.ui.borrow().metrics.clone();
        for run in scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .filter(|r| f64::from(r.baseline) >= top)
        {
            for (item, show, text) in fields {
                if !show {
                    continue;
                }
                let Some(at) = run.text.rfind(text) else {
                    continue;
                };
                let style = vel_minihtml::TextStyle {
                    font_family: run.family.clone().unwrap_or_else(|| "monospace".into()),
                    font_size: run.size,
                    font_weight: if run.style.bold { 700 } else { 400 },
                    italic: run.style.italic,
                    underline: false,
                    color: vel_minihtml::Color::rgb(255, 255, 255),
                };
                let x = f64::from(run.x + metrics.measure(&run.text[..at], &style).advance);
                let width = f64::from(metrics.measure(text, &style).advance);
                let left = x.max(run.clip.x);
                let right = (x + width).min(run.clip.x + run.clip.width);
                if right > left {
                    self.format_hits.push((
                        item,
                        Rect {
                            x: left,
                            y: top,
                            width: right - left,
                            height: bottom - top,
                        },
                    ));
                }
            }
        }
    }
    pub(super) fn format_menu(&self, item: FormatItem) -> Vec<MenuEntry> {
        // Paint, hit testing and menus use the same effective captions/model.
        let item = match item {
            FormatItem::Encoding => vel_ui::status::FormatItem::Encoding,
            FormatItem::LineEndings => vel_ui::status::FormatItem::LineEndings,
        };
        fn native(row: vel_ui::status::FormatMenuItem) -> MenuEntry {
            MenuEntry {
                caption: row.caption,
                command: row.command.map(str::to_owned),
                args: row.args.to_string(),
                children: row.children.into_iter().map(native).collect(),
                ..Default::default()
            }
        }
        crate::panels::status_format_menu(&self.workspace, item)
            .into_iter()
            .map(native)
            .collect()
    }
    pub(super) fn format_status_mouse(
        &mut self,
        window: Option<&WindowHandle>,
        event: &MouseEvent,
    ) -> bool {
        if event.kind != MouseKind::Down || event.button != 0 {
            return false;
        }
        let Some((item, _)) = self
            .format_hits
            .iter()
            .find(|(_, r)| vel_ui::contains(*r, event.position))
        else {
            return false;
        };
        let entries = self.format_menu(*item);
        if let Some(window) = window {
            window.show_context_menu(&entries);
        } else {
            self.last_format_menu = entries;
        }
        true
    }
}
