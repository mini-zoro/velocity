//! Retains published guide geometry while incremental analysis catches up.
use std::sync::Arc;
use vel_buffer::{Bias, Change, ChangeId, ChangeSet};
use vel_render::guides::{Block, GuideIndex};
use vel_text::Text;
#[derive(Debug, Default)]
pub struct Guides {
    index: Option<Arc<GuideIndex>>,
    rows: Vec<(ChangeSet, ChangeSet)>,
}
impl Guides {
    pub fn publish(&mut self, index: Arc<GuideIndex>) {
        self.index = Some(index);
        self.rows.clear();
    }
    /// Undo and shared views can deliver several retained edits at once. Compose
    /// only the changed intervals; row mapping never flattens either document.
    pub fn rebase(&mut self, old: &Text, new: &Text, maps: &[(ChangeId, ChangeSet)]) {
        if maps.len() == 1 {
            self.change(old, new, &maps[0].1);
            return;
        }
        let inverse: Vec<_> = maps.iter().map(|(_, m)| m.inverse()).collect();
        let mut ranges = Vec::new();
        for (i, (_, map)) in maps.iter().enumerate() {
            for c in map.changes() {
                let mut a = c.old_range.start;
                let mut b = c.old_range.end;
                for prior in inverse[..i].iter().rev() {
                    a = prior.transform_point(a, Bias::Left);
                    b = prior.transform_point(b, Bias::Right);
                }
                ranges.push(a..b);
            }
        }
        ranges.sort_unstable_by_key(|r| (r.start, r.end));
        let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
        for r in ranges {
            if let Some(last) = merged.last_mut()
                && r.start <= last.end
            {
                last.end = last.end.max(r.end);
            } else {
                merged.push(r);
            }
        }
        let changes = ChangeSet::new(
            merged
                .into_iter()
                .map(|range| {
                    let (mut a, mut b) = (range.start, range.end);
                    for (_, map) in maps {
                        a = map.transform_point(a, Bias::Left);
                        b = map.transform_point(b, Bias::Right);
                    }
                    Change {
                        old_range: range,
                        new_len: b.saturating_sub(a),
                    }
                })
                .collect(),
        );
        self.change(old, new, &changes);
    }
    pub fn change(&mut self, old: &Text, new: &Text, changes: &ChangeSet) {
        // A single replacement with equal total line counts cannot shift rows.
        // Typing ordinary characters needs no byte-to-line tree lookups.
        if self.index.is_none()
            || (changes.changes().len() <= 1 && old.len_lines() == new.len_lines())
        {
            return;
        }
        let mut delta = 0isize;
        let edits = changes
            .changes()
            .iter()
            .filter_map(|c| {
                let from = old.byte_to_line(c.old_range.start);
                let to = old.byte_to_line(c.old_range.end);
                let start = c.old_range.start.saturating_add_signed(delta);
                let added = new.byte_to_line(start + c.new_len) - new.byte_to_line(start);
                delta += c.new_len as isize - c.old_range.len() as isize;
                (to - from != added).then_some(Change {
                    old_range: from..to,
                    new_len: added,
                })
            })
            .collect::<Vec<_>>();
        if !edits.is_empty() {
            let map = ChangeSet::new(edits);
            let inverse = map.inverse();
            self.rows.push((map, inverse));
        }
    }
    fn original(&self, mut row: usize) -> usize {
        for (_, inverse) in self.rows.iter().rev() {
            row = inverse.transform_point(row, Bias::Left);
        }
        row
    }
    pub fn columns(&self, row: usize) -> Option<usize> {
        self.index.as_ref().map(|g| g.columns(self.original(row)))
    }
    pub fn active(&self, row: usize, tab: usize) -> Option<Block> {
        let mut block = self.index.as_ref()?.active(self.original(row), tab)?;
        for (map, _) in &self.rows {
            block.rows = map.transform_point(block.rows.start, Bias::Left)
                ..map.transform_point(block.rows.end, Bias::Right);
        }
        Some(block)
    }
}
