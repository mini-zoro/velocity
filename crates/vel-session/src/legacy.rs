//! Read compatibility for the round-seven command journal.
use super::history::{ViewState, selection};
use serde::{Deserialize, Deserializer};
use vel_buffer::{Buffer, Edit, EditKind, Selection, ViewId};
use vel_text::Text;
#[derive(Clone, Debug)]
struct Revision {
    text: Text,
    view: ViewState,
    before: ViewState,
}
/// Read-only decoder for profiles produced before native history support.
#[derive(Clone, Debug)]
pub struct History {
    revisions: Vec<Revision>,
    cursor: usize,
    saved: Option<usize>,
}

impl History {
    /// Replays groups through public transactions and restores the redo cursor/save point.
    /// `keep_history=false` retains contents while dropping undo after a disk change.
    pub fn restore(&self, keep_history: bool, dirty: bool) -> (Buffer, ViewId) {
        if !keep_history {
            let mut buffer = Buffer::new(self.revisions[self.cursor].text.clone());
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
#[derive(Deserialize)]
struct Wire {
    base: String,
    steps: Vec<Step>,
    cursor: usize,
    saved: Option<usize>,
    views: Vec<ViewState>,
    #[serde(default)]
    before: Vec<ViewState>,
}
#[derive(Deserialize)]
struct Step {
    start: usize,
    end: usize,
    text: String,
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
