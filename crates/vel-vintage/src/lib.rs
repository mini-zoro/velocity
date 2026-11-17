#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
mod insert;
mod motion;
pub use insert::InsertChange;
use motion::{Motion, bol, end, first, line_range, normal};
use std::{collections::BTreeMap, ops::Range};
use vel_buffer::{
    Bias, Buffer, BufferId, ChangeId, Edit, EditKind, Region, Selection, UndoMarker,
    VerticalLayout, ViewId,
};

/// A decoded editor key. Text input carries complete Unicode graphemes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// One character from a key event.
    Char(char),
    /// Committed text from an input method or paste event.
    Text(String),
    /// Escape/cancel.
    Escape,
    /// Return/accept prompt.
    Enter,
    /// Delete the preceding grapheme.
    Backspace,
    /// A control chord, with a lowercase character.
    Ctrl(char),
    /// An Alt/Option chord for sub-word motions.
    Alt(char),
    /// A committed ordinary-editor insertion or selection motion.
    HostInsert(InsertChange),
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
}
impl From<char> for Key {
    fn from(c: char) -> Self {
        Self::Char(c)
    }
}
/// The mode exposed to keymap context predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Text insertion.
    Insert,
    /// Vi command mode.
    Normal,
    /// Inclusive character selection.
    VisualChar,
    /// Whole line selection.
    VisualLine,
    /// Rectangular selection represented by one region per line.
    VisualBlock,
    /// Overwrite graphemes until Escape.
    Replace,
    /// Ex command or search prompt.
    CommandLine,
}
/// Public Vintage settings, independent of editor configuration storage.
#[derive(Debug, Clone, Copy, Default)]
pub struct Settings {
    /// Corresponds to `vintage_start_in_command_mode` (default false).
    pub vintage_start_in_command_mode: bool,
    /// Enables control chords such as Ctrl-R, Ctrl-V, Ctrl-F/B/D/U.
    pub vintage_ctrl_keys: bool,
    /// Bridges the unnamed and `+` registers to the host clipboard.
    pub vintage_use_clipboard: bool,
}
/// A host operation. File paths and clipboard text are ordinary argument values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Functional command name (`save`, `close`, `save_and_close`, `open_file`, `reindent`, `set_clipboard`).
    pub name: &'static str,
    /// Arguments for the host command dispatcher.
    pub args: BTreeMap<String, String>,
}
/// Result of key dispatch. A requested command may accompany a completed edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Key consumed; update selections and context predicates.
    Handled,
    /// Key belongs to the host keymap.
    PassThrough,
    /// Dispatch the contained command through the host's ordinary command system.
    NeedsCommand(Command),
}
/// Register selection shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterKind {
    /// Inline text.
    Character,
    /// Whole lines (fragments end in LF).
    Line,
    /// One fragment per rectangle row.
    Block,
}
/// Owned register contents, retaining one fragment per original cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Register {
    /// Text from each original selection.
    pub fragments: Vec<String>,
    /// Paste shape.
    pub kind: RegisterKind,
}
#[derive(Debug, Clone)]
struct Group {
    // Transactions, not native undo groups: continuous typing can merge them.
    steps: usize,
    before: Selection,
    after: Selection,
}
#[derive(Debug, Clone)]
struct Replacement {
    range: Range<usize>,
    text: String,
    caret: usize,
}
#[derive(Debug, Clone, Copy)]
struct Find {
    target: char,
    forward: bool,
    till: bool,
}
/// Per-view vi state. Use one instance per buffer/view pair; registers can be copied
/// with `register`/`set_register` when an application wants cross-view registers.
#[derive(Debug)]
pub struct Vintage {
    settings: Settings,
    mode: Mode,
    pending: String,
    count: usize,
    op_count: usize,
    operator: Option<String>,
    register: char,
    registers: BTreeMap<char, Register>,
    command_keys: Vec<Key>,
    last_change: Vec<Key>,
    change_ran: bool,
    steps: usize,
    before: Option<Selection>,
    undo_start: Option<UndoMarker>,
    host_action: Option<(Selection, Vec<Key>)>,
    undo: Vec<Group>,
    redo: Vec<Group>,
    bound: Option<(BufferId, ViewId)>,
    seen: Option<ChangeId>,
    visual: Vec<Region>,
    prompt: String,
    prompt_kind: char,
    prompt_origin: Mode,
    prompt_count: usize,
    replace_restore: Vec<Vec<Replacement>>,
    search: Option<(String, bool, bool)>,
    find: Option<Find>,
    marks: BTreeMap<char, (ChangeId, Selection)>,
    recording: Option<char>,
    macros: BTreeMap<char, Vec<Key>>,
    last_macro: Option<char>,
    replay_depth: usize,
    replay_budget: usize,
    repeating: bool,
    clipboard: Option<String>,
    request: Option<Command>,
    viewport_top: usize,
    viewport_height: Option<usize>,
    insert_count: usize,
    inserted: String,
    host_insert: bool,
    insert_snapshot: Option<insert::Snapshot>,
    // Session snapshots are cold outside insert mode. Keep them off the inline
    // engine/editor layout, including editors where Vintage is disabled.
    insert_session: Option<Box<insert::Session>>,
}
impl Default for Vintage {
    fn default() -> Self {
        Self::new(Settings::default())
    }
}
impl Vintage {
    /// Creates state using the supplied public Vintage settings.
    pub fn new(settings: Settings) -> Self {
        let mode = if settings.vintage_start_in_command_mode {
            Mode::Normal
        } else {
            Mode::Insert
        };
        Self {
            settings,
            mode,
            pending: String::new(),
            count: 0,
            op_count: 1,
            operator: None,
            register: '"',
            registers: BTreeMap::new(),
            command_keys: Vec::new(),
            last_change: Vec::new(),
            change_ran: false,
            steps: 0,
            before: None,
            undo_start: None,
            host_action: None,
            undo: Vec::new(),
            redo: Vec::new(),
            bound: None,
            seen: None,
            visual: Vec::new(),
            prompt: String::new(),
            prompt_kind: ':',
            prompt_origin: Mode::Normal,
            prompt_count: 1,
            replace_restore: Vec::new(),
            search: None,
            find: None,
            marks: BTreeMap::new(),
            recording: None,
            macros: BTreeMap::new(),
            last_macro: None,
            replay_depth: 0,
            replay_budget: 0,
            repeating: false,
            clipboard: None,
            request: None,
            viewport_top: 0,
            viewport_height: None,
            insert_count: 1,
            inserted: String::new(),
            host_insert: false,
            insert_snapshot: None,
            insert_session: None,
        }
    }
    /// Current mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }
    /// Pending prompt text, including `:`, `/` or `?`.
    pub fn command_line(&self) -> Option<String> {
        (self.mode == Mode::CommandLine).then(|| format!("{}{}", self.prompt_kind, self.prompt))
    }
    /// Boolean keymap contexts. Unknown keys return false.
    pub fn context(&self, name: &str) -> bool {
        match name {
            "setting.command_mode" => self.mode != Mode::Insert,
            "vi_mode_normal" => self.mode == Mode::Normal,
            "vi_mode_insert" => self.mode == Mode::Insert,
            "vi_mode_replace" => self.mode == Mode::Replace,
            "vi_mode_visual" => self.is_visual(),
            "vi_mode_visual_line" => self.mode == Mode::VisualLine,
            "vi_mode_visual_block" => self.mode == Mode::VisualBlock,
            "vi_command_line" => self.mode == Mode::CommandLine,
            "vi_has_action" | "vi_operator_pending" => self.operator.is_some(),
            "vi_has_motion" => !self.pending.is_empty(),
            "vi_has_count" => self.count > 0,
            "vi_recording_macro" => self.recording.is_some(),
            "setting.vintage_ctrl_keys" => self.settings.vintage_ctrl_keys,
            _ => false,
        }
    }
    /// Updates host viewport geometry for H/M/L; offsets remain logical LF lines.
    pub fn set_viewport(&mut self, top_line: usize, visible_lines: usize) {
        self.viewport_top = top_line;
        self.viewport_height = Some(visible_lines.max(1));
    }
    /// Supplies a clipboard snapshot before dispatch when clipboard bridging is enabled.
    pub fn set_clipboard(&mut self, text: impl Into<String>) {
        self.clipboard = Some(text.into());
    }
    /// Reads an engine register. Clipboard snapshots are consumed by paste commands.
    pub fn register(&self, name: char) -> Option<&Register> {
        self.registers.get(&name)
    }
    /// Installs register contents from an application-owned shared register store.
    pub fn set_register(&mut self, name: char, value: Register) {
        self.registers.insert(name, value);
    }
    /// Completes a requested `reindent` after the host has applied it. Pass the
    /// number of buffer undo groups produced by that dispatch (normally one).
    /// This keeps reindentation in vi undo/redo and dot-repeat history.
    pub fn complete_host_command(&mut self, buffer: &mut Buffer, view: ViewId, undo_groups: usize) {
        if let Some((before, keys)) = self.host_action.take() {
            let regions = buffer
                .view(view)
                .sel()
                .iter()
                .map(|r| Region::caret(normal(buffer.text(), r.begin())))
                .collect::<Vec<_>>();
            buffer.set_selection(
                view,
                Self::selection(regions, buffer.view(view).sel().primary_index()),
            );
            if undo_groups > 0 {
                self.undo.push(Group {
                    steps: undo_groups,
                    before,
                    after: buffer.view(view).sel().clone(),
                });
                self.redo.clear();
                self.last_change = keys;
            }
            self.seen = Some(buffer.change_id());
        }
    }
    fn is_visual(&self) -> bool {
        matches!(
            self.mode,
            Mode::VisualChar | Mode::VisualLine | Mode::VisualBlock
        )
    }
    /// Consumes one decoded key and applies all edits to every selection in `view`.
    /// Route undo/redo through this engine so an insertion/change session is one
    /// vi undo group, even across the buffer's punctuation/word typing boundaries.
    pub fn handle_key(
        &mut self,
        key: impl Into<Key>,
        buffer: &mut Buffer,
        view: ViewId,
        layout: &dyn VerticalLayout,
    ) -> Outcome {
        let key = key.into();
        if matches!(key,Key::Alt(c) if !matches!(c,'w'|'b')) {
            return Outcome::PassThrough;
        }
        self.observe_insert(buffer, view);
        if self.bound != Some((buffer.id(), view))
            || self.seen.is_some_and(|id| id != buffer.change_id())
        {
            self.undo.clear();
            self.redo.clear();
            self.steps = 0;
            self.change_ran = false;
            self.before = None;
            self.undo_start = None;
            self.host_action = None;
            self.replace_restore.clear();
            self.command_keys.clear();
            self.visual.clear();
            self.reset();
            if self.mode != Mode::Insert {
                self.mode = Mode::Normal;
            }
            if self.bound.is_some_and(|bound| bound != (buffer.id(), view)) {
                self.marks.clear();
                self.mode = Mode::Normal;
            }
            self.bound = Some((buffer.id(), view));
        }
        if self.mode == Mode::Normal && buffer.view(view).sel().iter().any(|r| !r.empty()) {
            self.mode = Mode::VisualChar;
            self.visual = buffer
                .view(view)
                .sel()
                .iter()
                .map(|r| {
                    if r.empty() {
                        r
                    } else if r.a <= r.b {
                        Region::new(r.a, buffer.text().prev_grapheme_boundary(r.b))
                    } else {
                        Region::new(buffer.text().prev_grapheme_boundary(r.a), r.b)
                    }
                })
                .collect();
            // The visual model stores inclusive endpoints. Publish it even for
            // empty host carets, so every operator sees one selected grapheme.
            self.render_visual(buffer, view, layout);
        }
        self.request = None;
        self.replay_budget = 10000;
        if self.mode == Mode::Insert && matches!(key, Key::Escape | Key::Ctrl('[')) {
            self.finish_host_insert(buffer, view);
        }
        if let Some(reg) = self.recording
            && !(key == Key::Char('q')
                && self.mode == Mode::Normal
                && self.pending.is_empty()
                && self.operator.is_none())
        {
            self.macros.entry(reg).or_default().push(key.clone());
        }
        let outcome = self.feed(key, buffer, view, layout);
        self.seen = Some(buffer.change_id());
        self.observe_insert(buffer, view);
        if let Some(command) = self.request.take() {
            Outcome::NeedsCommand(command)
        } else {
            outcome
        }
    }
    fn feed(
        &mut self,
        key: Key,
        buffer: &mut Buffer,
        view: ViewId,
        layout: &dyn VerticalLayout,
    ) -> Outcome {
        layout.synchronize(buffer, view);
        // Stored inclusive visual endpoints may outlive a host undo/redo. Check
        // scalar boundaries before any motion, object, selection or prompt uses them.
        for r in &mut self.visual {
            r.a = buffer
                .text()
                .floor_char_boundary(r.a.min(buffer.text().len_bytes()));
            r.b = buffer
                .text()
                .floor_char_boundary(r.b.min(buffer.text().len_bytes()));
        }
        if self.before.is_none() {
            self.before = Some(buffer.view(view).sel().clone());
            self.undo_start
                .get_or_insert_with(|| buffer.mark_undo_group());
            if self.mode == Mode::Insert && self.command_keys.is_empty() {
                self.command_keys.push(Key::Char('i'));
            }
        }
        self.command_keys.push(key.clone());
        let register_prefix = self.pending == "\"";
        let was_inserting = matches!(self.mode, Mode::Insert | Mode::Replace);
        let outcome = self.process(key, buffer, view, layout);
        // Entering insertion is a dot target even if Escape follows without text.
        // Keep this at the command boundary, independent of insert-session history.
        if !was_inserting && matches!(self.mode, Mode::Insert | Mode::Replace) {
            self.change_ran = true;
        }
        if !register_prefix
            && self.mode == Mode::Normal
            && self.pending.is_empty()
            && self.operator.is_none()
            && self.count == 0
        {
            if self.steps > 0 {
                let group = Group {
                    steps: self.steps,
                    before: self.before.take().unwrap(),
                    after: buffer.view(view).sel().clone(),
                };
                self.undo.push(group);
                self.redo.clear();
            }
            // Host insert sessions count observed transactions in `steps` without
            // going through `apply`, so a recorded step is a dot target as well.
            if (self.change_ran || self.steps > 0) && !self.repeating {
                self.last_change = self.command_keys.clone();
            }
            self.change_ran = false;
            self.steps = 0;
            self.before = None;
            self.undo_start = None;
            self.command_keys.clear();
            self.register = '"';
            buffer.break_undo_group();
        }
        outcome
    }
    fn reset(&mut self) {
        self.pending.clear();
        self.count = 0;
        self.op_count = 1;
        self.operator = None;
    }
    fn selection(regions: Vec<Region>, primary: usize) -> Selection {
        let active = regions.get(primary).copied();
        let mut selected = Selection::from_regions(regions);
        if let Some(active) = active {
            selected.add(active);
        }
        selected
    }
    fn normalize(&self, buffer: &mut Buffer, view: ViewId) {
        let regions = buffer
            .view(view)
            .sel()
            .iter()
            .map(|r| Region::caret(normal(buffer.text(), r.b)))
            .collect::<Vec<_>>();
        buffer.set_selection(
            view,
            Self::selection(regions, buffer.view(view).sel().primary_index()),
        );
    }
    fn escape(&mut self, buffer: &mut Buffer, view: ViewId) {
        if matches!(self.mode, Mode::Insert | Mode::Replace) {
            let recorded: Vec<_> = self
                .command_keys
                .iter()
                .filter(|k| matches!(k, Key::HostInsert(_)))
                .cloned()
                .collect();
            if self.insert_count > 1 && !recorded.is_empty() {
                for _ in 1..self.insert_count {
                    for key in &recorded {
                        if let Key::HostInsert(record) = key {
                            self.replay_insert(record.clone(), buffer, view);
                        }
                    }
                }
            } else if self.insert_count > 1 && !self.inserted.is_empty() {
                let text = self.inserted.repeat(self.insert_count - 1);
                self.insert_text(buffer, view, &text, false);
            }
            let regions = buffer
                .view(view)
                .sel()
                .iter()
                .map(|r| {
                    Region::caret(if r.b > bol(buffer.text(), r.b) {
                        buffer.text().prev_grapheme_boundary(r.b)
                    } else {
                        r.b
                    })
                })
                .collect::<Vec<_>>();
            buffer.set_selection(
                view,
                Self::selection(regions, buffer.view(view).sel().primary_index()),
            );
            // Our vi semantics: native per-word undo while inserting, then one
            // host undo/redo for the completed insertion/change. Public Vintage
            // docs confirm ordinary insert editing, but do not specify gluing:
            // https://www.sublimetext.com/docs/vintage.html#whats-not
            if let Some(marker) = self.undo_start.take() {
                buffer.glue_undo_groups(marker);
            }
        } else if self.is_visual() {
            self.collapse_visual(buffer, view);
        }
        self.mode = Mode::Normal;
        self.visual.clear();
        self.reset();
        self.normalize(buffer, view);
        self.insert_count = 1;
        self.inserted.clear();
        self.replace_restore.clear();
    }
    fn process(
        &mut self,
        key: Key,
        buffer: &mut Buffer,
        view: ViewId,
        layout: &dyn VerticalLayout,
    ) -> Outcome {
        if key == Key::Escape || key == Key::Ctrl('[') {
            if self.mode == Mode::CommandLine {
                self.mode = self.prompt_origin;
                self.prompt.clear();
                self.reset();
                return Outcome::Handled;
            }
            self.escape(buffer, view);
            return Outcome::Handled;
        }
        if self.mode == Mode::CommandLine {
            return self.prompt_key(key, buffer, view, layout);
        }
        if matches!(self.mode, Mode::Insert | Mode::Replace) {
            if let Key::HostInsert(record) = key {
                self.replay_insert(record, buffer, view);
                return Outcome::Handled;
            }
            // https://www.sublimetext.com/docs/vintage.html#whats-not:
            // the integrated host keeps its normal insert-mode key bindings.
            if self.mode == Mode::Insert && self.host_insert && !self.repeating {
                return Outcome::PassThrough;
            }
            match key {
                Key::Char(c) => {
                    let text = c.to_string();
                    self.insert_text(buffer, view, &text, self.mode == Mode::Replace);
                    self.inserted.push(c);
                }
                Key::Text(text) => {
                    self.insert_text(buffer, view, &text, self.mode == Mode::Replace);
                    self.inserted.push_str(&text);
                }
                Key::Enter => {
                    self.insert_text(buffer, view, "\n", false);
                    self.inserted.push('\n');
                }
                Key::Backspace if self.mode == Mode::Replace => {
                    if let Some(restore) = self.replace_restore.pop() {
                        self.apply(buffer, view, restore);
                        self.forget_inserted_grapheme();
                    } else {
                        let regions = buffer
                            .view(view)
                            .sel()
                            .iter()
                            .map(|r| {
                                Region::caret(
                                    buffer
                                        .text()
                                        .prev_grapheme_boundary(r.b)
                                        .max(bol(buffer.text(), r.b)),
                                )
                            })
                            .collect();
                        buffer.set_selection(
                            view,
                            Self::selection(regions, buffer.view(view).sel().primary_index()),
                        );
                    }
                }
                Key::Backspace => {
                    let reps = buffer
                        .view(view)
                        .sel()
                        .iter()
                        .map(|r| Replacement {
                            range: if r.empty() {
                                buffer.text().prev_grapheme_boundary(r.b)..r.b
                            } else {
                                r.begin()..r.end()
                            },
                            text: String::new(),
                            caret: 0,
                        })
                        .collect();
                    self.apply(buffer, view, reps);
                    self.forget_inserted_grapheme();
                }
                Key::Left | Key::Right | Key::Up | Key::Down => {
                    self.replace_restore.clear();
                    let name = match key {
                        Key::Left => "h",
                        Key::Right => "l",
                        Key::Up => "k",
                        _ => "j",
                    };
                    self.move_key(buffer, view, layout, name, 1, false);
                }
                _ => return Outcome::PassThrough,
            }
            return Outcome::Handled;
        }
        let ch = match key {
            Key::Char(c) => c,
            Key::Text(text) if text.chars().count() == 1 => text.chars().next().unwrap(),
            Key::Left => 'h',
            Key::Right => 'l',
            Key::Up => 'k',
            Key::Down => 'j',
            Key::Backspace => 'h',
            Key::Enter => {
                if self.pending == "r" {
                    '\n'
                } else {
                    '+'
                }
            }
            Key::Alt(c) if matches!(c, 'w' | 'b') => {
                self.move_key(
                    buffer,
                    view,
                    layout,
                    if c == 'w' { "alt+w" } else { "alt+b" },
                    self.count.max(1),
                    false,
                );
                self.count = 0;
                return Outcome::Handled;
            }
            Key::Ctrl(c) if self.settings.vintage_ctrl_keys => {
                match c {
                    'e' | 'y' => {
                        // https://www.sublimetext.com/docs/vintage.html#ctrl-keys
                        self.request = Some(Command {
                            name: "scroll_lines",
                            args: BTreeMap::from([(
                                "amount".into(),
                                (if c == 'e' { -1isize } else { 1 })
                                    .saturating_mul(self.count.max(1) as isize)
                                    .to_string(),
                            )]),
                        });
                        self.count = 0;
                    }
                    'r' => {
                        self.history(buffer, view, true, self.count.max(1));
                        self.reset();
                    }
                    'v' => {
                        self.visual_mode(buffer, view, Mode::VisualBlock, layout);
                        self.count = 0;
                    }
                    'f' | 'b' | 'd' | 'u' => {
                        let count = self.count.max(1)
                            * if matches!(c, 'd' | 'u') {
                                layout.lines_per_page().div_ceil(2)
                            } else {
                                layout.lines_per_page()
                            };
                        self.move_key(
                            buffer,
                            view,
                            layout,
                            if matches!(c, 'f' | 'd') { "j" } else { "k" },
                            count,
                            false,
                        );
                        self.count = 0;
                    }
                    _ => return Outcome::PassThrough,
                }
                return Outcome::Handled;
            }
            _ => return Outcome::PassThrough,
        };
        if !self.pending.is_empty() {
            return self.pending_key(ch, buffer, view, layout);
        }
        if ch.is_ascii_digit() && (ch != '0' || self.count > 0) {
            self.count =
                (self.count.saturating_mul(10) + (ch as usize - '0' as usize)).min(1_000_000);
            return Outcome::Handled;
        }
        let explicit = self.count > 0;
        let count = self
            .count
            .max(1)
            .saturating_mul(self.op_count)
            .min(1_000_000);
        if self.is_visual() && matches!(ch, 'i' | 'a') {
            self.pending.push(ch);
            return Outcome::Handled;
        }
        if self.operator.is_some() {
            if ch.to_string() == *self.operator.as_ref().unwrap()
                || (self.operator.as_ref().unwrap().len() == 2
                    && self.operator.as_ref().unwrap().ends_with(ch))
            {
                let regs = buffer
                    .view(view)
                    .sel()
                    .iter()
                    .map(|r| {
                        let start = bol(buffer.text(), r.b);
                        let row = buffer.text().byte_to_line(r.b);
                        let line = (row + count).min(buffer.text().len_lines());
                        start..if line == buffer.text().len_lines() {
                            buffer.text().len_bytes()
                        } else {
                            buffer.text().line_to_byte(line)
                        }
                    })
                    .collect();
                self.operate(buffer, view, regs, true);
                return Outcome::Handled;
            }
            if matches!(ch, 'i' | 'a') {
                self.pending.push(ch);
                return Outcome::Handled;
            }
        }
        match ch {
            'g' | 'f' | 'F' | 't' | 'T' | 'm' | '\'' | '`' | '"' | 'r' | '@' => {
                self.pending.push(ch)
            }
            'q' => {
                if self.recording.take().is_none() {
                    self.pending.push(ch);
                } else {
                    self.count = 0;
                }
            }
            'd' | 'c' | 'y' | '>' | '<' | '=' => {
                if self.is_visual() {
                    self.operator = Some(ch.to_string());
                    let ranges = buffer
                        .view(view)
                        .sel()
                        .iter()
                        .map(Region::to_range)
                        .collect();
                    self.operate(buffer, view, ranges, self.mode == Mode::VisualLine);
                } else {
                    self.operator = Some(ch.to_string());
                    self.op_count = count;
                    self.count = 0;
                }
            }
            'v' | 'V' => {
                self.visual_mode(
                    buffer,
                    view,
                    if ch == 'v' {
                        Mode::VisualChar
                    } else {
                        Mode::VisualLine
                    },
                    layout,
                );
                self.count = 0;
            }
            'o' | 'O' if self.is_visual() => {
                for region in &mut self.visual {
                    std::mem::swap(&mut region.a, &mut region.b);
                }
                self.render_visual(buffer, view, layout);
                self.count = 0;
            }
            'i' | 'a' | 'I' | 'A' | 'o' | 'O' | 'R' if self.operator.is_none() => {
                self.enter_insert(buffer, view, ch, count);
                self.count = 0;
            }
            'x' | 'X' | 's' | 'S' | 'C' | 'D' | 'Y' => {
                self.direct_edit(buffer, view, ch, count);
            }
            'p' | 'P' => {
                self.paste(buffer, view, ch == 'P', count);
                self.count = 0;
            }
            'J' => {
                self.join(buffer, view, count.max(2));
                self.count = 0;
            }
            '~' => {
                if self.is_visual() {
                    self.operator = Some("g~".into());
                    let ranges = buffer
                        .view(view)
                        .sel()
                        .iter()
                        .map(Region::to_range)
                        .collect();
                    self.operate(buffer, view, ranges, false);
                } else {
                    let ranges = buffer
                        .view(view)
                        .sel()
                        .iter()
                        .map(|r| {
                            let mut b = r.b;
                            for _ in 0..count {
                                b = buffer
                                    .text()
                                    .next_grapheme_boundary(b)
                                    .min(end(buffer.text(), r.b));
                            }
                            r.b..b
                        })
                        .collect();
                    self.operator = Some("swap".into());
                    self.operate(buffer, view, ranges, false);
                }
            }
            'u' => {
                if self.is_visual() {
                    self.operator = Some("gu".into());
                    let ranges = buffer
                        .view(view)
                        .sel()
                        .iter()
                        .map(Region::to_range)
                        .collect();
                    self.operate(buffer, view, ranges, false);
                } else {
                    self.history(buffer, view, false, count);
                    self.reset();
                }
            }
            'U' if self.is_visual() => {
                self.operator = Some("gU".into());
                let ranges = buffer
                    .view(view)
                    .sel()
                    .iter()
                    .map(Region::to_range)
                    .collect();
                self.operate(buffer, view, ranges, false);
            }
            '.' => {
                let keys = self.last_change.clone();
                self.command_keys.clear();
                self.count = 0;
                for _ in 0..count {
                    self.replay(&keys, buffer, view, layout);
                }
            }
            ':' | '/' | '?' => {
                self.prompt_origin = self.mode;
                self.mode = Mode::CommandLine;
                self.prompt_kind = ch;
                self.prompt_count = count;
                self.prompt.clear();
                self.count = 0;
            }
            'n' | 'N' => {
                if let Some((query, forward, whole)) = self.search.clone() {
                    self.search_move(
                        buffer,
                        view,
                        &query,
                        if ch == 'n' { forward } else { !forward },
                        whole,
                        count,
                    );
                }
                self.count = 0;
            }
            '*' | '#' => {
                if let Some(r) = buffer.view(view).sel().primary()
                    && let Some(range) = motion::object(buffer.text(), r.b, 'w', false, 1)
                {
                    let query = buffer.text().slice(range).to_string();
                    self.search = Some((query.clone(), ch == '*', true));
                    self.search_move(buffer, view, &query, ch == '*', true, count);
                }
                self.count = 0;
            }
            ';' | ',' => {
                if let Some(find) = self.find {
                    self.find_move(
                        buffer,
                        view,
                        layout,
                        Find {
                            forward: if ch == ';' {
                                find.forward
                            } else {
                                !find.forward
                            },
                            ..find
                        },
                        count,
                        true,
                    );
                }
                self.count = 0;
            }
            '+' => {
                self.move_key(buffer, view, layout, "j", count, false);
                let regions = buffer
                    .view(view)
                    .sel()
                    .iter()
                    .map(|r| Region::caret(first(buffer.text(), r.b)))
                    .collect::<Vec<_>>();
                buffer.set_selection(
                    view,
                    Self::selection(regions, buffer.view(view).sel().primary_index()),
                );
                self.count = 0;
            }
            _ => {
                if !self.move_key(buffer, view, layout, &ch.to_string(), count, explicit) {
                    self.reset();
                    return Outcome::PassThrough;
                }
                self.count = 0;
            }
        }
        Outcome::Handled
    }
    fn pending_key(
        &mut self,
        ch: char,
        buffer: &mut Buffer,
        view: ViewId,
        layout: &dyn VerticalLayout,
    ) -> Outcome {
        let pending = std::mem::take(&mut self.pending);
        let count = self
            .count
            .max(1)
            .saturating_mul(self.op_count)
            .min(1_000_000);
        let explicit = self.count > 0;
        match pending.as_str() {
            "\"" => {
                if ch.is_ascii_alphabetic() || matches!(ch, '0' | '"' | '+') {
                    self.register = ch;
                } else {
                    self.reset();
                }
            }
            "m" => {
                self.marks
                    .insert(ch, (buffer.change_id(), buffer.view(view).sel().clone()));
                self.count = 0;
            }
            "'" | "`" => {
                if let Some((id, selection)) = self.marks.get(&ch) {
                    let regions = selection
                        .iter()
                        .filter_map(|r| buffer.transform_point_from(r.b, *id, Bias::Right))
                        .map(|p| {
                            let p = buffer
                                .text()
                                .floor_char_boundary(p.min(buffer.text().len_bytes()));
                            Region::caret(if pending == "'" {
                                first(buffer.text(), p)
                            } else {
                                normal(buffer.text(), p)
                            })
                        })
                        .collect::<Vec<_>>();
                    if !regions.is_empty() {
                        let targets = regions
                            .iter()
                            .map(|r| Motion {
                                point: r.b,
                                inclusive: false,
                                linewise: pending == "'",
                                vertical: false,
                            })
                            .collect();
                        self.apply_motions(buffer, view, targets, layout);
                    }
                }
                self.count = 0;
            }
            "q" => {
                if ch.is_ascii_alphabetic() {
                    self.recording = Some(ch.to_ascii_lowercase());
                    self.macros.insert(ch.to_ascii_lowercase(), Vec::new());
                }
                self.count = 0;
            }
            "@" => {
                let reg = if ch == '@' {
                    self.last_macro.unwrap_or('"')
                } else {
                    ch
                };
                if let Some(keys) = self.macros.get(&reg).cloned() {
                    self.last_macro = Some(reg);
                    self.count = 0;
                    self.command_keys.clear();
                    for _ in 0..count {
                        self.replay(&keys, buffer, view, layout);
                    }
                } else {
                    self.reset();
                }
            }
            "r" => {
                use unicode_segmentation::UnicodeSegmentation;
                let visual = self.is_visual();
                let mut ranges = Vec::<Range<usize>>::new();
                for r in buffer.view(view).sel().iter() {
                    let range = if visual {
                        r.to_range()
                    } else {
                        let mut b = r.b;
                        let mut n = 0;
                        while n < count && b < end(buffer.text(), r.b) {
                            b = buffer.text().next_grapheme_boundary(b);
                            n += 1;
                        }
                        r.b..if n == count { b } else { r.b }
                    };
                    ranges.push(range);
                }
                ranges.sort_by_key(|r| (r.start, r.end));
                let mut merged: Vec<Range<usize>> = Vec::new();
                for range in ranges {
                    if let Some(last) = merged.last_mut()
                        && range.start < last.end
                    {
                        last.end = last.end.max(range.end);
                    } else {
                        merged.push(range);
                    }
                }
                let reps = merged
                    .into_iter()
                    .map(|range| {
                        let source = buffer.text().slice(range.clone()).to_string();
                        let text = source
                            .graphemes(true)
                            .map(|g| {
                                if g.contains('\n') {
                                    g.to_owned()
                                } else {
                                    ch.to_string()
                                }
                            })
                            .collect::<String>();
                        let caret = if visual {
                            0
                        } else {
                            text.len().saturating_sub(ch.len_utf8())
                        };
                        Replacement { range, text, caret }
                    })
                    .collect();
                self.mode = Mode::Normal;
                self.visual.clear();
                self.apply(buffer, view, reps);
                self.count = 0;
            }
            "f" | "F" | "t" | "T" => {
                let find = Find {
                    target: ch,
                    forward: matches!(pending.as_str(), "f" | "t"),
                    till: matches!(pending.as_str(), "t" | "T"),
                };
                self.find = Some(find);
                self.find_move(buffer, view, layout, find, count, false);
                self.count = 0;
            }
            "i" | "a" => {
                let carets = if self.is_visual() {
                    self.visual.clone()
                } else {
                    buffer.view(view).sel().iter().collect()
                };
                let ranges = carets
                    .iter()
                    .filter_map(|r| motion::object(buffer.text(), r.b, ch, pending == "a", count))
                    .collect::<Vec<_>>();
                if !ranges.is_empty() {
                    if self.operator.is_some() {
                        self.operate(buffer, view, ranges, false);
                    } else {
                        self.select_objects(buffer, view, ranges);
                    }
                } else {
                    self.reset();
                }
            }
            "g" => match ch {
                'g' | 'e' => {
                    self.move_key(buffer, view, layout, &format!("g{ch}"), count, explicit);
                    self.count = 0;
                }
                '~' | 'u' | 'U' => {
                    let op = format!("g{ch}");
                    if self.is_visual() {
                        self.operator = Some(op);
                        let ranges = buffer
                            .view(view)
                            .sel()
                            .iter()
                            .map(Region::to_range)
                            .collect();
                        self.operate(buffer, view, ranges, false);
                    } else {
                        self.operator = Some(op);
                        self.op_count = count;
                        self.count = 0;
                    }
                }
                _ => self.reset(),
            },
            _ => self.reset(),
        }
        Outcome::Handled
    }
    fn replay(
        &mut self,
        keys: &[Key],
        buffer: &mut Buffer,
        view: ViewId,
        layout: &dyn VerticalLayout,
    ) {
        if self.replay_depth >= 16 {
            return;
        }
        self.replay_depth += 1;
        let previous = self.repeating;
        self.repeating = true;
        for key in keys {
            if self.replay_budget == 0 {
                self.reset();
                break;
            }
            self.replay_budget -= 1;
            self.feed(key.clone(), buffer, view, layout);
        }
        self.repeating = previous;
        self.replay_depth -= 1;
    }
    fn move_key(
        &mut self,
        buffer: &mut Buffer,
        view: ViewId,
        layout: &dyn VerticalLayout,
        key: &str,
        count: usize,
        explicit: bool,
    ) -> bool {
        let regions = if self.is_visual() {
            self.visual.clone()
        } else {
            buffer.view(view).sel().iter().collect()
        };
        let mut targets = Vec::new();
        for r in regions {
            let key = if self.operator.as_deref() == Some("c")
                && matches!(key, "w" | "W")
                && r.b < buffer.text().len_bytes()
                && !buffer.text().char_at(r.b).is_whitespace()
            {
                if key == "w" { "e" } else { "E" }
            } else {
                key
            };
            let Some(target) = motion::motion(
                buffer.text(),
                r,
                key,
                count,
                explicit,
                layout,
                self.viewport_top,
                self.viewport_height
                    .unwrap_or_else(|| layout.lines_per_page()),
            ) else {
                return false;
            };
            targets.push(target);
        }
        self.apply_motions(buffer, view, targets, layout);
        true
    }
    fn apply_motions(
        &mut self,
        buffer: &mut Buffer,
        view: ViewId,
        mut targets: Vec<Motion>,
        layout: &dyn VerticalLayout,
    ) {
        for target in &mut targets {
            target.point = buffer
                .text()
                .floor_char_boundary(target.point.min(buffer.text().len_bytes()));
        }
        let regions = if self.is_visual() {
            self.visual.clone()
        } else {
            buffer.view(view).sel().iter().collect::<Vec<_>>()
        };
        if self.operator.is_some() {
            let linewise = targets.iter().all(|t| t.linewise);
            let mut ranges = Vec::new();
            for (r, m) in regions.iter().zip(&targets) {
                let mut range = r.b.min(m.point)..r.b.max(m.point);
                if m.inclusive && range.end < buffer.text().len_bytes() {
                    range.end = buffer.text().next_grapheme_boundary(range.end);
                }
                if linewise {
                    range = line_range(
                        buffer.text(),
                        range.start..buffer.text().next_grapheme_boundary(range.end),
                    );
                } else if !m.inclusive
                    && m.point > r.b
                    && bol(buffer.text(), m.point) == m.point
                    && m.point > 0
                {
                    range.end = buffer.text().prev_grapheme_boundary(m.point);
                    if r.b == first(buffer.text(), r.b) {
                        range = line_range(buffer.text(), range);
                    }
                }
                ranges.push(range);
            }
            self.operate(buffer, view, ranges, linewise);
        } else if self.is_visual() {
            for (r, m) in self.visual.iter_mut().zip(targets) {
                if m.vertical {
                    r.xpos = Some(r.xpos.unwrap_or_else(|| layout.x_of(buffer.text(), r.b)));
                } else {
                    r.xpos = None;
                }
                r.b = normal(buffer.text(), m.point);
            }
            self.render_visual(buffer, view, layout);
        } else {
            let selected = regions
                .into_iter()
                .zip(targets)
                .map(|(r, m)| {
                    let mut target = Region::caret(normal(buffer.text(), m.point));
                    if m.vertical {
                        target.xpos =
                            Some(r.xpos.unwrap_or_else(|| layout.x_of(buffer.text(), r.b)));
                    }
                    target
                })
                .collect::<Vec<_>>();
            buffer.set_selection(
                view,
                Self::selection(selected, buffer.view(view).sel().primary_index()),
            );
        }
    }
    fn find_move(
        &mut self,
        buffer: &mut Buffer,
        view: ViewId,
        layout: &dyn VerticalLayout,
        find: Find,
        count: usize,
        repeat: bool,
    ) {
        let regions = if self.is_visual() {
            self.visual.clone()
        } else {
            buffer.view(view).sel().iter().collect()
        };
        let targets = regions
            .iter()
            .map(|r| {
                let start = if repeat && find.till {
                    if find.forward {
                        buffer
                            .text()
                            .next_grapheme_boundary(r.b)
                            .min(end(buffer.text(), r.b))
                    } else {
                        buffer
                            .text()
                            .prev_grapheme_boundary(r.b)
                            .max(bol(buffer.text(), r.b))
                    }
                } else {
                    r.b
                };
                let point = motion::find(
                    buffer.text(),
                    start,
                    find.target,
                    find.forward,
                    find.till,
                    count,
                );
                Motion {
                    point: point.unwrap_or(r.b),
                    inclusive: find.forward && point.is_some(),
                    linewise: false,
                    vertical: false,
                }
            })
            .collect();
        self.apply_motions(buffer, view, targets, layout);
    }
    fn visual_mode(
        &mut self,
        buffer: &mut Buffer,
        view: ViewId,
        mode: Mode,
        layout: &dyn VerticalLayout,
    ) {
        if self.mode == mode {
            self.collapse_visual(buffer, view);
            self.mode = Mode::Normal;
            self.visual.clear();
            return;
        }
        if !self.is_visual() {
            self.visual = buffer
                .view(view)
                .sel()
                .iter()
                .map(|r| Region::caret(normal(buffer.text(), r.b)))
                .collect();
        }
        self.mode = mode;
        self.render_visual(buffer, view, layout);
    }
    fn render_visual(&self, buffer: &mut Buffer, view: ViewId, layout: &dyn VerticalLayout) {
        let mut regions = Vec::new();
        for r in &self.visual {
            let a = r.a.min(r.b);
            let b = r.a.max(r.b);
            let last = buffer.text().next_grapheme_boundary(b);
            match self.mode {
                Mode::VisualLine => {
                    let range = line_range(buffer.text(), a..last);
                    regions.push(Region::new(range.start, range.end));
                }
                Mode::VisualBlock => {
                    let x = layout.x_of(buffer.text(), r.a);
                    let y = layout.x_of(buffer.text(), r.b);
                    for row in buffer.text().byte_to_line(a)..=buffer.text().byte_to_line(b) {
                        let left = layout
                            .offset_at(buffer.text(), row, x.min(y))
                            .min(end(buffer.text(), buffer.text().line_to_byte(row)));
                        let right = layout
                            .offset_at(buffer.text(), row, x.max(y))
                            .min(end(buffer.text(), left));
                        regions.push(Region::new(
                            left,
                            buffer
                                .text()
                                .next_grapheme_boundary(right)
                                .min(end(buffer.text(), right)),
                        ));
                    }
                }
                _ => regions.push(if r.a <= r.b {
                    Region::new(a, last)
                } else {
                    Region::new(last, a)
                }),
            }
        }
        buffer.set_selection(
            view,
            Self::selection(regions, buffer.view(view).sel().primary_index()),
        );
    }
    fn collapse_visual(&self, buffer: &mut Buffer, view: ViewId) {
        buffer.set_selection(
            view,
            Selection::from_regions(self.visual.iter().map(|r| Region::caret(r.b))),
        );
    }
    fn select_objects(&mut self, buffer: &mut Buffer, view: ViewId, ranges: Vec<Range<usize>>) {
        self.visual = ranges
            .iter()
            .map(|r| {
                Region::new(
                    r.start,
                    if r.end > r.start {
                        buffer.text().prev_grapheme_boundary(r.end)
                    } else {
                        r.start
                    },
                )
            })
            .collect();
        buffer.set_selection(
            view,
            Selection::from_regions(ranges.into_iter().map(|r| Region::new(r.start, r.end))),
        );
        self.count = 0;
    }
    fn apply(&mut self, buffer: &mut Buffer, view: ViewId, mut reps: Vec<Replacement>) {
        // Dot remembers the attempted change even when the buffer filters identical edits.
        // Undo groups below still count only actual transactions.
        self.change_ran = true;
        for rep in &mut reps {
            rep.caret = rep.caret.min(rep.text.len());
            while !rep.text.is_char_boundary(rep.caret) {
                rep.caret -= 1;
            }
            rep.range.start = buffer
                .text()
                .floor_char_boundary(rep.range.start.min(buffer.text().len_bytes()));
            rep.range.end = buffer
                .text()
                .floor_char_boundary(rep.range.end.min(buffer.text().len_bytes()))
                .max(rep.range.start);
        }
        if reps.iter().all(|r| r.range.is_empty() && r.text.is_empty()) {
            return;
        }
        reps.sort_by_key(|r| (r.range.start, r.range.end));
        let mut merged: Vec<Replacement> = Vec::new();
        for rep in reps {
            if let Some(last) = merged.last_mut()
                && rep.range.start < last.range.end
            {
                last.range.end = last.range.end.max(rep.range.end);
                continue;
            }
            if merged.last().is_some_and(|last| last.range == rep.range) {
                continue;
            }
            merged.push(rep);
        }
        let active = buffer.view(view).sel().primary().map_or(0, |r| r.b);
        let primary = merged
            .iter()
            .position(|r| r.range.start <= active && active <= r.range.end)
            .unwrap_or_else(|| {
                merged
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, r)| r.range.start.abs_diff(active))
                    .map_or(0, |(i, _)| i)
            });
        let mut delta = 0isize;
        let mut carets = Vec::new();
        for r in &merged {
            let start = r
                .range
                .start
                .checked_add_signed(delta)
                .expect("ordered edit mapping");
            carets.push(Region::caret(start + r.caret.min(r.text.len())));
            delta += r.text.len() as isize - r.range.len() as isize;
        }
        let edits = merged
            .iter()
            .map(|r| Edit {
                range: r.range.clone(),
                text: &r.text,
            })
            .collect::<Vec<_>>();
        buffer.break_undo_group();
        let before = buffer.change_id();
        buffer.transact(
            view,
            EditKind::Other,
            &edits,
            Self::selection(carets, primary),
        );
        // The buffer filters identical replacements. Count actual transactions so
        // undo never consumes a preceding edit using this no-op's saved selection.
        self.steps += usize::from(buffer.change_id() != before);
        if !matches!(self.mode, Mode::Insert | Mode::Replace) {
            self.normalize(buffer, view);
        }
    }
    fn store(&mut self, fragments: Vec<String>, kind: RegisterKind, yank: bool) {
        let value = Register { fragments, kind };
        let name = self.register;
        if name.is_ascii_uppercase() {
            let target = self
                .registers
                .entry(name.to_ascii_lowercase())
                .or_insert(Register {
                    fragments: Vec::new(),
                    kind,
                });
            target.fragments.extend(value.fragments.clone());
        } else if name != '"' {
            self.registers.insert(name, value.clone());
        }
        if yank {
            self.registers.insert('0', value.clone());
        }
        self.registers.insert('"', value.clone());
        if self.settings.vintage_use_clipboard && (name == '"' || name == '+') {
            let text = value
                .fragments
                .join(if kind == RegisterKind::Line { "" } else { "\n" });
            self.clipboard = Some(text.clone());
            self.registers.insert('+', value);
            self.request = Some(Command {
                name: "set_clipboard",
                args: BTreeMap::from([("text".into(), text)]),
            });
        }
    }
    fn operate(
        &mut self,
        buffer: &mut Buffer,
        view: ViewId,
        mut ranges: Vec<Range<usize>>,
        linewise: bool,
    ) {
        let Some(op) = self.operator.take() else {
            return;
        };
        let was_visual = self.is_visual();
        let shape = if linewise {
            RegisterKind::Line
        } else if self.mode == Mode::VisualBlock {
            RegisterKind::Block
        } else {
            RegisterKind::Character
        };
        ranges.sort_by_key(|r| r.start);
        let mut unique: Vec<Range<usize>> = Vec::new();
        for r in ranges {
            if let Some(last) = unique.last_mut()
                && r.start < last.end
            {
                last.end = last.end.max(r.end);
            } else {
                unique.push(r);
            }
        }
        let ranges = unique;
        if matches!(op.as_str(), "y" | "d" | "c") {
            let fragments = ranges
                .iter()
                .map(|r| {
                    let mut text = buffer.text().slice(r.clone()).to_string();
                    if linewise && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text
                })
                .collect();
            self.store(fragments, shape, op == "y");
        }
        if op == "y" {
            if was_visual {
                buffer.set_selection(
                    view,
                    Selection::from_regions(ranges.iter().map(|r| Region::caret(r.start))),
                );
            }
            self.mode = Mode::Normal;
            self.visual.clear();
            self.reset();
            return;
        }
        if op == "=" {
            self.host_action = Some((
                self.before
                    .clone()
                    .unwrap_or_else(|| buffer.view(view).sel().clone()),
                self.command_keys.clone(),
            ));
            buffer.set_selection(
                view,
                Selection::from_regions(ranges.into_iter().map(|r| Region::new(r.start, r.end))),
            );
            self.request = Some(Command {
                name: "reindent",
                args: BTreeMap::new(),
            });
            self.mode = Mode::Normal;
            self.visual.clear();
            self.reset();
            return;
        }
        if matches!(op.as_str(), ">" | "<") {
            let mut rows = std::collections::BTreeSet::new();
            for range in &ranges {
                let line = line_range(buffer.text(), range.clone());
                let last = if line.end > line.start {
                    buffer.text().prev_grapheme_boundary(line.end)
                } else {
                    line.end
                };
                rows.extend(
                    buffer.text().byte_to_line(line.start)..=buffer.text().byte_to_line(last),
                );
            }
            let settings = buffer.view(view).settings.clone();
            let mut reps = Vec::new();
            for row in rows {
                let a = buffer.text().line_to_byte(row);
                if op == ">" {
                    let text = if settings.translate_tabs_to_spaces {
                        " ".repeat(settings.tab_size.max(1))
                    } else {
                        "\t".into()
                    };
                    reps.push(Replacement {
                        range: a..a,
                        caret: text.len(),
                        text,
                    });
                } else {
                    let mut b = a;
                    let mut columns = 0;
                    while b < end(buffer.text(), a) && columns < settings.tab_size.max(1) {
                        match buffer.text().char_at(b) {
                            ' ' => columns += 1,
                            '\t' => columns = settings.tab_size.max(1),
                            _ => break,
                        }
                        b = buffer.text().next_grapheme_boundary(b);
                    }
                    reps.push(Replacement {
                        range: a..b,
                        text: String::new(),
                        caret: 0,
                    });
                }
            }
            self.mode = Mode::Normal;
            self.apply(buffer, view, reps);
            self.visual.clear();
            self.reset();
            return;
        }
        let mut reps = Vec::new();
        for mut range in ranges {
            let mut caret = 0;
            let text = match op.as_str() {
                "d" => {
                    if linewise
                        && range.end == buffer.text().len_bytes()
                        && range.start > 0
                        && buffer.text().byte_at(range.end.saturating_sub(1)) != b'\n'
                    {
                        range.start = buffer.text().prev_grapheme_boundary(range.start);
                    }
                    String::new()
                }
                "c" => {
                    if linewise
                        && range.end > range.start
                        && buffer.text().byte_at(range.end - 1) == b'\n'
                    {
                        "\n".into()
                    } else {
                        String::new()
                    }
                }
                "gu" => buffer
                    .text()
                    .slice(range.clone())
                    .to_string()
                    .to_lowercase(),
                "gU" => buffer
                    .text()
                    .slice(range.clone())
                    .to_string()
                    .to_uppercase(),
                "g~" | "swap" => {
                    let text = buffer
                        .text()
                        .slice(range.clone())
                        .to_string()
                        .chars()
                        .flat_map(|c| {
                            if c.is_lowercase() {
                                c.to_uppercase().collect::<Vec<_>>()
                            } else {
                                c.to_lowercase().collect()
                            }
                        })
                        .collect::<String>();
                    if !was_visual && op == "swap" {
                        caret = text.len();
                    }
                    text
                }
                _ => String::new(),
            };
            reps.push(Replacement { range, text, caret });
        }
        self.mode = if op == "c" {
            Mode::Insert
        } else {
            Mode::Normal
        };
        self.apply(buffer, view, reps);
        self.visual.clear();
        self.reset();
        self.insert_count = 1;
        self.inserted.clear();
    }
    fn forget_inserted_grapheme(&mut self) {
        use unicode_segmentation::UnicodeSegmentation;
        let keep = self
            .inserted
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i);
        self.inserted.truncate(keep);
    }
    fn insert_text(&mut self, buffer: &mut Buffer, view: ViewId, text: &str, replace: bool) {
        use unicode_segmentation::UnicodeSegmentation;
        let n = text.graphemes(true).count();
        let reps: Vec<Replacement> = buffer
            .view(view)
            .sel()
            .iter()
            .map(|r| {
                let mut b = r.end();
                if replace && r.empty() {
                    for _ in 0..n {
                        b = buffer
                            .text()
                            .next_grapheme_boundary(b)
                            .min(end(buffer.text(), r.b));
                    }
                }
                Replacement {
                    range: r.begin()..b,
                    text: text.into(),
                    caret: text.len(),
                }
            })
            .collect();
        if self.mode == Mode::Replace {
            let mut delta = 0isize;
            let restore = reps
                .iter()
                .map(|r| {
                    let start = r
                        .range
                        .start
                        .checked_add_signed(delta)
                        .expect("replace mapping");
                    let text = buffer.text().slice(r.range.clone()).to_string();
                    delta += r.text.len() as isize - r.range.len() as isize;
                    Replacement {
                        range: start..start + r.text.len(),
                        text,
                        caret: 0,
                    }
                })
                .collect();
            self.replace_restore.push(restore);
        }
        self.apply(buffer, view, reps);
    }
    fn enter_insert(&mut self, buffer: &mut Buffer, view: ViewId, key: char, count: usize) {
        buffer.break_undo_group();
        self.insert_count = count;
        self.inserted.clear();
        self.replace_restore.clear();
        if self.is_visual() {
            self.collapse_visual(buffer, view);
            self.visual.clear();
        }
        self.mode = if key == 'R' {
            Mode::Replace
        } else {
            Mode::Insert
        };
        let regions = buffer
            .view(view)
            .sel()
            .iter()
            .map(|r| {
                Region::caret(match key {
                    'a' => buffer
                        .text()
                        .next_grapheme_boundary(r.b)
                        .min(end(buffer.text(), r.b)),
                    'I' => first(buffer.text(), r.b),
                    'A' => end(buffer.text(), r.b),
                    _ => r.b,
                })
            })
            .collect::<Vec<_>>();
        buffer.set_selection(
            view,
            Self::selection(regions, buffer.view(view).sel().primary_index()),
        );
        if matches!(key, 'o' | 'O') {
            let reps = buffer
                .view(view)
                .sel()
                .iter()
                .map(|r| {
                    let start = bol(buffer.text(), r.b);
                    let indent = buffer
                        .text()
                        .slice(start..first(buffer.text(), r.b))
                        .to_string();
                    let e = end(buffer.text(), r.b);
                    if key == 'O' {
                        let text = format!("{indent}\n");
                        Replacement {
                            range: start..start,
                            caret: indent.len(),
                            text,
                        }
                    } else if e < buffer.text().len_bytes() {
                        let text = format!("{indent}\n");
                        Replacement {
                            range: e + 1..e + 1,
                            caret: indent.len(),
                            text,
                        }
                    } else {
                        let text = format!("\n{indent}");
                        Replacement {
                            range: e..e,
                            caret: text.len(),
                            text,
                        }
                    }
                })
                .collect();
            self.apply(buffer, view, reps);
        }
    }
    fn direct_edit(&mut self, buffer: &mut Buffer, view: ViewId, key: char, count: usize) {
        if self.is_visual() {
            self.operator = Some(
                if matches!(key, 's' | 'S' | 'C') {
                    "c"
                } else if key == 'Y' {
                    "y"
                } else {
                    "d"
                }
                .into(),
            );
            let ranges = buffer
                .view(view)
                .sel()
                .iter()
                .map(Region::to_range)
                .collect();
            self.operate(buffer, view, ranges, matches!(key, 'S' | 'C' | 'D' | 'Y'));
            return;
        }
        let linewise = matches!(key, 'S' | 'Y');
        let ranges = buffer
            .view(view)
            .sel()
            .iter()
            .map(|r| {
                if linewise {
                    let row = buffer.text().byte_to_line(r.b);
                    let last = (row + count).min(buffer.text().len_lines());
                    bol(buffer.text(), r.b)..if last == buffer.text().len_lines() {
                        buffer.text().len_bytes()
                    } else {
                        buffer.text().line_to_byte(last)
                    }
                } else if matches!(key, 'C' | 'D') {
                    let row = (buffer.text().byte_to_line(r.b) + count - 1)
                        .min(buffer.text().len_lines() - 1);
                    r.b..end(buffer.text(), buffer.text().line_to_byte(row))
                } else if key == 'X' {
                    let mut a = r.b;
                    for _ in 0..count {
                        a = buffer
                            .text()
                            .prev_grapheme_boundary(a)
                            .max(bol(buffer.text(), r.b));
                    }
                    a..r.b
                } else {
                    let mut b = r.b;
                    for _ in 0..count {
                        b = buffer
                            .text()
                            .next_grapheme_boundary(b)
                            .min(end(buffer.text(), r.b));
                    }
                    r.b..b
                }
            })
            .collect();
        self.operator = Some(
            if matches!(key, 's' | 'S' | 'C') {
                "c"
            } else if key == 'Y' {
                "y"
            } else {
                "d"
            }
            .into(),
        );
        self.operate(buffer, view, ranges, linewise);
    }
    fn paste(&mut self, buffer: &mut Buffer, view: ViewId, before: bool, count: usize) {
        let name = self.register;
        let clip = self.settings.vintage_use_clipboard && matches!(name, '"' | '+');
        let value = if clip {
            self.clipboard.as_ref().map(|s| Register {
                fragments: vec![s.clone()],
                kind: if s.ends_with('\n') {
                    RegisterKind::Line
                } else {
                    RegisterKind::Character
                },
            })
        } else {
            self.registers.get(&name).cloned()
        };
        let Some(value) = value else {
            return;
        };
        if value.fragments.is_empty() {
            return;
        }
        let visual = self.is_visual();
        let regions = buffer.view(view).sel().iter().collect::<Vec<_>>();
        let mut reps = Vec::new();
        if value.kind == RegisterKind::Block && !visual {
            for r in &regions {
                let row = buffer.text().byte_to_line(r.b);
                let col = r.b - bol(buffer.text(), r.b);
                for (i, part) in value.fragments.iter().enumerate() {
                    if row + i >= buffer.text().len_lines() {
                        break;
                    }
                    let start = buffer.text().line_to_byte(row + i);
                    let e = end(buffer.text(), start);
                    let at = buffer.text().floor_char_boundary((start + col).min(e));
                    let at = if before {
                        at
                    } else {
                        buffer.text().next_grapheme_boundary(at).min(e)
                    };
                    let text = part.repeat(count);
                    reps.push(Replacement {
                        range: at..at,
                        text,
                        caret: 0,
                    });
                }
            }
        } else {
            for (i, r) in regions.iter().enumerate() {
                let fragment = if value.fragments.len() == regions.len() {
                    &value.fragments[i]
                } else {
                    &value.fragments.join(if value.kind == RegisterKind::Line {
                        ""
                    } else {
                        "\n"
                    })
                };
                let mut text = fragment.repeat(count);
                let range = if visual {
                    r.to_range()
                } else if value.kind == RegisterKind::Line {
                    let at = if before {
                        bol(buffer.text(), r.b)
                    } else {
                        let e = end(buffer.text(), r.b);
                        if e < buffer.text().len_bytes() {
                            e + 1
                        } else {
                            e
                        }
                    };
                    if at == buffer.text().len_bytes()
                        && at > 0
                        && buffer.text().byte_at(at - 1) != b'\n'
                    {
                        text.insert(0, '\n');
                    }
                    at..at
                } else {
                    let at = if before {
                        r.b
                    } else {
                        buffer
                            .text()
                            .next_grapheme_boundary(r.b)
                            .min(end(buffer.text(), r.b))
                    };
                    at..at
                };
                let caret = if value.kind == RegisterKind::Line {
                    usize::from(text.starts_with('\n'))
                } else {
                    text.len()
                        .saturating_sub(text.chars().next_back().map_or(0, char::len_utf8))
                };
                reps.push(Replacement { range, text, caret });
            }
        }
        self.mode = Mode::Normal;
        self.visual.clear();
        self.apply(buffer, view, reps);
    }
    fn join(&mut self, buffer: &mut Buffer, view: ViewId, count: usize) {
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for r in buffer.view(view).sel().iter() {
            let row = buffer.text().byte_to_line(r.begin());
            let last = if self.is_visual() {
                buffer
                    .text()
                    .byte_to_line(buffer.text().prev_grapheme_boundary(r.end()))
            } else {
                (row + count - 1).min(buffer.text().len_lines() - 1)
            }
            .max(row);
            let mut a = buffer.text().line_to_byte(row);
            let b = end(buffer.text(), buffer.text().line_to_byte(last));
            if a == b {
                continue;
            }
            if let Some(previous) = ranges.last_mut()
                && a <= previous.end
            {
                previous.end = previous.end.max(b);
            } else {
                a = buffer.text().line_to_byte(row);
                ranges.push(a..b);
            }
        }
        let reps = ranges
            .into_iter()
            .filter_map(|range| {
                let source = buffer.text().slice(range.clone()).to_string();
                if !source.contains('\n') {
                    return None;
                }
                let caret = source.find('\n').unwrap();
                let mut text = String::new();
                for part in source.split('\n') {
                    if text.is_empty() {
                        text.push_str(part.trim_end_matches([' ', '\t']));
                    } else {
                        let part = part.trim_start_matches([' ', '\t']);
                        if !text.ends_with(' ') && !part.starts_with(')') {
                            text.push(' ');
                        }
                        text.push_str(part.trim_end_matches([' ', '\t']));
                    }
                }
                Some(Replacement { range, text, caret })
            })
            .collect();
        self.mode = Mode::Normal;
        self.visual.clear();
        self.apply(buffer, view, reps);
    }
    fn history(&mut self, buffer: &mut Buffer, view: ViewId, redo: bool, count: usize) {
        for _ in 0..count {
            let group = if redo {
                self.redo.pop()
            } else {
                self.undo.pop()
            };
            if let Some(group) = group {
                let mut ok = true;
                let mut remaining = group.steps;
                while remaining > 0 {
                    let from = buffer.change_id();
                    ok = if redo {
                        buffer.redo(view)
                    } else {
                        buffer.undo(view)
                    };
                    if !ok {
                        break;
                    }
                    // A native typing group may contain several transactions.
                    // Count the transactions actually restored, never undo past it.
                    remaining = remaining.saturating_sub((buffer.change_id().0 - from.0) as usize);
                }
                if ok {
                    let saved = if redo { &group.after } else { &group.before };
                    let clip = |p: usize| {
                        buffer
                            .text()
                            .floor_char_boundary(p.min(buffer.text().len_bytes()))
                    };
                    let bounded = Self::selection(
                        saved
                            .iter()
                            .map(|r| Region {
                                a: clip(r.a),
                                b: clip(r.b),
                                ..r
                            })
                            .collect(),
                        saved.primary_index(),
                    );
                    buffer.set_selection(view, bounded);
                    if redo {
                        self.undo.push(group);
                    } else {
                        self.redo.push(group);
                    }
                } else {
                    self.undo.clear();
                    self.redo.clear();
                    break;
                }
            } else {
                if redo {
                    buffer.redo(view);
                } else {
                    buffer.undo(view);
                }
                break;
            }
        }
        self.normalize(buffer, view);
    }
    fn search_move(
        &mut self,
        buffer: &mut Buffer,
        view: ViewId,
        query: &str,
        forward: bool,
        whole: bool,
        count: usize,
    ) {
        let regex = if whole {
            regex::Regex::new(&format!(r"\b{}\b", regex::escape(query)))
        } else {
            regex::Regex::new(query)
        };
        let Ok(regex) = regex else {
            self.reset();
            return;
        };
        let text = buffer.text().to_string();
        let points = regex
            .find_iter(&text)
            .map(|m| m.start())
            .filter(|p| {
                *p == 0
                    || buffer
                        .text()
                        .prev_grapheme_boundary(buffer.text().next_grapheme_boundary(*p))
                        == *p
            })
            .collect::<Vec<_>>();
        if points.is_empty() {
            return;
        }
        let regions = if self.is_visual() {
            self.visual.clone()
        } else {
            buffer.view(view).sel().iter().collect()
        };
        let targets = regions
            .into_iter()
            .map(|r| {
                let mut at = r.b;
                for _ in 0..count {
                    at = if forward {
                        points
                            .iter()
                            .copied()
                            .find(|p| *p > at)
                            .unwrap_or(points[0])
                    } else {
                        points
                            .iter()
                            .copied()
                            .rev()
                            .find(|p| *p < at)
                            .unwrap_or(*points.last().unwrap())
                    };
                }
                Motion {
                    point: at,
                    inclusive: false,
                    linewise: false,
                    vertical: false,
                }
            })
            .collect();
        self.apply_motions(
            buffer,
            view,
            targets,
            &vel_buffer::MonospaceLayout::default(),
        );
    }
    fn prompt_key(
        &mut self,
        key: Key,
        buffer: &mut Buffer,
        view: ViewId,
        layout: &dyn VerticalLayout,
    ) -> Outcome {
        match key {
            Key::Char(c) => self.prompt.push(c),
            Key::Text(s) => self.prompt.push_str(&s),
            Key::Backspace => {
                self.prompt.pop();
            }
            Key::Enter => {
                let text = std::mem::take(&mut self.prompt);
                self.mode = self.prompt_origin;
                if self.prompt_kind != ':' {
                    if text.is_empty() {
                        if let Some((query, _, whole)) = self.search.clone() {
                            self.search = Some((query.clone(), self.prompt_kind == '/', whole));
                            self.search_move(
                                buffer,
                                view,
                                &query,
                                self.prompt_kind == '/',
                                whole,
                                self.prompt_count,
                            );
                        }
                    } else {
                        self.search = Some((text.clone(), self.prompt_kind == '/', false));
                        self.search_move(
                            buffer,
                            view,
                            &text,
                            self.prompt_kind == '/',
                            false,
                            self.prompt_count,
                        );
                    }
                } else if let Ok(line) = text.trim().parse::<usize>() {
                    self.move_key(buffer, view, layout, "gg", line.max(1), true);
                } else {
                    let (verb, arg) = text
                        .trim()
                        .split_once(char::is_whitespace)
                        .unwrap_or((text.trim(), ""));
                    let (verb, force) = verb.strip_suffix('!').map_or((verb, false), |s| (s, true));
                    let name = match verb {
                        "w" => Some("save"),
                        "wq" => Some("save_and_close"),
                        "q" => Some("close"),
                        "e" => Some("open_file"),
                        _ => None,
                    };
                    if let Some(name) = name {
                        let mut args = BTreeMap::new();
                        if !arg.trim().is_empty() {
                            args.insert("file".into(), arg.trim().into());
                        }
                        if force {
                            args.insert("force".into(), "true".into());
                        }
                        self.request = Some(Command { name, args });
                    }
                }
                self.count = 0;
            }
            _ => return Outcome::PassThrough,
        }
        Outcome::Handled
    }
}

#[cfg(test)]
mod review_tests;
