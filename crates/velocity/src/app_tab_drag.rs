//! Pointer capture, paint-only tab order, and the process-wide detached gesture.
use super::*;
use crate::session::{DragSlot, DragTab, ScreenTabStrip, SharedTabDrag};
use std::{collections::BTreeMap, rc::Rc};

const THRESHOLD: f64 = 4.;
const SLIDE_TIME: Duration = Duration::from_millis(150);
const IMAGE_PAD: f64 = 12.;

#[derive(Clone, Copy, Debug)]
pub(super) struct TabStrip {
    pub group: usize,
    pub bounds: Rect,
    pub clip: Rect,
    pub step: f64,
    pub scroll: f64,
}

impl TabStrip {
    fn origin(self) -> f64 {
        self.clip.x - self.scroll
    }

    fn tab_width(self) -> f64 {
        (self.step - 1.).max(self.step * 0.5)
    }
}

#[derive(Debug)]
pub(super) struct TabPress {
    pub sheet: u64,
    start: Point,
    grab: Point,
    pub drag: Option<TabDrag>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TabDrag {
    pub group: usize,
    index: usize,
    x: f64,
    pub(super) detached: bool,
    popup_loaded: bool,
}

#[derive(Debug)]
pub(super) struct TabSlides {
    start: Instant,
    origins: BTreeMap<u64, (usize, f64)>,
}

fn over_threshold(start: Point, point: Point) -> bool {
    (point.x - start.x).powi(2) + (point.y - start.y).powi(2) >= THRESHOLD.powi(2)
}

fn outside_detach_band(strip: TabStrip, point: Point, window_width: f64) -> bool {
    let band = strip.bounds.height;
    point.y < strip.bounds.y - band
        || point.y > strip.bounds.y + strip.bounds.height + band
        || point.x < -band
        || point.x > window_width + band
}

fn strip_at(strips: &[TabStrip], point: Point) -> Option<TabStrip> {
    strips
        .iter()
        .copied()
        .find(|s| vel_ui::contains(s.bounds, point))
}

// Slots are the unanimated layout with the dragged tab occupying its current gap.
// Crossing back requires passing the neighbour's new midpoint, not its edge.
fn insertion_index(
    widths: impl Iterator<Item = f64>,
    mut left: f64,
    current: Option<usize>,
    x: f64,
) -> usize {
    let mut insertion = 0;
    for (i, width) in widths.enumerate() {
        if current != Some(i) {
            if x < left + width / 2. {
                break;
            }
            insertion += 1;
        }
        left += width;
    }
    insertion
}

fn preview_order(group: &[u64], sheet: u64, insert: Option<usize>) -> Vec<u64> {
    let mut ordered: Vec<_> = group.iter().copied().filter(|id| *id != sheet).collect();
    if let Some(at) = insert {
        ordered.insert(at.min(ordered.len()), sheet);
    }
    ordered
}

impl App {
    pub(super) fn press_tab(&mut self, sheet: u64, point: Point) {
        let Some(group) = self.workspace.workbench.group_of(sheet) else {
            return;
        };
        let Some(strip) = self.tab_strips.iter().find(|strip| strip.group == group) else {
            return;
        };
        let order = &self.workspace.workbench.groups[group].sheets;
        let Some(index) = order.iter().position(|id| *id == sheet) else {
            return;
        };
        let left = self
            .tab_painted
            .iter()
            .find(|(id, _, _)| *id == sheet)
            .map_or(strip.origin() + strip.step * index as f64, |&(_, _, x)| x);
        self.tab_press = Some(TabPress {
            sheet,
            start: point,
            grab: Point {
                x: point.x - left,
                y: point.y - strip.bounds.y,
            },
            drag: None,
        });
    }

    pub(super) fn handle_tab_gesture(
        &mut self,
        window: Option<&WindowHandle>,
        event: &MouseEvent,
    ) -> bool {
        if event.button != 0 || !matches!(event.kind, MouseKind::Drag | MouseKind::Up) {
            return false;
        }
        if self.cancel_orphaned_tab_drag(window) {
            return true;
        }
        let Some(mut press) = self.tab_press.take() else {
            return false;
        };
        if press.drag.is_some_and(|drag| drag.detached) {
            self.detached_tab_gesture(window, event, press);
            return true;
        }
        let Some(source) = self.workspace.workbench.group_of(press.sheet) else {
            return true;
        };
        let Some(mut drag) = press.drag.or_else(|| {
            if event.kind != MouseKind::Drag || !over_threshold(press.start, event.position) {
                return None;
            }
            let index = self.workspace.workbench.groups[source]
                .sheets
                .iter()
                .position(|id| *id == press.sheet)?;
            Some(TabDrag {
                group: source,
                index,
                x: press.start.x,
                detached: false,
                popup_loaded: false,
            })
        }) else {
            if event.kind == MouseKind::Drag {
                self.tab_press = Some(press);
            }
            return true;
        };
        self.tab_click = None;
        let local = strip_at(&self.tab_strips, event.position);
        let source_strip = self.tab_strips.iter().find(|s| s.group == source).copied();
        if local.is_none()
            && let Some(strip) = source_strip
            && outside_detach_band(strip, event.position, self.window_size.width)
            && self.start_detached_tab(window, &press, strip)
        {
            drag.detached = true;
            press.drag = Some(drag);
            self.detached_tab_gesture(window, event, press);
            return true;
        }
        let previous = (drag.group, drag.index);
        if let Some(strip) = local {
            let count = self.workspace.workbench.groups[strip.group].sheets.len();
            let same_group = strip.group == drag.group;
            let slots = count + usize::from(strip.group != source && same_group)
                - usize::from(strip.group == source && !same_group);
            drag.index = insertion_index(
                std::iter::repeat_n(strip.step, slots),
                strip.origin(),
                same_group.then_some(drag.index),
                event.position.x,
            );
            drag.group = strip.group;
        }
        let moved = drag.x != event.position.x || press.drag.is_none();
        drag.x = event.position.x;
        if event.kind == MouseKind::Up {
            self.begin_tab_slide();
            if local.is_some() {
                self.commit_tab_drag(press.sheet, drag.group, drag.index);
            }
            self.named_changed(window, false);
        } else {
            let order_changed = previous != (drag.group, drag.index);
            if order_changed {
                self.begin_tab_slide();
            }
            press.drag = Some(drag);
            self.tab_press = Some(press);
            if (order_changed || moved)
                && let Some(window) = window
            {
                window.request_frame();
            }
        }
        true
    }

    fn start_detached_tab(
        &mut self,
        window: Option<&WindowHandle>,
        press: &TabPress,
        strip: TabStrip,
    ) -> bool {
        let Some(index) = self.workspace.sheet_index(press.sheet) else {
            return false;
        };
        // Unsupported transfers (for example a cloned view of the same buffer) can still
        // reorder in this window. Do not lift them out of the strip.
        if !self.transferable_tab(index) {
            return false;
        }
        let tab = self.drag_tab_presentation(index);
        let scale = window.map_or(1., WindowHandle::scale_factor);
        match self.render_drag_tab(&tab, strip, scale) {
            Ok(image) => self.drag_image = Some(image),
            Err(error) => {
                self.named_error(window, &error);
                return false;
            }
        }
        self.begin_tab_slide();
        self.sessions.borrow_mut().tab_drag = Some(SharedTabDrag {
            source: self.session_id,
            tab: Rc::new(tab),
            slot: None,
            pointer_x: press.start.x,
            grab_x: press.grab.x,
        });
        if let Some(window) = window {
            window.request_frame();
        }
        true
    }

    fn detached_tab_gesture(
        &mut self,
        window: Option<&WindowHandle>,
        event: &MouseEvent,
        mut press: TabPress,
    ) {
        let Some(screen) = self.tab_screen_point(window, event.position) else {
            self.tab_press = Some(press);
            self.cancel_tab_drag(window);
            return;
        };
        let drag = press.drag.as_mut().unwrap();
        let (slot, previous) = {
            let mut sessions = self.sessions.borrow_mut();
            let previous = sessions.tab_drag.as_ref().and_then(|drag| drag.slot);
            let hit = sessions.tab_strip_at(window, screen);
            let (slot, pointer_x) = if let Some((id, strip)) = hit {
                let gap = previous
                    .filter(|slot| slot.window == id && slot.group == strip.group)
                    .map(|slot| slot.index);
                let (index, pointer_x) = strip.drag_position(
                    screen.x,
                    (id == self.session_id).then_some(press.sheet),
                    gap,
                );
                (
                    Some(DragSlot {
                        window: id,
                        group: strip.group,
                        index,
                    }),
                    pointer_x,
                )
            } else {
                (None, 0.)
            };
            if let Some(slot) = slot.filter(|slot| slot.window == self.session_id) {
                drag.group = slot.group;
            }
            let shared = sessions.tab_drag.as_mut().unwrap();
            shared.slot = slot;
            shared.pointer_x = pointer_x;
            if previous != slot {
                if let Some(previous) = previous {
                    sessions.request_window_frame(previous.window);
                }
                if let Some(slot) = slot
                    && previous.is_none_or(|previous| previous.window != slot.window)
                {
                    sessions.request_window_frame(slot.window);
                }
            }
            (slot, previous)
        };
        if event.kind == MouseKind::Up {
            self.clear_detached_tab(window);
            let result = match slot {
                Some(slot) if slot.window == self.session_id => {
                    self.commit_tab_drag(press.sheet, slot.group, slot.index);
                    Ok(())
                }
                Some(slot) => self.merge_dragged_tab(window, press.sheet, slot),
                None => {
                    let tab = screen_offset(window, screen, press.grab);
                    self.tear_off_tab(window, press.sheet, tab)
                }
            };
            if let Err(error) = result {
                self.named_error(window, &error);
            }
            self.named_changed(window, false);
            return;
        }

        if let Some(window) = window {
            match slot {
                None if drag.popup_loaded => window.move_drag_image(screen),
                None => {
                    if let Some(image) = &self.drag_image {
                        window.show_drag_image(
                            image,
                            screen,
                            Point {
                                x: press.grab.x + IMAGE_PAD,
                                y: press.grab.y + IMAGE_PAD,
                            },
                        );
                        drag.popup_loaded = true;
                    }
                }
                Some(_) if previous.is_none() && drag.popup_loaded => window.pause_drag_image(),
                Some(_) => {}
            }
        }
        self.tab_press = Some(press);
    }

    fn merge_dragged_tab(
        &mut self,
        window: Option<&WindowHandle>,
        sheet: u64,
        slot: DragSlot,
    ) -> Result<(), String> {
        self.move_tab_to_window_at(window, sheet, slot.window, slot.group, Some(slot.index))?;
        let target = {
            let mut sessions = self.sessions.borrow_mut();
            sessions.active_window = Some(slot.window);
            sessions.handles.get(&slot.window).cloned()
        };
        if let Some(target) = target {
            target.focus();
            target.request_frame();
        }
        Ok(())
    }

    fn commit_tab_drag(&mut self, sheet: u64, group: usize, index: usize) {
        let workbench = &mut self.workspace.workbench;
        let Some(source) = workbench.group_of(sheet) else {
            return;
        };
        if source != group {
            workbench.groups[source].remove(sheet);
        }
        let target = &mut workbench.groups[group];
        target.sheets = preview_order(&target.sheets, sheet, Some(index));
        if source != group
            && let Some(index) = self.workspace.sheet_index(sheet)
        {
            self.workspace.select(index);
        }
    }

    fn clear_detached_tab(&mut self, window: Option<&WindowHandle>) {
        self.drag_image = None;
        if let Some(window) = window {
            window.hide_drag_image();
        }
        self.sessions
            .borrow_mut()
            .cancel_window_tab_drag(self.session_id);
    }

    pub(super) fn cancel_tab_drag(&mut self, window: Option<&WindowHandle>) -> bool {
        let Some(press) = self.tab_press.take() else {
            return false;
        };
        if let Some(drag) = press.drag {
            self.begin_tab_slide();
            if drag.detached {
                self.clear_detached_tab(window);
            }
            if let Some(window) = window {
                window.request_frame();
            }
        }
        true
    }

    pub(super) fn cancel_orphaned_tab_drag(&mut self, window: Option<&WindowHandle>) -> bool {
        if self
            .tab_press
            .as_ref()
            .and_then(|press| press.drag)
            .is_some_and(|drag| drag.detached)
            && self
                .sessions
                .borrow()
                .tab_drag
                .as_ref()
                .is_none_or(|drag| drag.source != self.session_id)
        {
            return self.cancel_tab_drag(window);
        }
        false
    }

    pub(super) fn shared_tab_preview(&self) -> Option<SharedTabDrag> {
        self.sessions
            .borrow()
            .tab_drag
            .as_ref()
            .filter(|drag| drag.slot.is_some_and(|slot| slot.window == self.session_id))
            .cloned()
    }

    pub(super) fn tab_preview_order(&self, group: usize, sheets: &[u64]) -> Option<Vec<u64>> {
        if let Some(shared) = self.shared_tab_preview()
            && let Some(slot) = shared.slot.filter(|slot| slot.group == group)
        {
            return Some(preview_order(sheets, shared.tab.sheet, Some(slot.index)));
        }
        let press = self.tab_press.as_ref()?;
        let drag = press.drag?;
        ((!drag.detached && drag.group == group) || sheets.contains(&press.sheet)).then(|| {
            preview_order(
                sheets,
                press.sheet,
                (!drag.detached && drag.group == group).then_some(drag.index),
            )
        })
    }

    pub(super) fn lifted_tab_x(
        &self,
        shared: Option<&SharedTabDrag>,
        sheet: u64,
        clip: Rect,
        width: f64,
    ) -> Option<f64> {
        let x = if let Some(shared) = shared.filter(|drag| drag.tab.sheet == sheet) {
            shared.pointer_x - shared.grab_x.clamp(0., width)
        } else {
            let press = self
                .tab_press
                .as_ref()
                .filter(|press| press.sheet == sheet)?;
            let drag = press.drag.filter(|drag| !drag.detached)?;
            drag.x - press.grab.x.clamp(0., width)
        };
        Some(x.clamp(clip.x, clip.x + (clip.width - width).max(0.)))
    }

    pub(super) fn publish_tab_strips(&self, window: Option<&WindowHandle>) {
        let mut sessions = self.sessions.borrow_mut();
        // Settings and any non-editor surface are never merge destinations.
        if self.settings_window
            || self.workspace.settings_page.is_some()
            || self.tab_strips.is_empty()
        {
            sessions.tab_strips.remove(&self.session_id);
            return;
        }
        if let Some(window) = window {
            sessions
                .handles
                .entry(self.session_id)
                .or_insert_with(|| window.clone());
        }
        let x_scale = if cfg!(windows) {
            window.map_or(1., WindowHandle::scale_factor)
        } else {
            1.
        };
        let strips = sessions.tab_strips.entry(self.session_id).or_default();
        strips.truncate(self.tab_strips.len());
        for (index, strip) in self.tab_strips.iter().enumerate() {
            let (Some(top), Some(bottom)) = (
                self.tab_screen_point(
                    window,
                    Point {
                        x: strip.bounds.x,
                        y: strip.bounds.y,
                    },
                ),
                self.tab_screen_point(
                    window,
                    Point {
                        x: strip.bounds.x + strip.bounds.width,
                        y: strip.bounds.y + strip.bounds.height,
                    },
                ),
            ) else {
                strips.clear();
                return;
            };
            if index == strips.len() {
                strips.push(ScreenTabStrip::default());
            }
            let item = &mut strips[index];
            item.group = strip.group;
            item.bounds = Rect {
                x: top.x,
                y: top.y.min(bottom.y),
                width: (bottom.x - top.x).max(0.),
                height: (bottom.y - top.y).abs(),
            };
            item.view_x = strip.bounds.x;
            item.x_scale = x_scale;
            item.origin = top.x + (strip.origin() - strip.bounds.x) * x_scale;
            item.step = strip.step * x_scale;
            item.sheets
                .clone_from(&self.workspace.workbench.groups[strip.group].sheets);
        }
    }

    fn tab_screen_point(&self, window: Option<&WindowHandle>, point: Point) -> Option<Point> {
        if let Some(window) = window {
            return window.screen_point(point);
        }
        #[cfg(test)]
        if let Some([x, y, _, _]) = self.window_frame {
            return Some(Point {
                x: point.x + x,
                y: point.y + y,
            });
        }
        Some(point)
    }

    pub(super) fn begin_tab_slide(&mut self) {
        if !self.tab_animation_enabled() {
            self.tab_slides = None;
        } else {
            self.tab_slides = Some(TabSlides {
                start: Instant::now(),
                origins: self
                    .tab_painted
                    .iter()
                    .map(|&(sheet, group, x)| (sheet, (group, x)))
                    .collect(),
            });
        }
    }

    fn tab_animation_enabled(&self) -> bool {
        !self.accessibility_preferences.reduce_motion
            && self
                .workspace
                .editor()
                .settings
                .bool("animation_enabled", true)
    }

    pub(super) fn tab_slides_active(&self, now: Instant) -> bool {
        self.tab_animation_enabled()
            && self
                .tab_slides
                .as_ref()
                .is_some_and(|slides| now < slides.start + SLIDE_TIME)
    }

    pub(super) fn sliding_tab_x(&self, sheet: u64, group: usize, x: f64, now: Instant) -> f64 {
        let Some(slides) = &self.tab_slides else {
            return x;
        };
        let Some(&(old_group, old_x)) = slides.origins.get(&sheet) else {
            return x;
        };
        if old_group != group {
            return x;
        }
        let t = (now.saturating_duration_since(slides.start).as_secs_f64()
            / SLIDE_TIME.as_secs_f64())
        .clamp(0., 1.);
        x + (old_x - x) * (1. - t).powi(3)
    }

    pub(super) fn drag_tab_presentation(&self, index: usize) -> DragTab {
        let tab = &self.workspace.tabs[index];
        let label = tab.ui_label(&self.workspace.ui);
        let label = if let Some((read, total)) = tab.progress() {
            format!("{label} · {}%", read.saturating_mul(100) / total.max(1))
        } else if let Some(label) = label.strip_suffix(" •") {
            label.to_owned()
        } else {
            label
        };
        DragTab {
            sheet: tab.sheet,
            label,
            icon: crate::panels::tab_icon(&self.workspace, tab).map(str::to_owned),
            dirty: tab.dirty(),
            transient: tab.transient,
        }
    }

    // The same selected, lifted tab is used in a strip and in the transparent drag image.
    pub(super) fn paint_lifted_tab(
        &self,
        layer: &mut SceneLayer,
        tab: &DragTab,
        rect: Rect,
        clip: Rect,
    ) {
        let settings = &self.workspace.editor().settings;
        let mut theme = self.workspace.themes.ui.borrow_mut();
        let mut attrs = ["selected", "focused", "dragging", "", "", ""];
        let mut n = 4;
        attrs[3] = if self.workspace.dark {
            "file_dark"
        } else {
            "file_light"
        };
        for (enabled, attr) in [(tab.dirty, "dirty"), (tab.transient, "transient")] {
            if enabled {
                attrs[n] = attr;
                n += 1;
            }
        }
        let path = [
            vel_theme::Element::new("window", &[]),
            vel_theme::Element::new("tabset_control", &[]),
            vel_theme::Element::new("tab_control", &attrs[..n]),
        ];
        let style = theme.style(100 + tab.sheet, &vel_theme::ElementPath(&path), settings);
        theme.surface(layer, &style, rect, clip);
        let label = theme.style(
            crate::panels::StyleId::TabLabel.at(tab.sheet),
            &vel_theme::ElementPath(&[
                path[0],
                path[1],
                path[2],
                vel_theme::Element::new(
                    "tab_label",
                    if tab.dirty && settings.bool("highlight_modified_tabs", false) {
                        &["dirty"]
                    } else {
                        &[]
                    },
                ),
            ]),
            settings,
        );
        let s = self.panels.ui_scale;
        let narrow = rect.width < 48. * s;
        let close = settings.bool("show_tab_close_buttons", true) && !narrow;
        let left =
            close && (style.close_left || settings.bool("show_tab_close_buttons_on_left", false));
        let label_clip = vel_ui::minihtml::intersection(clip, rect);
        let inset = if left {
            30.
        } else if narrow {
            4.
        } else {
            10.
        };
        let mut x = rect.x + inset * s;
        if let Some(icon) = tab.icon.as_deref().filter(|_| !narrow) {
            theme.icon(
                layer,
                icon,
                Rect {
                    x,
                    y: rect.y + ((rect.height - 16. * s) / 2.).round(),
                    width: 16. * s,
                    height: 16. * s,
                },
                label_clip,
                label.foreground,
            );
            x += 22. * s;
        }
        x += 2. * s;
        let baseline = rect.y + f64::from(label.size) + 8. * s;
        let mut end = rect.x + rect.width - (if close && !left { 30. } else { 10. }) * s;
        if close {
            let style = theme.style(
                crate::panels::StyleId::TabClose.at(tab.sheet),
                &vel_theme::ElementPath(&[
                    path[0],
                    path[1],
                    path[2],
                    vel_theme::Element::new("tab_close_button", &[]),
                ]),
                settings,
            );
            theme.icon_surface(
                layer,
                &style,
                Rect {
                    x: if left {
                        rect.x + 6. * s
                    } else {
                        rect.x + rect.width - 26. * s
                    },
                    y: rect.y + ((rect.height - 20. * s) / 2.).round(),
                    width: 20. * s,
                    height: 20. * s,
                },
                label_clip,
            );
        } else if tab.dirty {
            end -= theme.advance(&label, "•");
            theme.text(layer, &label, "•", end, baseline, label_clip);
            end -= 3. * s;
        }
        let shown = theme.fit_label(&label, &tab.label, end - x);
        if !shown.is_empty() {
            theme.text(layer, &label, &shown, x, baseline, label_clip);
        }
    }

    fn render_drag_tab(
        &mut self,
        tab: &DragTab,
        strip: TabStrip,
        scale: f64,
    ) -> Result<DragImage, String> {
        let rect = Rect {
            x: IMAGE_PAD,
            y: IMAGE_PAD,
            width: strip.tab_width().min(strip.clip.width),
            height: strip.bounds.height,
        };
        let width = ((rect.width + 2. * IMAGE_PAD) * scale).ceil() as u32;
        let height = ((rect.height + 2. * IMAGE_PAD) * scale).ceil() as u32;
        let mut layer = SceneLayer::default();
        self.paint_lifted_tab(
            &mut layer,
            tab,
            rect,
            Rect {
                x: 0.,
                y: 0.,
                width: f64::from(width) / scale,
                height: f64::from(height) / scale,
            },
        );
        let scene = vel_render::scene::Scene {
            background: [0.; 4],
            layers: vec![layer],
        };
        let rgba = if let (Some(renderer), Some(fonts)) = (&mut self.renderer, &mut self.fonts) {
            renderer.render_rgba_at_size(&scene, fonts, scale as f32, [width, height])?
        } else {
            // A headless gesture still owns and releases a drag buffer, without requiring a GPU.
            #[cfg(not(test))]
            return Err("The tab drag renderer is not ready".into());
            #[cfg(test)]
            vec![0; width as usize * height as usize * 4]
        };
        Ok(DragImage {
            width,
            height,
            scale,
            rgba,
        })
    }
}

impl ScreenTabStrip {
    fn drag_position(
        &self,
        screen_x: f64,
        source_sheet: Option<u64>,
        gap: Option<usize>,
    ) -> (usize, f64) {
        // Removing the source sheet leaves a compact layout for a re-entry into its group.
        let count = self.sheets.len()
            - usize::from(source_sheet.is_some_and(|sheet| self.sheets.contains(&sheet)));
        // Count crossed midpoints without a scan over the destination's tabs.
        let gap = gap.map(|index| index.min(count));
        let crossed = (((screen_x - self.origin) / self.step + 0.5).floor() as usize)
            .min(count + usize::from(gap.is_some()));
        let index = crossed - usize::from(gap.is_some_and(|index| index < crossed));
        (
            index,
            self.view_x + (screen_x - self.bounds.x) / self.x_scale,
        )
    }
}

fn screen_offset(window: Option<&WindowHandle>, screen: Point, grab: Point) -> Point {
    // AppKit screen y increases upward. Win32 uses physical pixels, y increasing downward.
    if let Some(window) = window {
        return window.screen_point_offset(
            screen,
            Point {
                x: -grab.x,
                y: -grab.y,
            },
        );
    }
    Point {
        x: screen.x - grab.x,
        y: screen.y - grab.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_hit_testing_and_threshold_are_in_logical_points() {
        let bounds = Rect {
            x: 100.,
            y: 40.,
            width: 200.,
            height: 30.,
        };
        let strips = [TabStrip {
            group: 1,
            bounds,
            clip: bounds,
            step: 80.,
            scroll: 0.,
        }];
        assert_eq!(
            strip_at(&strips, Point { x: 299., y: 69. }).unwrap().group,
            1
        );
        assert!(strip_at(&strips, Point { x: 299., y: 70. }).is_none());
        assert!(strip_at(&strips, Point { x: 300., y: 60. }).is_none());
        let press = Point { x: 125., y: 50. };
        assert!(!over_threshold(press, Point { x: 127., y: 52. }));
        assert!(over_threshold(press, Point { x: 125., y: 54. }));
        assert!(!outside_detach_band(
            strips[0],
            Point { x: 150., y: 95. },
            500.,
        ));
        assert!(outside_detach_band(
            strips[0],
            Point { x: 150., y: 5. },
            500.,
        ));
        assert!(outside_detach_band(
            strips[0],
            Point { x: -31., y: 50. },
            500.,
        ));
        for (x, y) in [(150., 10.), (150., 100.), (-30., 50.), (530., 50.)] {
            assert!(!outside_detach_band(strips[0], Point { x, y }, 500.));
        }
        assert!(outside_detach_band(
            strips[0],
            Point { x: 150., y: 101. },
            500.
        ));
    }

    #[test]
    fn detached_tab_uses_scaled_scrolled_slots_and_skips_the_current_gap() {
        let strip = ScreenTabStrip {
            origin: 360.,
            step: 80.,
            bounds: Rect {
                x: 400.,
                ..Rect::default()
            },
            x_scale: 2.,
            view_x: 130.,
            sheets: vec![1, 2, 3],
            ..Default::default()
        };
        assert_eq!(strip.drag_position(430., None, None), (1, 145.));
        assert_eq!(strip.drag_position(500., None, Some(1)).0, 1);
        assert_eq!(strip.drag_position(560., None, Some(1)).0, 2);
        assert_eq!(strip.drag_position(900., Some(2), None).0, 2);
    }

    #[test]
    fn swaps_at_midpoints_without_oscillating_for_unequal_tabs() {
        for (dragged, other) in [(25., 140.), (175., 44.)] {
            let crossing = dragged + other / 2.;
            let index =
                |widths: [f64; 2], at, x| insertion_index(widths.into_iter(), 0., Some(at), x);
            assert_eq!(index([dragged, other], 0, crossing - 0.1), 0);
            assert_eq!(index([dragged, other], 0, crossing), 1);
            for x in [crossing, crossing - 1., other / 2.] {
                assert_eq!(
                    index([other, dragged], 1, x),
                    1,
                    "{dragged} / {other}, x={x}"
                );
            }
            assert_eq!(index([other, dragged], 1, other / 2. - 0.1), 0);
        }
    }
}
