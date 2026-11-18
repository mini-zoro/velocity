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
        if !self.panels.chrome || !self.panels.message.is_empty() {
            return;
        }
        let tab = &self.workspace.tabs[self.workspace.active];
        let fields = [
            (
                FormatItem::Encoding,
                editor.settings.bool("show_encoding", false),
                tab.format.encoding.as_str(),
            ),
            (
                FormatItem::LineEndings,
                editor.settings.bool("show_line_endings", false),
                tab.format.line_ending.name(),
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
        // The status popup has the same resource rows and validation/checks as the
        // main menu. Public status-menu names and command arguments:
        // https://docs.sublimetext.io/guide/customization/menus.html#available-menus
        // https://docs.sublimetext.io/reference/commands.html#set-line-ending
        // Lane A's round-20 panels::status_format_menu is not in this checkout.
        // Reuse the installed menu resources until that model is integrated; do
        // not register a placeholder set_line_ending command.
        let mut rows = vec![];
        fn collect(
            items: &[vel_config::ui::MenuItem],
            names: &[&str],
            out: &mut Vec<vel_config::ui::MenuItem>,
        ) {
            for row in items {
                if names.contains(&row.caption.as_str()) {
                    out.push(row.clone());
                } else {
                    collect(&row.children, names, out);
                }
            }
        }
        let names: &[&str] = match item {
            FormatItem::Encoding => &["Reopen with Encoding", "Save with Encoding"],
            FormatItem::LineEndings => &["Line Endings"],
        };
        collect(&self.workspace.menus, names, &mut rows);
        if item == FormatItem::LineEndings {
            rows = rows.into_iter().flat_map(|r| r.children).collect();
        }
        native_menu(&rows, &[])
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
