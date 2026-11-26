//! Theme-drawn tab controls and their native/headless pointer actions.
use super::*;

const TAB_DROPDOWN: &str = "tab_dropdown_button";

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
        if count > 1 && bar.width >= 60. {
            let rect = Rect {
                x: right - 24.,
                width: 24.,
                ..bar
            };
            let hovered = self.hovered_tab_button == Some((group, TAB_DROPDOWN));
            let path = [
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("tabset_control", &[]),
                vel_theme::Element::new("tab_label", if hovered { &["hover"] } else { &[] }),
            ];
            let style = theme.style(
                crate::panels::StyleId::TabBarButton.at(group as u64 * 4 + 3),
                &vel_theme::ElementPath(&path),
                settings,
            );
            let icon = "▾";
            let x = rect.x + (rect.width - theme.advance(&style, icon)) / 2.;
            theme.text(layer, &style, icon, x, rect.y + rect.height / 2. + 4., rect);
            self.tab_button_hits.push((group, TAB_DROPDOWN, rect));
            right = rect.x;
        }
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
        let Some((group, class, rect)) = self
            .tab_button_hits
            .iter()
            .copied()
            .find(|(_, _, r)| vel_ui::contains(*r, event.position))
        else {
            return false;
        };
        if class == TAB_DROPDOWN {
            self.panels.tab_dropdown(&mut self.workspace, group, rect);
        } else if class == "new_tab_button" {
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
    fn press(a: &mut App, at: Point) {
        for kind in [MouseKind::Down, MouseKind::Up] {
            a.handle_mouse_event(
                None,
                MouseEvent {
                    position: at,
                    kind,
                    button: 0,
                    click_count: 1,
                    modifiers: vel_platform::Modifiers(0),
                    timestamp: 0.,
                },
            );
        }
    }
    fn clean_tabs(count: usize, width: f64, ui: f64, scrolling: bool) -> App {
        let options = Options::default();
        let mut workspace =
            Workspace::new(vel_text::Text::new(), None, None, false).expect("workspace");
        for _ in 1..count {
            workspace.new_file().expect("tab");
        }
        for (key, value) in [
            ("ui_scale", json!(ui)),
            ("enable_tab_scrolling", json!(scrolling)),
        ] {
            workspace
                .settings
                .set(vel_config::settings::Layer::User, key, value.clone());
            for tab in &mut workspace.tabs {
                tab.editor
                    .settings
                    .set(vel_config::settings::Layer::User, key, value.clone());
            }
        }
        let mut a = App::new(
            options.clone(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            Reporter::for_options(&options, Instant::now()).expect("reporter"),
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
        );
        a.window_size = Size {
            width,
            height: 600.,
        };
        a.scene();
        a
    }
    /// A double click on a partly visible tab: the first click reveals the tab and slides the
    /// strip, the second still belongs to it and never closes or selects a neighbour.
    #[test]
    fn double_click_stays_on_the_tab_that_the_first_click_revealed() {
        let mut a = clean_tabs(12, 800., 1., true);
        a.workspace.workbench.groups[0].tab_scroll = 90.;
        a.scene();
        let (sheet, hit) = *a.tab_hits.first().expect("clipped first tab");
        let at = Point {
            x: hit.x + hit.width / 2.,
            y: hit.y + hit.height / 2.,
        };
        for clicks in [1, 2] {
            for kind in [MouseKind::Down, MouseKind::Up] {
                a.handle_mouse_event(
                    None,
                    MouseEvent {
                        position: at,
                        kind,
                        button: 0,
                        click_count: clicks,
                        modifiers: vel_platform::Modifiers(0),
                        timestamp: 0.,
                    },
                );
            }
            a.scene();
        }
        assert_eq!(a.workspace.tabs.len(), 12);
        assert_eq!(a.workspace.workbench.groups[0].focused, Some(sheet));
    }
    /// The click map agrees with the painted strip: every painted tab's centre selects that
    /// tab and never closes it, and every close button's centre closes exactly that tab.
    #[test]
    fn every_painted_tab_and_close_button_centre_hits_itself() {
        let mut failures = Vec::new();
        let mut clicks = 0;
        for width in [400., 520., 640., 800., 1000., 1200., 1400.] {
            for count in [1, 2, 3, 5, 8, 12, 16, 20, 30, 40] {
                for scrolling in [true, false] {
                    for ui in [1., 2.] {
                        let label = format!("{width}w {count} tabs scroll={scrolling} ui{ui}");
                        let mut a = clean_tabs(count, width, ui, scrolling);
                        // Each centre comes from the frame the click lands on.
                        for position in 0..count {
                            a.scene();
                            let sheets = a.workspace.workbench.groups[0].sheets.clone();
                            let Some((sheet, hit)) = a
                                .tab_hits
                                .iter()
                                .find(|(s, _)| *s == sheets[position])
                                .copied()
                            else {
                                continue;
                            };
                            let centre = Point {
                                x: hit.x + hit.width / 2.,
                                y: hit.y + hit.height / 2.,
                            };
                            press(&mut a, centre);
                            clicks += 1;
                            let focused = a.workspace.workbench.groups[0].focused;
                            if a.workspace.tabs.len() != count {
                                failures
                                    .push(format!("{label}: centre of {position} closed a tab"));
                                break;
                            }
                            if focused != Some(sheet) {
                                failures.push(format!(
                                    "{label}: centre of {position} selected {focused:?}"
                                ));
                            }
                        }
                        // Close buttons, each from a fresh frame, until none is painted.
                        let mut left = a.workspace.tabs.len();
                        while left > 1 {
                            a.scene();
                            let Some((sheet, close)) = a.tab_close_hits.last().copied() else {
                                break;
                            };
                            press(
                                &mut a,
                                Point {
                                    x: close.x + close.width / 2.,
                                    y: close.y + close.height / 2.,
                                },
                            );
                            clicks += 1;
                            left -= 1;
                            if a.workspace.tabs.len() != left
                                || a.workspace.sheet_index(sheet).is_some()
                            {
                                failures.push(format!("{label}: close of {sheet} missed"));
                                break;
                            }
                        }
                    }
                }
            }
        }
        assert!(clicks > 1000, "{clicks}");
        assert!(
            failures.is_empty(),
            "{} of {clicks}:\n{}",
            failures.len(),
            failures
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    fn run_width(a: &App, run: &vel_render::scene::TextRun) -> f64 {
        use vel_minihtml::FontMetrics;
        let style = vel_minihtml::TextStyle {
            font_family: run.family.clone().unwrap_or_else(|| "monospace".into()),
            font_size: run.size,
            font_weight: if run.style.bold { 700 } else { 400 },
            italic: run.style.italic,
            underline: false,
            color: vel_minihtml::Color::rgb(255, 255, 255),
        };
        let metrics = a.workspace.themes.ui.borrow().metrics.clone();
        f64::from(metrics.measure(&run.text, &style).advance)
    }
    /// Two-letter labels are measured like any other: with 18+ fitted tabs they never run
    /// into the close button or the dirty dot, at UI scale 1 and 2.
    #[test]
    fn short_tab_labels_are_fitted_too() {
        let mut checked = 0;
        for ui in [1., 2.] {
            for count in [18, 22, 26] {
                let mut a = clean_tabs(count, 1200., ui, false);
                for index in 0..a.workspace.tabs.len() {
                    a.workspace.select(index);
                    a.workspace
                        .editor_mut()
                        .run("insert", &json!({"characters": "ab"}))
                        .expect("edit");
                }
                let scene = a.scene();
                for (sheet, tab) in a.tab_hits.clone() {
                    let close = a
                        .tab_close_hits
                        .iter()
                        .find(|(s, _)| *s == sheet)
                        .map(|c| c.1);
                    let runs: Vec<_> = scene
                        .layers
                        .iter()
                        .flat_map(|l| &l.text)
                        .filter(|r| {
                            !r.text.is_empty()
                                && f64::from(r.x) >= tab.x
                                && f64::from(r.x) < tab.x + tab.width
                                && f64::from(r.baseline) > tab.y
                                && f64::from(r.baseline) < tab.y + tab.height
                        })
                        .collect();
                    let bullet = runs.iter().find(|r| r.text == "•").map(|r| f64::from(r.x));
                    let limit = close.map(|c| c.x).or(bullet).unwrap_or(tab.x + tab.width);
                    for run in runs.iter().filter(|r| r.text != "•") {
                        let end = f64::from(run.x) + run_width(&a, run);
                        assert!(
                            end <= limit + 0.01,
                            "ui{ui} {count}: {:?} ends at {end}, limit {limit}",
                            run.text
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 0);
    }
    /// In the reproduction window (1000 × 280, caret on line 42) the completion popup stays
    /// below the tab bar, and a click on the tab bar never commits a hidden row.
    #[test]
    fn completion_popup_stays_out_of_the_tab_bar_in_short_windows() {
        let options = Options::default();
        let text = (1..=80)
            .map(|i| format!("let token_{i} = {i};\n"))
            .collect::<String>();
        let mut workspace = Workspace::new(vel_text::Text::from(text.as_str()), None, None, false)
            .expect("workspace");
        workspace.set_file_type("Rust").expect("syntax");
        let mut a = App::new(
            options.clone(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            Reporter::for_options(&options, Instant::now()).expect("reporter"),
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
        );
        a.window_size = Size {
            width: 1000.,
            height: 280.,
        };
        a.scene();
        let at = a.workspace.editor().text.line_to_byte(41);
        a.workspace.editor_mut().set_caret(at, false);
        a.workspace
            .editor_mut()
            .run("insert", &json!({"characters": "t"}))
            .expect("type");
        a.workspace
            .editor_mut()
            .run("auto_complete", &json!({}))
            .expect("complete");
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while !a.workspace.editor().completion.visible() {
            a.workspace.editor_mut().poll_language();
            a.workspace.editor_mut().poll_completion();
            assert!(Instant::now() < deadline, "no completions");
            std::thread::park_timeout(std::time::Duration::from_millis(2));
        }
        a.scene();
        let popup = a.workspace.editor().completion.bounds.get();
        assert!(popup.y >= a.view.tab_height - 0.01, "{popup:?}");
        assert!(
            popup.y + popup.height <= a.view.size.height + 0.01,
            "{popup:?}"
        );
        let before = a.workspace.editor().text.to_string();
        let (_, tab) = a.tab_hits[0];
        let at = Point {
            x: a.content.x + popup.x + 40.,
            y: tab.y + tab.height / 2.,
        };
        press(&mut a, at);
        assert_eq!(a.workspace.editor().text.to_string(), before);
    }
    /// Narrow side bars: Git badges stay, labels shorten (to nothing), icons and disclosure
    /// triangles are left out before they would overlap a badge; indent guides sit under
    /// the ancestors' triangles.
    #[test]
    fn narrow_side_bar_keeps_badges_and_drops_overlapping_icons() {
        use vel_git::{Badge, FileStatus, Head, StatusSnapshot};
        let dir = tempfile::tempdir().expect("dir");
        let root = dir.path().canonicalize().expect("root");
        for name in [
            "src/db",
            "src/render/mod.rs",
            "src/render/deep/ab",
            "README.md",
        ] {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            std::fs::write(&path, "x\n").expect("file");
        }
        let mut a = clean_tabs(1, 1000., 1., true);
        a.run_named(None, "add_folder", &json!({"dir": root}));
        a.panels.side_visible = true;
        let deep = root.join("src/render/deep/ab");
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let _ = a.panels.reveal_sidebar_path(&mut a.workspace, deep.clone());
            let viewport = a.viewport();
            a.panels.poll(&mut a.workspace, viewport);
            if a.panels.side_rows.iter().any(|r| r.path == deep) {
                break;
            }
            assert!(Instant::now() < deadline, "tree never listed {deep:?}");
            std::thread::park_timeout(std::time::Duration::from_millis(5));
        }
        let mut files = std::collections::BTreeMap::new();
        for name in [
            "src/db",
            "src/render/mod.rs",
            "src/render/deep/ab",
            "README.md",
        ] {
            files.insert(
                std::path::PathBuf::from(name),
                FileStatus {
                    statuses: [Badge::Modified].into_iter().collect(),
                },
            );
        }
        let mut folders = std::collections::BTreeMap::new();
        for name in ["", "src", "src/render", "src/render/deep"] {
            folders.insert(std::path::PathBuf::from(name), Badge::Modified);
        }
        a.workspace.git.insert_snapshot(StatusSnapshot {
            root: root.clone(),
            generation: 1,
            head: Head::Branch {
                name: "main".into(),
                unborn: false,
            },
            ahead_behind: None,
            files,
            folders,
        });
        let mut guides = 0;
        for width in [320., 360., 400., 480., 560., 740.] {
            a.window_size = Size {
                width,
                height: 600.,
            };
            let scene = a.scene();
            let side = a.panels.side_bounds;
            let inside = |x: f64| x >= side.x && x < side.x + side.width;
            let badges: Vec<_> = scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .filter(|r| r.text == "M" && inside(f64::from(r.x)))
                .collect();
            assert!(badges.len() >= 6, "{width}: {} badges", badges.len());
            for badge in &badges {
                let (x, baseline) = (f64::from(badge.x), f64::from(badge.baseline));
                let row_top = baseline - 17. * a.panels.ui_scale;
                let row = |y: f64, h: f64| y < row_top + 24. && y + h > row_top;
                for run in scene.layers.iter().flat_map(|l| &l.text).filter(|r| {
                    r.baseline == badge.baseline && r.x < badge.x && inside(f64::from(r.x))
                }) {
                    let end = f64::from(run.x) + run_width(&a, run);
                    assert!(
                        end <= x + 0.01,
                        "{width}: {:?} runs into the badge",
                        run.text
                    );
                }
                for surface in scene.layers.iter().flat_map(|l| &l.surfaces) {
                    // Icons only (not the side bar's hairline edge textures).
                    let rect = match surface {
                        vel_render::scene::Surface::Image(q)
                            if q.rect.width >= 8. && q.rect.height <= 24. =>
                        {
                            q.rect
                        }
                        _ => continue,
                    };
                    if inside(rect.x) && row(rect.y, rect.height) {
                        assert!(
                            rect.x + rect.width <= x - 1.,
                            "{width}: icon {rect:?} under the badge at {x}"
                        );
                    }
                }
            }
            // Hairline guides: one per ancestor level of each nested row.
            guides += scene
                .layers
                .iter()
                .flat_map(|l| &l.surfaces)
                .filter(|s| matches!(s, vel_render::scene::Surface::Fill(r) if r.rect.width == 1. && inside(r.rect.x) && r.rect.height >= 23.))
                .count();
        }
        assert!(guides > 0, "no indent guides");
    }
    fn write_png(path: &std::path::Path) {
        write_png_sized(path, 6, 4);
    }
    fn write_png_sized(path: &std::path::Path, width: u32, height: u32) {
        image::RgbaImage::from_pixel(width, height, image::Rgba([40, 120, 200, 255]))
            .save(path)
            .expect("png");
    }
    fn open_path(a: &mut App, path: &std::path::Path) {
        a.workspace
            .open(crate::cli::FileLocation {
                path: path.to_owned(),
                line: 1,
                column: 1,
            })
            .expect("open");
    }
    /// Frames until the visible tabs finished loading and decoding.
    fn settle_images(a: &mut App) {
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        loop {
            a.workspace.poll();
            a.scene();
            let loading = a
                .workspace
                .tabs
                .iter()
                .any(|t| t.loading.is_some() || t.binary_loading.is_some());
            if !loading && !a.view.image_preview.busy() {
                return;
            }
            assert!(Instant::now() < deadline, "images never settled");
            std::thread::park_timeout(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn closing_an_inactive_image_view_releases_its_pixels_across_fifty_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preview.png");
        write_png_sized(&file, 6, 4);
        let mut a = clean_tabs(2, 1000., 1., true);
        for _ in 0..50 {
            open_path(&mut a, &file);
            settle_images(&mut a);
            let closed = a.workspace.active;
            let sheet = a.workspace.tabs[closed].sheet;
            let pixels = a.view.image_preview.image().expect("decoded");
            let weak = Arc::downgrade(&pixels);
            drop(pixels);
            a.workspace.activate_sheet(0);
            a.sync_active_view();
            assert!(a.sheet_views.contains_key(&sheet));
            a.workspace.close(closed).unwrap();
            a.sync_active_view();
            assert!(!a.sheet_views.contains_key(&sheet));
            assert!(weak.upgrade().is_none());
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn r33_open_as_dispatch_enablement_and_session_choice() {
        use crate::open_requests::OpenAs;
        let dir = tempfile::tempdir().unwrap();
        let webp = dir.path().join("test.WeBP");
        std::fs::write(&webp, crate::image_decode::WEBP_FIXTURE).unwrap();
        let mut a = clean_tabs(1, 1000., 1., true);
        open_path(&mut a, &webp);
        settle_images(&mut a);
        assert_eq!(
            a.view.image_preview.image().unwrap().logical_size,
            [240., 144.]
        );
        let file_menu = a
            .application_menu()
            .into_iter()
            .find(|m| m.id == "file")
            .unwrap();
        let menu_text = file_menu
            .children
            .iter()
            .find(|m| m.command.as_deref() == Some("open_as_text"))
            .unwrap();
        assert!(a.command_enabled("open_as_text", &menu_text.args));
        assert!(!a.command_enabled("open_as_image", "{}"));
        a.run_named(
            None,
            "open_as_text",
            &serde_json::from_str(&menu_text.args).unwrap(),
        );
        settle_images(&mut a);
        assert!(a.view.image_preview.image().is_none());
        assert!(a.workspace.editor().binary.is_some());
        assert_eq!(
            a.workspace.tabs[a.workspace.active].open_as(),
            Some(OpenAs::Text)
        );

        a.workspace.close(a.workspace.active).unwrap();
        open_path(&mut a, &webp);
        settle_images(&mut a);
        assert!(
            a.workspace.tabs[a.workspace.active]
                .label()
                .ends_with("[hex]")
        );
        let sheet = a.workspace.tabs[a.workspace.active].sheet;
        a.workspace.new_file().unwrap();
        a.scene();
        assert!(!a.command_enabled("open_as_image", "{}"));
        assert!(!a.command_enabled("open_as_text", "{}"));
        let (_, rect) = a.tab_hits.iter().find(|(id, _)| *id == sheet).unwrap();
        let menu = a.headless_context_menu(Point {
            x: rect.x + 4.,
            y: rect.y + 4.,
        });
        let image = menu
            .iter()
            .find(|m| m.command.as_deref() == Some("open_as_image"))
            .unwrap();
        a.run_named(
            None,
            "open_as_image",
            &serde_json::from_str(&image.args).unwrap(),
        );
        assert_eq!(a.workspace.tabs[a.workspace.active].sheet, sheet);
        settle_images(&mut a);
        assert!(a.view.image_preview.image().is_some());

        a.run_named(None, "show_overlay", &json!({"overlay":"command_palette"}));
        a.handle_text_input(
            None,
            TextInput::Insert {
                text: "File: Open as Text".into(),
                replacement: None,
            },
        );
        assert!(a.dispatch_key_binding(
            None,
            &vel_config::keymap::Key::parse("enter").unwrap(),
            Instant::now(),
            a.viewport()
        ));
        settle_images(&mut a);
        assert_eq!(
            a.workspace.tabs[a.workspace.active].open_as(),
            Some(OpenAs::Text)
        );
        assert!(a.view.image_preview.image().is_none());
    }
    /// Image tabs are named by the file, before and after decoding, whether or not they
    /// were ever shown; only an image whose decode failed (and whose bytes are shown) says
    /// "[hex]". The failure note goes away with the document it was shown for.
    #[test]
    fn image_tabs_say_hex_only_after_a_failed_decode() {
        let dir = tempfile::tempdir().expect("dir");
        let (first, broken, second) = (
            dir.path().join("first.png"),
            dir.path().join("broken.png"),
            dir.path().join("second.png"),
        );
        write_png(&first);
        write_png(&second);
        std::fs::write(&broken, b"\x89PNG\r\n\x1a\nnot an image").expect("broken");
        let mut a = clean_tabs(1, 1000., 1., true);
        let labels = |a: &App| {
            a.workspace
                .tabs
                .iter()
                .filter(|t| t.path.is_some())
                .map(|t| t.label())
                .collect::<Vec<_>>()
        };
        // Opened in turn without a frame in between: none was decoded yet.
        for path in [&first, &broken, &second] {
            open_path(&mut a, path);
        }
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        while a.workspace.tabs.iter().any(|t| t.binary_loading.is_some()) {
            a.workspace.poll();
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(std::time::Duration::from_millis(5));
        }
        assert_eq!(labels(&a), ["first.png", "broken.png", "second.png"]);
        settle_images(&mut a);
        // Each shown in turn, as a user clicking through them.
        for name in ["first.png", "broken.png", "second.png"] {
            let index = a
                .workspace
                .tabs
                .iter()
                .position(|t| t.label().starts_with(name))
                .expect("tab");
            a.workspace.select(index);
            settle_images(&mut a);
            let failed = name == "broken.png";
            assert_eq!(
                a.panels.message == crate::app::IMAGE_FALLBACK_NOTE,
                failed,
                "{name}: {:?}",
                a.panels.message
            );
        }
        assert_eq!(labels(&a), ["first.png", "broken.png [hex]", "second.png"]);
        // From the damaged image back to a decoded one: the note does not linger.
        let index = a
            .workspace
            .tabs
            .iter()
            .position(|t| t.label() == "broken.png [hex]")
            .expect("broken");
        a.workspace.select(index);
        settle_images(&mut a);
        assert_eq!(a.panels.message, crate::app::IMAGE_FALLBACK_NOTE);
        let index = a
            .workspace
            .tabs
            .iter()
            .position(|t| t.label() == "first.png")
            .expect("first");
        a.workspace.select(index);
        settle_images(&mut a);
        assert_ne!(a.panels.message, crate::app::IMAGE_FALLBACK_NOTE);
    }
    /// A decoded image that is decoded again (a reload, a settings change) keeps showing its
    /// picture, and its label never says "[hex]" in between; its size is reported again.
    #[test]
    fn image_redecoding_keeps_the_picture_and_the_label() {
        let dir = tempfile::tempdir().expect("dir");
        let file = dir.path().join("picture.png");
        write_png_sized(&file, 6, 4);
        let mut a = clean_tabs(1, 1000., 1., true);
        open_path(&mut a, &file);
        settle_images(&mut a);
        assert_eq!(a.panels.message, "6 × 4");
        let size = |a: &mut App| {
            let image = a.view.image_preview.image().expect("decoded");
            image.logical_size
        };
        assert_eq!(size(&mut a), [6., 4.]);
        // Settings changed: the file is decoded again under the same path.
        let revision = a.workspace.editor().settings.revision();
        a.workspace.editor_mut().settings.set(
            vel_config::settings::Layer::User,
            "image_after_settings",
            json!(true),
        );
        assert!(a.workspace.editor().settings.revision() > revision);
        for _ in 0..3 {
            a.scene();
            assert!(a.view.image_preview.image().is_some(), "picture dropped");
            assert!(!a.view.image_preview.failed);
            assert!(!a.workspace.tabs[0].label().contains("[hex]"));
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
        settle_images(&mut a);
        assert_eq!(a.panels.message, "6 × 4");
        assert_eq!(a.workspace.tabs[0].label(), "picture.png");
    }
    /// A picture that changes size on disk reports the new size, not the old one, and the
    /// report goes away with the document it was shown for.
    #[test]
    fn image_status_follows_the_picture_on_disk() {
        let dir = tempfile::tempdir().expect("dir");
        let root = dir.path().canonicalize().expect("root");
        let (picture, text) = (root.join("p.png"), root.join("t.txt"));
        write_png_sized(&picture, 6, 4);
        std::fs::write(&text, "text\n").expect("text");
        let mut a = clean_tabs(1, 1000., 1., true);
        a.workspace.watcher = Some(vel_fileio::Watcher::new().unwrap());
        open_path(&mut a, &picture);
        settle_images(&mut a);
        assert_eq!(a.panels.message, "6 × 4");
        // The picture is replaced on disk, the way another program would.
        let stamp = std::fs::metadata(&picture).unwrap().modified().unwrap();
        write_png_sized(&picture, 9, 3);
        std::fs::File::open(&picture)
            .unwrap()
            .set_modified(stamp + std::time::Duration::from_secs(2))
            .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        while a.panels.message != "9 × 3" {
            assert!(Instant::now() < deadline, "{}", a.panels.message);
            a.workspace.poll();
            a.scene();
            std::thread::park_timeout(std::time::Duration::from_millis(5));
        }
        open_path(&mut a, &text);
        settle_images(&mut a);
        let index = a
            .workspace
            .tabs
            .iter()
            .position(|t| t.path.as_deref() == Some(text.as_path()))
            .expect("text tab");
        a.workspace.select(index);
        settle_images(&mut a);
        assert_ne!(a.panels.message, "9 × 3");
    }
    /// Project-relative `//` rules in `image_file_patterns` match against the project folder,
    /// as the decoder reads them, so a picture below it is a picture and its tab never says
    /// "[hex]".
    #[test]
    fn project_relative_image_patterns_follow_the_project_folder() {
        let dir = tempfile::tempdir().expect("dir");
        let root = dir.path().canonicalize().expect("root");
        let picture = root.join("art/logo.png");
        std::fs::create_dir_all(picture.parent().unwrap()).expect("art");
        write_png_sized(&picture, 6, 4);
        let mut a = clean_tabs(1, 1000., 1., true);
        a.workspace.folders.push(vel_search::project::Folder {
            path: root.clone(),
            name: "root".into(),
            rules: Default::default(),
            follow_links: true,
        });
        open_path(&mut a, &picture);
        // Decoding runs on a worker; the label answers before a frame has been painted.
        assert!(
            a.workspace.tabs[0].editor.binary.is_some()
                || a.workspace.tabs[0].binary_loading.is_some()
        );
        for tab in &mut a.workspace.tabs {
            tab.editor.settings.set(
                vel_config::settings::Layer::User,
                "image_file_patterns",
                json!(["//art/*.png"]),
            );
        }
        assert!(a.workspace.tabs[0].is_image_file(), "before frame");
        a.scene();
        assert!(a.workspace.tabs[0].is_image_file(), "after frame");
        assert_eq!(a.workspace.tabs[0].label(), "logo.png");
        settle_images(&mut a);
        assert_eq!(a.panels.message, "6 × 4");
        // A file below the project that the rule does not cover stays bytes.
        let other = root.join("notes/logo.png");
        std::fs::create_dir_all(other.parent().unwrap()).expect("notes");
        write_png_sized(&other, 6, 4);
        open_path(&mut a, &other);
        for tab in &mut a.workspace.tabs {
            tab.editor.settings.set(
                vel_config::settings::Layer::User,
                "image_file_patterns",
                json!(["//art/*.png"]),
            );
        }
        settle_images(&mut a);
        assert_eq!(a.workspace.tabs[1].label(), "logo.png [hex]");
        assert_eq!(a.workspace.tabs[1].label(), "logo.png [hex]");
    }
    /// A renamed picture changes its label with its name, without the cached image decision
    /// outliving the path it was made for: the renamed file keeps its picture, the old name
    /// is gone.
    #[test]
    fn renaming_a_picture_updates_its_label() {
        let dir = tempfile::tempdir().expect("dir");
        let root = dir.path().canonicalize().expect("root");
        let (old, new) = (root.join("old.png"), root.join("new.png"));
        write_png_sized(&old, 6, 4);
        let mut a = clean_tabs(1, 1000., 1., true);
        open_path(&mut a, &old);
        settle_images(&mut a);
        assert_eq!(a.workspace.tabs[0].label(), "old.png");
        std::fs::rename(&old, &new).expect("rename");
        crate::context_menu::apply_rename(&mut a.workspace, &old, &new).expect("retarget");
        assert_eq!(a.workspace.tabs[0].path.as_deref(), Some(new.as_path()));
        a.scene();
        assert_eq!(a.workspace.tabs[0].label(), "new.png");
        settle_images(&mut a);
        assert_eq!(a.panels.message, "6 × 4");
        // Renamed to a name the image rules do not cover: shown as bytes, under that name.
        let bytes = root.join("picture.dat");
        std::fs::rename(&new, &bytes).expect("rename");
        crate::context_menu::apply_rename(&mut a.workspace, &new, &bytes).expect("retarget");
        settle_images(&mut a);
        assert_eq!(a.workspace.tabs[0].label(), "picture.dat [hex]");
    }
    /// A selected tab is painted with the scheme's background. Under a theme of the other
    /// luminosity (the dark theme with a light scheme) its label therefore takes the scheme's
    /// own ink, which stays readable on it, instead of the theme's near-white.
    #[test]
    fn selected_tabs_of_the_other_luminosity_keep_a_readable_label() {
        let dir = tempfile::tempdir().expect("dir");
        let profile = dir.path().join("profile");
        let user = profile.join("Packages/User");
        std::fs::create_dir_all(&user).expect("user");
        std::fs::write(
            user.join("Preferences.sublime-settings"),
            r#"{"color_scheme":"Paper.sublime-color-scheme"}"#,
        )
        .expect("prefs");
        std::fs::write(
            user.join("Paper.sublime-color-scheme"),
            r##"{"globals":{"background":"#fbf7ee","foreground":"#20303a"}}"##,
        )
        .expect("scheme");
        let options = Options::default();
        let mut workspace =
            Workspace::new(vel_text::Text::new(), Some(profile.clone()), None, false)
                .expect("workspace");
        workspace.wait_resources();
        workspace.new_file().expect("tab");
        let mut a = App::new(
            options.clone(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            Reporter::for_options(&options, Instant::now()).expect("reporter"),
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(
                Some(profile.as_path()),
            ))),
        );
        assert!(a.workspace.dark, "the default UI theme is the dark one");
        a.scene();
        let metrics = a.workspace.themes.ui.borrow().metrics.clone();
        let tab = a.view_buffer.expect("active");
        let hit = a
            .tab_hits
            .iter()
            .find(|(sheet, _)| *sheet == tab)
            .map(|(_, rect)| *rect)
            .expect("tab");
        let label = a
            .scene()
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .find(|r| {
                f64::from(r.x) >= hit.x
                    && f64::from(r.x) < hit.x + hit.width
                    && f64::from(r.baseline) > hit.y
                    && f64::from(r.baseline) < hit.y + hit.height
                    && !r.text.is_empty()
            })
            .cloned()
            .expect("tab label");
        // The label's ink against the light scheme paper it sits on, in WCAG relative
        // luminance; body text needs 4.5:1, and the scheme's ink gives about 12:1.
        let luminance = |rgb: [f64; 3]| {
            let channel = |c: f64| {
                if c <= 0.03928 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
        };
        // The scheme's own foreground ("#20303a").
        assert!(
            label.color[..3]
                .iter()
                .zip([0x20, 0x30, 0x3a])
                .all(
                    |(painted, scheme)| ((f64::from(*painted) * 255.) - f64::from(scheme)).abs()
                        < 1.5
                ),
            "{:?}",
            label.color
        );
        let ink = luminance([0.1254902, 0.1882353, 0.22745098]);
        let paper = luminance([0xfb as f64 / 255., 0xf7 as f64 / 255., 0xee as f64 / 255.]);
        let contrast = (paper + 0.05) / (ink + 0.05);
        assert!(
            contrast >= 7.,
            "label ink {ink} on paper {paper}: {contrast}x"
        );
        let _ = metrics;
    }
    /// Image and hex views paint the active colour scheme's paper, not a built-in one.
    #[test]
    fn image_and_hex_views_use_the_active_scheme() {
        let dir = tempfile::tempdir().expect("dir");
        let profile = dir.path().join("profile");
        let user = profile.join("Packages/User");
        std::fs::create_dir_all(&user).expect("user");
        std::fs::write(
            user.join("Preferences.sublime-settings"),
            r#"{"color_scheme":"Paper.sublime-color-scheme"}"#,
        )
        .expect("prefs");
        std::fs::write(
            user.join("Paper.sublime-color-scheme"),
            r##"{"globals":{"background":"#204060","foreground":"#f0e0d0"}}"##,
        )
        .expect("scheme");
        let (picture, broken) = (dir.path().join("p.png"), dir.path().join("b.png"));
        write_png(&picture);
        std::fs::write(&broken, b"\x89PNG\r\n\x1a\nnot an image").expect("broken");
        let options = Options::default();
        let mut workspace =
            Workspace::new(vel_text::Text::new(), Some(profile.clone()), None, false)
                .expect("workspace");
        workspace.wait_resources();
        let mut a = App::new(
            options.clone(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            Reporter::for_options(&options, Instant::now()).expect("reporter"),
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(
                Some(profile.as_path()),
            ))),
        );
        let paper = [
            0x20 as f32 / 255.,
            0x40 as f32 / 255.,
            0x60 as f32 / 255.,
            1.,
        ];
        for path in [&picture, &broken] {
            open_path(&mut a, path);
            settle_images(&mut a);
            let scene = a.scene();
            let painted = scene.layers.iter().flat_map(|l| &l.surfaces).any(|s| {
                matches!(s, vel_render::scene::Surface::Fill(r)
                    if r.color == paper && r.rect.width >= a.content.width - 1.)
            });
            assert!(painted, "{}: no scheme paper", path.display());
        }
    }
    /// The dirty dot of a very narrow fitted tab stays inside the tab (or is left out when
    /// even it does not fit).
    #[test]
    fn dirty_dots_stay_inside_very_narrow_tabs() {
        let mut checked = 0;
        for count in [120, 160, 220] {
            let mut a = clean_tabs(count, 1200., 1., false);
            for index in 0..a.workspace.tabs.len() {
                a.workspace.select(index);
                a.workspace
                    .editor_mut()
                    .run("insert", &json!({"characters": "x"}))
                    .expect("edit");
            }
            let scene = a.scene();
            for (_, tab) in a.tab_hits.clone() {
                for dot in scene.layers.iter().flat_map(|l| &l.text).filter(|r| {
                    r.text == "•"
                        && f64::from(r.x) >= tab.x - 2.
                        && f64::from(r.x) < tab.x + tab.width
                        && f64::from(r.baseline) > tab.y
                        && f64::from(r.baseline) < tab.y + tab.height
                }) {
                    let (x, end) = (f64::from(dot.x), f64::from(dot.x) + run_width(&a, dot));
                    assert!(
                        x >= tab.x - 0.01 && end <= tab.x + tab.width + 0.01,
                        "{count} tabs: dot {x}..{end} in {tab:?}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 100, "{checked}");
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
