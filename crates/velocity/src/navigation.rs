//! Window-local, bounded jump history. Shared selection snapshots keep the hot hook O(1).
use super::Editor;
use crate::workspace::Workspace;
use std::hash::{Hash, Hasher};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};
use vel_buffer::{Bias, BufferId, ChangeId, Selection};
use vel_config::Value;

const MAX_JUMP_REGIONS: usize = 64;

#[derive(Clone, Debug)]
pub(crate) struct Jump {
    pub sheet: u64,
    pub buffer: BufferId,
    pub version: ChangeId,
    pub selection: Selection,
    pub caret_count: usize,
    pub scroll: (f64, f64),
    pub path: Option<Arc<PathBuf>>,
    pub position: vel_text::Point,
}
impl Jump {
    pub(crate) fn same_position(&self, other: &Self) -> bool {
        self.sheet == other.sheet
            && self.buffer == other.buffer
            && self.caret_count == other.caret_count
            && if self.caret_count <= MAX_JUMP_REGIONS {
                self.selection == other.selection
            } else {
                self.selection.primary() == other.selection.primary()
            }
    }
    fn compact(mut self) -> Self {
        if self.selection.len() > MAX_JUMP_REGIONS {
            self.selection = Selection::from_regions(self.selection.primary());
        }
        debug_assert!(self.caret_count >= self.selection.len());
        self
    }
    fn advance(&mut self, editor: &Editor, budget: usize) -> usize {
        let Some(maps) = editor.buffer.retained_changes_since(self.version) else {
            return 0;
        };
        // Records are already compact: at most 64 regions, independent of the
        // live view's caret count. Keep the primary and selection direction.
        let mut regions: Vec<_> = self.selection.iter().collect();
        let primary = self.selection.primary_index();
        let mut to = self.version;
        let mut work = 0;
        for (id, map) in maps.take(budget) {
            for region in &mut regions {
                region.a = map.transform_point(region.a, Bias::Right);
                region.b = map.transform_point(region.b, Bias::Right);
            }
            to = id;
            work += 1;
        }
        if to != self.version {
            let primary = regions.remove(primary);
            regions.push(primary);
            self.selection = Selection::from_regions(regions);
            self.version = to;
            if to == editor.buffer.change_id() {
                self.position = position(editor.buffer.text(), primary.b);
            }
        }
        work
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Editing,
    Explicit,
    Silent,
}
#[derive(Clone, Copy, Debug)]
struct Command {
    id: u64,
    class: Class,
    typing: bool,
}
#[derive(Debug, Default)]
pub(crate) struct History {
    pub back: VecDeque<Jump>,
    pub forward: VecDeque<Jump>,
    current: Option<Jump>,
    anchor: Option<Jump>,
    last_command: Option<Command>,
    command: Option<Command>,
    sequence: u64,
    selection_command: u64,
    last_click: Option<(f64, f64, usize)>,
    active: Option<u64>,
    unfocused: bool,
    requested: usize,
    maintenance_cursor: usize,
    pub notice: Option<String>,
    pub pending_restore: Option<u64>,
    suppressed: usize,
    #[cfg(test)]
    pub disabled: bool,
}
pub(crate) type Shared = Rc<RefCell<History>>;
pub(crate) fn position(text: &vel_text::Text, byte: usize) -> vel_text::Point {
    let row = text.byte_to_line(byte);
    vel_text::Point {
        row,
        column: text.byte_to_char(byte) - text.byte_to_char(text.line_to_byte(row)),
    }
}
pub(crate) fn position_byte(text: &vel_text::Text, point: vel_text::Point) -> usize {
    let row = point.row.min(text.len_lines() - 1);
    let start = text.line_to_byte(row);
    let end = if row + 1 < text.len_lines() {
        text.line_to_byte(row + 1).saturating_sub(1)
    } else {
        text.len_bytes()
    };
    text.char_to_byte((text.byte_to_char(start) + point.column).min(text.byte_to_char(end)))
}
struct Identity(u64);
impl Hasher for Identity {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }
}
fn command_id(name: &str, args: &Value) -> u64 {
    let mut hash = Identity(0xcbf29ce484222325);
    name.hash(&mut hash);
    args.hash(&mut hash);
    hash.finish()
}
fn class(name: &str) -> Class {
    if name.starts_with("goto_")
        || name.starts_with("jump_")
        || matches!(
            name,
            "find_next"
                | "find_prev"
                | "find_all"
                | "find_under"
                | "find_under_prev"
                | "find_result"
                | "next_result"
                | "prev_result"
                | "activate_view"
                | "open_file"
                | "next_bookmark"
                | "prev_bookmark"
                | "select_bookmarks"
                | "select_all_bookmarks"
                | "select_to_mark"
                | "swap_with_mark"
                | "drag_select"
        )
    {
        Class::Explicit
    } else {
        Class::Editing
    }
}
fn push(stack: &mut VecDeque<Jump>, jump: Jump) -> bool {
    if stack.back().is_some_and(|old| old.same_position(&jump)) {
        return false;
    }
    if stack.len() == 128 {
        stack.pop_front();
    }
    stack.push_back(jump.compact());
    true
}
impl History {
    pub fn observe(&mut self, next: Jump, fallback: &str) {
        if self.suppressed != 0
            || (self.unfocused && self.requested == 0)
            || self.active != Some(next.sheet)
            || self.pending_restore == Some(next.sheet)
        {
            return;
        }
        let command = self.command.unwrap_or(Command {
            id: if fallback == "selection" {
                self.selection_command
            } else {
                command_id(fallback, &Value::Null)
            },
            class: if fallback == "selection" {
                Class::Explicit
            } else {
                class(fallback)
            },
            typing: fallback == "insert",
        });
        if let Some(old) = self.current.take()
            && !old.same_position(&next)
        {
            // Jump records: https://www.sublimetext.com/download (4107, add_jump_record).
            // Stable command/argument identities merge editing runs. Explicit jumps
            // retain their origin; vi non-jump motions never create records.
            let new_view = old.sheet != next.sheet;
            let mut recorded = false;
            if command.class == Class::Silent && !new_view {
                // Deliberately no distance heuristic for counted vi motions.
            } else if command.class == Class::Editing && !new_view {
                let anchor = self.anchor.get_or_insert_with(|| old.clone().compact());
                let same_run = self
                    .last_command
                    .is_some_and(|last| last.id == command.id && last.class == command.class);
                let continuing_edit = self
                    .last_command
                    .is_some_and(|last| last.class == Class::Editing);
                if !same_run
                    && continuing_edit
                    && anchor.position.row.abs_diff(next.position.row) > 10
                {
                    recorded = push(&mut self.back, anchor.clone());
                    self.anchor = Some(next.clone().compact());
                } else if self.back.is_empty() && !command.typing {
                    recorded = push(&mut self.back, anchor.clone());
                }
            } else if new_view || self.last_command.is_none_or(|last| last.id != command.id) {
                recorded = push(&mut self.back, old);
                self.anchor = None;
            }
            if recorded {
                self.forward.clear();
            }
            self.last_command = Some(command);
        }
        self.current = Some(next);
    }
    pub fn remember(&mut self, jump: Jump) {
        push(&mut self.back, jump);
        self.forward.clear();
        self.anchor = None;
        self.last_command = None;
    }
    pub fn reset_current(&mut self, jump: Jump) {
        self.current = Some(jump);
        self.anchor = None;
        self.last_command = None;
    }
    pub fn installed(&mut self, jump: Jump) {
        if self.active == Some(jump.sheet) && self.suppressed == 0 {
            self.pending_restore = None;
            self.reset_current(jump);
        }
    }
    pub fn mouse_down(&mut self, x: f64, y: f64, button: usize, count: usize) {
        let continues = count > 1
            && self
                .last_click
                .is_some_and(|(last_x, last_y, last_button)| {
                    last_button == button && (x - last_x).powi(2) + (y - last_y).powi(2) <= 16.
                });
        if !continues {
            self.sequence = self.sequence.wrapping_add(1);
            self.selection_command = self.sequence;
        }
        self.last_click = Some((x, y, button));
    }
    fn rebase(&mut self, editor: &Editor, closing: bool) {
        if !closing && !editor.buffer.change_log_pressure() {
            return;
        }
        let buffer = editor.buffer.id();
        let version = editor.buffer.change_id();
        let len = self.back.len() + self.forward.len() + usize::from(self.anchor.is_some());
        if len == 0 {
            return;
        }
        let mut budget = if closing { usize::MAX } else { 256 };
        let visits = if closing { len } else { 4.min(len) };
        for _ in 0..visits {
            let index = self.maintenance_cursor % len;
            self.maintenance_cursor = self.maintenance_cursor.wrapping_add(1);
            let jump = if index < self.back.len() {
                self.back.get_mut(index)
            } else if index < self.back.len() + self.forward.len() {
                self.forward.get_mut(index - self.back.len())
            } else {
                self.anchor.as_mut()
            };
            if let Some(jump) = jump
                && jump.buffer == buffer
                && jump.version != version
            {
                let work = jump.advance(editor, budget.min(if closing { usize::MAX } else { 64 }));
                budget -= work;
            }
            if budget == 0 {
                break;
            }
        }
    }
}
/// Scoped command identity or suppression, including early returns and failed commands.
#[derive(Debug)]
pub(crate) struct Guard {
    history: Shared,
    previous: Option<Command>,
    suppress: bool,
    requested: bool,
}
impl Guard {
    pub fn command(history: &Shared, name: &str) -> Self {
        Self::command_args(history, name, &Value::Null)
    }
    pub fn command_args(history: &Shared, name: &str, args: &Value) -> Self {
        Self::with_class(history, name, args, class(name))
    }
    pub fn command_kind(history: &Shared, name: &str, non_jump: bool) -> Self {
        Self::with_class(
            history,
            name,
            &Value::Null,
            if non_jump {
                Class::Silent
            } else {
                Class::Explicit
            },
        )
    }
    fn with_class(history: &Shared, name: &str, args: &Value, class: Class) -> Self {
        let mut h = history.borrow_mut();
        let typing = matches!(
            name,
            "insert" | "insert_snippet" | "left_delete" | "right_delete"
        );
        let id = if class == Class::Explicit {
            h.sequence = h.sequence.wrapping_add(1);
            h.sequence
        } else if typing {
            // Text, auto-pairs, deletes and IME commits are a single typing run,
            // regardless of inserted characters or snippet arguments.
            command_id("typing", &Value::Null)
        } else {
            command_id(name, args)
        };
        let previous = h.command.replace(Command { id, class, typing });
        Self {
            history: history.clone(),
            previous,
            suppress: false,
            requested: false,
        }
    }
    fn request(history: &Shared) -> Self {
        let mut guard = Self::with_class(history, "open_file", &Value::Null, Class::Explicit);
        history.borrow_mut().requested += 1;
        guard.requested = true;
        guard
    }
    pub fn suppress(history: &Shared) -> Self {
        history.borrow_mut().suppressed += 1;
        Self {
            history: history.clone(),
            previous: None,
            suppress: true,
            requested: false,
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let mut history = self.history.borrow_mut();
        if self.suppress {
            history.suppressed -= 1;
        } else {
            if self.requested {
                history.requested -= 1;
            }
            history.command = self.previous;
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Observer {
    pub history: Shared,
    pub sheet: u64,
    pub scroll: (f64, f64),
    pub path: Option<Arc<PathBuf>>,
    pub rebased: Cell<ChangeId>,
}
impl Editor {
    pub(crate) fn navigation_snapshot(&self) -> Option<Jump> {
        self.navigation.as_ref().map(|observer| Jump {
            sheet: observer.sheet,
            buffer: self.buffer.id(),
            version: self.buffer.change_id(),
            selection: self.buffer.view(self.id).sel().clone(),
            caret_count: self.buffer.view(self.id).sel().len(),
            scroll: observer.scroll,
            path: observer.path.clone(),
            position: position(
                self.buffer.text(),
                self.buffer.view(self.id).sel().primary().map_or(0, |r| r.b),
            ),
        })
    }
    pub(crate) fn rebase_navigation(&self, closing: bool) {
        #[cfg(test)]
        let _timing = timing::Probe::start();
        #[cfg(test)]
        if self
            .navigation
            .as_ref()
            .is_some_and(|o| o.history.borrow().disabled)
        {
            return;
        }
        if let Some(observer) = &self.navigation
            && (closing || observer.rebased.get() != self.buffer.change_id())
        {
            observer.history.borrow_mut().rebase(self, closing);
            observer.rebased.set(self.buffer.change_id());
        }
    }
    pub(crate) fn observe_navigation(&self, command: &str) {
        #[cfg(test)]
        let _timing = timing::Probe::start();
        #[cfg(test)]
        if self
            .navigation
            .as_ref()
            .is_some_and(|o| o.history.borrow().disabled)
        {
            return;
        }
        self.rebase_navigation(false);
        if let Some(observer) = &self.navigation
            && let Some(jump) = self.navigation_snapshot()
        {
            observer.history.borrow_mut().observe(jump, command);
        }
    }
}
impl Workspace {
    pub(crate) fn navigation_request(&mut self) -> Guard {
        self.observe_navigation();
        let snapshot = crate::intelligence::capture(self);
        self.jump_history.borrow_mut().reset_current(snapshot);
        Guard::request(&self.jump_history)
    }
    pub(crate) fn navigation_focus(&mut self, focused: bool) {
        let was_unfocused = self.jump_history.borrow().unfocused;
        self.jump_history.borrow_mut().unfocused = !focused;
        if focused && was_unfocused {
            let current = crate::intelligence::capture(self);
            self.jump_history.borrow_mut().reset_current(current);
        }
        if focused {
            self.observe_navigation();
        }
    }
    pub(crate) fn observe_navigation(&mut self) {
        if self.empty {
            return;
        }
        self.refresh_appearance(self.active);
        let sheet = self.tabs[self.active].sheet;
        let old_sheet = self.jump_history.borrow().active;
        if let Some(old_sheet) = old_sheet.filter(|&old| old != sheet)
            && let Some(tab) = self.tabs.iter().find(|tab| tab.sheet == old_sheet)
            && let Some(current) = &mut self.jump_history.borrow_mut().current
        {
            current.scroll = tab.scroll;
        }
        self.jump_history.borrow_mut().active = Some(sheet);
        let tab = &mut self.tabs[self.active];
        if tab
            .editor
            .navigation
            .as_ref()
            .is_none_or(|o| o.sheet != sheet || o.path.as_deref() != tab.path.as_ref())
        {
            tab.editor.navigation = Some(Observer {
                history: self.jump_history.clone(),
                sheet,
                scroll: tab.scroll,
                path: tab.path.clone().map(Arc::new),
                rebased: Cell::new(tab.editor.buffer.change_id()),
            });
        }
        if let Some(observer) = &mut tab.editor.navigation {
            observer.scroll = tab.scroll;
        }
        if self.jump_history.borrow().pending_restore == Some(sheet)
            && tab.loading.is_none()
            && tab.lazy.is_none()
        {
            let snapshot = tab.editor.navigation_snapshot().expect("active observer");
            let mut history = self.jump_history.borrow_mut();
            history.pending_restore = None;
            history.reset_current(snapshot);
        }
        tab.editor.observe_navigation("activate_view");
    }
    pub(crate) fn jump_history(&mut self, forward: bool) {
        self.observe_navigation();
        let current = crate::intelligence::capture(self);
        let _guard = Guard::suppress(&self.jump_history);
        loop {
            let target = {
                let mut h = self.jump_history.borrow_mut();
                if forward {
                    h.forward.pop_back()
                } else {
                    h.back.pop_back()
                }
            };
            let Some(target) = target else {
                break;
            };
            if !crate::intelligence::restore(self, &target) {
                continue;
            }
            let restored = crate::intelligence::capture(self);
            if restored.same_position(&current) {
                continue;
            }
            let mut h = self.jump_history.borrow_mut();
            push(if forward { &mut h.back } else { &mut h.forward }, current);
            h.reset_current(restored);
            break;
        }
    }
}

/// Test-only timing of history hooks, excluding input/edit/layout work. Nested
/// observation/rebase calls are counted once, on the invoking test's UI thread.
#[cfg(test)]
pub(crate) mod timing {
    use std::{
        cell::Cell,
        time::{Duration, Instant},
    };
    thread_local! {
        static DEPTH: Cell<usize> = const { Cell::new(0) };
        static ELAPSED: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    }
    pub struct Probe(Option<Instant>);
    impl Probe {
        pub fn start() -> Self {
            let outer = DEPTH.with(|depth| {
                let outer = depth.get() == 0;
                depth.set(depth.get() + 1);
                outer
            });
            Self(outer.then(Instant::now))
        }
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            DEPTH.with(|depth| depth.set(depth.get() - 1));
            if let Some(start) = self.0 {
                ELAPSED.with(|elapsed| elapsed.set(elapsed.get() + start.elapsed()));
            }
        }
    }
    pub fn take() -> Duration {
        ELAPSED.with(|elapsed| elapsed.replace(Duration::ZERO))
    }
}
