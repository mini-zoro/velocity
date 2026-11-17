//! Records ordinary host editing as relative changes for vi dot/count replay.
use super::*;
use std::sync::Arc;

/// An observed host edit (including selection motion), replayed at the new carets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertChange {
    data: Arc<InsertData>,
}
#[derive(Debug, PartialEq, Eq)]
struct InsertData {
    parts: Vec<Part>,
    edits: Vec<(Range<isize>, Range<usize>)>,
    text: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Part {
    edits: Range<usize>,
    a: isize,
    b: isize,
}
// Bound disjoint edit metadata as well as ordinary contiguous typing. If a
// session moves through more independent locations, retire its repeat record
// rather than retaining an unbounded patch program or replaying an older one.
const SPANS_PER_CARET: usize = 32;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recording {
    Insert,
    History,
    External,
}
#[derive(Debug)]
pub(super) struct Snapshot {
    identity: (BufferId, ViewId, ChangeId),
    record: Recording,
    selection: Selection,
}
/// One net delta, with shared rope storage, regardless of the number of keys.
#[derive(Debug)]
pub(super) struct Session {
    original: vel_text::Text,
    origins: Selection,
    changes: vel_buffer::ChangeSet,
    replayable: bool,
}
impl Vintage {
    /// Bytes retained for insert replay metadata and payloads, excluding shared
    /// rope nodes and the host's undo history. Useful for multi-caret diagnostics.
    pub fn insert_recording_bytes(&self) -> usize {
        let keys = self.command_keys.capacity() * std::mem::size_of::<Key>();
        let snapshot = self
            .insert_snapshot
            .as_ref()
            .map_or(0, |s| s.selection.storage_bytes());
        let session = self.insert_session.as_ref().map_or(0, |s| {
            s.origins.storage_bytes()
                + s.changes.changes().len()
                    * (std::mem::size_of::<vel_buffer::Change>() + std::mem::size_of::<usize>())
        });
        keys + snapshot
            + session
            + self
                .command_keys
                .iter()
                .map(|k| match k {
                    Key::HostInsert(record) => {
                        record.data.parts.capacity() * std::mem::size_of::<Part>()
                            + record.data.edits.capacity()
                                * std::mem::size_of::<(Range<isize>, Range<usize>)>()
                            + record.data.text.capacity()
                    }
                    _ => 0,
                })
                .sum::<usize>()
    }
    /// Uses ordinary host editing while observing committed changes for vi replay.
    pub fn use_host_insert(&mut self) {
        self.host_insert = true;
    }

    /// Classifies the next host command without changing its undo grouping.
    pub fn prepare_host_command(&mut self, name: &str, buffer: &mut Buffer, view: ViewId) {
        self.observe_insert(buffer, view);
        if !self.host_insert || self.mode != Mode::Insert {
            return;
        }
        self.undo_start
            .get_or_insert_with(|| buffer.mark_undo_group());
        if let Some(snapshot) = &mut self.insert_snapshot {
            snapshot.record = match name {
                "undo" | "redo" | "soft_undo" | "soft_redo" | "redo_or_repeat" => {
                    Recording::History
                }
                "insert"
                | "left_delete"
                | "right_delete"
                | "delete_word"
                | "paste"
                | "paste_and_indent"
                | "insert_snippet"
                | "insert_best_completion"
                | "commit_completion"
                | "replace_completion_with_next_completion"
                | "indent"
                | "unindent" => Recording::Insert,
                _ => Recording::External,
            };
        }
    }
    fn invalidate_insert(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.steps = 0;
        self.before = None;
        self.host_action = None;
        self.replace_restore.clear();
        // Preserve the session entry (a/A/I/o/change/count), but never reuse an
        // older dot command if an intervening host command invalidates this one.
        self.last_change.clear();
        self.insert_count = 1;
        self.inserted.clear();
    }
    fn history_insert_entry(&mut self) -> bool {
        let count = self
            .command_keys
            .iter()
            .take_while(|k| matches!(k, Key::Char('0'..='9')))
            .count();
        if matches!(
            &self.command_keys[count..],
            [] | [Key::Char('i' | 'a' | 'I' | 'A')]
        ) {
            // Host history cancelled the count. Repeat the surviving net text
            // once, at the original entry position (not as an implicit `i`).
            self.command_keys.drain(..count);
            true
        } else {
            // Change/open-line commands edited before the host snapshot began;
            // after host history their entry effect cannot be replayed reliably.
            false
        }
    }
    /// Accumulates only changed intervals. No per-key replay arrays or document diff.
    pub fn observe_insert(&mut self, buffer: &mut Buffer, view: ViewId) {
        if !self.host_insert || self.mode != Mode::Insert || self.repeating {
            self.insert_snapshot = None;
            return;
        }
        let identity = (buffer.id(), view, buffer.change_id());
        if let Some(old) = self.insert_snapshot.take()
            && old.identity.0 == identity.0
            && old.identity.1 == view
        {
            let changed = old.identity.2 != identity.2;
            if !changed
                && old.selection != *buffer.view(view).sel()
                && let Some(session) = &mut self.insert_session
                && session.changes.changes().is_empty()
            {
                // A mouse/multi-caret selection before the first insertion is
                // the session's true origin, not the selection at enable time.
                session.origins = buffer.view(view).sel().clone();
            }
            if changed {
                let maps = buffer.changes_since(old.identity.2);
                let entry = old.record != Recording::History || self.history_insert_entry();
                if let Some(maps) = maps.as_ref().filter(|_| old.record == Recording::Insert) {
                    if self.before.is_none() {
                        self.before = Some(old.selection.clone());
                    }
                    if self.command_keys.is_empty() {
                        self.command_keys.push(Key::Char('i'));
                    }
                    self.steps += maps.len();
                } else {
                    self.invalidate_insert();
                    if old.record == Recording::External {
                        self.undo_start = None;
                    }
                }
                if let Some(session) = &mut self.insert_session {
                    session.replayable &=
                        entry && old.record != Recording::External && maps.is_some();
                    if session.replayable
                        && let Some(maps) = maps
                    {
                        for (_, map) in maps {
                            session.changes = compose_pair(&session.changes, &map);
                            if session.changes.changes().len()
                                > session.origins.len().saturating_mul(SPANS_PER_CARET)
                            {
                                session.replayable = false;
                                session.changes = vel_buffer::ChangeSet::default();
                                break;
                            }
                        }
                    }
                }
            }
        } else {
            self.undo_start
                .get_or_insert_with(|| buffer.mark_undo_group());
            self.insert_session = Some(Box::new(Session {
                original: buffer.text().clone(),
                origins: buffer.view(view).sel().clone(),
                changes: vel_buffer::ChangeSet::default(),
                replayable: true,
            }));
        }
        self.bound = Some((buffer.id(), view));
        self.seen = Some(buffer.change_id());
        self.insert_snapshot = Some(Snapshot {
            identity,
            selection: buffer.view(view).sel().clone(),
            record: Recording::External,
        });
    }
    pub(super) fn finish_host_insert(&mut self, buffer: &Buffer, view: ViewId) {
        let Some(session) = self.insert_session.take() else {
            return;
        };
        if !session.replayable || session.origins.len() != buffer.view(view).sel().len() {
            self.last_change.clear();
            self.command_keys.clear();
            return;
        }
        let origins: Vec<_> = session.origins.iter().map(|r| r.b).collect();
        let references = left_points(&session.changes, &origins);
        let mut parts: Vec<_> = references
            .iter()
            .zip(buffer.view(view).sel().iter())
            .map(|(&reference, r)| Part {
                edits: 0..0,
                a: r.a as isize - reference as isize,
                b: r.b as isize - reference as isize,
            })
            .collect();
        let mut left = 0;
        let mut delta = 0isize;
        let mut edits = Vec::new();
        let mut text = String::new();
        for change in session.changes.changes() {
            let start = change.old_range.start.saturating_add_signed(delta);
            let payload = buffer.text().slice(start..start + change.new_len);
            let original = session.original.slice(change.old_range.clone());
            delta += change.new_len as isize - change.old_range.len() as isize;
            if original == payload {
                continue;
            }
            while left + 1 < origins.len() && origins[left + 1] <= change.old_range.start {
                left += 1;
            }
            let owner = if left + 1 < origins.len()
                && origins[left + 1].abs_diff(change.old_range.start)
                    < origins[left].abs_diff(change.old_range.start)
            {
                left + 1
            } else {
                left
            };
            if parts[owner].edits.is_empty() {
                parts[owner].edits.start = edits.len();
            }
            let text_start = text.len();
            for chunk in payload.chunks() {
                text.push_str(chunk);
            }
            let origin = origins[owner] as isize;
            edits.push((
                change.old_range.start as isize - origin..change.old_range.end as isize - origin,
                text_start..text.len(),
            ));
            parts[owner].edits.end = edits.len();
        }
        if edits.is_empty() {
            self.command_keys.clear();
            return;
        }
        if self.command_keys.is_empty() {
            self.command_keys.push(Key::Char('i'));
        }
        let record = Key::HostInsert(InsertChange {
            data: Arc::new(InsertData { parts, edits, text }),
        });
        if let Some(reg) = self.recording {
            self.macros.entry(reg).or_default().push(record.clone());
        }
        self.command_keys.push(record);
        if self.steps == 0 && !self.repeating {
            // Native undo/redo invalidated vi undo accounting, but a surviving
            // net insertion is still a valid dot command without another key.
            self.last_change = self.command_keys.clone();
            self.last_change.push(Key::Escape);
        }
    }
    pub(super) fn replay_insert(
        &mut self,
        record: InsertChange,
        buffer: &mut Buffer,
        view: ViewId,
    ) {
        let record = &record.data;
        if record.parts.is_empty() {
            return;
        }
        let old = buffer.view(view).sel().clone();
        let mut reps = Vec::new();
        for (i, r) in old.iter().enumerate() {
            let part = &record.parts[i % record.parts.len()];
            for (range, text) in &record.edits[part.edits.clone()] {
                let text = &record.text[text.clone()];
                let a =
                    r.b.saturating_add_signed(range.start)
                        .min(buffer.text().len_bytes());
                let b =
                    r.b.saturating_add_signed(range.end)
                        .min(buffer.text().len_bytes());
                reps.push(Replacement {
                    range: buffer.text().floor_char_boundary(a)
                        ..buffer.text().floor_char_boundary(b.max(a)),
                    text: text.to_owned(),
                    caret: text.len(),
                });
            }
        }
        let changes = vel_buffer::ChangeSet::new(
            reps.iter()
                .map(|r| vel_buffer::Change {
                    old_range: r.range.clone(),
                    new_len: r.text.len(),
                })
                .collect(),
        );
        if !reps.is_empty() {
            self.apply(buffer, view, reps);
        }
        let regions = old
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let reference = changes.transform_point(r.b, Bias::Left);
                let part = &record.parts[i % record.parts.len()];
                let at = |d| {
                    buffer.text().floor_char_boundary(
                        reference
                            .saturating_add_signed(d)
                            .min(buffer.text().len_bytes()),
                    )
                };
                Region::new(at(part.a), at(part.b))
            })
            .collect();
        buffer.set_selection(view, Self::selection(regions, old.primary_index()));
    }
}
/// Left-biased mapping of ordered caret origins, in one pass over the changes.
fn left_points(changes: &vel_buffer::ChangeSet, points: &[usize]) -> Vec<usize> {
    let mut at = 0;
    let mut delta = 0isize;
    points
        .iter()
        .map(|&p| {
            while let Some(c) = changes.changes().get(at) {
                if c.old_range.end > p || (c.old_range.end == p && c.old_range.is_empty()) {
                    break;
                }
                delta += c.new_len as isize - c.old_range.len() as isize;
                at += 1;
            }
            let p = changes
                .changes()
                .get(at)
                .filter(|c| c.old_range.start <= p)
                .map_or(p, |c| c.old_range.start);
            p.saturating_add_signed(delta)
        })
        .collect()
}
/// Ordered endpoint mapping. Right edges inside a replacement include its full
/// output, unlike caret mapping, so replacements compose without losing bytes.
fn edges(
    map: &vel_buffer::ChangeSet,
    points: impl Iterator<Item = usize>,
    right: bool,
) -> Vec<usize> {
    let mut at = 0;
    let mut delta = 0isize;
    points
        .map(|p| {
            while let Some(c) = map.changes().get(at) {
                if c.old_range.end > p || (c.old_range.end == p && c.old_range.is_empty()) {
                    break;
                }
                delta += c.new_len as isize - c.old_range.len() as isize;
                at += 1;
            }
            if let Some(c) = map.changes().get(at).filter(|c| c.old_range.start <= p) {
                c.old_range.start.saturating_add_signed(delta) + if right { c.new_len } else { 0 }
            } else {
                p.saturating_add_signed(delta)
            }
        })
        .collect()
}
/// Merge two ordered deltas in O(changes), retaining no per-key history.
fn compose_pair(a: &vel_buffer::ChangeSet, b: &vel_buffer::ChangeSet) -> vel_buffer::ChangeSet {
    if a.changes().is_empty() {
        return b.clone();
    }
    if b.changes().is_empty() {
        return a.clone();
    }
    let inverse = a.inverse();
    let starts = edges(
        &inverse,
        b.changes().iter().map(|c| c.old_range.start),
        false,
    );
    let ends = edges(&inverse, b.changes().iter().map(|c| c.old_range.end), true);
    let mut newer = starts.into_iter().zip(ends).map(|(s, e)| s..e).peekable();
    let mut older = a.changes().iter().map(|c| c.old_range.clone()).peekable();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    while older.peek().is_some() || newer.peek().is_some() {
        let r = if newer.peek().is_none()
            || older
                .peek()
                .is_some_and(|o| o.start <= newer.peek().unwrap().start)
        {
            older.next().unwrap()
        } else {
            newer.next().unwrap()
        };
        if let Some(last) = ranges.last_mut()
            && r.start <= last.end
        {
            last.end = last.end.max(r.end);
        } else {
            ranges.push(r);
        }
    }
    let starts = edges(a, ranges.iter().map(|r| r.start), false);
    let ends = edges(a, ranges.iter().map(|r| r.end), true);
    let starts = edges(b, starts.into_iter(), false);
    let ends = edges(b, ends.into_iter(), true);
    vel_buffer::ChangeSet::new(
        ranges
            .into_iter()
            .zip(starts.into_iter().zip(ends))
            .map(|(old_range, (start, end))| vel_buffer::Change {
                old_range,
                new_len: end - start,
            })
            .collect(),
    )
}
