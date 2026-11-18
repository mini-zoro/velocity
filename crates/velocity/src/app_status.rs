//! Click dispatch for the format portions of the painted status text.
use super::*;
use vel_minihtml::FontMetrics;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FormatItem {
    Encoding,
    LineEndings,
}
impl App {
    pub(super) fn capture_format_hits(&mut self, scene: &vel_render::scene::Scene) {
        self.format_hits.clear();
        let editor = self.workspace.editor();
        // The status text shows the format fields unless a message replaces them; the find
        // panels show their message themselves.
        if !self.panels.chrome
            || (!self.panels.message.is_empty() && !self.panels.message_in_find_panel())
        {
            return;
        }
        let tab = &self.workspace.tabs[self.workspace.active];
        let fields = [
            (
                FormatItem::Encoding,
                editor.settings.bool("show_encoding", false),
                tab.format.display_encoding(),
            ),
            (
                FormatItem::LineEndings,
                editor.settings.bool("show_line_endings", false),
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
