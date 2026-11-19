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
        let key = (
            std::rc::Rc::as_ptr(&self.workspace.themes.ui) as usize,
            theme.revision,
            settings.revision(),
        );
        if self
            .absent_tab_buttons
            .as_ref()
            .is_none_or(|(previous, _)| *previous != key)
        {
            self.absent_tab_buttons = Some((key, [false; 3]));
        }
        let absent = &mut self.absent_tab_buttons.as_mut().unwrap().1;
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
            if hide || absent[slot as usize] {
                continue;
            }
            let hovered = self.hovered_tab_button == Some((group, class));
            let path = [
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("tabset_control", &[]),
                vel_theme::Element::new(class, if hovered { &["hover"] } else { &[] }),
            ];
            let style = theme.style(
                crate::panels::StyleId::TabBarButton.at(group as u64 * 4 + slot),
                &vel_theme::ElementPath(&path),
                settings,
            );
            let extent = f64::from((style.margin.left + style.margin.right).max(style.min_size[0]));
            if extent <= 0. {
                // Themes without this optional control need no resolver work on
                // subsequent keys. Resource/settings changes invalidate the cache.
                absent[slot as usize] = true;
                continue;
            }
            // The element is sized by its content margins and centred in the bar, with a
            // small gap between controls; the drawn element is also the click target.
            let gap = 4f64.min((right - bar.x).max(0.));
            let extent = extent.max(16.).min((right - bar.x - gap).max(0.));
            let height = f64::from((style.margin.top + style.margin.bottom).max(style.min_size[1]));
            let height = if height > 0. {
                height.max(16.).min(bar.height)
            } else {
                bar.height
            };
            let rect = Rect {
                x: right - gap - extent,
                y: bar.y + ((bar.height - height) / 2.).round(),
                width: extent,
                height,
            };
            right = rect.x;
            theme.icon_surface(layer, &style, rect, bar);
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

#[cfg(test)]
mod tests {
    use super::*;
    fn app(tabs: usize) -> App {
        let options = Options::default();
        let mut workspace =
            Workspace::new(vel_text::Text::from("one"), None, None, false).expect("workspace");
        for _ in 1..tabs {
            workspace.new_file().expect("tab");
        }
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"dirty "}))
            .expect("edit");
        App::new(
            options.clone(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            Reporter::for_options(&options, Instant::now()).expect("reporter"),
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
        )
    }
    fn set(a: &mut App, name: &str, value: Value) {
        a.run_named(None, "set_setting", &json!({"setting":name,"value":value}));
    }
    fn dirty_label(a: &mut App) -> Option<String> {
        let scene = a.scene();
        let dirty = a.workspace.tabs.iter().find(|t| t.dirty())?.sheet;
        let rect = a.tab_hits.iter().find(|(sheet, _)| *sheet == dirty)?.1;
        scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .filter(|t| {
                f64::from(t.x) >= rect.x
                    && f64::from(t.x) < rect.x + rect.width
                    && f64::from(t.baseline) < rect.y + rect.height
            })
            .map(|t| t.text.clone())
            .reduce(|a, b| a + "|" + &b)
    }
    /// Hidden close buttons leave no invisible close target, and the dirty bullet returns.
    #[test]
    fn hidden_close_buttons_have_no_target_and_keep_the_dirty_marker() {
        let mut a = app(3);
        a.scene();
        assert_eq!(a.tab_close_hits.len(), 3);
        assert!(!dirty_label(&mut a).unwrap().contains('•'));
        set(&mut a, "show_tab_close_buttons", json!(false));
        a.scene();
        assert!(a.tab_close_hits.is_empty());
        assert!(dirty_label(&mut a).unwrap().contains('•'));
    }
    /// Tabs clipped by the strip edges or the tab-bar buttons: close targets stay inside the
    /// visible part of their tab, never under a button, and every visible dirty tab is marked.
    #[test]
    fn clipped_tabs_keep_targets_and_dirty_marks_inside_the_visible_strip() {
        let mut a = app(24);
        for i in 0..a.workspace.tabs.len() {
            a.workspace.select(i);
            a.workspace
                .editor_mut()
                .run("insert", &json!({"characters":"x"}))
                .expect("edit");
        }
        a.scene();
        let max = a.tab_bars[0].2;
        assert!(max > 0.);
        let inside = |r: Rect, o: Rect| {
            r.x >= o.x - 0.01
                && r.y >= o.y - 0.01
                && r.x + r.width <= o.x + o.width + 0.01
                && r.y + r.height <= o.y + o.height + 0.01
        };
        let mut checked = 0;
        let mut offset = 0.;
        while offset <= max {
            a.workspace.workbench.groups[0].tab_scroll = offset;
            let scene = a.scene();
            let bar = a.tab_bars[0].1;
            for (sheet, close) in &a.tab_close_hits {
                let hit = a.tab_hits.iter().find(|(s, _)| s == sheet).unwrap().1;
                assert!(
                    inside(*close, hit) && inside(*close, bar),
                    "{offset}: {close:?}"
                );
                for (_, _, button) in &a.tab_button_hits {
                    let x = (close.x + close.width).min(button.x + button.width)
                        - close.x.max(button.x);
                    assert!(x <= 0.01, "{offset}: close {close:?} under {button:?}");
                }
            }
            for (sheet, hit) in &a.tab_hits {
                if hit.width < 14. {
                    continue;
                }
                checked += 1;
                let closed = a.tab_close_hits.iter().any(|(s, _)| s == sheet);
                let bullet = scene.layers.iter().flat_map(|l| &l.text).any(|t| {
                    t.text == "•"
                        && f64::from(t.x) >= hit.x
                        && f64::from(t.x) < hit.x + hit.width
                        && f64::from(t.baseline) < hit.y + hit.height
                });
                assert!(
                    closed || bullet,
                    "{offset}: dirty tab {hit:?} has no marker"
                );
            }
            offset += 37.;
        }
        assert!(checked > 100);
    }
    /// Tabs too narrow for a close button still show that they are dirty.
    /// Tab labels never spill over close buttons: a label with no room for even an ellipsis
    /// is left out.
    #[test]
    fn narrow_tab_labels_never_draw_over_close_buttons() {
        use vel_minihtml::FontMetrics;
        let mut checked = 0;
        for tabs in [12, 14, 16, 18, 20] {
            let mut a = app(tabs);
            set(&mut a, "enable_tab_scrolling", json!(false));
            let scene = a.scene();
            let metrics = a.workspace.themes.ui.borrow().metrics.clone();
            for (sheet, close) in &a.tab_close_hits {
                let tab = a.tab_hits.iter().find(|(s, _)| s == sheet).unwrap().1;
                for run in scene.layers.iter().flat_map(|l| &l.text).filter(|r| {
                    !r.text.is_empty()
                        && f64::from(r.x) >= tab.x
                        && f64::from(r.x) < close.x
                        && f64::from(r.baseline) > tab.y
                        && f64::from(r.baseline) < tab.y + tab.height
                }) {
                    let style = vel_minihtml::TextStyle {
                        font_family: run.family.clone().unwrap_or_else(|| "monospace".into()),
                        font_size: run.size,
                        font_weight: if run.style.bold { 700 } else { 400 },
                        italic: run.style.italic,
                        underline: false,
                        color: vel_minihtml::Color::rgb(255, 255, 255),
                    };
                    let end = f64::from(run.x + metrics.measure(&run.text, &style).advance);
                    assert!(
                        end <= close.x + 0.01,
                        "{tabs} tabs: {:?} at {} runs into the close button at {}",
                        run.text,
                        run.x,
                        close.x
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0);
    }
    #[test]
    fn narrow_fitted_tabs_keep_the_dirty_marker() {
        let mut a = app(80);
        set(&mut a, "enable_tab_scrolling", json!(false));
        a.scene();
        let dirty = a.workspace.tabs.iter().find(|t| t.dirty()).unwrap().sheet;
        let rect = a.tab_hits.iter().find(|(s, _)| *s == dirty).unwrap().1;
        assert!(rect.width < 28., "{rect:?}");
        assert!(a.tab_close_hits.iter().all(|(s, _)| *s != dirty));
        assert!(dirty_label(&mut a).unwrap().contains('•'));
    }
}

#[cfg(test)]
mod overlay_first_frames {
    use super::*;
    fn app(workspace: Workspace) -> App {
        let options = Options::default();
        App::new(
            options.clone(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            Reporter::for_options(&options, Instant::now()).expect("reporter"),
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
        )
    }
    /// Opens an overlay with its key binding, then advances frame by frame (poll + scene, as
    /// the native loop does) through ranking, asserting on every frame that the selected row is
    /// fully inside the visible list.
    fn open_and_watch(a: &mut App, key: &str, ui_scale: f64) -> usize {
        a.workspace.editor_mut().settings.set(
            vel_config::settings::Layer::User,
            "ui_scale",
            json!(ui_scale),
        );
        a.scene();
        let viewport = a.viewport();
        assert!(a.dispatch_key_binding(
            None,
            &vel_config::keymap::Key::parse(key).expect("key"),
            Instant::now(),
            viewport,
        ));
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let mut frames_with_rows = 0;
        let mut settled = 0;
        while settled < 5 {
            let viewport = a.viewport();
            a.panels.poll(&mut a.workspace, viewport);
            a.scene();
            assert!(a.panels.overlay.is_some());
            let s = a.panels.ui_scale;
            let _ = s;
            let top = a.panels.quick_list_top();
            let bottom = top + a.panels.quick.scroll.viewport;
            if let Some(item) = a.panels.quick.items.get(a.panels.quick.selected) {
                frames_with_rows += 1;
                let id = (1 << 40) + item.value as u64 * 5;
                let row = a.panels.tree.nodes[&id].rect;
                assert!(
                    row.y >= top - 0.01 && row.y + row.height <= bottom + 0.01,
                    "frame {frames_with_rows}: selected row {row:?} outside list {top}..{bottom} \
                     (offset {})",
                    a.panels.quick.scroll.offset
                );
                if a.panels.quick.selected == 0 {
                    assert_eq!(a.panels.quick.scroll.offset, 0.);
                }
            }
            if a.panels.ranking_pending() || a.panels.quick.items.is_empty() {
                settled = 0;
            } else {
                settled += 1;
            }
            assert!(Instant::now() < deadline, "ranking did not finish");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        frames_with_rows
    }
    #[test]
    fn command_palette_first_frames_keep_the_selected_row_visible() {
        for scale in [1., 2.] {
            let workspace =
                Workspace::new(vel_text::Text::from("text"), None, None, false).expect("ws");
            let mut a = app(workspace);
            assert!(open_and_watch(&mut a, "super+shift+p", scale) > 0);
        }
    }
    #[test]
    fn goto_anything_first_frames_keep_the_selected_row_visible() {
        let dir = tempfile::tempdir().expect("dir");
        let root = dir.path().canonicalize().expect("canonical");
        for i in 0..300 {
            std::fs::write(root.join(format!("file_{i:03}.rs")), "fn main() {}\n").expect("file");
        }
        let project = root.join("p.sublime-project");
        std::fs::write(&project, r#"{"folders":[{"path":"."}]}"#).expect("project");
        let workspace =
            Workspace::new(vel_text::Text::new(), None, Some(project), false).expect("workspace");
        let mut a = app(workspace);
        assert!(open_and_watch(&mut a, "super+p", 1.) > 0);
    }
}
