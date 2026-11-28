//! Pointer capture, paint-only tab order, and the process-wide floating tab.
use super::*;
use crate::session::{DragImageHide, DragSlot, DragTab, ScreenTabStrip, SharedTabDrag};
use std::{collections::BTreeMap, rc::Rc};

const THRESHOLD: f64 = 4.;
const SLIDE_TIME: Duration = Duration::from_millis(150);
/// A dropped or cancelled tab eases into its slot over this long.
const LAND_TIME: Duration = Duration::from_millis(120);
/// The landed image outlives the receiving window's next presented frame.
const IMAGE_HIDE_DELAY: Duration = Duration::from_millis(34);
const IMAGE_HIDE_TIMEOUT: Duration = Duration::from_millis(250);
const IMAGE_PAD: f64 = 12.;
/// The floating tab closes the click threshold's lead on the pointer over this long.
const CATCH_UP: Duration = Duration::from_millis(200);

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
    /// `Some(true)` once the floating image is uploaded, still hidden.
    pub image: Option<bool>,
}

/// A tab drag after the threshold. The tab is always a floating image that
/// follows the pointer in both axes; strips only show the gap it would fill.
#[derive(Clone, Copy, Debug)]
pub(super) struct TabDrag {
    /// Where the tab sat when the drag began; Escape and in-place drops land here.
    pub home: DragSlot,
    /// The pointer has left the source strip's detach band, so an off-strip drop
    /// tears the tab off. It never changes how the tab follows the pointer.
    pub detached: bool,
    pub transferable: bool,
    /// Whether the floating image exists. Without it strips paint the tab in the gap.
    pub floating: bool,
    /// The pointer's latest screen point.
    pub pointer: Point,
    /// When `pointer` was current, in `MouseEvent::timestamp` units. A live read
    /// between renders can be newer than events still queued behind it.
    seen: f64,
    /// How far the pointer had moved when the drag began. The floating tab first
    /// covers the strip tab exactly, then closes this lead over `CATCH_UP`.
    lead: Point,
    started: Instant,
}

impl TabDrag {
    /// The screen point the floating image is anchored to: the pointer, less the
    /// part of the threshold lead not yet closed. The grab offset is in the image.
    pub(super) fn anchor(&self, now: Instant) -> Point {
        let k = 1. - smoothstep(self.progress(now));
        Point {
            x: self.pointer.x - self.lead.x * k,
            y: self.pointer.y - self.lead.y * k,
        }
    }

    fn progress(&self, now: Instant) -> f64 {
        now.saturating_duration_since(self.started).as_secs_f64() / CATCH_UP.as_secs_f64()
    }

    #[cfg(test)]
    pub(super) fn started_at(&mut self, at: Instant) {
        self.started = at;
    }

    /// Advances the anchor to `now`; once the lead is closed it is dropped for good.
    fn settle(&mut self, now: Instant) -> Point {
        let anchor = self.anchor(now);
        if self.progress(now) >= 1. {
            self.lead = Point::default();
        }
        anchor
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Landing {
    /// Insert into this slot once the floating tab reaches it.
    Insert(DragSlot),
    /// Fly back to where the drag began and leave the order alone.
    Return,
}

/// A released tab on its way into a slot.
#[derive(Clone, Copy, Debug)]
pub(super) struct TabLanding {
    pub sheet: u64,
    pub action: Landing,
    from: Point,
    to: Point,
    start: Instant,
    time: Duration,
    floating: bool,
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
            image: None,
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
        let Some(press) = self.tab_press.as_ref() else {
            return false;
        };
        if press.drag.is_none() {
            if event.kind == MouseKind::Up {
                // A click: nothing moved, so the prepared image goes away unseen.
                if let Some(press) = self.tab_press.take() {
                    self.release_tab_image(window, &press);
                }
                return true;
            }
            // No frame has rendered since the press yet, so render before the threshold.
            self.prepare_tab_image(window);
            if !self
                .tab_press
                .as_ref()
                .is_some_and(|press| over_threshold(press.start, event.position))
                || !self.start_tab_drag(window, event.position)
            {
                return true;
            }
        }
        self.tab_drag_gesture(window, event);
        true
    }

    /// Renders and uploads the floating tab while it is still hidden, normally in
    /// the frame after the press, so crossing the threshold only moves a window.
    pub(super) fn prepare_tab_image(&mut self, window: Option<&WindowHandle>) {
        let Some(press) = self
            .tab_press
            .as_ref()
            .filter(|press| press.image.is_none())
        else {
            return;
        };
        let (sheet, grab) = (press.sheet, press.grab);
        self.retire_landed_tab_image(window);
        let source = self.workspace.workbench.group_of(sheet).and_then(|group| {
            let strip = *self.tab_strips.iter().find(|strip| strip.group == group)?;
            Some((strip, self.workspace.sheet_index(sheet)?))
        });
        let loaded = source.is_some_and(|(strip, index)| {
            let tab = self.drag_tab_presentation(index);
            let scale = window.map_or(1., WindowHandle::scale_factor);
            match self.render_drag_tab(&tab, strip, scale) {
                Ok(image) => {
                    if let Some(window) = window {
                        let grab = Point {
                            x: grab.x + IMAGE_PAD,
                            y: grab.y + IMAGE_PAD,
                        };
                        window.load_drag_image(&image, grab);
                    }
                    true
                }
                // Most presses are clicks, so no message: without an image the
                // drag still works, painting the tab in its gap instead.
                Err(_) => false,
            }
        });
        if let Some(press) = &mut self.tab_press {
            press.image = Some(loaded);
        }
    }

    fn release_tab_image(&self, window: Option<&WindowHandle>, press: &TabPress) {
        if press.image == Some(true)
            && let Some(window) = window
        {
            window.hide_drag_image();
        }
    }

    /// Replaces the tab with its floating image at once. The strip keeps a gap
    /// where the tab was until the pointer finds another slot.
    fn start_tab_drag(&mut self, window: Option<&WindowHandle>, position: Point) -> bool {
        let Some(press) = self.tab_press.as_ref() else {
            return false;
        };
        let (sheet, start) = (press.sheet, press.start);
        let floating = press.image == Some(true);
        let (Some(from), Some(pointer)) = (
            self.tab_screen_point(window, start),
            self.tab_screen_point(window, position),
        ) else {
            return false;
        };
        let home = self.workspace.workbench.group_of(sheet).and_then(|group| {
            let index = self.workspace.workbench.groups[group]
                .sheets
                .iter()
                .position(|id| *id == sheet)?;
            Some(DragSlot {
                window: self.session_id,
                group,
                index,
            })
        });
        let (Some(home), Some(index)) = (home, self.workspace.sheet_index(sheet)) else {
            if let Some(press) = self.tab_press.take() {
                self.release_tab_image(window, &press);
            }
            return false;
        };
        let transferable = self.transferable_tab(index);
        let tab = Rc::new(self.drag_tab_presentation(index));
        self.sessions.borrow_mut().tab_drag = Some(SharedTabDrag {
            source: self.session_id,
            tab,
            slot: Some(home),
            floating,
        });
        self.tab_click = None;
        if let Some(press) = &mut self.tab_press {
            press.drag = Some(TabDrag {
                home,
                detached: false,
                transferable,
                floating,
                pointer,
                seen: f64::NEG_INFINITY,
                lead: Point {
                    x: pointer.x - from.x,
                    y: pointer.y - from.y,
                },
                started: Instant::now(),
            });
        }
        if let Some(window) = window {
            window.request_frame();
            // Pointer moves close the lead; this frame closes it if the pointer stops.
            if floating {
                window.request_frame_at(Instant::now() + CATCH_UP);
            }
        }
        true
    }

    /// Moves the floating tab 1:1 with the pointer in both axes and tracks the
    /// slot under it in any window. Strips only repaint when that slot changes.
    fn tab_drag_gesture(&mut self, window: Option<&WindowHandle>, event: &MouseEvent) {
        let Some(screen) = self.tab_screen_point(window, event.position) else {
            self.cancel_tab_drag(window);
            return;
        };
        let Some((sheet, grab, mut drag)) = self
            .tab_press
            .as_ref()
            .and_then(|press| Some((press.sheet, press.grab, press.drag?)))
        else {
            return;
        };
        let previous = self
            .sessions
            .borrow()
            .tab_drag
            .as_ref()
            .and_then(|drag| drag.slot);
        let hit = self
            .sessions
            .borrow()
            .tab_strip_at(window, screen)
            .filter(|(id, _)| drag.transferable || *id == self.session_id)
            .map(|(id, strip)| {
                let gap = previous
                    .filter(|slot| slot.window == id && slot.group == strip.group)
                    .map(|slot| slot.index);
                let source = (id == self.session_id).then_some(sheet);
                DragSlot {
                    window: id,
                    group: strip.group,
                    index: strip.drag_position(screen.x, source, gap),
                }
            });
        if hit.is_none() && !drag.detached {
            let width = self.window_size.width;
            drag.detached = self
                .tab_strips
                .iter()
                .find(|strip| strip.group == drag.home.group)
                .is_some_and(|strip| outside_detach_band(*strip, event.position, width));
        }
        // Inside the detach band an off-strip pointer keeps the last gap here.
        let slot = hit
            .or_else(|| previous.filter(|slot| !drag.detached && slot.window == self.session_id));
        self.set_shared_slot(slot);
        // An event older than a live read already shown would pull the tab back.
        let fresh = event.timestamp >= drag.seen;
        if fresh {
            drag.pointer = screen;
            drag.seen = event.timestamp;
        }
        if event.kind == MouseKind::Up {
            self.tab_press = None;
            self.drop_tab(window, sheet, grab, drag, hit);
            return;
        }
        if drag.floating
            && fresh
            && let Some(window) = window
        {
            window.move_drag_image(drag.settle(Instant::now()));
        }
        if let Some(press) = &mut self.tab_press {
            press.drag = Some(drag);
        }
    }

    fn set_shared_slot(&self, slot: Option<DragSlot>) {
        let mut sessions = self.sessions.borrow_mut();
        let Some(shared) = sessions.tab_drag.as_mut() else {
            return;
        };
        let previous = std::mem::replace(&mut shared.slot, slot);
        if previous != slot {
            for window in previous.into_iter().chain(slot).map(|slot| slot.window) {
                sessions.request_window_frame(window);
            }
        }
    }

    fn drop_tab(
        &mut self,
        window: Option<&WindowHandle>,
        sheet: u64,
        grab: Point,
        drag: TabDrag,
        hit: Option<DragSlot>,
    ) {
        let action = match hit {
            Some(slot) => Landing::Insert(slot),
            None if drag.detached && drag.transferable => {
                // Off every strip and out of the band: a new window, exactly where the tab is.
                self.end_shared_tab_drag(window);
                let anchor = screen_offset(window, drag.pointer, grab);
                if let Err(error) = self.tear_off_tab(window, sheet, anchor) {
                    self.named_error(window, &error);
                }
                self.named_changed(window, false);
                return;
            }
            None => Landing::Return,
        };
        self.land_tab(window, sheet, grab, drag, action);
    }

    /// Eases the floating tab into its slot. The order changes only when it arrives.
    fn land_tab(
        &mut self,
        window: Option<&WindowHandle>,
        sheet: u64,
        grab: Point,
        drag: TabDrag,
        action: Landing,
    ) {
        let slot = match action {
            Landing::Insert(slot) => slot,
            Landing::Return => drag.home,
        };
        self.set_shared_slot(Some(slot));
        let start = Instant::now();
        let from = drag.anchor(start);
        let to = self.slot_anchor(window, slot, grab).unwrap_or(from);
        let time = if drag.floating && self.tab_animation_enabled() {
            LAND_TIME
        } else {
            Duration::ZERO
        };
        self.tab_landing = Some(TabLanding {
            sheet,
            action,
            from,
            to,
            start,
            time,
            floating: drag.floating,
        });
        self.advance_tab_landing(window, start);
    }

    /// The pointer position that puts the floating tab exactly on `slot`.
    fn slot_anchor(
        &self,
        window: Option<&WindowHandle>,
        slot: DragSlot,
        grab: Point,
    ) -> Option<Point> {
        let sessions = self.sessions.borrow();
        let strip = sessions
            .tab_strips
            .get(&slot.window)?
            .iter()
            .find(|strip| strip.group == slot.group)?;
        let left = Point {
            x: strip.origin + strip.step * slot.index as f64,
            y: strip.top_y,
        };
        Some(screen_offset(
            window,
            left,
            Point {
                x: -grab.x,
                y: -grab.y,
            },
        ))
    }

    /// Closes the threshold lead between pointer events, from the frame loop.
    /// Puts the floating tab under the pointer's live location, from the frame loop
    /// before and after rendering: events queued while a frame renders would only
    /// arrive after it, so the tab would skip a frame and then lurch.
    pub(super) fn follow_live_pointer(&mut self, window: Option<&WindowHandle>) {
        let Some(window) = window else {
            return;
        };
        let Some(drag) = self
            .tab_press
            .as_mut()
            .and_then(|press| press.drag.as_mut())
            .filter(|drag| drag.floating)
        else {
            return;
        };
        if let Some((point, at)) = window.pointer_screen_location()
            && at > drag.seen
        {
            drag.pointer = point;
            drag.seen = at;
            window.move_drag_image(drag.settle(Instant::now()));
        }
    }

    /// Closes the rest of the threshold lead when the pointer stopped during it.
    pub(super) fn catch_up_tab_drag(&mut self, window: Option<&WindowHandle>, now: Instant) {
        let Some(drag) = self
            .tab_press
            .as_mut()
            .and_then(|press| press.drag.as_mut())
            .filter(|drag| drag.floating && drag.lead != Point::default())
        else {
            return;
        };
        let anchor = drag.settle(now);
        let pending = drag.lead != Point::default();
        let due = drag.started + CATCH_UP;
        if let Some(window) = window {
            window.move_drag_image(anchor);
            if pending {
                window.request_frame_at(due);
            }
        }
    }

    /// Steps a released tab toward its slot from the frame loop. On arrival the
    /// tab is inserted, and its image stays until the receiving window has
    /// presented the inserted tab underneath it.
    pub(super) fn advance_tab_landing(&mut self, window: Option<&WindowHandle>, now: Instant) {
        let Some(landing) = self.tab_landing else {
            return;
        };
        let live = self
            .sessions
            .borrow()
            .tab_drag
            .as_ref()
            .is_some_and(|drag| drag.source == self.session_id);
        let elapsed = now.saturating_duration_since(landing.start);
        if live && elapsed < landing.time {
            let t = ease_out(elapsed.as_secs_f64() / landing.time.as_secs_f64());
            if let Some(window) = window {
                window.move_drag_image(Point {
                    x: landing.from.x + (landing.to.x - landing.from.x) * t,
                    y: landing.from.y + (landing.to.y - landing.from.y) * t,
                });
                window.request_frame();
            }
            return;
        }
        self.tab_landing = None;
        if !live {
            // The receiving window went away mid-flight; the tab stays where it was.
            if let Some(window) = window {
                window.hide_drag_image();
                window.request_frame();
            }
            return;
        }
        if landing.floating
            && let Some(window) = window
        {
            window.move_drag_image(landing.to);
        }
        {
            let mut sessions = self.sessions.borrow_mut();
            if let Some(slot) = sessions.tab_drag.take().and_then(|drag| drag.slot) {
                sessions.request_window_frame(slot.window);
            }
            sessions.request_window_frame(self.session_id);
        }
        let result = match landing.action {
            Landing::Insert(slot) if slot.window == self.session_id => {
                self.commit_tab_drag(landing.sheet, slot.group, slot.index);
                Ok(self.session_id)
            }
            Landing::Insert(slot) => self
                .merge_dragged_tab(window, landing.sheet, slot)
                .map(|()| slot.window),
            Landing::Return => Ok(self.session_id),
        };
        match result {
            Ok(receiver) if landing.floating => {
                self.sessions.borrow_mut().drag_image_hide = Some(DragImageHide {
                    window: receiver,
                    after: now + IMAGE_HIDE_DELAY,
                    rendered: false,
                });
            }
            Ok(_) => {}
            Err(error) => {
                if let Some(window) = window {
                    window.hide_drag_image();
                }
                self.named_error(window, &error);
            }
        }
        if let Some(window) = window {
            window.request_frame();
        }
        self.named_changed(window, false);
    }

    /// Hides a landed tab's image once its receiving window has presented the
    /// inserted tab. Every other window keeps a deadline frame, so the image can
    /// never stay behind when the receiver cannot present.
    pub(super) fn release_landed_tab_image(
        &mut self,
        window: Option<&WindowHandle>,
        now: Instant,
        presented: bool,
    ) {
        let mut sessions = self.sessions.borrow_mut();
        let Some(hide) = sessions.drag_image_hide.as_mut() else {
            return;
        };
        let deadline = hide.after + IMAGE_HIDE_TIMEOUT;
        let receiver = hide.window == self.session_id && presented;
        if now >= deadline || (receiver && hide.rendered && now >= hide.after) {
            sessions.drag_image_hide = None;
            drop(sessions);
            if let Some(window) = window {
                window.hide_drag_image();
            }
            return;
        }
        let next = if receiver {
            hide.rendered = true;
            hide.after
        } else {
            deadline
        };
        drop(sessions);
        if let Some(window) = window {
            window.request_frame_at(next);
        }
    }

    /// Completes a released drop at once, for input that cannot wait for it.
    /// The image still waits for the receiving window's next frame.
    pub(super) fn finish_tab_landing(&mut self, window: Option<&WindowHandle>) {
        if let Some(landing) = self.tab_landing {
            self.advance_tab_landing(window, landing.start + landing.time);
        }
    }

    /// A new press needs the popup: retire a landed image still waiting for its frame.
    fn retire_landed_tab_image(&self, window: Option<&WindowHandle>) {
        let pending = self.sessions.borrow_mut().drag_image_hide.take();
        if pending.is_some()
            && let Some(window) = window
        {
            window.hide_drag_image();
        }
    }

    fn end_shared_tab_drag(&mut self, window: Option<&WindowHandle>) {
        if let Some(window) = window {
            window.hide_drag_image();
        }
        self.sessions
            .borrow_mut()
            .cancel_window_tab_drag(self.session_id);
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

    /// Ends a gesture without dropping. A released tab still lands, at once.
    pub(super) fn cancel_tab_drag(&mut self, window: Option<&WindowHandle>) -> bool {
        self.finish_tab_landing(window);
        let Some(press) = self.tab_press.take() else {
            return false;
        };
        if press.drag.is_some() {
            self.begin_tab_slide();
            self.end_shared_tab_drag(window);
            if let Some(window) = window {
                window.request_frame();
            }
        } else {
            self.release_tab_image(window, &press);
        }
        true
    }

    /// Escape: the floating tab eases back into its original slot.
    pub(super) fn return_tab_drag(&mut self, window: Option<&WindowHandle>) -> bool {
        let Some((sheet, grab, drag)) = self
            .tab_press
            .as_ref()
            .and_then(|press| Some((press.sheet, press.grab, press.drag?)))
        else {
            return self.cancel_tab_drag(window);
        };
        self.tab_press = None;
        self.land_tab(window, sheet, grab, drag, Landing::Return);
        true
    }

    pub(super) fn cancel_orphaned_tab_drag(&mut self, window: Option<&WindowHandle>) -> bool {
        if self
            .tab_press
            .as_ref()
            .is_some_and(|press| press.drag.is_some())
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
        let sessions = self.sessions.borrow();
        let drag = sessions.tab_drag.as_ref()?;
        if let Some(slot) = drag
            .slot
            .filter(|slot| slot.window == self.session_id && slot.group == group)
        {
            return Some(preview_order(sheets, drag.tab.sheet, Some(slot.index)));
        }
        // The tab is over another strip or off every strip: its own strip closes up.
        (drag.source == self.session_id && sheets.contains(&drag.tab.sheet))
            .then(|| preview_order(sheets, drag.tab.sheet, None))
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
            item.top_y = top.y;
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
    fn drag_position(&self, screen_x: f64, source_sheet: Option<u64>, gap: Option<usize>) -> usize {
        // Removing the source sheet leaves a compact layout for a re-entry into its group.
        let count = self.sheets.len()
            - usize::from(source_sheet.is_some_and(|sheet| self.sheets.contains(&sheet)));
        // Count crossed midpoints without a scan over the destination's tabs.
        let gap = gap.map(|index| index.min(count));
        let crossed = (((screen_x - self.origin) / self.step + 0.5).floor() as usize)
            .min(count + usize::from(gap.is_some()));
        crossed - usize::from(gap.is_some_and(|index| index < crossed))
    }
}

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn ease_out(t: f64) -> f64 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
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
    fn threshold_and_detach_band_are_in_logical_points() {
        let bounds = Rect {
            x: 100.,
            y: 40.,
            width: 200.,
            height: 30.,
        };
        let strip = TabStrip {
            group: 1,
            bounds,
            clip: bounds,
            step: 80.,
            scroll: 0.,
        };
        let press = Point { x: 125., y: 50. };
        assert!(!over_threshold(press, Point { x: 127., y: 52. }));
        assert!(over_threshold(press, Point { x: 125., y: 54. }));
        assert!(!outside_detach_band(strip, Point { x: 150., y: 95. }, 500.));
        assert!(outside_detach_band(strip, Point { x: 150., y: 5. }, 500.));
        assert!(outside_detach_band(strip, Point { x: -31., y: 50. }, 500.));
        for (x, y) in [(150., 10.), (150., 100.), (-30., 50.), (530., 50.)] {
            assert!(!outside_detach_band(strip, Point { x, y }, 500.));
        }
        assert!(outside_detach_band(strip, Point { x: 150., y: 101. }, 500.));
    }

    #[test]
    fn screen_slots_use_scaled_scrolled_steps_and_skip_the_current_gap() {
        let strip = ScreenTabStrip {
            origin: 360.,
            step: 80.,
            bounds: Rect {
                x: 400.,
                ..Rect::default()
            },
            sheets: vec![1, 2, 3],
            ..Default::default()
        };
        assert_eq!(strip.drag_position(430., None, None), 1);
        assert_eq!(strip.drag_position(500., None, Some(1)), 1);
        assert_eq!(strip.drag_position(560., None, Some(1)), 2);
        assert_eq!(strip.drag_position(900., Some(2), None), 2);
    }

    /// The in-window reorder the floating drag replaced: midpoint crossings in
    /// the unanimated layout with the dragged tab occupying its current gap.
    fn former_insertion_index(
        widths: &[f64],
        mut left: f64,
        current: Option<usize>,
        x: f64,
    ) -> usize {
        let mut insertion = 0;
        for (i, width) in widths.iter().enumerate() {
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

    #[test]
    fn own_strip_slots_match_the_former_in_window_reorder() {
        let (origin, step) = (37.5, 80.);
        for count in 1..=5usize {
            let sheets: Vec<u64> = (1..=count as u64).collect();
            let strip = ScreenTabStrip {
                origin,
                step,
                sheets: sheets.clone(),
                ..Default::default()
            };
            let widths = vec![step; count];
            for &dragged in &sheets {
                for gap in 0..count {
                    let mut x = origin - step;
                    while x < origin + step * (count + 1) as f64 {
                        assert_eq!(
                            strip.drag_position(x, Some(dragged), Some(gap)),
                            former_insertion_index(&widths, origin, Some(gap), x),
                            "count={count} dragged={dragged} gap={gap} x={x}"
                        );
                        x += 2.5;
                    }
                }
            }
        }
    }

    #[test]
    fn landing_eases_out_from_the_release_point_and_ends_exactly_on_the_slot() {
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
        assert_eq!(ease_out(2.), 1.);
        let mut last = 0.;
        for step in 1..=12 {
            let t = ease_out(step as f64 / 12.);
            assert!(t > last, "ease_out must keep moving toward the slot");
            last = t;
        }
        assert!(ease_out(0.5) > 0.8, "most of the distance is covered early");
    }
}
