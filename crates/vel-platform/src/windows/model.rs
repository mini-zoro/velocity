//! Win32 policy without window handles or native calls.
use crate::{Command, FrameReason, Modifiers, Point, Size, TextInput, Utf16Range};
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

pub(super) const BASE_DPI: u32 = 96;

pub(super) fn scale(dpi: u32) -> f64 {
    f64::from(if dpi == 0 { BASE_DPI } else { dpi }) / f64::from(BASE_DPI)
}

pub(super) fn pixels(points: f64, dpi: u32) -> i32 {
    (points * scale(dpi)).round() as i32
}

pub(super) fn logical_point(x: i32, y: i32, dpi: u32) -> Point {
    let scale = scale(dpi);
    Point {
        x: f64::from(x) / scale,
        y: f64::from(y) / scale,
    }
}

pub(super) fn logical_size(x: i32, y: i32, dpi: u32) -> Size {
    let point = logical_point(x, y, dpi);
    Size {
        width: point.x,
        height: point.y,
    }
}

pub(super) fn signed_pair(value: isize) -> (i32, i32) {
    (i32::from(value as i16), i32::from((value >> 16) as i16))
}

#[derive(Default)]
pub(super) struct Frames {
    dirty: Option<FrameReason>,
    deadline: Option<Instant>,
    next_tick: Option<Instant>,
    animating: bool,
}

impl Frames {
    pub(super) fn request(&mut self, reason: FrameReason) {
        if self.dirty.is_none() || reason != FrameReason::DisplayLink {
            self.dirty = Some(reason);
        }
    }

    pub(super) fn at(&mut self, deadline: Instant) {
        self.deadline = Some(self.deadline.map_or(deadline, |old| old.min(deadline)));
    }

    pub(super) fn animate(&mut self, active: bool) {
        self.animating = active;
    }

    pub(super) fn wake(&self, now: Instant) -> Option<Instant> {
        let paced = (self.dirty.is_some() || self.animating).then(|| self.next_tick.unwrap_or(now));
        self.deadline.into_iter().chain(paced).min()
    }

    pub(super) fn take(&mut self, now: Instant, period: Duration) -> Option<FrameReason> {
        let timer = self.deadline.is_some_and(|at| at <= now);
        if !timer && self.wake(now).is_none_or(|at| at > now) {
            return None;
        }
        if timer {
            self.deadline = None;
        }
        self.next_tick = Some(now + period);
        self.dirty.take().or(Some(if self.animating || !timer {
            FrameReason::DisplayLink
        } else {
            FrameReason::Timer
        }))
    }
}

// The shared keymap still consumes AppKit's physical codes for these keys. Do
// not put a Win32 scancode into that namespace; unknown printable keys use text.
pub(super) fn physical_key(vk: u16, scan: u8, extended: bool) -> u16 {
    match vk {
        VK_SHIFT if scan == 0x36 => 60,
        VK_SHIFT | VK_LSHIFT => 56,
        VK_RSHIFT => 60,
        VK_CONTROL if extended => 54,
        VK_CONTROL | VK_LCONTROL => 55,
        VK_RCONTROL => 54,
        VK_MENU if extended => 61,
        VK_MENU | VK_LMENU => 58,
        VK_RMENU => 61,
        VK_RETURN if extended => 76,
        VK_RETURN => 36,
        VK_SPACE => 49,
        VK_TAB => 48,
        VK_BACK => 51,
        VK_DELETE => 117,
        VK_ESCAPE => 53,
        VK_LEFT => 123,
        VK_RIGHT => 124,
        VK_DOWN => 125,
        VK_UP => 126,
        VK_HOME => 115,
        VK_END => 119,
        VK_PRIOR => 116,
        VK_NEXT => 121,
        VK_DECIMAL | VK_SEPARATOR => 65,
        VK_MULTIPLY => 67,
        VK_ADD => 69,
        VK_DIVIDE => 75,
        VK_SUBTRACT => 78,
        VK_NUMPAD0..=VK_NUMPAD9 => {
            [82, 83, 84, 85, 86, 87, 88, 89, 91, 92][usize::from(vk - VK_NUMPAD0)]
        }
        VK_F1..=VK_F15 => [
            122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113,
        ][usize::from(vk - VK_F1)],
        _ => 0x100 | u16::from(scan) | (u16::from(extended) << 9),
    }
}

pub(super) fn modifier_key(vk: u16, scan: u8, extended: bool) -> Option<u16> {
    match vk {
        VK_SHIFT if scan == 0x36 => Some(VK_RSHIFT),
        VK_SHIFT => Some(VK_LSHIFT),
        VK_CONTROL if extended => Some(VK_RCONTROL),
        VK_CONTROL => Some(VK_LCONTROL),
        VK_MENU if extended => Some(VK_RMENU),
        VK_MENU => Some(VK_LMENU),
        VK_LSHIFT | VK_RSHIFT | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU => Some(vk),
        _ => None,
    }
}

pub(super) fn alt_graph(state: &[u8; 256]) -> bool {
    is_down(state, VK_CONTROL) && is_down(state, VK_RMENU)
}

pub(super) fn is_down(state: &[u8; 256], vk: u16) -> bool {
    state[usize::from(vk)] & 0x80 != 0
}

pub(super) fn modifiers(state: &[u8; 256]) -> Modifiers {
    let mut flags = 0;
    for (left, right, aggregate, l_bit, r_bit, group) in [
        (VK_LSHIFT, VK_RSHIFT, VK_SHIFT, 0x2, 0x4, 1 << 17),
        (VK_LMENU, VK_RMENU, VK_MENU, 0x20, 0x40, 1 << 19),
        (VK_LCONTROL, VK_RCONTROL, VK_CONTROL, 0x8, 0x10, 1 << 20),
    ] {
        if alt_graph(state) && aggregate != VK_SHIFT {
            continue;
        }
        if is_down(state, left) {
            flags |= l_bit;
        }
        if is_down(state, right) {
            flags |= r_bit;
        }
        if is_down(state, aggregate) || is_down(state, left) || is_down(state, right) {
            flags |= group;
        }
    }
    Modifiers(flags)
}

pub(super) fn responder(vk: u16, m: Modifiers) -> Option<Command> {
    let extend = m.shift();
    let word = m.command();
    if m.option() {
        return None;
    }
    Some(match (vk, word) {
        (VK_BACK, false) => Command::DeleteBackward,
        (VK_DELETE, false) => Command::DeleteForward,
        (VK_BACK, true) => Command::DeleteWord(false),
        (VK_DELETE, true) => Command::DeleteWord(true),
        (VK_LEFT, false) => Command::Left(extend),
        (VK_RIGHT, false) => Command::Right(extend),
        (VK_LEFT, true) => Command::WordLeft(extend),
        (VK_RIGHT, true) => Command::WordRight(extend),
        (VK_UP, false) => Command::Up(extend),
        (VK_DOWN, false) => Command::Down(extend),
        (VK_HOME, false) => Command::Home(extend),
        (VK_END, false) => Command::End(extend),
        (VK_HOME, true) => Command::DocumentStart(extend),
        (VK_END, true) => Command::DocumentEnd(extend),
        (VK_PRIOR, true) => Command::ScrollPage(false),
        (VK_NEXT, true) => Command::ScrollPage(true),
        (VK_PRIOR, false) if extend => Command::SelectPage(false),
        (VK_NEXT, false) if extend => Command::SelectPage(true),
        (VK_PRIOR, false) => Command::PageUp,
        (VK_NEXT, false) => Command::PageDown,
        (VK_RETURN, false) => Command::Newline,
        (VK_TAB, false) if extend => Command::Backtab,
        (VK_TAB, false) => Command::Tab,
        (VK_ESCAPE, false) => Command::Cancel,
        (0x41, true) => Command::SelectAll,
        (0x43, true) => Command::Copy,
        (0x56, true) => Command::Paste,
        (0x58, true) => Command::Cut,
        (0x59, true) | (0x5a, true) if vk == 0x59 || extend => Command::Redo,
        (0x5a, true) => Command::Undo,
        _ => return None,
    })
}

// Sub-detent messages, typical of precision touchpads and high-resolution
// wheels, are kept as fractional point deltas. Win32 doesn't distinguish them.
pub(super) fn wheel(raw: i32, setting: u32, horizontal: bool, page: f64) -> (f64, bool) {
    let notches = f64::from(if horizontal { -raw } else { raw }) / 120.;
    let smooth = raw % 120 != 0;
    if setting == u32::MAX {
        (notches * page.max(1.), true)
    } else if smooth {
        (notches * f64::from(setting) * 16., true)
    } else {
        (notches * f64::from(setting), false)
    }
}

#[derive(Default)]
pub(super) struct Clicks {
    previous: Option<(usize, u32, (i32, i32))>,
    count: usize,
}

impl Clicks {
    pub(super) fn press(
        &mut self,
        button: usize,
        time: u32,
        position: (i32, i32),
        interval: u32,
        tolerance: (i32, i32),
    ) -> usize {
        self.count = if self.previous.is_some_and(|(old_button, old_time, p)| {
            button == old_button
                && time.wrapping_sub(old_time) <= interval
                && (p.0 - position.0).abs() <= tolerance.0 / 2
                && (p.1 - position.1).abs() <= tolerance.1 / 2
        }) {
            self.count % 3 + 1
        } else {
            1
        };
        self.previous = Some((button, time, position));
        self.count
    }
}

#[derive(Default)]
pub(super) struct Utf16Input(Option<u16>);

impl Utf16Input {
    pub(super) fn push(&mut self, unit: u16) -> Option<String> {
        let previous = self.0.take();
        if (0xd800..=0xdbff).contains(&unit) {
            self.0 = Some(unit);
            return previous.map(|_| "\u{fffd}".into());
        }
        if unit < 0x20 || unit == 0x7f {
            return previous.map(|_| "\u{fffd}".into());
        }
        if let Some(high) = previous {
            Some(String::from_utf16_lossy(&[high, unit]))
        } else {
            Some(String::from_utf16_lossy(&[unit]))
        }
    }
}

#[derive(Default)]
pub(super) struct Composition {
    pub(super) active: bool,
    marked: bool,
}

impl Composition {
    pub(super) fn start(&mut self) {
        self.active = true;
    }

    pub(super) fn update(
        &mut self,
        result: Option<String>,
        marked: Option<(String, usize)>,
        cancelled: bool,
    ) -> Vec<TextInput> {
        let mut events = Vec::new();
        if let Some(text) = result
            && !text.is_empty()
        {
            events.push(TextInput::Insert {
                text,
                replacement: None,
            });
            self.marked = false;
        }
        if let Some((text, cursor)) = marked.filter(|(text, _)| !text.is_empty()) {
            self.active = true;
            let cursor = cursor.min(text.encode_utf16().count());
            events.push(TextInput::Marked {
                text,
                selected: Utf16Range::new(cursor, 0),
                replacement: None,
            });
            self.marked = true;
        } else if cancelled {
            self.active = false;
            if std::mem::take(&mut self.marked) {
                events.push(TextInput::Discard);
            }
        }
        events
    }

    pub(super) fn end(&mut self, commit_marked: bool) -> Option<TextInput> {
        self.active = false;
        std::mem::take(&mut self.marked).then_some(if commit_marked {
            TextInput::Unmark
        } else {
            TextInput::Discard
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_converts_once_and_message_coordinates_sign_extend() {
        assert_eq!(pixels(100.5, 144), 151);
        assert_eq!(
            logical_size(1800, 1200, 144),
            Size {
                width: 1200.,
                height: 800.
            }
        );
        assert_eq!(logical_point(-24, 48, 192), Point { x: -12., y: 24. });
        assert_eq!(signed_pair(0xfff0_ffe0_u32 as isize), (-32, -16));
        assert_eq!(scale(0), 1.);
    }

    #[test]
    fn display_ticks_coalesce_and_deadlines_are_one_shot() {
        let now = Instant::now();
        let period = Duration::from_millis(16);
        let deadline = now + Duration::from_millis(50);
        let mut frames = Frames::default();
        frames.request(FrameReason::Resize);
        frames.request(FrameReason::DisplayLink);
        frames.at(deadline + period);
        frames.at(deadline);
        assert_eq!(frames.take(now, period), Some(FrameReason::Resize));
        assert_eq!(frames.wake(now), Some(deadline));
        frames.animate(true);
        assert_eq!(frames.wake(now), Some(now + period));
        assert_eq!(frames.take(now, period), None);
        assert_eq!(
            frames.take(now + period, period),
            Some(FrameReason::DisplayLink)
        );
        frames.animate(false);
        assert_eq!(frames.take(deadline, period), Some(FrameReason::Timer));
        assert_eq!(frames.wake(deadline), None);
    }

    #[test]
    fn ctrl_is_command_altgr_is_text_and_extended_enter_is_keypad() {
        let mut keys = [0; 256];
        keys[usize::from(VK_CONTROL)] = 0x80;
        keys[usize::from(VK_LCONTROL)] = 0x80;
        let flags = modifiers(&keys);
        assert!(flags.command() && !flags.control());
        assert!(flags.sides()[6]);
        assert_eq!(
            responder(VK_HOME, flags),
            Some(Command::DocumentStart(false))
        );
        keys[usize::from(VK_RMENU)] = 0x80;
        assert!(alt_graph(&keys));
        assert_eq!(modifiers(&keys), Modifiers::default());
        assert_eq!(physical_key(VK_RETURN, 0x1c, true), 76);
        assert_eq!(physical_key(VK_RETURN, 0x1c, false), 36);
        assert_eq!(physical_key(VK_F12, 0, false), 111);
    }

    #[test]
    fn scroll_and_click_fractions_survive_message_boundaries() {
        assert_eq!(wheel(120, 3, false, 250.), (3., false));
        assert_eq!(wheel(15, 3, false, 250.), (6., true));
        assert_eq!(wheel(120, 1, true, 250.), (-1., false));
        assert_eq!(wheel(-120, u32::MAX, false, 250.), (-250., true));
        let mut clicks = Clicks::default();
        assert_eq!(clicks.press(0, u32::MAX - 10, (1, 1), 500, (8, 8)), 1);
        assert_eq!(clicks.press(0, 5, (2, 1), 500, (8, 8)), 2);
        assert_eq!(clicks.press(0, 300, (3, 1), 500, (8, 8)), 3);
        assert_eq!(clicks.press(1, 301, (3, 1), 500, (8, 8)), 1);
    }

    #[test]
    fn ime_commit_replaces_marked_once_and_cancel_discards() {
        let mut ime = Composition::default();
        ime.start();
        let events = ime.update(None, Some(("ni".into(), 20)), false);
        assert_eq!(
            events,
            [TextInput::Marked {
                text: "ni".into(),
                selected: Utf16Range::new(2, 0),
                replacement: None,
            }]
        );
        assert_eq!(
            ime.update(Some("\u{4f60}".into()), None, false),
            [TextInput::Insert {
                text: "\u{4f60}".into(),
                replacement: None
            }]
        );
        assert_eq!(ime.end(false), None);
        // The final text may arrive after END. It is still an insert, not
        // a new composition that would swallow subsequent key bindings.
        assert_eq!(ime.update(Some("x".into()), None, false).len(), 1);
        assert!(!ime.active);
        ime.update(None, Some(("a".into(), 1)), false);
        assert_eq!(ime.end(false), Some(TextInput::Discard));
        ime.update(None, Some(("a".into(), 1)), false);
        assert_eq!(ime.end(true), Some(TextInput::Unmark));
        let mut chars = Utf16Input::default();
        assert_eq!(chars.push(0xd83e), None);
        assert_eq!(chars.push(0xdd80).as_deref(), Some("\u{1f980}"));
        assert_eq!(chars.push(0x08), None);
    }
}
