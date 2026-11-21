#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
use imara_diff::{Algorithm, Diff, InternedInput};
use std::ops::Range;
use vel_buffer::{Bias, ChangeId, ChangeSet};
use vel_text::Text;

/// Gutter classification of a changed line interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    /// New lines only.
    Added,
    /// Replaced lines.
    Modified,
    /// Removed lines; `new` is an empty boundary interval.
    Deleted,
}
/// Half-open, zero-based physical line ranges. LF is part of its preceding line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Reference line interval.
    pub old: Range<usize>,
    /// Current line interval (possibly the EOF boundary).
    pub new: Range<usize>,
}
impl Hunk {
    /// Classification for a gutter marker.
    pub fn kind(&self) -> HunkKind {
        if self.old.is_empty() {
            HunkKind::Added
        } else if self.new.is_empty() {
            HunkKind::Deleted
        } else {
            HunkKind::Modified
        }
    }
}
/// Owned edit plan to apply in one ordinary `Buffer::transact` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertEdit {
    /// Current UTF-8 byte interval.
    pub range: Range<usize>,
    /// Original reference contents, including any terminating LF.
    pub text: String,
}
/// Scalar/word difference inside a line hunk, in absolute UTF-8 byte coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineChange {
    /// Changed reference bytes.
    pub old: Range<usize>,
    /// Changed current bytes.
    pub new: Range<usize>,
}
/// Token granularity for an inline diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineMode {
    /// Unicode scalars; never splits UTF-8.
    Character,
    /// Alphanumeric/underscore runs and individual punctuation/whitespace scalars.
    Word,
}
/// Work performed by the last incremental update, for scheduling and diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdateStats {
    /// Number of independent stable-anchor windows.
    pub windows: usize,
    /// Reference lines re-tokenized.
    pub old_lines: usize,
    /// Current lines re-tokenized.
    pub new_lines: usize,
}
/// A stale update, invalid change descriptor or stale hunk.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// Versions must advance; reset the engine when intermediate changes were lost.
    #[error("stale diff version")]
    StaleVersion,
    /// Lengths or UTF-8 endpoints do not match the old/new snapshots.
    #[error("invalid diff change set")]
    InvalidChange,
    /// Re-query hunks after every update/reference reset.
    #[error("hunk does not belong to the current diff")]
    StaleHunk,
}
/// A reference and a current rope snapshot, plus only their changed line intervals.
/// Own on an analysis worker for initial/reset computations.
#[derive(Debug)]
pub struct DocumentDiff {
    reference: Text,
    text: Text,
    version: ChangeId,
    hunks: Vec<Hunk>,
    stats: UpdateStats,
}
impl DocumentDiff {
    /// Computes the initial histogram diff. Run this on the editor's analysis worker.
    pub fn new(reference: impl Into<Text>, text: Text, version: ChangeId) -> Self {
        let reference = reference.into();
        let old_count = line_count(&reference);
        let new_count = line_count(&text);
        let (reference, hunks) = if reference == text {
            // The saved version can come from an independent read or Git blob.
            // A clean buffer needs no second rope and has no hunks to compute.
            (text.clone(), Vec::new())
        } else {
            let hunks = line_diff(&reference, 0..old_count, &text, 0..new_count);
            (reference, hunks)
        };
        Self {
            reference,
            text,
            version,
            hunks,
            stats: UpdateStats {
                windows: 1,
                old_lines: old_count,
                new_lines: new_count,
            },
        }
    }
    /// Version of current text.
    pub fn version(&self) -> ChangeId {
        self.version
    }
    /// Reference rope; `set_reference_document` replaces it.
    pub fn reference(&self) -> &Text {
        &self.reference
    }
    /// Current rope snapshot.
    pub fn text(&self) -> &Text {
        &self.text
    }
    /// Current gutter hunks, ordered by current line.
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }
    /// Last update's affected windows and line counts.
    pub fn stats(&self) -> UpdateStats {
        self.stats
    }
    /// Replaces the saved-file/HEAD/index reference and recomputes all hunks.
    pub fn set_reference_document(&mut self, reference: impl Into<Text>) {
        *self = Self::new(reference, self.text.clone(), self.version);
    }
    /// Sets the reference to current contents (for a successful ordinary save).
    pub fn reset_reference_document(&mut self) {
        self.set_reference_document(self.text.clone());
    }
    /// Updates only changed line windows expanded through existing hunks to unchanged anchors.
    /// The supplied `ChangeSet` describes exactly this old-to-new text transition.
    /// Disjoint edits retain disjoint windows; unchanged file tails are never tokenized.
    pub fn apply_change(
        &mut self,
        text: Text,
        version: ChangeId,
        changes: &ChangeSet,
    ) -> Result<UpdateStats, Error> {
        if version <= self.version {
            return Err(Error::StaleVersion);
        }
        validate_change(&self.text, &text, changes)?;
        if changes.changes().is_empty() {
            self.text = text;
            self.version = version;
            self.stats = UpdateStats::default();
            return Ok(self.stats);
        }
        let count = line_count(&self.text);
        let mut windows = Vec::<Range<usize>>::new();
        for c in changes.changes() {
            let mut window = self.text.byte_to_line(c.old_range.start).saturating_sub(2)
                ..(self.text.byte_to_line(c.old_range.end) + 3).min(count);
            // Existing differences cannot serve as anchors. Expand to encompass them.
            loop {
                let before = window.clone();
                for h in &self.hunks {
                    if h.new.start <= window.end && h.new.end >= window.start {
                        window.start = window.start.min(h.new.start.saturating_sub(2));
                        window.end = window.end.max((h.new.end + 2).min(count));
                    }
                }
                if before == window {
                    break;
                }
            }
            if let Some(last) = windows.last_mut()
                && window.start <= last.end
            {
                last.end = last.end.max(window.end);
            } else {
                windows.push(window);
            }
        }
        let mut next = Vec::with_capacity(self.hunks.len() + windows.len());
        let mut stats = UpdateStats::default();
        let mut hunk_cursor = 0;
        for window in windows {
            while let Some(h) = self.hunks.get(hunk_cursor) {
                if h.new.end >= window.start {
                    break;
                }
                next.push(relocate(h, &self.text, &text, changes));
                hunk_cursor += 1;
            }
            let old = self.reference_boundary(window.start, false)
                ..self.reference_boundary(window.end, true);
            let new_start =
                changes.transform_point(line_byte(&self.text, window.start), Bias::Left);
            let new_end = changes.transform_point(line_byte(&self.text, window.end), Bias::Right);
            let new = text.byte_to_line(new_start).min(line_count(&text))
                ..if new_end == text.len_bytes() {
                    line_count(&text)
                } else {
                    text.byte_to_line(new_end)
                };
            stats.windows += 1;
            stats.old_lines += old.len();
            stats.new_lines += new.len();
            next.extend(line_diff(&self.reference, old, &text, new));
            while self
                .hunks
                .get(hunk_cursor)
                .is_some_and(|h| h.new.start <= window.end)
            {
                hunk_cursor += 1;
            }
        }
        next.extend(
            self.hunks[hunk_cursor..]
                .iter()
                .map(|h| relocate(h, &self.text, &text, changes)),
        );
        self.hunks = next;
        self.text = text;
        self.version = version;
        self.stats = stats;
        Ok(stats)
    }
    fn reference_boundary(&self, line: usize, end: bool) -> usize {
        let mut old_base = 0;
        let mut new_base = 0;
        for h in &self.hunks {
            if line < h.new.start {
                break;
            }
            if line <= h.new.end {
                return if line == h.new.end && (!h.new.is_empty() || end) {
                    h.old.end
                } else {
                    h.old.start
                };
            }
            old_base = h.old.end;
            new_base = h.new.end;
        }
        old_base + line - new_base
    }
    /// First changed boundary strictly after `line`; optionally wraps.
    pub fn next_modification(&self, line: usize, wrap: bool) -> Option<usize> {
        self.hunks
            .iter()
            .find(|h| h.new.start > line)
            .or_else(|| wrap.then(|| self.hunks.first()).flatten())
            .map(|h| h.new.start)
    }
    /// Last changed boundary strictly before `line`; optionally wraps.
    pub fn prev_modification(&self, line: usize, wrap: bool) -> Option<usize> {
        self.hunks
            .iter()
            .rev()
            .find(|h| h.new.start < line)
            .or_else(|| wrap.then(|| self.hunks.last()).flatten())
            .map(|h| h.new.start)
    }
    /// Returns one current-coordinate edit that restores this hunk exactly.
    pub fn revert_hunk(&self, hunk: &Hunk) -> Result<RevertEdit, Error> {
        if !self.hunks.contains(hunk) {
            return Err(Error::StaleHunk);
        }
        Ok(RevertEdit {
            range: byte_range(&self.text, hunk.new.clone()),
            text: self
                .reference
                .slice(byte_range(&self.reference, hunk.old.clone()))
                .to_string(),
        })
    }
    /// Nonoverlapping old-coordinate edits to restore all differences in one transaction.
    pub fn revert_all(&self) -> Vec<RevertEdit> {
        self.hunks
            .iter()
            .map(|h| self.revert_hunk(h).expect("current hunk"))
            .collect()
    }
    /// Computes inline changes only inside the selected current hunk.
    pub fn inline_diff(&self, hunk: &Hunk, mode: InlineMode) -> Result<Vec<InlineChange>, Error> {
        if !self.hunks.contains(hunk) {
            return Err(Error::StaleHunk);
        }
        let old_range = byte_range(&self.reference, hunk.old.clone());
        let new_range = byte_range(&self.text, hunk.new.clone());
        let old = self.reference.slice(old_range.clone()).to_string();
        let new = self.text.slice(new_range.clone()).to_string();
        let a = tokens(&old, mode);
        let b = tokens(&new, mode);
        let mut input = InternedInput::default();
        input.update_before(a.iter().copied());
        input.update_after(b.iter().copied());
        let diff = Diff::compute(Algorithm::Histogram, &input);
        let offsets = |t: &[&str]| {
            let mut out = vec![0];
            for s in t {
                out.push(out.last().unwrap() + s.len());
            }
            out
        };
        let a = offsets(&a);
        let b = offsets(&b);
        Ok(diff
            .hunks()
            .map(|h| InlineChange {
                old: old_range.start + a[h.before.start as usize]
                    ..old_range.start + a[h.before.end as usize],
                new: new_range.start + b[h.after.start as usize]
                    ..new_range.start + b[h.after.end as usize],
            })
            .collect())
    }
}
fn tokens(s: &str, mode: InlineMode) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut word = false;
    for (i, ch) in s.char_indices() {
        let current_word = ch.is_alphanumeric() || ch == '_';
        if i > start && (mode == InlineMode::Character || !word || !current_word) {
            result.push(&s[start..i]);
            start = i;
        }
        word = current_word;
    }
    if start < s.len() {
        result.push(&s[start..]);
    }
    result
}
fn line_count(t: &Text) -> usize {
    if t.is_empty() {
        0
    } else {
        t.len_lines() - usize::from(t.byte_at(t.len_bytes() - 1) == b'\n')
    }
}
fn line_byte(t: &Text, line: usize) -> usize {
    if line >= line_count(t) {
        t.len_bytes()
    } else {
        t.line_to_byte(line)
    }
}
fn byte_range(t: &Text, lines: Range<usize>) -> Range<usize> {
    line_byte(t, lines.start)..line_byte(t, lines.end)
}
fn line_diff(old: &Text, a: Range<usize>, new: &Text, b: Range<usize>) -> Vec<Hunk> {
    let before = old.slice(byte_range(old, a.clone())).to_string();
    let after = new.slice(byte_range(new, b.clone())).to_string();
    let input = InternedInput::new(before.as_str(), after.as_str());
    Diff::compute(Algorithm::Histogram, &input)
        .hunks()
        .map(|h| Hunk {
            old: a.start + h.before.start as usize..a.start + h.before.end as usize,
            new: b.start + h.after.start as usize..b.start + h.after.end as usize,
        })
        .collect()
}
fn relocate(h: &Hunk, old: &Text, new: &Text, changes: &ChangeSet) -> Hunk {
    let map = |line| {
        let point = changes.transform_point(line_byte(old, line), Bias::Right);
        if point == new.len_bytes() {
            line_count(new)
        } else {
            new.byte_to_line(point)
        }
    };
    Hunk {
        old: h.old.clone(),
        new: map(h.new.start)..map(h.new.end),
    }
}
/// Validates descriptor geometry and new UTF-8 boundaries without scanning unchanged text.
/// The caller guarantees that unchanged spans have identical contents.
pub fn validate_change(old: &Text, new: &Text, changes: &ChangeSet) -> Result<(), Error> {
    let mut len = old.len_bytes();
    let mut old_end = 0;
    let mut new_end = 0usize;
    for c in changes.changes() {
        if c.old_range.start < old_end
            || !old.is_char_boundary(c.old_range.start)
            || !old.is_char_boundary(c.old_range.end)
        {
            return Err(Error::InvalidChange);
        }
        let start = new_end
            .checked_add(c.old_range.start - old_end)
            .ok_or(Error::InvalidChange)?;
        new_end = start.checked_add(c.new_len).ok_or(Error::InvalidChange)?;
        if !new.is_char_boundary(start) || !new.is_char_boundary(new_end) {
            return Err(Error::InvalidChange);
        }
        len = len
            .checked_sub(c.old_range.len())
            .and_then(|n| n.checked_add(c.new_len))
            .ok_or(Error::InvalidChange)?;
        old_end = c.old_range.end;
    }
    if len != new.len_bytes() {
        return Err(Error::InvalidChange);
    }
    Ok(())
}
