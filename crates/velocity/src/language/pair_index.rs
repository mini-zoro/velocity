//! Packed pair intervals and their max-end search tree.
use super::Pair;

#[derive(Debug)]
struct CompactPair {
    open_and_kind: u32,
    close_or_extra: u32,
}

#[derive(Debug)]
struct ExtraPair {
    open_end: u32,
    close: [u32; 2],
    kind: char,
}

impl CompactPair {
    const MAX_BYTE: usize = (u32::MAX >> 3) as usize;
    const BRACKETS: [char; 4] = ['(', '[', '{', '<'];

    fn open(&self) -> u32 {
        self.open_and_kind >> 3
    }

    fn kind(&self) -> usize {
        (self.open_and_kind & 7) as usize
    }

    fn new(pair: Pair, extras: &mut Vec<ExtraPair>) -> Self {
        let open = pair.open.start as u32;
        if pair.open.len() == 1
            && pair.close.len() == 1
            && let Some(kind) = Self::BRACKETS.iter().position(|&c| c == pair.kind)
        {
            return Self {
                open_and_kind: (open << 3) | (kind as u32 + 1),
                close_or_extra: pair.close.start as u32,
            };
        }
        let index = extras.len() as u32;
        extras.push(ExtraPair {
            open_end: pair.open.end as u32,
            close: [pair.close.start as u32, pair.close.end as u32],
            kind: pair.kind,
        });
        Self {
            open_and_kind: open << 3,
            close_or_extra: index,
        }
    }

    fn expanded(&self, extras: &[ExtraPair]) -> Pair {
        let open = self.open() as usize;
        let close = self.close_or_extra as usize;
        match self.kind().checked_sub(1) {
            Some(kind) => Pair {
                open: open..open + 1,
                close: close..close + 1,
                kind: Self::BRACKETS[kind],
            },
            None => {
                let extra = &extras[close];
                Pair {
                    open: open..extra.open_end as usize,
                    close: extra.close[0] as usize..extra.close[1] as usize,
                    kind: extra.kind,
                }
            }
        }
    }

    fn close_end(&self, extras: &[ExtraPair]) -> u32 {
        if self.kind() == 0 {
            extras[self.close_or_extra as usize].close[1]
        } else {
            self.close_or_extra + 1
        }
    }
}

#[derive(Debug)]
enum Storage {
    Compact {
        pairs: Vec<CompactPair>,
        extras: Vec<ExtraPair>,
        ends: Vec<u32>,
    },
    Wide(Vec<Pair>, Vec<usize>),
}

#[derive(Debug)]
pub struct PairIndex(Storage);

impl PairIndex {
    pub(super) fn new(text_bytes: usize) -> Self {
        Self(if text_bytes <= CompactPair::MAX_BYTE {
            Storage::Compact {
                pairs: Vec::new(),
                extras: Vec::new(),
                ends: Vec::new(),
            }
        } else {
            Storage::Wide(Vec::new(), Vec::new())
        })
    }

    pub(super) fn push(&mut self, pair: Pair) {
        match &mut self.0 {
            Storage::Compact { pairs, extras, .. } => pairs.push(CompactPair::new(pair, extras)),
            Storage::Wide(pairs, _) => pairs.push(pair),
        }
    }

    pub(super) fn finish(&mut self) {
        match &mut self.0 {
            Storage::Compact {
                pairs,
                extras,
                ends,
            } => {
                pairs.sort_by_key(CompactPair::open);
                *ends = max_ends(pairs.len(), |i| pairs[i].close_end(extras));
            }
            Storage::Wide(pairs, ends) => {
                pairs.sort_by_key(|p| p.open.start);
                *ends = max_ends(pairs.len(), |i| pairs[i].close.end);
            }
        }
    }

    pub(super) fn storage_bytes(&self) -> usize {
        match &self.0 {
            Storage::Compact {
                pairs,
                extras,
                ends,
            } => {
                pairs.capacity() * size_of::<CompactPair>()
                    + extras.capacity() * size_of::<ExtraPair>()
                    + ends.capacity() * size_of::<u32>()
            }
            Storage::Wide(pairs, ends) => {
                pairs.capacity() * size_of::<Pair>() + ends.capacity() * size_of::<usize>()
            }
        }
    }

    pub fn len(&self) -> usize {
        match &self.0 {
            Storage::Compact { pairs, .. } => pairs.len(),
            Storage::Wide(pairs, _) => pairs.len(),
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = Pair> + '_ {
        (0..self.len()).map(|i| self.get(i).unwrap())
    }

    pub(super) fn get(&self, index: usize) -> Option<Pair> {
        match &self.0 {
            Storage::Compact { pairs, extras, .. } => pairs.get(index).map(|p| p.expanded(extras)),
            Storage::Wide(pairs, _) => pairs.get(index).cloned(),
        }
    }

    pub(super) fn starts_at_or_before(&self, byte: usize) -> usize {
        match &self.0 {
            Storage::Compact { pairs, .. } => pairs.partition_point(|p| p.open() as usize <= byte),
            Storage::Wide(pairs, _) => pairs.partition_point(|p| p.open.start <= byte),
        }
    }

    pub(super) fn leaf_count(&self) -> usize {
        match &self.0 {
            Storage::Compact { ends, .. } => ends.len(),
            Storage::Wide(_, ends) => ends.len(),
        }
    }

    pub(super) fn max_end(&self, node: usize) -> usize {
        match &self.0 {
            Storage::Compact {
                pairs,
                extras,
                ends,
            } => node_end(ends, node, |i| {
                pairs.get(i).map_or(0, |p| p.close_end(extras))
            }) as usize,
            Storage::Wide(pairs, ends) => {
                node_end(ends, node, |i| pairs.get(i).map_or(0, |p| p.close.end))
            }
        }
    }
}

// Leaves already exist in the pair records. Store only the internal tree.
fn max_ends<T: Copy + Default + Ord>(len: usize, end: impl Fn(usize) -> T) -> Vec<T> {
    if len == 0 {
        return Vec::new();
    }
    let base = len.next_power_of_two();
    let mut tree = vec![T::default(); base];
    let leaf = |i| if i < len { end(i) } else { T::default() };
    for i in (1..base).rev() {
        tree[i] = node_end(&tree, i * 2, leaf).max(node_end(&tree, i * 2 + 1, leaf));
    }
    tree
}

fn node_end<T: Copy>(tree: &[T], node: usize, leaf: impl Fn(usize) -> T) -> T {
    if node < tree.len() {
        tree[node]
    } else {
        leaf(node - tree.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_endpoints_and_tree_are_exact_in_both_widths() {
        for shift in [0, usize::MAX.saturating_sub(100)] {
            let mut index = PairIndex::new(shift + 100);
            let pairs = [
                Pair {
                    open: shift + 8..shift + 9,
                    close: shift + 26..shift + 27,
                    kind: '[',
                },
                Pair {
                    open: shift + 3..shift + 12,
                    close: shift + 40..shift + 51,
                    kind: 't',
                },
            ];
            for pair in pairs.iter().cloned() {
                index.push(pair);
            }
            index.finish();
            assert_eq!(index.starts_at_or_before(shift + 7), 1);
            assert_eq!(index.starts_at_or_before(shift + 8), 2);
            assert_eq!(index.leaf_count(), 2);
            assert_eq!(index.max_end(1), shift + 51);
            assert_eq!(index.max_end(3), shift + 27);
            for (i, pair) in pairs.iter().rev().enumerate() {
                let actual = index.get(i).unwrap();
                assert_eq!(
                    (&actual.open, &actual.close, actual.kind),
                    (&pair.open, &pair.close, pair.kind)
                );
            }
        }
        assert_eq!(size_of::<CompactPair>(), 8);
    }
}
