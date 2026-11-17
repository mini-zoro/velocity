//! Compact indentation geometry built off the scene thread.
use std::ops::Range;

/// A containing indentation block, with an exclusive last row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Guide column, measured in display cells.
    pub column: usize,
    /// Physical document rows occupied by the block.
    pub rows: Range<usize>,
}

/// Per-row indentation and a minimum tree for logarithmic block-boundary queries.
/// Blank rows inherit the greater indentation of their surrounding nonblank rows.
#[derive(Debug, Default)]
pub struct GuideIndex {
    columns: Vec<u32>,
    minimum: Vec<u32>,
}
impl GuideIndex {
    /// Builds from indentation columns (`None` denotes a blank row).
    pub fn new(rows: impl IntoIterator<Item = Option<usize>>) -> Self {
        let mut columns: Vec<_> = rows
            .into_iter()
            .map(|n| n.map_or(u32::MAX, |n| n.min((1usize << 31) - 1) as u32))
            .collect();
        let mut next = 0;
        for n in columns.iter_mut().rev() {
            if *n == u32::MAX {
                *n = next | (1 << 31);
            } else {
                next = *n;
            }
        }
        let mut previous = 0;
        for n in &mut columns {
            if *n & (1 << 31) != 0 {
                *n = (*n & !(1 << 31)).max(previous);
            } else {
                previous = *n;
            }
        }
        // Unindented/minified documents have no guides or containing blocks.
        // Avoid retaining an all-zero tree on every analysis/typing publication.
        if columns.iter().all(|n| *n == 0) {
            return Self::default();
        }
        let base = columns.len().max(1).next_power_of_two();
        let mut minimum = vec![u32::MAX; base * 2];
        minimum[base..base + columns.len()].copy_from_slice(&columns);
        for i in (1..base).rev() {
            minimum[i] = minimum[i * 2].min(minimum[i * 2 + 1]);
        }
        Self { columns, minimum }
    }
    /// Resolved indentation in display cells; absent rows have no guides.
    pub fn columns(&self, row: usize) -> usize {
        self.columns.get(row).copied().unwrap_or(0) as usize
    }

    /// Finds the deepest guide containing `row`. No scan of document lines occurs.
    pub fn active(&self, row: usize, tab: usize) -> Option<Block> {
        let columns = self.columns(row);
        if columns == 0 {
            return None;
        }
        let column = (columns - 1) / tab.max(1) * tab.max(1);
        let before = self.boundary(1, 0..self.minimum.len() / 2, 0..row, column as u32, true);
        let after = self.boundary(
            1,
            0..self.minimum.len() / 2,
            row + 1..self.columns.len(),
            column as u32,
            false,
        );
        Some(Block {
            column,
            rows: before.map_or(0, |r| r + 1)..after.unwrap_or(self.columns.len()),
        })
    }
    fn boundary(
        &self,
        node: usize,
        rows: Range<usize>,
        query: Range<usize>,
        column: u32,
        reverse: bool,
    ) -> Option<usize> {
        if rows.end <= query.start || rows.start >= query.end || self.minimum[node] > column {
            return None;
        }
        if rows.len() == 1 {
            return Some(rows.start);
        }
        let mid = (rows.start + rows.end) / 2;
        let children = if reverse {
            [(node * 2 + 1, mid..rows.end), (node * 2, rows.start..mid)]
        } else {
            [(node * 2, rows.start..mid), (node * 2 + 1, mid..rows.end)]
        };
        children
            .into_iter()
            .find_map(|(child, rows)| self.boundary(child, rows, query.clone(), column, reverse))
    }
}

/// Guide columns start at zero and stop before the text's first column.
pub fn columns(indentation: usize, tab: usize) -> impl Iterator<Item = usize> {
    (0..indentation).step_by(tab.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn columns_blank_rows_and_separate_blocks() {
        assert_eq!(columns(8, 4).collect::<Vec<_>>(), [0, 4]);
        assert_eq!(columns(6, 4).collect::<Vec<_>>(), [0, 4]);
        let g = GuideIndex::new([
            Some(0),
            Some(4),
            Some(8),
            None,
            Some(8),
            Some(4),
            Some(0),
            Some(8),
        ]);
        assert_eq!(g.columns(3), 8);
        assert_eq!(
            g.active(2, 4),
            Some(Block {
                column: 4,
                rows: 2..5
            })
        );
        assert_eq!(
            g.active(5, 4),
            Some(Block {
                column: 0,
                rows: 1..6
            })
        );
        assert_eq!(
            g.active(7, 4),
            Some(Block {
                column: 4,
                rows: 7..8
            })
        );
        assert_eq!(g.active(0, 4), None);
    }
}
#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;
    proptest! {
        #[test]
        fn tree_matches_a_linear_block_model(rows in prop::collection::vec(prop::option::of(0usize..24),1..256), tab in 1usize..9) {
            let g=GuideIndex::new(rows.clone());
            let expected:Vec<_>=rows.iter().enumerate().map(|(i,n)|n.unwrap_or_else(|| {
                let left=rows[..i].iter().rev().find_map(|n|*n).unwrap_or(0);
                let right=rows[i..].iter().find_map(|n|*n).unwrap_or(0);
                left.max(right)
            })).collect();
            for row in 0..rows.len() {
                prop_assert_eq!(g.columns(row),expected[row]);
                if expected[row]==0 { prop_assert_eq!(g.active(row,tab),None);continue; }
                let col=(expected[row]-1)/tab*tab;
                let first=(0..row).rev().find(|r|expected[*r]<=col).map_or(0,|r|r+1);
                let last=(row+1..rows.len()).find(|r|expected[*r]<=col).unwrap_or(rows.len());
                prop_assert_eq!(g.active(row,tab),Some(Block{column:col,rows:first..last}));
            }
        }
    }
}
