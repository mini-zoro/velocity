//! Public-API undo journal. Wire format stores one base plus UTF-8-safe replacement deltas.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use vel_buffer::{Buffer, Edit, EditKind, Region, Selection, ViewId};
use vel_text::Text;

/// Directed UTF-8 region, including the vertical movement goal.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Position {
    /// Anchor byte.
    pub a: usize,
    /// Caret byte.
    pub b: usize,
    /// Optional preferred visual column.
    pub xpos: Option<f64>,
}
impl From<Region> for Position {
    fn from(r: Region) -> Self {
        Self {
            a: r.a,
            b: r.b,
            xpos: r.xpos,
        }
    }
}
/// View-local session data. All byte positions are clipped to valid boundaries on restore.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViewState {
    /// Sorted directed selections.
    pub selections: Vec<Position>,
    /// Primary selection index.
    pub primary: usize,
    /// Horizontal and vertical logical scroll offsets.
    pub scroll: (f64, f64),
    /// Folded regions.
    pub folds: Vec<Position>,
}
impl Default for ViewState {
    fn default() -> Self {
        Self {
            selections: vec![Position::default()],
            primary: 0,
            scroll: (0., 0.),
            folds: vec![],
        }
    }
}
impl ViewState {
    /// Copies only selections/folds, retaining no document contents.
    pub fn capture(buffer: &Buffer, id: ViewId, scroll: (f64, f64)) -> Self {
        let sel = buffer.view(id).sel();
        Self {
            selections: sel.iter().map(Position::from).collect(),
            primary: sel.primary_index(),
            scroll,
            folds: buffer
                .view(id)
                .get_regions("velocity.folds")
                .map(|s| s.regions.iter().copied().map(Position::from).collect())
                .unwrap_or_default(),
        }
    }
    /// Rejects invalid floating point values or oversized selection arrays.
    pub fn validate(&self) -> Result<(), String> {
        if self.selections.is_empty()
            || self.selections.len() > 1_000_000
            || self.primary >= self.selections.len()
            || [self.scroll.0, self.scroll.1]
                .iter()
                .any(|n| !n.is_finite() || *n < 0.)
            || self
                .selections
                .iter()
                .chain(&self.folds)
                .any(|p| p.xpos.is_some_and(|n| !n.is_finite()))
        {
            return Err("invalid session view state".into());
        }
        Ok(())
    }
    /// Applies view state through the buffer's public selection/region APIs.
    pub fn apply(&self, buffer: &mut Buffer, id: ViewId) {
        let selection = selection(&self.selections, self.primary, buffer.text());
        buffer.set_selection(id, selection);
        let text = buffer.text();
        let folds = self
            .folds
            .iter()
            .map(|p| {
                Region::new(
                    text.floor_char_boundary(p.a.min(text.len_bytes())),
                    text.floor_char_boundary(p.b.min(text.len_bytes())),
                )
            })
            .filter(|r| !r.empty())
            .collect();
        buffer.add_regions(id, "velocity.folds", folds, true);
    }
}
fn selection(positions: &[Position], primary: usize, text: &Text) -> Selection {
    let mut regions: Vec<_> = positions
        .iter()
        .map(|p| Region {
            a: text.floor_char_boundary(p.a.min(text.len_bytes())),
            b: text.floor_char_boundary(p.b.min(text.len_bytes())),
            xpos: p.xpos,
        })
        .collect();
    // Selection::from_regions makes the last inserted region primary.
    if primary < regions.len() {
        let r = regions.remove(primary);
        regions.push(r);
    }
    Selection::from_regions(regions)
}
#[derive(Clone, Debug)]
struct Revision {
    text: Text,
    view: ViewState,
    before: ViewState,
}
/// Bounded mirror of user-visible undo groups. It is independent of private buffer internals.
/// Soft selection history and transaction maps are not serialized (see README API request).
#[derive(Clone, Debug)]
pub struct History {
    revisions: Vec<Revision>,
    cursor: usize,
    saved: Option<usize>,
    version: vel_buffer::ChangeId,
    identity: vel_buffer::BufferId,
    grouping: Option<String>,
    last_view: Option<ViewId>,
    has_word: bool,
    pending_before: Option<ViewState>,
    retained: usize,
}
impl History {
    /// Captures the starting state in O(1) text space.
    pub fn new(buffer: &Buffer, id: ViewId) -> Self {
        Self {
            revisions: vec![Revision {
                text: buffer.text().clone(),
                view: ViewState::capture(buffer, id, (0., 0.)),
                before: ViewState::capture(buffer, id, (0., 0.)),
            }],
            cursor: 0,
            saved: (!buffer.is_dirty()).then_some(0),
            version: buffer.change_id(),
            identity: buffer.id(),
            grouping: None,
            last_view: None,
            has_word: false,
            pending_before: None,
            retained: 0,
        }
    }
    /// Current unsaved contents, even when undo history must be discarded.
    pub fn text(&self) -> &Text {
        &self.revisions[self.cursor].text
    }
    /// Records the active view selection immediately before a primitive command.
    pub fn before_command(&mut self, buffer: &Buffer, id: ViewId) {
        self.pending_before = Some(ViewState::capture(buffer, id, (0., 0.)));
    }
    /// Ends typing coalescence after an explicit user selection change.
    pub fn break_group(&mut self) {
        self.grouping = None;
    }
    /// Rebinds a deserialized journal to the newly reconstructed public buffer.
    pub fn rebind(mut self, buffer: &Buffer, id: ViewId) -> Self {
        self.identity = buffer.id();
        self.version = buffer.change_id();
        self.revisions[self.cursor].view = ViewState::capture(buffer, id, (0., 0.));
        self
    }
    /// Number of retained user-visible groups, including redo.
    pub fn groups(&self) -> usize {
        self.revisions.len() - 1
    }
    /// Observes a primitive command or external transaction. `hint` is the command name,
    /// or `typing:word`, `typing:space`, `typing:punct`, `typing:newline` for insertions.
    pub fn observe(&mut self, buffer: &Buffer, id: ViewId, hint: &str) {
        if self.identity != buffer.id() {
            *self = Self::new(buffer, id);
            return;
        }
        if self.version == buffer.change_id() {
            if !buffer.is_dirty() {
                self.saved = Some(self.cursor);
                self.grouping = None;
            }
            return;
        }
        self.version = buffer.change_id();
        let revision = Revision {
            text: buffer.text().clone(),
            view: ViewState::capture(buffer, id, (0., 0.)),
            before: self
                .pending_before
                .take()
                .unwrap_or_else(|| self.revisions[self.cursor].view.clone()),
        };
        if matches!(hint, "undo" | "redo" | "soft_undo" | "soft_redo") {
            let range: Box<dyn Iterator<Item = usize>> = if matches!(hint, "undo" | "soft_undo") {
                Box::new((0..self.cursor).rev())
            } else {
                Box::new(self.cursor + 1..self.revisions.len())
            };
            if let Some(index) = range
                .into_iter()
                .find(|i| self.revisions[*i].text == revision.text)
            {
                self.cursor = index;
                self.grouping = None;
                if !buffer.is_dirty() {
                    self.saved = Some(index);
                }
                return;
            }
        }
        let merge = self.last_view == Some(id)
            && self.cursor > 0
            && self.cursor + 1 == self.revisions.len()
            && hint.starts_with("typing:")
            && hint != "typing:mixed"
            && self.grouping.as_deref().is_some_and(|old| {
                old == hint
                    || (old == "typing:word" && hint == "typing:space")
                    || (!self.has_word && old == "typing:space" && hint == "typing:word")
            });
        if self.saved.is_some_and(|s| s > self.cursor) {
            self.saved = None;
        }
        self.revisions.truncate(self.cursor + 1);
        let retained = if hint.starts_with("typing:") {
            16 * 1024
                + buffer.last_undo_metadata_bytes()
                + self.text().len_bytes().abs_diff(revision.text.len_bytes())
        } else {
            self.text().estimated_retained_bytes(&revision.text) + 256
        };
        self.retained = self.retained.saturating_add(retained);
        if merge {
            if self.saved == Some(self.cursor) {
                self.saved = None;
            }
            let before = self.revisions[self.cursor].before.clone();
            self.revisions[self.cursor] = Revision { before, ..revision };
        } else {
            self.revisions.push(revision);
            self.cursor += 1;
        }
        self.has_word = (merge && self.has_word) || hint == "typing:word";
        self.last_view = Some(id);
        self.grouping =
            (hint.starts_with("typing:") && hint != "typing:mixed").then(|| hint.to_owned());
        if !buffer.is_dirty() {
            self.saved = Some(self.cursor);
        }
        let limits = buffer.history_limits();
        while self.revisions.len() > 1
            && (self.groups() > limits.entries || self.retained > limits.bytes)
        {
            self.revisions.remove(0);
            self.cursor = self.cursor.saturating_sub(1);
            self.saved = self.saved.and_then(|s| s.checked_sub(1));
            self.retained = self
                .revisions
                .windows(2)
                .map(|p| p[0].text.estimated_retained_bytes(&p[1].text) + 256)
                .sum();
        }
    }
    /// Replays groups through public transactions and restores the redo cursor/save point.
    /// `keep_history=false` retains contents while dropping undo after a disk change.
    pub fn restore(&self, keep_history: bool, dirty: bool) -> (Buffer, ViewId) {
        if !keep_history {
            let mut buffer = Buffer::new(self.text().clone());
            let id = buffer.new_view();
            if dirty {
                mark_dirty_without_history(&mut buffer, id);
            }
            self.revisions[self.cursor].view.apply(&mut buffer, id);
            return (buffer, id);
        }
        let mut buffer = Buffer::new(self.revisions[0].text.clone());
        let id = buffer.new_view();
        if self.saved.is_none() {
            mark_dirty_without_history(&mut buffer, id);
        }
        self.revisions[0].view.apply(&mut buffer, id);
        for (index, revision) in self.revisions.iter().enumerate().skip(1) {
            let (range, inserted) = difference(buffer.text(), &revision.text);
            let before = selection(
                &revision.before.selections,
                revision.before.primary,
                buffer.text(),
            );
            buffer.set_selection(id, before);
            let after = selection(
                &revision.view.selections,
                revision.view.primary,
                &revision.text,
            );
            buffer.transact(
                id,
                EditKind::Other,
                &[Edit {
                    range,
                    text: &inserted,
                }],
                after,
            );
            buffer.break_undo_group();
            if self.saved == Some(index) {
                buffer.mark_saved();
            }
        }
        for _ in self.cursor + 1..self.revisions.len() {
            buffer.undo(id);
        }
        if !dirty {
            buffer.mark_saved();
        }
        self.revisions[self.cursor].view.apply(&mut buffer, id);
        (buffer, id)
    }
}
fn difference(before: &Text, after: &Text) -> (std::ops::Range<usize>, String) {
    let a = before.to_string();
    let b = after.to_string();
    let mut start = a.bytes().zip(b.bytes()).take_while(|(a, b)| a == b).count();
    while !a.is_char_boundary(start) || !b.is_char_boundary(start) {
        start -= 1;
    }
    let mut suffix = a[start..]
        .bytes()
        .rev()
        .zip(b[start..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !a.is_char_boundary(a.len() - suffix) || !b.is_char_boundary(b.len() - suffix) {
        suffix -= 1;
    }
    (
        start..a.len() - suffix,
        b[start..b.len() - suffix].to_owned(),
    )
}
#[derive(Serialize, Deserialize)]
struct Wire {
    base: String,
    steps: Vec<Step>,
    cursor: usize,
    saved: Option<usize>,
    views: Vec<ViewState>,
    #[serde(default)]
    before: Vec<ViewState>,
}
#[derive(Serialize, Deserialize)]
struct Step {
    start: usize,
    end: usize,
    text: String,
}
impl Serialize for History {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let steps = self
            .revisions
            .windows(2)
            .map(|p| {
                let (range, text) = difference(&p[0].text, &p[1].text);
                Step {
                    start: range.start,
                    end: range.end,
                    text,
                }
            })
            .collect();
        Wire {
            base: self.revisions[0].text.to_string(),
            steps,
            cursor: self.cursor,
            saved: self.saved,
            views: self.revisions.iter().map(|r| r.view.clone()).collect(),
            before: self.revisions.iter().map(|r| r.before.clone()).collect(),
        }
        .serialize(s)
    }
}
impl<'de> Deserialize<'de> for History {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = Wire::deserialize(d)?;
        let fail = |s| serde::de::Error::custom(s);
        if wire.steps.len() > 10_000
            || wire.cursor > wire.steps.len()
            || wire.views.len() != wire.steps.len() + 1
            || wire.saved.is_some_and(|i| i > wire.steps.len())
        {
            return Err(fail("invalid history bounds"));
        }
        if !wire.before.is_empty() && wire.before.len() != wire.views.len() {
            return Err(fail("invalid history selections"));
        }
        for view in wire.views.iter().chain(&wire.before) {
            view.validate().map_err(serde::de::Error::custom)?;
        }
        let mut text = Text::from(wire.base.as_str());
        let mut revisions = vec![Revision {
            text: text.clone(),
            view: wire.views[0].clone(),
            before: wire.views[0].clone(),
        }];
        for (index, step) in wire.steps.into_iter().enumerate() {
            if step.start > step.end
                || step.end > text.len_bytes()
                || text.floor_char_boundary(step.start) != step.start
                || text.floor_char_boundary(step.end) != step.end
            {
                return Err(fail("invalid history edit"));
            }
            text.apply_edits(&[Edit {
                range: step.start..step.end,
                text: &step.text,
            }]);
            revisions.push(Revision {
                text: text.clone(),
                view: wire.views[index + 1].clone(),
                before: wire
                    .before
                    .get(index + 1)
                    .unwrap_or(&wire.views[index])
                    .clone(),
            });
        }
        Ok(Self {
            revisions,
            cursor: wire.cursor,
            saved: wire.saved,
            version: vel_buffer::ChangeId(0),
            identity: vel_buffer::BufferId(0),
            grouping: None,
            last_view: None,
            has_word: false,
            pending_before: None,
            retained: 0,
        })
    }
}

fn mark_dirty_without_history(buffer: &mut Buffer, id: ViewId) {
    let limits = buffer.history_limits();
    buffer.set_history_limits(vel_buffer::HistoryLimits {
        entries: 0,
        ..limits
    });
    buffer.transact(id, EditKind::Other, &[], Selection::default());
    buffer.set_history_limits(limits);
}
