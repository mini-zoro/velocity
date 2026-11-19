use super::{Insertion, Line, Row};
use std::{path::Path, sync::Arc};

const PAGE: usize = 1024;
const BRANCH: u32 = 1 << 31;
const NONE: u32 = u32::MAX;

/// A prefix-compressed radix index over the sidebar sort keys. The packed
/// children store display-row counts, including implicit headers and gaps.
#[derive(Clone, Debug, Default)]
pub struct Rows {
    records: Vec<Vec<Record>>,
    files: usize,
    tails: Vec<Vec<Next>>,
    keys: Vec<u8>,
    prefixes: Vec<Prefix>,
    branches: Vec<Node>,
    route: Vec<(u32, u32)>,
    directory_epoch: usize,
    root: u32,
    count: usize,
    pub(super) matched: usize,
}

#[derive(Clone, Debug)]
struct Record {
    first: Line,
    start: u32,
    end: u32,
    tail: u32,
    prefix: u16,
    matched: bool,
}

#[derive(Clone, Debug)]
struct Next {
    at: usize,
    line: Line,
}

#[derive(Clone, Debug)]
struct Prefix {
    key: Box<[u8]>,
    route: Vec<(u32, u32)>,
    epoch: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Edge {
    link: u32,
    rows: u32,
}

#[derive(Clone, Debug)]
struct Node {
    digit: u32,
    sample: u32,
    present: u32,
    edges: [Edge; 17],
}

impl Node {
    fn index(&self, bit: u32) -> usize {
        (self.present & (bit - 1)).count_ones() as usize
    }
    fn active(&self) -> &[Edge] {
        &self.edges[..self.present.count_ones() as usize]
    }
}

fn bit_at(key: &[u8], digit: usize) -> u32 {
    // An end-of-key sorts before every real digit.
    1 << key.get(digit / 2).map_or(0, |byte| {
        let nibble = if digit.is_multiple_of(2) {
            byte >> 4
        } else {
            byte & 15
        };
        nibble + 1
    })
}

fn difference(a: &[u8], b: &[u8]) -> Option<usize> {
    let mut checked = 0;
    for (x, y) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
        let diff = u64::from_be_bytes(*x) ^ u64::from_be_bytes(*y);
        if diff != 0 {
            return Some(checked * 2 + diff.leading_zeros() as usize / 4);
        }
        checked += 8;
    }
    for (&x, &y) in a[checked..].iter().zip(&b[checked..]) {
        if x != y {
            return Some(checked * 2 + usize::from(x >> 4 == y >> 4));
        }
        checked += 1;
    }
    (a.len() != b.len()).then_some(checked * 2)
}

impl Rows {
    /// Display row count, including headers and context separators.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether any display rows have arrived.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn record(&self, file: u32) -> &Record {
        let i = file as usize;
        &self.records[i / PAGE][i % PAGE]
    }

    fn suffix(&self, file: u32) -> &[u8] {
        let record = self.record(file);
        &self.keys[record.start as usize..record.end as usize]
    }

    pub(super) fn prefix(&self, id: u16) -> &[u8] {
        if id == 0 {
            &[]
        } else {
            &self.prefixes[id as usize - 1].key
        }
    }

    pub(super) fn push_prefix(&mut self, bytes: &[u8]) -> u16 {
        self.prefixes.push(Prefix {
            key: bytes.into(),
            route: Vec::new(),
            epoch: None,
        });
        self.prefixes.len() as u16
    }

    pub(super) fn copy_key(&self, file: u32, key: &mut Vec<u8>) -> u16 {
        let id = self.record(file).prefix;
        key.extend_from_slice(self.prefix(id));
        key.extend_from_slice(self.suffix(file));
        id
    }

    fn file_bit(&self, file: u32, digit: usize) -> u32 {
        let prefix = self.prefix(self.record(file).prefix);
        if digit < prefix.len() * 2 {
            bit_at(prefix, digit)
        } else {
            bit_at(self.suffix(file), digit - prefix.len() * 2)
        }
    }

    fn split_digit(&self, file: u32, key: &[u8], id: u16) -> Option<usize> {
        let other = self.record(file).prefix;
        let prefix = self.prefix(other);
        if (id == other || key.starts_with(prefix))
            && let Some(suffix) = key.get(prefix.len()..)
        {
            difference(suffix, self.suffix(file)).map(|n| n + prefix.len() * 2)
        } else {
            difference(key, prefix)
        }
    }

    pub(super) fn path(&self, file: u32) -> &Arc<Path> {
        &self.record(file).first.path
    }

    fn push_file(&mut self, key: &[u8], prefix: u16, first: Line) -> u32 {
        let file = self.files as u32;
        assert!(file < BRANCH, "file index fits in memory");
        if self.files.is_multiple_of(PAGE) {
            self.records.push(Vec::with_capacity(PAGE));
        }
        let start = self.keys.len();
        let prefix_len = self.prefix(prefix).len();
        self.keys.extend_from_slice(&key[prefix_len..]);
        let matched = first.match_count > 0;
        self.matched += usize::from(matched);
        self.records.last_mut().expect("file page").push(Record {
            first,
            start: start as u32,
            end: u32::try_from(self.keys.len()).expect("sort keys fit in memory"),
            tail: NONE,
            prefix,
            matched,
        });
        self.files += 1;
        file
    }

    fn append(&mut self, file: u32, mut line: Line) -> (usize, usize) {
        let i = file as usize;
        let record = &mut self.records[i / PAGE][i % PAGE];
        if !Arc::ptr_eq(&record.first.path, &line.path) {
            line.path = record.first.path.clone();
        }
        if !record.matched && line.match_count > 0 {
            record.matched = true;
            self.matched += 1;
        }
        if record.tail == NONE {
            record.tail = self.tails.len() as u32;
            self.tails.push(Vec::new());
        }
        let list = &mut self.tails[record.tail as usize];
        let (at, last) = list
            .last()
            .map_or((1, record.first.number), |next| (next.at, next.line.number));
        let added = 1 + usize::from(line.number > last + 1);
        list.push(Next {
            at: at + added,
            line,
        });
        (at + 1, added)
    }

    fn sample(&self, link: u32) -> u32 {
        if link & BRANCH == 0 {
            link
        } else {
            self.branches[(link & !BRANCH) as usize].sample
        }
    }

    fn branch(&mut self, old: Edge, file: u32, key: &[u8], digit: usize) -> u32 {
        let sample = self.sample(old.link);
        let old_bit = self.file_bit(sample, digit);
        let new_bit = bit_at(key, digit);
        debug_assert_ne!(old_bit, new_bit);
        let new = Edge {
            link: file,
            rows: 2,
        };
        let link = self.branches.len() as u32 | BRANCH;
        let mut edges = [Edge::default(); 17];
        if new_bit < old_bit {
            edges[..2].copy_from_slice(&[new, old]);
        } else {
            edges[..2].copy_from_slice(&[old, new]);
        }
        self.branches.push(Node {
            digit: digit as u32,
            sample,
            present: old_bit | new_bit,
            edges,
        });
        link
    }

    fn cache_route(&mut self, prefix: u16) {
        if prefix == 0 {
            return;
        }
        let entry = &mut self.prefixes[prefix as usize - 1];
        if entry.epoch == Some(self.directory_epoch) {
            return;
        }
        entry.route.clear();
        let mut link = self.root;
        while link & BRANCH != 0 {
            let at = link & !BRANCH;
            let node = &self.branches[at as usize];
            if node.digit as usize >= entry.key.len() * 2 {
                break;
            }
            let index = node.index(bit_at(&entry.key, node.digit as usize));
            entry.route.push((at, index as u32));
            link = node.edges[index].link;
        }
        entry.epoch = Some(self.directory_epoch);
    }

    pub(super) fn push(
        &mut self,
        key: &[u8],
        prefix: u16,
        line: Line,
        known: Option<u32>,
    ) -> Insertion {
        if self.is_empty() {
            self.root = self.push_file(key, prefix, line);
            self.count = 2;
            self.cache_route(prefix);
            return Insertion {
                file: self.root,
                offset: 0,
                added: 2,
            };
        }
        // Find a representative key (a missing digit can terminate the descent).
        // Keep the traversed edges on the stack for subtree updates.
        self.route.clear();
        let mut link = self.root;
        if prefix != 0 {
            let entry = &self.prefixes[prefix as usize - 1];
            if entry.epoch == Some(self.directory_epoch) {
                // An insertion below the directory prefix may replace the
                // suffix subtree, but not these ancestor slots.
                self.route.extend_from_slice(&entry.route);
                for &(node, edge) in &entry.route {
                    let edge = &mut self.branches[node as usize].edges[edge as usize];
                    edge.rows += 2;
                    link = edge.link;
                }
            }
        }
        while link & BRANCH != 0 {
            let node = &mut self.branches[(link & !BRANCH) as usize];
            let bit = bit_at(key, node.digit as usize);
            if node.present & bit == 0 {
                break;
            }
            let index = node.index(bit);
            self.route.push((link & !BRANCH, index as u32));
            let edge = &mut node.edges[index];
            link = edge.link;
            // The common case adds two rows (a new header, or a gap). Undo the
            // provisional increment if the insert branches higher up instead.
            edge.rows += 2;
        }
        let sample = self.sample(link);
        let split = if known == Some(sample) {
            None
        } else {
            self.split_digit(sample, key, prefix)
        };
        let (file, offset, added) = if split.is_some() {
            (self.push_file(key, prefix, line), 0, 2)
        } else {
            let (rank, added) = self.append(sample, line);
            (sample, rank, added)
        };

        if let Some(digit) = split {
            if prefix == 0 || digit < self.prefix(prefix).len() * 2 {
                self.directory_epoch += 1;
            }
            while let Some(&(at, index)) = self.route.last() {
                let node = &mut self.branches[at as usize];
                if digit >= node.digit as usize {
                    break;
                }
                node.edges[index as usize].rows -= 2;
                self.route.pop();
            }
            let parent = self
                .route
                .last()
                .map(|&(at, index)| (at as usize, index as usize));
            let old = parent.map_or(
                Edge {
                    link: self.root,
                    rows: self.count as u32,
                },
                |(at, i)| {
                    let edge = self.branches[at].edges[i];
                    Edge {
                        link: edge.link,
                        rows: edge.rows - 2,
                    }
                },
            );
            let replacement = if old.link & BRANCH != 0
                && self.branches[(old.link & !BRANCH) as usize].digit as usize == digit
            {
                let node = &mut self.branches[(old.link & !BRANCH) as usize];
                let bit = bit_at(key, digit);
                let i = node.index(bit);
                let end = node.present.count_ones() as usize;
                node.edges.copy_within(i..end, i + 1);
                node.edges[i] = Edge {
                    link: file,
                    rows: 2,
                };
                node.present |= bit;
                old.link
            } else {
                self.branch(old, file, key, digit)
            };
            if let Some((node, edge)) = parent {
                self.branches[node].edges[edge].link = replacement;
            } else {
                self.root = replacement;
            }
        } else if added != 2 {
            for &(at, index) in &self.route {
                self.branches[at as usize].edges[index as usize].rows -= 1;
            }
        }
        self.count += added;
        self.cache_route(prefix);
        Insertion {
            file,
            offset,
            added,
        }
    }

    pub(super) fn rank(&self, inserted: Insertion) -> usize {
        let mut rank = inserted.offset;
        let mut link = self.root;
        while link & BRANCH != 0 {
            let node = &self.branches[(link & !BRANCH) as usize];
            let bit = self.file_bit(inserted.file, node.digit as usize);
            let i = node.index(bit);
            rank += node.edges[..i].iter().map(|edge| edge.rows).sum::<u32>() as usize;
            link = node.edges[i].link;
        }
        debug_assert_eq!(link, inserted.file);
        rank
    }

    fn row(&self, file: u32, i: usize) -> Row<'_> {
        let record = self.record(file);
        match i {
            0 => Row::Header(&record.first.path),
            1 => Row::Line(&record.first),
            _ => {
                let tail = &self.tails[record.tail as usize];
                match tail.binary_search_by_key(&i, |n| n.at) {
                    Ok(j) => Row::Line(&tail[j].line),
                    Err(_) => Row::Gap,
                }
            }
        }
    }

    /// Resolves a display index without materializing headers or context separators.
    pub fn get(&self, mut i: usize) -> Option<Row<'_>> {
        if i >= self.count {
            return None;
        }
        let mut link = self.root;
        while link & BRANCH != 0 {
            let node = &self.branches[(link & !BRANCH) as usize];
            link = node
                .active()
                .iter()
                .find(|edge| {
                    if i < edge.rows as usize {
                        true
                    } else {
                        i -= edge.rows as usize;
                        false
                    }
                })
                .expect("row in subtree")
                .link;
        }
        Some(self.row(link, i))
    }

    /// Display rows in navigation order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = Row<'_>> {
        let files = Files {
            rows: self,
            front: Vec::new(),
            back: Vec::new(),
            remaining: self.files,
            first: true,
            last: true,
        };
        files.flat_map(move |link| {
            let record = self.record(link);
            let end = if record.tail == NONE {
                2
            } else {
                self.tails[record.tail as usize]
                    .last()
                    .expect("tail row")
                    .at
                    + 1
            };
            (0..end).map(move |i| self.row(link, i))
        })
    }
}

struct Files<'a> {
    rows: &'a Rows,
    front: Vec<&'a [Edge]>,
    back: Vec<&'a [Edge]>,
    remaining: usize,
    first: bool,
    last: bool,
}

impl Files<'_> {
    fn next_file(&mut self, reverse: bool) -> Option<u32> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let (stack, start) = if reverse {
            (&mut self.back, &mut self.last)
        } else {
            (&mut self.front, &mut self.first)
        };
        let mut link = if std::mem::take(start) {
            self.rows.root
        } else {
            next_edge(stack, reverse)
        };
        while link & BRANCH != 0 {
            stack.push(self.rows.branches[(link & !BRANCH) as usize].active());
            link = next_edge(stack, reverse);
        }
        Some(link)
    }
}

fn next_edge(stack: &mut Vec<&[Edge]>, reverse: bool) -> u32 {
    while let Some(top) = stack.last_mut() {
        let part = if reverse {
            top.split_last()
        } else {
            top.split_first()
        };
        if let Some((edge, rest)) = part {
            *top = rest;
            return edge.link;
        }
        stack.pop();
    }
    unreachable!("remaining file")
}

impl Iterator for Files<'_> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        self.next_file(false)
    }
}

impl DoubleEndedIterator for Files<'_> {
    fn next_back(&mut self) -> Option<u32> {
        self.next_file(true)
    }
}
