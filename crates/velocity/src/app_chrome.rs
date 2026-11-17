//! Theme-drawn tab controls and their native/headless pointer actions.
use super::*;
impl App {
    pub(super) fn paint_tab_buttons(
        &mut self,
        layer: &mut SceneLayer,
        group: usize,
        bar: Rect,
        count: usize,
        width: f64,
    ) -> Rect {
        let settings = &self.workspace.editor().settings;
        let mut theme = self.workspace.themes.ui.borrow_mut();
        let mut right = bar.x + bar.width;
        // Themes opt into drawing these controls through their content insets.
        // https://www.sublimetext.com/docs/themes.html#settings
        for (slot, class, hide) in [
            (
                0,
                "new_tab_button",
                settings.bool("hide_new_tab_button", false),
            ),
            (
                1,
                "scroll_tabs_right_button",
                settings.bool("hide_tab_scrolling_buttons", false)
                    || !settings.bool("enable_tab_scrolling", true)
                    || count as f64 * width <= bar.width,
            ),
            (
                2,
                "scroll_tabs_left_button",
                settings.bool("hide_tab_scrolling_buttons", false)
                    || !settings.bool("enable_tab_scrolling", true)
                    || count as f64 * width <= bar.width,
            ),
        ] {
            if hide {
                continue;
            }
            let path = [
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("tabset_control", &[]),
                vel_theme::Element::new(class, &[]),
            ];
            let style = theme.style(
                0xDC00 + group as u64 * 4 + slot,
                &vel_theme::ElementPath(&path),
                settings,
            );
            let extent = f64::from((style.margin.left + style.margin.right).max(style.min_size[0]));
            if extent <= 0. {
                continue;
            }
            let extent = extent.max(16.).min((right - bar.x).max(0.));
            let rect = Rect {
                x: right - extent,
                width: extent,
                ..bar
            };
            right -= extent;
            theme.surface(layer, &style, rect, bar);
            self.tab_button_hits.push((group, class, rect));
        }
        Rect {
            width: (right - bar.x).max(1.),
            ..bar
        }
    }
    pub(super) fn tab_button_mouse(
        &mut self,
        window: Option<&WindowHandle>,
        event: &MouseEvent,
    ) -> bool {
        if event.kind != MouseKind::Down || event.button != 0 {
            return false;
        }
        let Some((group, class, _)) = self
            .tab_button_hits
            .iter()
            .copied()
            .find(|(_, _, r)| vel_ui::contains(*r, event.position))
        else {
            return false;
        };
        if class == "new_tab_button" {
            self.workspace.workbench.active = group;
            self.run_named(window, "new_file", &json!({}));
        } else if let Some((_, bar, max)) = self.tab_bars.iter().find(|(g, _, _)| *g == group) {
            let delta = bar.width
                * if class == "scroll_tabs_left_button" {
                    -0.8
                } else {
                    0.8
                };
            let tabs = &mut self.workspace.workbench.groups[group];
            tabs.tab_scroll = (tabs.tab_scroll + delta).clamp(0., *max);
        }
        self.named_changed(window, false);
        true
    }
}
