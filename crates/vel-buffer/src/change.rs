use crate::{Region, RegionSet, Selection};
use std::ops::Range;

/// Which side of newly inserted text a boundary point sticks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    /// Before inserted text.
    Left,
    /// After inserted text.
    Right,
}
/// Monotonic transaction sequence number, local to one buffer.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ChangeId(pub u64);
/// A single replacement in pre-transaction coordinates.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Change {
    /// Replaced range.
    pub old_range: Range<usize>,
    /// Length of inserted UTF-8.
    pub new_len: usize,
}
/// Sorted replacements with a prefix index for O(log edits) point mapping.
#[derive(Clone, Debug, Default)]
pub struct ChangeSet {
    changes: Vec<Change>,
    starts: Vec<usize>,
    moves: Option<Box<RegionMoves>>,
}
#[derive(Clone, Debug)]
pub(crate) struct RegionMoves {
    pub name: &'static str,
    /// Ranges of intact content in the old and new order. Only the trailing LF
    /// may differ in length (exchanging a line with the last line of a file).
    pub ranges: Vec<(Range<usize>, Range<usize>)>,
    /// Unindent undo restores indentation *inside* a fold starting at that edit.
    pub keep_start: bool,
    moves_separator: bool,
}

#[cfg(test)]
#[test]
fn batched_reverse_projection_agrees_with_the_forward_map() {
    let mut state = 81_u32;
    let mut next = || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (state >> 16) as usize
    };
    for _ in 0..2000 {
        let mut len = 40;
        let mut maps = Vec::new();
        for _ in 0..4 {
            let mut at = 0;
            let mut edits = Vec::new();
            let mut new_len = len;
            for _ in 0..12 {
                let start = (at + next() % 4).min(len);
                at = (start + next() % 6).min(len);
                let inserted = next() % 5;
                new_len = new_len - (at - start) + inserted;
                edits.push(Change {
                    old_range: start..at,
                    new_len: inserted,
                });
            }
            maps.push(ChangeSet::new(edits));
            len = new_len;
        }
        let mut projected: Vec<_> = (0..=len).collect();
        for map in maps.iter().rev() {
            map.unmap_left_points(&mut projected);
        }
        let forward: Vec<_> = (0..=40)
            .map(|point| {
                maps.iter()
                    .fold(point, |p, map| map.transform_point(p, Bias::Left))
            })
            .collect();
        for (p, old) in projected.into_iter().enumerate() {
            assert_eq!(old, forward.partition_point(|&image| image <= p) - 1);
        }
    }
}
impl RegionMoves {
    pub fn lines(name: &'static str, ranges: Vec<(Range<usize>, Range<usize>)>) -> Self {
        let moves_separator = ranges.iter().any(|(from, to)| from.len() != to.len());
        Self {
            name,
            ranges,
            keep_start: false,
            moves_separator,
        }
    }
    pub fn unindent(name: &'static str) -> Self {
        Self {
            keep_start: true,
            ..Self::lines(name, Vec::new())
        }
    }
}
impl ChangeSet {
    /// Validates ordering and computes post-edit start positions.
    pub fn new(changes: Vec<Change>) -> Self {
        let mut starts = Vec::with_capacity(changes.len());
        let mut old = 0;
        let mut new = 0;
        for c in &changes {
            assert!(
                c.old_range.start >= old && c.old_range.end >= c.old_range.start,
                "changes must be sorted and non-overlapping"
            );
            new += c.old_range.start - old;
            starts.push(new);
            new += c.new_len;
            old = c.old_range.end;
        }
        Self {
            changes,
            starts,
            moves: None,
        }
    }
    pub(crate) fn with_region_moves(mut self, moves: Option<RegionMoves>) -> Self {
        self.moves = moves.map(Box::new);
        self
    }
    /// Replacements in pre-edit coordinates.
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }
    /// Maps one point. Deleted interior points collapse to the replacement start.
    pub fn transform_point(&self, point: usize, bias: Bias) -> usize {
        let i = self.changes.partition_point(|c| {
            c.old_range.end < point || (c.old_range.end == point && !c.old_range.is_empty())
        });
        if let Some(c) = self.changes.get(i)
            && c.old_range.start <= point
        {
            let start = self.starts[i];
            if point > c.old_range.start {
                return start;
            }
            if bias == Bias::Left {
                return start;
            }
            let mut end = start + c.new_len;
            let mut j = i + 1;
            while let Some(next) = self.changes.get(j) {
                if next.old_range.start != point {
                    break;
                }
                end = self.starts[j] + next.new_len;
                j += 1;
            }
            return end;
        }
        if i == 0 {
            point
        } else {
            self.starts[i - 1] + self.changes[i - 1].new_len + point
                - self.changes[i - 1].old_range.end
        }
    }
    /// Maps sorted post-edit byte points to their rightmost old byte whose
    /// left-biased image is at or before the point. Unlike an undo ChangeSet,
    /// this maps into the last byte of replaced text when the image collapses.
    pub(crate) fn unmap_left_points(&self, points: &mut [usize]) {
        let Some(&first) = points.first() else { return };
        let mut i = self.starts.partition_point(|&start| start <= first);
        for point in points {
            while self.starts.get(i).is_some_and(|&start| start <= *point) {
                i += 1;
            }
            if i == 0 {
                continue;
            }
            let change = &self.changes[i - 1];
            let end = self.starts[i - 1] + change.new_len;
            *point = if *point < end {
                change
                    .old_range
                    .end
                    .saturating_sub(1)
                    .max(change.old_range.start)
            } else {
                change.old_range.end + (*point - end)
            };
        }
    }
    /// Maps a named region: begin right, end left; empty regions stick right.
    pub fn transform_region(&self, r: Region) -> Region {
        // ST-BEHAVIOR: unverified [M1]: named boundaries exclude new text; collapsed regions survive.
        if r.empty() {
            return Region {
                xpos: r.xpos,
                ..Region::caret(self.transform_point(r.b, Bias::Right))
            };
        }
        let a = self.transform_point(r.begin(), Bias::Right);
        let b = self.transform_point(r.end(), Bias::Left);
        r.directed(a, b.max(a))
    }
    /// Maps a named annotation, following intact content reordered by a line swap.
    pub fn transform_named_region(&self, name: &str, r: Region) -> Region {
        self.transform_with_separator(name, r, false, 0).0
    }
    fn transform_with_separator(
        &self,
        name: &str,
        r: Region,
        missing_lf: bool,
        end: usize,
    ) -> (Region, bool) {
        if let Some(moves) = self.moves.as_ref().filter(|m| m.name == name)
            && (!r.empty() || missing_lf)
        {
            let i = moves
                .ranges
                .partition_point(|(old, _)| old.start <= r.begin());
            if let Some((old, new)) = i.checked_sub(1).and_then(|i| moves.ranges.get(i))
                && r.end() <= old.end
            {
                // Relocate by offset within the moved lines. The separator
                // exists only when the run is followed by another line. Keep
                // its inclusion bit even when that byte cannot exist at EOF.
                let a = new.start + r.begin() - old.start;
                let b = new.start + r.end() - old.start + usize::from(missing_lf);
                return (r.directed(a.min(new.end), b.min(new.end)), b > new.end);
            }
            if moves.keep_start {
                let a = self.transform_point(r.begin(), Bias::Left);
                let b = self.transform_point(r.end(), Bias::Left);
                return (r.directed(a, b.max(a)), missing_lf && b == end);
            }
        }
        let mapped = self.transform_region(r);
        (mapped, missing_lf && mapped.end() == end)
    }
    /// Map a set whose folds can straddle the implicit separator at EOF.
    /// Returns false for the ordinary, allocation-free region mapping path.
    pub(crate) fn transform_separator_regions(
        &self,
        name: &str,
        set: &mut RegionSet,
        end: usize,
    ) -> bool {
        let Some(moves) = self
            .moves
            .as_ref()
            .filter(|m| m.name == name && !m.ranges.is_empty())
        else {
            // Ordinary edits only rebase the small set of EOF affinities;
            // don't allocate or resort every fold on a keystroke.
            set.missing_lf.retain_mut(|r| {
                let (mapped, keep) = self.transform_with_separator(name, *r, true, end);
                *r = mapped;
                keep
            });
            return false;
        };
        if set.missing_lf.is_empty() && !moves.moves_separator {
            return false;
        }
        let mut latent = Vec::new();
        let mut flags = std::collections::BTreeMap::new();
        for r in std::mem::take(&mut set.missing_lf) {
            if r.empty() && set.drop_empty {
                latent.push(r);
            } else {
                *flags.entry((r.a, r.b)).or_insert(0usize) += 1;
            }
        }
        let mut moved = Vec::with_capacity(set.regions.len());
        for r in set.regions.drain(..) {
            let missing = flags.get_mut(&(r.a, r.b)).is_some_and(|n| {
                let present = *n > 0;
                *n = n.saturating_sub(1);
                present
            });
            moved.push(self.transform_with_separator(name, r, missing, end));
        }
        moved.extend(
            latent
                .into_iter()
                .map(|r| self.transform_with_separator(name, r, true, end)),
        );
        moved.sort_by_key(|(r, _)| (r.begin(), std::cmp::Reverse(r.end())));
        for (r, missing) in moved {
            if missing {
                set.missing_lf.push(r);
            }
            if !r.empty() || !set.drop_empty {
                set.regions.push(r);
            }
        }
        true
    }
    /// Maps selection endpoints right, preserving primary and direction.
    pub fn transform_selection(&self, s: &Selection) -> Selection {
        s.map(|r| {
            Region::new(
                self.transform_point(r.a, Bias::Right),
                self.transform_point(r.b, Bias::Right),
            )
        })
    }
    /// The reverse mapping in post-edit coordinates.
    pub fn inverse(&self) -> Self {
        Self::new(
            self.changes
                .iter()
                .zip(&self.starts)
                .map(|(c, &start)| Change {
                    old_range: start..start + c.new_len,
                    new_len: c.old_range.len(),
                })
                .collect(),
        )
        .with_region_moves(self.moves.as_ref().map(|m| {
            let mut ranges: Vec<_> = m
                .ranges
                .iter()
                .map(|(old, new)| (new.clone(), old.clone()))
                .collect();
            ranges.sort_by_key(|(old, _)| old.start);
            RegionMoves {
                name: m.name,
                ranges,
                keep_start: m.keep_start,
                moves_separator: m.moves_separator,
            }
        }))
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        self.changes.capacity() * std::mem::size_of::<Change>()
            + self.starts.capacity() * std::mem::size_of::<usize>()
            + self.moves.as_ref().map_or(0, |m| {
                std::mem::size_of::<RegionMoves>()
                    + m.ranges.capacity() * std::mem::size_of::<(Range<usize>, Range<usize>)>()
            })
    }
}
