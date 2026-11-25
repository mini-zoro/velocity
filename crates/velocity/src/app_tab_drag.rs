//! Pointer capture and a reversible, paint-only tab order during a drag.
use super::*;
use std::collections::BTreeMap;

const THRESHOLD: f64 = 4.;
const SLIDE_TIME: Duration = Duration::from_millis(150);

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
}

#[derive(Debug)]
pub(super) struct TabPress {
    pub sheet: u64,
    start: Point,
    grab: f64,
    pub drag: Option<TabDrag>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TabDrag {
    pub group: usize,
    index: usize,
    x: f64,
}

#[derive(Debug)]
pub(super) struct TabSlides {
    start: Instant,
    origins: BTreeMap<u64, (usize, f64)>,
}

fn over_threshold(start: Point, point: Point) -> bool {
    (point.x - start.x).powi(2) + (point.y - start.y).powi(2) >= THRESHOLD.powi(2)
}

fn strip_at(strips: &[TabStrip], point: Point) -> Option<TabStrip> {
    strips
        .iter()
        .copied()
        .find(|s| vel_ui::contains(s.bounds, point))
}

// Slots are the unanimated layout with the dragged tab occupying its current gap.
// For arbitrary widths, crossing to the right moves that neighbour *left* of
// the pointer; crossing back requires passing its new midpoint, not its edge.
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
        let grab = point.x - left;
        self.tab_press = Some(TabPress {
            sheet,
            start: point,
            grab,
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
        let Some(mut press) = self.tab_press.take() else {
            return false;
        };
        let Some(source) = self.workspace.workbench.group_of(press.sheet) else {
            return true;
        };
        let order = &self.workspace.workbench.groups[source].sheets;
        let Some(index) = order.iter().position(|id| *id == press.sheet) else {
            return true;
        };
        let Some(mut drag) = press.drag.or_else(|| {
            (event.kind == MouseKind::Drag && over_threshold(press.start, event.position))
                .then_some(TabDrag {
                    group: source,
                    index,
                    x: press.start.x,
                })
        }) else {
            if event.kind == MouseKind::Drag {
                self.tab_press = Some(press);
            }
            return true;
        };
        self.tab_click = None;
        let strip = strip_at(&self.tab_strips, event.position);
        let previous = (drag.group, drag.index);
        if let Some(strip) = strip {
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
        drag.x = event.position.x;
        if event.kind == MouseKind::Up {
            self.begin_tab_slide();
            if strip.is_some() {
                self.commit_tab_drag(press.sheet, source, drag);
            } else {
                let position = window.and_then(|w| w.screen_point(event.position));
                if let Err(error) = self.tear_off_tab(window, press.sheet, position) {
                    self.named_error(window, &error);
                }
            }
            self.named_changed(window, false);
        } else {
            if previous != (drag.group, drag.index) {
                self.begin_tab_slide();
            }
            press.drag = Some(drag);
            self.tab_press = Some(press);
            if let Some(window) = window {
                window.request_frame();
            }
        }
        true
    }

    fn commit_tab_drag(&mut self, sheet: u64, source: usize, drag: TabDrag) {
        let workbench = &mut self.workspace.workbench;
        if source != drag.group {
            workbench.groups[source].remove(sheet);
        }
        let target = &mut workbench.groups[drag.group];
        target.sheets = preview_order(&target.sheets, sheet, Some(drag.index));
        if source != drag.group
            && let Some(index) = self.workspace.sheet_index(sheet)
        {
            self.workspace.select(index);
        }
    }

    pub(super) fn cancel_tab_drag(&mut self, window: Option<&WindowHandle>) -> bool {
        let Some(press) = self.tab_press.take() else {
            return false;
        };
        if press.drag.is_some() {
            self.begin_tab_slide();
            if let Some(window) = window {
                window.request_frame();
            }
        }
        true
    }

    pub(super) fn tab_preview_order(&self, group: usize, sheets: &[u64]) -> Option<Vec<u64>> {
        let press = self.tab_press.as_ref()?;
        let drag = press.drag?;
        (drag.group == group || sheets.contains(&press.sheet)).then(|| {
            preview_order(
                sheets,
                press.sheet,
                (drag.group == group).then_some(drag.index),
            )
        })
    }

    pub(super) fn lifted_tab_x(&self, sheet: u64, clip: Rect, width: f64) -> Option<f64> {
        let press = self
            .tab_press
            .as_ref()
            .filter(|press| press.sheet == sheet)?;
        let drag = press.drag?;
        Some(
            (drag.x - press.grab.clamp(0., width))
                .clamp(clip.x, clip.x + (clip.width - width).max(0.)),
        )
    }

    fn begin_tab_slide(&mut self) {
        if self.accessibility_preferences.reduce_motion
            || !self
                .workspace
                .editor()
                .settings
                .bool("animation_enabled", true)
        {
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

    pub(super) fn tab_slides_active(&self, now: Instant) -> bool {
        !self.accessibility_preferences.reduce_motion
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
