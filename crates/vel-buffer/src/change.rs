use crate::{Region, Selection};
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
        Self { changes, starts }
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
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        self.changes.capacity() * std::mem::size_of::<Change>()
            + self.starts.capacity() * std::mem::size_of::<usize>()
    }
}
