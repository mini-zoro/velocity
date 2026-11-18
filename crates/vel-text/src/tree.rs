//! Original implementation of a copy-on-write B-tree; no third-party rope code is copied.
use std::{io, sync::Arc};
mod progressive;
pub(crate) use progressive::from_reader_snapshots;

#[cfg(not(test))]
pub(crate) const CHUNK: usize = 6144;
#[cfg(test)]
pub(crate) const CHUNK: usize = 128;
const FANOUT: usize = 32;
const EDIT_CHUNK: usize = CHUNK * 4;
#[cfg(not(test))]
const FILL: usize = 4096;
#[cfg(test)]
const FILL: usize = CHUNK * 2 / 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub bytes: usize,
    pub chars: usize,
    pub utf16: usize,
    pub newlines: usize,
}

impl Summary {
    pub fn of(s: &str) -> Self {
        let mut sum = Self::fragment(s);
        sum.newlines = memchr::memchr_iter(b'\n', s.as_bytes()).count();
        sum
    }

    pub fn fragment(s: &str) -> Self {
        let bytes = s.len();
        let (chars, utf16) = if s.is_ascii() {
            (bytes, bytes)
        } else {
            let chars = s.chars().count();
            // In valid UTF-8 every byte >= F0 is exactly one astral scalar's lead.
            // This count vectorizes; encoding every scalar into UTF-16 does not.
            let astral = s.as_bytes().iter().filter(|&&b| b >= 0xf0).count();
            (chars, chars + astral)
        };
        Self {
            bytes,
            chars,
            utf16,
            newlines: 0,
        }
    }

    pub fn plus(self, rhs: Self) -> Self {
        Self {
            bytes: self.bytes + rhs.bytes,
            chars: self.chars + rhs.chars,
            utf16: self.utf16 + rhs.utf16,
            newlines: self.newlines + rhs.newlines,
        }
    }

    pub fn minus(self, rhs: Self) -> Self {
        Self {
            bytes: self.bytes - rhs.bytes,
            chars: self.chars - rhs.chars,
            utf16: self.utf16 - rhs.utf16,
            newlines: self.newlines - rhs.newlines,
        }
    }

    fn get(self, metric: Metric) -> usize {
        match metric {
            Metric::Byte => self.bytes,
            Metric::Char => self.chars,
            Metric::Utf16 => self.utf16,
            Metric::Line => self.newlines,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Metric {
    Byte,
    Char,
    Utf16,
    Line,
}

#[derive(Clone, Debug)]
pub(crate) struct Child {
    pub node: Arc<Node>,
    pub end: Summary,
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    Leaf(Leaf),
    Branch(Vec<Child>),
}

// Small replacements share the original allocation instead of copying kilobytes
// for each distant cursor. Adjacent typing extends the bounded inline piece;
// fragment leaves below handle longer runs without an edit-history chain.
#[derive(Clone, Debug)]
pub(crate) enum Leaf {
    Flat(Flat),
    Patched(Patched),
    Spliced(Spliced),
}

// Batch typing shares the untouched flat allocation and copies only bounded
// fragment metadata / shared insertions. Never retain another spliced leaf: undo
// snapshots must not turn this representation into an edit-history chain.
#[derive(Clone, Debug)]
pub(crate) struct Spliced {
    original: Arc<Node>,
    pieces: Vec<Piece>,
}

#[derive(Clone, Debug)]
struct Piece {
    source: PieceSource,
    end: Summary,
}

#[derive(Clone, Debug)]
enum PieceSource {
    Original(std::ops::Range<usize>),
    Inserted(Arc<str>),
}

impl PieceSource {
    fn text<'a>(&'a self, original: &'a str) -> &'a str {
        match self {
            Self::Original(range) => &original[range.clone()],
            Self::Inserted(text) => text,
        }
    }
}

#[derive(Clone)]
struct Parts<'a> {
    leaf: &'a Leaf,
    range: std::ops::Range<usize>,
}

impl<'a> Iterator for Parts<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<Self::Item> {
        self.range.next().map(|i| self.leaf.part(i))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}

impl DoubleEndedIterator for Parts<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.range.next_back().map(|i| self.leaf.part(i))
    }
}

impl ExactSizeIterator for Parts<'_> {}

// Per batch only. Repeated caret text shares its allocation, including appended
// runs, without interning a document or retaining earlier edits. The comparison
// and copy are bounded by the ordinary leaf-size insertion-piece limit.
#[derive(Default)]
struct InsertionCache {
    reuse: bool,
    text: Option<Arc<str>>,
    prefix: Option<Arc<str>>,
    joined: Vec<Option<Joined>>,
}
type Joined = (Arc<str>, Arc<str>, Arc<str>);
impl InsertionCache {
    fn intern(slot: &mut Option<Arc<str>>, text: &str) -> Arc<str> {
        if let Some(old) = slot
            && old.as_ref() == text
        {
            return old.clone();
        }
        let new = Arc::<str>::from(text);
        *slot = Some(new.clone());
        new
    }
    fn text(&mut self, text: &str) -> Arc<str> {
        if self.reuse {
            Self::intern(&mut self.text, text)
        } else {
            Arc::from(text)
        }
    }
    fn prefix(&mut self, text: &str) -> Arc<str> {
        if self.reuse {
            Self::intern(&mut self.prefix, text)
        } else {
            Arc::from(text)
        }
    }
    fn join(&mut self, a: &Arc<str>, b: &str) -> Arc<str> {
        // Index by bounded run length, not growing strings. Always verify the
        // contents: equal lengths are a hint, never an equality oracle.
        let key = a.len();
        if self.reuse
            && let Some((old_a, old_b, joined)) = self.joined.get(key).and_then(Option::as_ref)
            && (Arc::ptr_eq(old_a, a) || old_a.as_ref() == a.as_ref())
            && old_b.as_ref() == b
        {
            return joined.clone();
        }
        let mut text = String::with_capacity(a.len() + b.len());
        text.push_str(a);
        text.push_str(b);
        let joined = Arc::<str>::from(text);
        if self.reuse {
            if self.joined.len() <= key {
                self.joined.resize_with(key + 1, || None);
            }
            self.joined[key] = Some((a.clone(), Arc::from(b), joined.clone()));
        }
        joined
    }
}

impl Spliced {
    fn push(
        &mut self,
        source: PieceSource,
        summary: Summary,
        cache: &mut InsertionCache,
    ) -> Option<()> {
        if summary.bytes == 0 {
            return Some(());
        }
        let end = self.pieces.last().map_or(summary, |p| p.end.plus(summary));
        if let Some(previous) = self.pieces.last_mut() {
            match (&mut previous.source, &source) {
                (PieceSource::Original(a), PieceSource::Original(b)) if a.end == b.start => {
                    a.end = b.end;
                    previous.end = end;
                    return Some(());
                }
                (PieceSource::Inserted(a), PieceSource::Inserted(b)) => {
                    if a.len() + b.len() > CHUNK {
                        return None;
                    }
                    *a = cache.join(a, b);
                    previous.end = end;
                    return Some(());
                }
                _ => {}
            }
        }
        if self.pieces.len() == 128 {
            return None;
        }
        self.pieces.push(Piece { source, end });
        Some(())
    }

    fn insertions(
        node: &Arc<Node>,
        edits: &[crate::Edit<'_>],
        start: usize,
        cache: &mut InsertionCache,
    ) -> Option<Leaf> {
        if edits.is_empty()
            || edits
                .iter()
                .any(|e| !e.range.is_empty() || e.text.len() > 32)
        {
            return None;
        }
        let Kind::Leaf(old) = &node.kind else {
            return None;
        };
        let scratch;
        let (original, pieces): (&Arc<Node>, &[Piece]) = match old {
            Leaf::Flat(_) => {
                // Dense one-off batches copy one small flat run more cheaply
                // than constructing two fragment descriptors per caret.
                if edits.len() > 16 {
                    return None;
                }
                scratch = vec![Piece {
                    source: PieceSource::Original(0..node.summary.bytes),
                    end: node.summary,
                }];
                (node, &scratch)
            }
            Leaf::Patched(p) => {
                let prefix = p.prefix_summary();
                let inserted = prefix.plus(Summary::of(old.part(1)));
                scratch = vec![
                    Piece {
                        source: PieceSource::Original(0..p.range.start),
                        end: prefix,
                    },
                    Piece {
                        source: PieceSource::Inserted(cache.prefix(old.part(1))),
                        end: inserted,
                    },
                    Piece {
                        source: PieceSource::Original(p.range.end..p.original.summary.bytes),
                        end: node.summary,
                    },
                ];
                (&p.original, &scratch)
            }
            Leaf::Spliced(s) => (&s.original, &s.pieces),
        };
        let Kind::Leaf(Leaf::Flat(flat)) = &original.kind else {
            unreachable!()
        };
        let mut out = Self {
            original: original.clone(),
            pieces: Vec::with_capacity(if matches!(old, Leaf::Spliced(_)) {
                pieces.len()
            } else {
                (pieces.len() + edits.len() * 2).min(128)
            }),
        };
        let mut index = 0;
        let mut consumed = 0;
        let mut copy_until = |out: &mut Self,
                              until: usize,
                              append: Option<&str>,
                              cache: &mut InsertionCache|
         -> Option<bool> {
            while consumed < until {
                let piece = &pieces[index];
                let before = if index == 0 {
                    Summary::default()
                } else {
                    pieces[index - 1].end
                };
                let end = until.min(piece.end.bytes);
                let range = consumed - before.bytes..end - before.bytes;
                let text = piece.source.text(flat);
                if range.len() == text.len()
                    && end == until
                    && let (PieceSource::Inserted(old), Some(append)) = (&piece.source, append)
                {
                    if old.len() + append.len() > CHUNK {
                        return None;
                    }
                    let joined = cache.join(old, append);
                    out.push(
                        PieceSource::Inserted(joined),
                        piece.end.minus(before).plus(Summary::of(append)),
                        cache,
                    )?;
                    consumed = end;
                    index += 1;
                    return Some(true);
                }
                if range.len() == text.len() {
                    out.push(piece.source.clone(), piece.end.minus(before), cache)?;
                } else if !range.is_empty() {
                    let summary = Summary::of(&text[range.clone()]);
                    let source = match &piece.source {
                        PieceSource::Original(r) => {
                            PieceSource::Original(r.start + range.start..r.start + range.end)
                        }
                        PieceSource::Inserted(_) => {
                            PieceSource::Inserted(Arc::from(&text[range.clone()]))
                        }
                    };
                    out.push(source, summary, cache)?;
                }
                consumed = end;
                if consumed == piece.end.bytes {
                    index += 1;
                }
            }
            Some(false)
        };
        for edit in edits {
            // Append directly from the borrowed old piece. Cloning and then
            // replacing its Arc would add two atomic operations per caret.
            if !copy_until(&mut out, edit.range.start - start, Some(edit.text), cache)? {
                let text = cache.text(edit.text);
                out.push(PieceSource::Inserted(text), Summary::of(edit.text), cache)?;
            }
        }
        copy_until(&mut out, node.summary.bytes, None, cache)?;
        (!out.pieces.is_empty()).then_some(Leaf::Spliced(out))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Flat {
    text: String,
    newlines: LineIndex,
}

// A typical 4 KiB leaf has fewer than 256 LFs: one low byte per LF plus a
// cumulative-count byte for each 256-byte block. Dense LF input uses u16 offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LineIndex {
    data: Box<[u8]>,
    blocks: u8,
    wide: bool,
}
impl LineIndex {
    fn new(text: &str, count: usize) -> Self {
        if count > 255 {
            let mut data = Vec::with_capacity(count * 2);
            for i in memchr::memchr_iter(b'\n', text.as_bytes()) {
                data.extend_from_slice(&(i as u16).to_le_bytes());
            }
            return Self {
                data: data.into_boxed_slice(),
                blocks: 0,
                wide: true,
            };
        }
        let blocks = text.len().div_ceil(256);
        let mut data = vec![0; blocks + count];
        let mut block = 0;
        let mut at = 0;
        for i in memchr::memchr_iter(b'\n', text.as_bytes()) {
            while block < i / 256 {
                data[block] = at as u8;
                block += 1;
            }
            data[blocks + at] = i as u8;
            at += 1;
        }
        data[block..blocks].fill(at as u8);
        Self {
            data: data.into_boxed_slice(),
            blocks: blocks as u8,
            wide: false,
        }
    }
    fn len(&self) -> usize {
        if self.wide {
            self.data.len() / 2
        } else {
            self.data.len() - self.blocks as usize
        }
    }
    fn at(&self, index: usize) -> usize {
        if self.wide {
            u16::from_le_bytes([self.data[index * 2], self.data[index * 2 + 1]]) as usize
        } else {
            let blocks = self.blocks as usize;
            let block = self.data[..blocks].partition_point(|&n| n as usize <= index);
            block * 256 + self.data[blocks + index] as usize
        }
    }
    fn get(&self, index: usize) -> Option<usize> {
        (index < self.len()).then(|| self.at(index))
    }
    fn before(&self, byte: usize, len: usize) -> usize {
        if byte == len {
            return self.len();
        }
        if self.wide {
            let (mut low, mut high) = (0, self.len());
            while low < high {
                let mid = (low + high) / 2;
                if self.at(mid) < byte {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            low
        } else {
            let block = byte / 256;
            let blocks = self.blocks as usize;
            let before = if block == 0 {
                0
            } else {
                self.data[block - 1] as usize
            };
            let end = self.data[block] as usize;
            before
                + self.data[blocks + before..blocks + end]
                    .partition_point(|&n| (n as usize) < byte % 256)
        }
    }
}
impl Flat {
    fn new(text: String, count: usize) -> Self {
        let newlines = LineIndex::new(&text, count);
        Self { text, newlines }
    }
    fn removed(&self, range: std::ops::Range<usize>, summary: Summary) -> Summary {
        if range.is_empty() {
            return Summary::default();
        }
        if summary.bytes == summary.chars {
            Summary {
                bytes: range.len(),
                chars: range.len(),
                utf16: range.len(),
                newlines: self.newlines.before(range.end, self.len())
                    - self.newlines.before(range.start, self.len()),
            }
        } else {
            Summary::of(&self[range])
        }
    }
}
impl std::ops::Deref for Flat {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Patched {
    original: Arc<Node>,
    range: std::ops::Range<usize>,
    inserted: [u8; 32],
    len: u8,
    // These fit the old inline leaf's padding. ASCII prefixes use the LF index;
    // Unicode prefixes are measured once, then shared by adjacent typing.
    prefix: [u16; 3],
}

impl Patched {
    // Keep the Unicode scan out of the ordinary ASCII edit path. Otherwise the
    // vectorized summary loop increases the hot edit frame and register spills.
    #[inline(never)]
    fn unicode_prefix(flat: &str, at: usize) -> [u16; 3] {
        let summary = Summary::of(&flat[..at]);
        [
            summary.chars as u16,
            summary.utf16 as u16,
            summary.newlines as u16,
        ]
    }

    fn prefix_summary(&self) -> Summary {
        if self.prefix[0] != u16::MAX {
            return Summary {
                bytes: self.range.start,
                chars: self.prefix[0] as usize,
                utf16: self.prefix[1] as usize,
                newlines: self.prefix[2] as usize,
            };
        }
        let Kind::Leaf(Leaf::Flat(flat)) = &self.original.kind else {
            unreachable!("patched leaves retain one flat leaf")
        };
        flat.removed(0..self.range.start, self.original.summary)
    }
}

impl Leaf {
    fn part_count(&self) -> usize {
        match self {
            Self::Flat(_) => 1,
            Self::Patched(_) => 3,
            Self::Spliced(s) => s.pieces.len(),
        }
    }
    fn part(&self, index: usize) -> &str {
        match self {
            Self::Flat(s) => s,
            Self::Patched(p) => {
                let Kind::Leaf(Self::Flat(original)) = &p.original.kind else {
                    unreachable!("patched leaves retain exactly one flat leaf")
                };
                match index {
                    0 => &original[..p.range.start],
                    1 => std::str::from_utf8(&p.inserted[..p.len as usize])
                        .expect("validated inline UTF-8"),
                    2 => &original[p.range.end..],
                    _ => unreachable!(),
                }
            }
            Self::Spliced(s) => {
                let Kind::Leaf(Self::Flat(original)) = &s.original.kind else {
                    unreachable!("spliced leaves retain exactly one flat leaf")
                };
                s.pieces[index].source.text(original)
            }
        }
    }
    fn parts(&self) -> Parts<'_> {
        Parts {
            leaf: self,
            range: 0..self.part_count(),
        }
    }
    #[inline(never)]
    fn edge_part(&self, forward: bool) -> (usize, &str) {
        let parts = self.parts();
        let part = if forward {
            parts.clone().position(|s| !s.is_empty())
        } else {
            parts.clone().rposition(|s| !s.is_empty())
        }
        .unwrap_or(0);
        (part, self.part(part))
    }
    fn boundary(&self, byte: usize) -> bool {
        if let Self::Spliced(s) = self {
            let i = s
                .pieces
                .partition_point(|p| p.end.bytes <= byte)
                .min(s.pieces.len() - 1);
            let before = if i == 0 { 0 } else { s.pieces[i - 1].end.bytes };
            return self.part(i).is_char_boundary(byte - before);
        }
        let mut before = 0;
        for part in self.parts() {
            if byte <= before + part.len() {
                return part.is_char_boundary(byte - before);
            }
            before += part.len();
        }
        false
    }
    fn len(&self) -> usize {
        match self {
            Self::Flat(s) => s.len(),
            Self::Patched(p) => p.original.summary.bytes - p.range.len() + p.len as usize,
            Self::Spliced(s) => s.pieces.last().expect("nonempty pieces").end.bytes,
        }
    }
    fn flatten(&self) -> String {
        let mut s = String::with_capacity(self.len());
        for part in self.parts() {
            s.push_str(part);
        }
        s
    }
    // The callers already prove the flat/small case. Inline the bounded record
    // construction to avoid an extra large return-value copy for every caret.
    #[inline(always)]
    fn small(original: &Arc<Node>, range: std::ops::Range<usize>, text: &str) -> Option<Self> {
        let Kind::Leaf(Self::Flat(_)) = &original.kind else {
            return None;
        };
        if text.len() > 32 {
            return None;
        }
        let mut inserted = [0; 32];
        inserted[..text.len()].copy_from_slice(text.as_bytes());
        let prefix = if original.summary.bytes == original.summary.chars {
            [u16::MAX; 3]
        } else {
            let Kind::Leaf(Self::Flat(flat)) = &original.kind else {
                unreachable!()
            };
            Patched::unicode_prefix(flat, range.start)
        };
        Some(Self::Patched(Patched {
            original: original.clone(),
            range,
            inserted,
            len: text.len() as u8,
            prefix,
        }))
    }
    fn extend(&self, at: usize, text: &str) -> Option<Self> {
        let Self::Patched(p) = self else { return None };
        let len = p.len as usize;
        if at != p.range.start + len || len + text.len() > p.inserted.len() {
            return None;
        }
        let mut next = p.clone();
        next.inserted[len..len + text.len()].copy_from_slice(text.as_bytes());
        next.len = (len + text.len()) as u8;
        Some(Self::Patched(next))
    }
    fn seek(&self, offset: usize, metric: Metric) -> (&str, Summary) {
        if let Self::Flat(s) = self {
            return (s, Summary::default());
        }
        if let Self::Spliced(s) = self {
            let i = s
                .pieces
                .partition_point(|p| p.end.get(metric) <= offset)
                .min(s.pieces.len() - 1);
            return (
                self.part(i),
                if i == 0 {
                    Summary::default()
                } else {
                    s.pieces[i - 1].end
                },
            );
        }
        let parts = self.parts();
        let last = parts.clone().rposition(|s| !s.is_empty()).unwrap_or(0);
        let mut before = Summary::default();
        for (i, s) in parts.enumerate() {
            if s.is_empty() && i != last {
                continue;
            }
            // The final piece is selected regardless of its metrics. Byte
            // lookups can also choose the current piece by length alone. Do not
            // scan its Unicode summary merely to return it to the caller.
            if i == last || (matches!(metric, Metric::Byte) && offset < before.bytes + s.len()) {
                return (s, before);
            }
            let sum = if i == 0
                && let Self::Patched(p) = self
            {
                p.prefix_summary()
            } else {
                Summary::of(s)
            };
            if offset < before.get(metric) + sum.get(metric) {
                return (s, before);
            }
            before = before.plus(sum);
        }
        unreachable!()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub summary: Summary,
    pub height: u8,
    pub kind: Kind,
}

impl Node {
    fn leaf(text: String) -> Arc<Self> {
        let summary = Summary::of(&text);
        Arc::new(Self {
            summary,
            height: 0,
            kind: Kind::Leaf(Leaf::Flat(Flat::new(text, summary.newlines))),
        })
    }

    fn leaf_str(text: &str) -> Arc<Self> {
        Self::leaf(text.to_owned())
    }

    fn branch(nodes: Vec<Arc<Self>>) -> Arc<Self> {
        let height = nodes[0].height + 1;
        let mut sum = Summary::default();
        let children = nodes
            .into_iter()
            .map(|node| {
                sum = sum.plus(node.summary);
                Child { node, end: sum }
            })
            .collect();
        Arc::new(Self {
            summary: sum,
            height,
            kind: Kind::Branch(children),
        })
    }

    fn occupancy(&self) -> usize {
        match &self.kind {
            Kind::Leaf(s) => s.len(),
            Kind::Branch(c) => c.len(),
        }
    }

    fn minimum(&self) -> usize {
        if self.height == 0 {
            CHUNK / 2
        } else {
            FANOUT / 2
        }
    }
}

/// Packs a stream of validated UTF-8 into bounded leaves, without retaining its source.
#[derive(Debug, Default)]
pub(crate) struct Builder {
    leaves: Vec<Arc<Node>>,
    pending: String,
}

impl Builder {
    pub fn push(&mut self, mut text: &str) {
        while !text.is_empty() {
            if self.pending.is_empty() && text.len() >= FILL {
                let n = text.floor_char_boundary(FILL);
                self.leaves.push(Node::leaf_str(&text[..n]));
                text = &text[n..];
                continue;
            }
            let mut n = (FILL - self.pending.len()).min(text.len());
            while !text.is_char_boundary(n) {
                n -= 1;
            }
            if n == 0 {
                self.flush();
                continue;
            }
            if self.pending.capacity() == 0 {
                self.pending = String::with_capacity(FILL);
            }
            self.pending.push_str(&text[..n]);
            text = &text[n..];
            if self.pending.len() == FILL {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        if !self.pending.is_empty() {
            self.leaves
                .push(Node::leaf(std::mem::take(&mut self.pending)));
        }
    }

    fn leaves(mut self) -> Vec<Arc<Node>> {
        self.flush();
        repair(&mut self.leaves);
        self.leaves
    }

    pub fn finish(self) -> Arc<Node> {
        root_from_nodes(self.leaves())
    }
}

pub(crate) fn from_reader(mut reader: impl io::Read) -> io::Result<Arc<Node>> {
    let mut builder = Builder::default();
    let mut buf = vec![0; 1024 * 1024];
    let mut carry = 0;
    loop {
        let n = match reader.read(&mut buf[carry..]) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            other => other?,
        };
        if n == 0 {
            if carry != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "incomplete UTF-8 at end of text",
                ));
            }
            return Ok(builder.finish());
        }
        let len = carry + n;
        // Separate a possibly incomplete final scalar before SIMD validation.
        let mut lead = len - 1;
        while lead > 0 && buf[lead] & 0xc0 == 0x80 && len - lead <= 3 {
            lead -= 1;
        }
        let width = match buf[lead] {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => 1,
        };
        let valid = if width > len - lead { lead } else { len };
        let text = simdutf8::basic::from_utf8(&buf[..valid])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        builder.push(text);
        carry = len - valid;
        buf.copy_within(valid..len, 0);
    }
}

// The reader stays on the caller's thread, so Read need not be Send or 'static.
// One bounded job per worker limits staging allocations independently of file size.
pub(crate) fn from_reader_parallel(mut reader: impl io::Read) -> io::Result<Arc<Node>> {
    const BLOCK: usize = 2 * 1024 * 1024;
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    if workers == 1 {
        return from_reader(reader);
    }
    std::thread::scope(|scope| {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for _ in 0..workers {
            let (send, receive) = std::sync::mpsc::sync_channel::<(Vec<u8>, usize)>(1);
            let (result, output) = std::sync::mpsc::channel();
            scope.spawn(move || {
                while let Ok((buf, len)) = receive.recv() {
                    let leaves = simdutf8::basic::from_utf8(&buf[..len])
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
                        .map(load_leaves);
                    if result.send((buf, leaves)).is_err() {
                        break;
                    }
                }
            });
            inputs.push(send);
            outputs.push(output);
        }
        let mut nodes = Vec::new();
        let mut carry = Vec::<u8>::new();
        let mut submitted = 0;
        let mut collected = 0;
        let mut serial_blocks = 0;
        let mut spare = None;
        loop {
            let slot = submitted % workers;
            let mut buf = if let Some(buf) = spare.take() {
                buf
            } else if submitted >= workers {
                let (buf, leaves) = outputs[slot].recv().map_err(io::Error::other)?;
                nodes.extend(leaves?.into_iter().map(loaded_node));
                collected += 1;
                buf
            } else {
                vec![0; BLOCK]
            };
            buf[..carry.len()].copy_from_slice(&carry);
            let mut len = carry.len();
            let mut eof = false;
            while len < BLOCK {
                match reader.read(&mut buf[len..]) {
                    Ok(0) => {
                        eof = true;
                        break;
                    }
                    Ok(n) => len += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            let mut valid = len;
            if !eof && len > 0 {
                let mut lead = len - 1;
                while lead > 0 && buf[lead] & 0xc0 == 0x80 && len - lead <= 3 {
                    lead -= 1;
                }
                let width = match buf[lead] {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => 1,
                };
                if width > len - lead {
                    valid = lead;
                }
            }
            carry.clear();
            carry.extend_from_slice(&buf[valid..len]);
            if valid > 0 {
                if serial_blocks < 2 {
                    // Keep the opening lines' indexes local to the editing thread.
                    let text = simdutf8::basic::from_utf8(&buf[..valid])
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    let mut builder = Builder::default();
                    builder.push(text);
                    nodes.extend(builder.leaves());
                    serial_blocks += 1;
                    spare = Some(buf);
                } else {
                    inputs[slot].send((buf, valid)).map_err(io::Error::other)?;
                    submitted += 1;
                }
            }
            if eof {
                break;
            }
        }
        drop(inputs);
        while collected < submitted {
            let (_, leaves) = outputs[collected % workers]
                .recv()
                .map_err(io::Error::other)?;
            nodes.extend(leaves?.into_iter().map(loaded_node));
            collected += 1;
        }
        repair(&mut nodes);
        Ok(root_from_nodes(nodes))
    })
}

// Workers prepare content and indexes; allocate Arc<Node> on the reader thread
// in document order so traversal does not alternate among worker allocations.
fn load_leaves(mut text: &str) -> Vec<(Summary, Flat)> {
    let mut leaves = Vec::with_capacity(text.len().div_ceil(FILL));
    while !text.is_empty() {
        let n = text.floor_char_boundary(FILL.min(text.len()));
        let value = text[..n].to_owned();
        let summary = Summary::of(&value);
        let flat = Flat::new(value, summary.newlines);
        leaves.push((summary, flat));
        text = &text[n..];
    }
    leaves
}
fn loaded_node((summary, flat): (Summary, Flat)) -> Arc<Node> {
    Arc::new(Node {
        summary,
        height: 0,
        kind: Kind::Leaf(Leaf::Flat(flat)),
    })
}

fn groups<T>(items: Vec<T>, max: usize) -> Vec<Vec<T>> {
    if items.is_empty() {
        return Vec::new();
    }
    let count = items.len().div_ceil(max);
    let small = items.len() / count;
    let extra = items.len() % count;
    let mut iter = items.into_iter();
    (0..count)
        .map(|i| iter.by_ref().take(small + usize::from(i < extra)).collect())
        .collect()
}

fn root_from_nodes(mut nodes: Vec<Arc<Node>>) -> Arc<Node> {
    if nodes.is_empty() {
        return Node::leaf(String::new());
    }
    while nodes.len() > 1 {
        nodes = groups(nodes, FANOUT)
            .into_iter()
            .map(Node::branch)
            .collect();
    }
    let mut root = nodes.pop().expect("one root");
    loop {
        match &root.kind {
            Kind::Branch(c) if c.len() == 1 => root = c[0].node.clone(),
            _ => return root,
        }
    }
}

fn find_leaf(mut node: &Node, mut offset: usize, metric: Metric) -> (&Leaf, Summary, usize) {
    let mut prefix = Summary::default();
    loop {
        match &node.kind {
            Kind::Leaf(s) => return (s, prefix, offset),
            Kind::Branch(children) => {
                let i = children
                    .partition_point(|c| c.end.get(metric) <= offset)
                    .min(children.len() - 1);
                if i > 0 {
                    let before = children[i - 1].end;
                    offset -= before.get(metric);
                    prefix = prefix.plus(before);
                }
                node = &children[i].node;
            }
        }
    }
}

pub(crate) fn seek(node: &Node, offset: usize, metric: Metric) -> (&str, Summary) {
    let (leaf, before, offset) = find_leaf(node, offset, metric);
    match leaf {
        Leaf::Flat(s) => (s, before),
        _ => {
            let (s, part) = leaf.seek(offset, metric);
            (s, before.plus(part))
        }
    }
}

pub(crate) fn is_char_boundary(root: &Node, byte: usize) -> bool {
    let (leaf, _, local) = find_leaf(root, byte, Metric::Byte);
    leaf.boundary(local)
}

pub(crate) fn byte_line(root: &Node, byte: usize, ascii: bool) -> usize {
    let (leaf, before, local) = find_leaf(root, byte, Metric::Byte);
    if let Leaf::Flat(s) = leaf {
        assert!(
            ascii || s.is_char_boundary(local),
            "byte offset is not a scalar boundary"
        );
        before.newlines + s.newlines.before(local, s.len())
    } else {
        let (s, part) = leaf.seek(local, Metric::Byte);
        let part = before.plus(part);
        let prefix = s
            .get(..byte - part.bytes)
            .expect("byte offset is not a scalar boundary");
        part.newlines + memchr::memchr_iter(b'\n', prefix.as_bytes()).count()
    }
}

pub(crate) fn byte_point(root: &Node, byte: usize, ascii: bool) -> (usize, usize) {
    let (leaf, before, local) = find_leaf(root, byte, Metric::Byte);
    if let Leaf::Flat(s) = leaf {
        assert!(
            ascii || s.is_char_boundary(local),
            "byte offset is not a scalar boundary"
        );
        let count = s.newlines.before(local, s.len());
        let row = before.newlines + count;
        let start = if count == 0 {
            line_to_byte(root, row)
        } else {
            before.bytes + s.newlines.at(count - 1) + 1
        };
        (row, byte - start)
    } else {
        let (s, part) = leaf.seek(local, Metric::Byte);
        let part = before.plus(part);
        let prefix = s
            .get(..byte - part.bytes)
            .expect("byte offset is not a scalar boundary");
        let row = part.newlines + memchr::memchr_iter(b'\n', prefix.as_bytes()).count();
        let start = memchr::memrchr(b'\n', prefix.as_bytes())
            .map_or_else(|| line_to_byte(root, row), |i| part.bytes + i + 1);
        (row, byte - start)
    }
}

pub(crate) fn prefix(root: &Node, byte: usize) -> Summary {
    assert!(
        byte <= root.summary.bytes,
        "byte offset {byte} exceeds text length {}",
        root.summary.bytes
    );
    if byte == root.summary.bytes {
        return root.summary;
    }
    let (s, before) = seek(root, byte, Metric::Byte);
    let part = s
        .get(..byte - before.bytes)
        .expect("byte offset is not a UTF-8 scalar boundary");
    before.plus(Summary::of(part))
}

pub(crate) fn validate_boundaries(root: &Node, edits: &[crate::Edit<'_>]) {
    fn visit(
        node: &Node,
        start: usize,
        points: &mut std::iter::Peekable<impl Iterator<Item = usize>>,
    ) {
        let end = start + node.summary.bytes;
        while points.peek().is_some_and(|&p| p == start || p == end) {
            points.next();
        }
        if points.peek().is_none_or(|&p| p > end) {
            return;
        }
        match &node.kind {
            Kind::Leaf(s) => {
                while let Some(&p) = points.peek() {
                    if p > end {
                        break;
                    }
                    // Boundary validation needs bytes only, never Unicode summaries.
                    assert!(
                        s.boundary(p - start),
                        "edit endpoint is not a UTF-8 scalar boundary"
                    );
                    points.next();
                }
            }
            Kind::Branch(children) => {
                let mut offset = start;
                for child in children {
                    visit(&child.node, offset, points);
                    offset = start + child.end.bytes;
                }
            }
        }
    }
    let mut previous = None;
    let mut points = edits
        .iter()
        .flat_map(|e| [e.range.start, e.range.end])
        .filter(|&point| previous.replace(point) != Some(point))
        .peekable();
    visit(root, 0, &mut points);
    assert!(
        points.next().is_none(),
        "validated edit endpoints exhausted"
    );
}

pub(crate) fn equal(a: &Arc<Node>, b: &Arc<Node>) -> bool {
    if Arc::ptr_eq(a, b) {
        return true;
    }
    if a.summary != b.summary {
        return false;
    }
    match (&a.kind, &b.kind) {
        (Kind::Leaf(Leaf::Flat(a)), Kind::Leaf(Leaf::Flat(b))) => a.text == b.text,
        (Kind::Branch(a), Kind::Branch(b))
            if a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.end == b.end) =>
        {
            a.iter().zip(b).all(|(a, b)| equal(&a.node, &b.node))
        }
        _ => {
            let mut a = Cursor::at(a, 0);
            let mut b = Cursor::at(b, 0);
            let (mut ai, mut bi) = (0, 0);
            loop {
                let n = (a.text.len() - ai).min(b.text.len() - bi);
                if a.text.as_bytes()[ai..ai + n] != b.text.as_bytes()[bi..bi + n] {
                    return false;
                }
                ai += n;
                bi += n;
                let mut done = false;
                if ai == a.text.len() {
                    done = !a.advance();
                    ai = 0;
                }
                if bi == b.text.len() {
                    done |= !b.advance();
                    bi = 0;
                }
                if done {
                    return true;
                }
            }
        }
    }
}

pub(crate) fn storage_bytes(root: &Arc<Node>) -> usize {
    storage_excluding(root, None)
}

pub(crate) fn storage_excluding(root: &Arc<Node>, shared: Option<&Arc<Node>>) -> usize {
    if shared.is_some_and(|other| Arc::ptr_eq(root, other)) {
        return 0;
    }
    fn mark(
        node: &Arc<Node>,
        seen: &mut std::collections::HashSet<*const Node>,
        strings: &mut std::collections::HashSet<*const u8>,
    ) {
        if !seen.insert(Arc::as_ptr(node)) {
            return;
        }
        match &node.kind {
            Kind::Leaf(Leaf::Patched(p)) => mark(&p.original, seen, strings),
            Kind::Leaf(Leaf::Spliced(s)) => {
                mark(&s.original, seen, strings);
                for piece in &s.pieces {
                    if let PieceSource::Inserted(text) = &piece.source {
                        strings.insert(text.as_ptr());
                    }
                }
            }
            Kind::Branch(children) => {
                for child in children {
                    mark(&child.node, seen, strings);
                }
            }
            Kind::Leaf(Leaf::Flat(_)) => {}
        }
    }
    fn visit(
        node: &Arc<Node>,
        seen: &mut std::collections::HashSet<*const Node>,
        strings: &mut std::collections::HashSet<*const u8>,
    ) -> usize {
        if !seen.insert(Arc::as_ptr(node)) {
            return 0;
        }
        // Arc counters and reachable allocations, excluding allocator bookkeeping.
        let own = std::mem::size_of::<Node>() + 2 * std::mem::size_of::<usize>();
        own + match &node.kind {
            Kind::Leaf(Leaf::Flat(s)) => s.text.capacity() + s.newlines.data.len(),
            Kind::Leaf(Leaf::Patched(p)) => visit(&p.original, seen, strings),
            Kind::Leaf(Leaf::Spliced(s)) => {
                s.pieces.capacity() * std::mem::size_of::<Piece>()
                    + visit(&s.original, seen, strings)
                    + s.pieces
                        .iter()
                        .map(|p| match &p.source {
                            PieceSource::Inserted(text) if strings.insert(text.as_ptr()) => {
                                text.len() + 2 * std::mem::size_of::<usize>()
                            }
                            _ => 0,
                        })
                        .sum::<usize>()
            }
            Kind::Branch(c) => {
                c.capacity() * std::mem::size_of::<Child>()
                    + c.iter()
                        .map(|c| visit(&c.node, seen, strings))
                        .sum::<usize>()
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut strings = std::collections::HashSet::new();
    if let Some(other) = shared {
        mark(other, &mut seen, &mut strings);
    }
    visit(root, &mut seen, &mut strings)
}

pub(crate) fn metric_to_byte(root: &Node, value: usize, metric: Metric) -> usize {
    assert!(
        value <= root.summary.get(metric),
        "coordinate {value} exceeds {:?} length {}",
        metric,
        root.summary.get(metric)
    );
    if value == root.summary.get(metric) {
        return root.summary.bytes;
    }
    let (s, before) = seek(root, value, metric);
    let local = value - before.get(metric);
    if s.is_ascii() && !matches!(metric, Metric::Line) {
        return before.bytes + local;
    }
    let byte = match metric {
        Metric::Char => s.char_indices().nth(local).map_or(s.len(), |(i, _)| i),
        Metric::Utf16 => {
            let mut units = 0;
            let mut result = s.len();
            for (i, c) in s.char_indices() {
                if units == local {
                    result = i;
                    break;
                }
                units += c.len_utf16();
                assert!(units <= local, "UTF-16 offset is inside a surrogate pair");
            }
            result
        }
        Metric::Line => {
            memchr::memchr_iter(b'\n', s.as_bytes())
                .nth(local)
                .expect("newline summary")
                + 1
        }
        Metric::Byte => local,
    };
    before.bytes + byte
}

pub(crate) fn line_start_chunk(root: &Node, line: usize) -> (&str, Summary, usize) {
    assert!(
        line <= root.summary.newlines,
        "line index {line} exceeds last line {}",
        root.summary.newlines
    );
    if line == 0 {
        let (s, before) = seek(root, 0, Metric::Byte);
        return (s, before, 0);
    }
    let (leaf, before, local) = find_leaf(root, line - 1, Metric::Line);
    match leaf {
        Leaf::Flat(s) => (s, before, s.newlines.at(local) + 1),
        _ => {
            let (s, part) = leaf.seek(local, Metric::Line);
            let local = memchr::memchr_iter(b'\n', s.as_bytes())
                .nth(local - part.newlines)
                .expect("newline summary")
                + 1;
            (s, before.plus(part), local)
        }
    }
}

pub(crate) fn line_to_byte(root: &Node, line: usize) -> usize {
    let (_, before, local) = line_start_chunk(root, line);
    before.bytes + local
}

pub(crate) fn line_range(root: &Node, line: usize) -> (usize, usize) {
    assert!(
        line <= root.summary.newlines,
        "line index exceeds last line"
    );
    let metric = if line == 0 {
        Metric::Byte
    } else {
        Metric::Line
    };
    let (leaf, before, local) = find_leaf(root, line.saturating_sub(1), metric);
    if let Leaf::Flat(s) = leaf {
        let (start, next) = if line == 0 {
            (0, 0)
        } else {
            (before.bytes + s.newlines.at(local) + 1, local + 1)
        };
        let end = if let Some(lf) = s.newlines.get(next) {
            before.bytes + lf
        } else if line == root.summary.newlines {
            root.summary.bytes
        } else {
            line_to_byte(root, line + 1) - 1
        };
        (start, end)
    } else {
        let start = line_to_byte(root, line);
        let end = if line == root.summary.newlines {
            root.summary.bytes
        } else {
            line_to_byte(root, line + 1) - 1
        };
        (start, end)
    }
}

/// Rebalances only underfull neighbors; all other subtrees remain shared.
fn repair(nodes: &mut Vec<Arc<Node>>) {
    let mut i = 0;
    while i + 1 < nodes.len() {
        if nodes[i].occupancy() >= nodes[i].minimum()
            && nodes[i + 1].occupancy() >= nodes[i + 1].minimum()
        {
            i += 1;
            continue;
        }
        let right = nodes.remove(i + 1);
        let left = &nodes[i];
        let replacement = match (&left.kind, &right.kind) {
            (Kind::Leaf(a), Kind::Leaf(b)) => {
                let mut s = String::with_capacity(a.len() + b.len());
                for part in a.parts().chain(b.parts()) {
                    s.push_str(part);
                }
                if s.len() <= CHUNK {
                    vec![Node::leaf(s)]
                } else if s.len() > CHUNK * 2 {
                    // Fragment leaves have bounded headroom for insertions.
                    // Materializing a large neighbor must still produce ordinary
                    // bounded flat leaves and valid compact LF indexes.
                    let mut builder = Builder::default();
                    builder.push(&s);
                    builder.leaves()
                } else {
                    // Both ordinary leaves must fit, including a 4-byte scalar
                    // straddling the midpoint. The feasible boundary is [len-CHUNK, CHUNK].
                    let mid = s.ceil_char_boundary((s.len() / 2).max(s.len() - CHUNK));
                    if mid <= CHUNK {
                        let tail = s.split_off(mid);
                        vec![Node::leaf(s), Node::leaf(tail)]
                    } else {
                        let mut builder = Builder::default();
                        builder.push(&s);
                        builder.leaves()
                    }
                }
            }
            (Kind::Branch(a), Kind::Branch(b)) => {
                let mut children = a.iter().chain(b).map(|c| c.node.clone()).collect();
                repair(&mut children);
                groups(children, FANOUT)
                    .into_iter()
                    .map(Node::branch)
                    .collect()
            }
            _ => unreachable!("balanced tree has equal-height siblings"),
        };
        let n = replacement.len();
        nodes.splice(i..=i, replacement);
        if n == 2 {
            i += 1;
        } else {
            i = i.saturating_sub(1);
        }
    }
}

struct Batch<'a, 'b> {
    edits: &'a [crate::Edit<'b>],
    next: usize,
    deleted_until: usize,
    len: usize,
    inserted: Option<(&'b str, Summary)>,
    pieces: InsertionCache,
}

impl Batch<'_, '_> {
    fn touches(&self, start: usize, end: usize) -> bool {
        self.deleted_until > start
            || self
                .edits
                .get(self.next)
                .is_some_and(|e| e.range.start < end || (end == self.len && e.range.start == end))
    }
}

// Multicursor typing usually reuses one immutable replacement string. Reuse its
// metrics across leaves too; pointer + length equality is sufficient, without
// retaining text or comparing/scanning the replacement on every caret.
fn inserted_summary<'a>(cached: &mut Option<(&'a str, Summary)>, text: &'a str) -> Summary {
    if let Some((previous, summary)) = *cached
        && previous.len() == text.len()
        && std::ptr::eq(previous.as_ptr(), text.as_ptr())
    {
        return summary;
    }
    let summary = Summary::of(text);
    *cached = Some((text, summary));
    summary
}

enum Change {
    Keep,
    Remove,
    Split(Vec<Arc<Node>>),
}

fn edit_node(node: &mut Arc<Node>, start: usize, batch: &mut Batch<'_, '_>) -> Change {
    let end = start + node.summary.bytes;
    if !batch.touches(start, end) {
        return Change::Keep;
    }
    if batch.deleted_until >= end
        && start != end
        && !(end == batch.len
            && batch
                .edits
                .get(batch.next)
                .is_some_and(|e| e.range.start == end))
    {
        return Change::Remove;
    }
    if let Kind::Leaf(old) = &node.kind {
        if matches!(old, Leaf::Patched(_))
            && let Some(edit) = batch.edits.get(batch.next)
            && batch.deleted_until <= start
            && edit.range.start >= start
            && edit.range.is_empty()
            && batch
                .edits
                .get(batch.next + 1)
                .is_none_or(|e| e.range.start >= end && !(end == batch.len && e.range.start == end))
            && node.summary.bytes + edit.text.len() <= CHUNK
            && let Some(leaf) = old.extend(edit.range.start - start, edit.text)
        {
            let summary = node
                .summary
                .plus(inserted_summary(&mut batch.inserted, edit.text));
            batch.next += 1;
            batch.deleted_until = edit.range.end;
            set_leaf(node, leaf, summary);
            return Change::Keep;
        }
        if let Leaf::Flat(original) = old
            && let Some(edit) = batch.edits.get(batch.next)
        {
            let one = batch.edits.get(batch.next + 1).is_none_or(|e| {
                e.range.start >= end && !(end == batch.len && e.range.start == end)
            });
            if one
                && batch.deleted_until <= start
                && edit.range.start >= start
                && edit.range.end <= end
                && edit.text.len() <= 32
            {
                let range = edit.range.start - start..edit.range.end - start;
                let removed = original.removed(range.clone(), node.summary);
                let summary = node
                    .summary
                    .minus(removed)
                    .plus(inserted_summary(&mut batch.inserted, edit.text));
                if summary.bytes <= CHUNK && summary.bytes != 0 {
                    let leaf =
                        Leaf::small(node, range, edit.text).expect("flat inline replacement");
                    batch.deleted_until = edit.range.end;
                    batch.next += 1;
                    set_leaf(node, leaf, summary);
                    return Change::Keep;
                }
            }
        }
        let count = batch.edits[batch.next..]
            .partition_point(|e| e.range.start < end || (end == batch.len && e.range.start == end));
        let local = &batch.edits[batch.next..batch.next + count];
        let insert_bytes = local.iter().map(|e| e.text.len()).sum::<usize>();
        let new_len = node.summary.bytes + insert_bytes;
        if batch.deleted_until <= start
            && new_len <= EDIT_CHUNK
            && (!matches!(old, Leaf::Flat(_)) || local.len() <= 16)
            && let Some(leaf) = Spliced::insertions(node, local, start, &mut batch.pieces)
        {
            let summary = match &leaf {
                Leaf::Spliced(s) => s.pieces.last().unwrap().end,
                _ => unreachable!(),
            };
            batch.next += count;
            batch.deleted_until = local.last().unwrap().range.end;
            set_leaf(node, leaf, summary);
            return Change::Keep;
        }
        let owned;
        let old = match old {
            Leaf::Flat(s) => &**s,
            _ => {
                owned = old.flatten();
                &owned
            }
        };
        let mut out = String::with_capacity(old.len() + insert_bytes);
        let mut at = batch.deleted_until.saturating_sub(start).min(old.len());
        let mut summary = if at == 0 {
            node.summary
        } else {
            node.summary.minus(Summary::of(&old[..at]))
        };
        while let Some(edit) = batch.edits.get(batch.next) {
            if edit.range.start >= end && !(end == batch.len && edit.range.start == end) {
                break;
            }
            let until = edit.range.start.saturating_sub(start);
            out.push_str(&old[at..until]);
            out.push_str(edit.text);
            batch.deleted_until = edit.range.end;
            at = edit.range.end.saturating_sub(start).min(old.len());
            if until != at {
                summary = summary.minus(Summary::of(&old[until..at]));
            }
            summary = summary.plus(inserted_summary(&mut batch.inserted, edit.text));
            batch.next += 1;
        }
        out.push_str(&old[at..]);
        if out.is_empty() {
            return Change::Remove;
        }
        if out.len() <= CHUNK {
            set_leaf(node, Leaf::Flat(Flat::new(out, summary.newlines)), summary);
            Change::Keep
        } else {
            let mut builder = Builder::default();
            builder.push(&out);
            let mut nodes = builder.leaves().into_iter();
            *node = nodes.next().expect("nonempty replacement");
            Change::Split(nodes.collect())
        }
    } else {
        let unique = Arc::make_mut(node);
        let Kind::Branch(children) = &mut unique.kind else {
            unreachable!()
        };
        let original_count = children.len();
        let mut i = 0;
        let mut offset = start;
        let mut sum = Summary::default();
        let mut structural = false;
        for _ in 0..original_count {
            let len = children[i].node.summary.bytes;
            let change = if batch.touches(offset, offset + len) {
                edit_node(&mut children[i].node, offset, batch)
            } else {
                Change::Keep
            };
            offset += len;
            match change {
                Change::Keep => {
                    sum = sum.plus(children[i].node.summary);
                    children[i].end = sum;
                    i += 1;
                    structural |= children[i - 1].node.occupancy() < children[i - 1].node.minimum();
                }
                Change::Remove => {
                    children.remove(i);
                    structural = true;
                }
                Change::Split(extra) => {
                    sum = sum.plus(children[i].node.summary);
                    children[i].end = sum;
                    i += 1;
                    let count = extra.len();
                    children.splice(
                        i..i,
                        extra.into_iter().map(|node| Child {
                            node,
                            end: Summary::default(),
                        }),
                    );
                    for child in &mut children[i..i + count] {
                        sum = sum.plus(child.node.summary);
                        child.end = sum;
                    }
                    i += count;
                    structural = true;
                }
            }
        }
        unique.summary = sum;
        if !structural {
            return Change::Keep;
        }
        let mut nodes = std::mem::take(children)
            .into_iter()
            .map(|c| c.node)
            .collect();
        repair(&mut nodes);
        if nodes.is_empty() {
            return Change::Remove;
        }
        let mut grouped = groups(nodes, FANOUT).into_iter();
        let first = grouped.next().expect("nonempty branch");
        let mut sum = Summary::default();
        *children = first
            .into_iter()
            .map(|node| {
                sum = sum.plus(node.summary);
                Child { node, end: sum }
            })
            .collect();
        unique.summary = sum;
        let extra: Vec<_> = grouped.map(Node::branch).collect();
        if extra.is_empty() {
            Change::Keep
        } else {
            Change::Split(extra)
        }
    }
}

fn set_leaf(node: &mut Arc<Node>, leaf: Leaf, summary: Summary) {
    if Arc::strong_count(node) == 1
        && let Some(unique) = Arc::get_mut(node)
    {
        unique.kind = Kind::Leaf(leaf);
        unique.summary = summary;
    } else {
        *node = Arc::new(Node {
            summary,
            height: 0,
            kind: Kind::Leaf(leaf),
        });
    }
}

/// The common one-leaf edit updates summaries by a delta and retains node allocations.
fn single(node: &mut Arc<Node>, start: usize, edit: &crate::Edit<'_>, is_root: bool) -> bool {
    match &node.kind {
        Kind::Leaf(old) => {
            let a = edit.range.start - start;
            let b = edit.range.end - start;
            let len = old.len() - (b - a) + edit.text.len();
            let maximum = if matches!(old, Leaf::Spliced(_)) {
                EDIT_CHUNK
            } else {
                CHUNK
            };
            if len > maximum || (!is_root && len < CHUNK / 2) {
                return false;
            }
            if matches!(old, Leaf::Patched(_))
                && a == b
                && let Some(leaf) = old.extend(a, edit.text)
            {
                let summary = node.summary.plus(Summary::of(edit.text));
                set_leaf(node, leaf, summary);
                return true;
            }
            if matches!(old, Leaf::Spliced(_))
                && let Some(leaf) = Spliced::insertions(
                    node,
                    std::slice::from_ref(edit),
                    start,
                    &mut InsertionCache::default(),
                )
            {
                let Leaf::Spliced(s) = &leaf else {
                    unreachable!()
                };
                let summary = s.pieces.last().unwrap().end;
                set_leaf(node, leaf, summary);
                return true;
            }
            if len > CHUNK {
                return false;
            }
            let owned;
            let original = match old {
                Leaf::Flat(s) => &**s,
                _ => {
                    owned = old.flatten();
                    &owned
                }
            };
            let removed = match old {
                Leaf::Flat(flat) => flat.removed(a..b, node.summary),
                _ => Summary::of(&original[a..b]),
            };
            let summary = node.summary.minus(removed).plus(Summary::of(edit.text));
            let leaf = if let Some(leaf) = Leaf::small(node, a..b, edit.text) {
                leaf
            } else {
                let mut s = String::with_capacity(len);
                s.push_str(&original[..a]);
                s.push_str(edit.text);
                s.push_str(&original[b..]);
                Leaf::Flat(Flat::new(s, summary.newlines))
            };
            set_leaf(node, leaf, summary);
            true
        }
        Kind::Branch(children) => {
            let local = edit.range.start - start;
            let i = children
                .partition_point(|c| c.end.bytes <= local)
                .min(children.len() - 1);
            if edit.range.end > start + children[i].end.bytes {
                return false;
            }
            let child_start = start + if i == 0 { 0 } else { children[i - 1].end.bytes };
            let unique = Arc::make_mut(node);
            let Kind::Branch(children) = &mut unique.kind else {
                unreachable!()
            };
            let old = children[i].node.summary;
            if !single(&mut children[i].node, child_start, edit, false) {
                return false;
            }
            let new = children[i].node.summary;
            for child in &mut children[i..] {
                child.end = child.end.minus(old).plus(new);
            }
            unique.summary = unique.summary.minus(old).plus(new);
            true
        }
    }
}

pub(crate) fn apply(root: &mut Arc<Node>, edits: &[crate::Edit<'_>]) {
    if edits.is_empty() {
        return;
    }
    if edits.len() == 1 && single(root, 0, &edits[0], true) {
        return;
    }
    let mut batch = Batch {
        edits,
        next: 0,
        deleted_until: 0,
        len: root.summary.bytes,
        inserted: None,
        pieces: InsertionCache {
            // Sparse/single edits cannot amortize a length-indexed table. The
            // ordinary persistent leaf path stays allocation-light for them.
            reuse: edits.len() >= 8,
            ..Default::default()
        },
    };
    match edit_node(root, 0, &mut batch) {
        Change::Keep => {}
        Change::Remove => *root = Node::leaf(String::new()),
        Change::Split(extra) => {
            let mut nodes = vec![root.clone()];
            nodes.extend(extra);
            *root = root_from_nodes(nodes);
        }
    }
    while let Kind::Branch(c) = &root.kind {
        if c.len() != 1 {
            break;
        }
        *root = c[0].node.clone();
    }
    assert_eq!(batch.next, edits.len(), "all validated edits consumed");
}

/// A zipper into the tree. Moving between adjacent leaves is amortized O(1).
#[derive(Clone, Debug)]
pub(crate) struct Cursor<'a> {
    pub text: &'a str,
    pub start: usize,
    leaf: &'a Leaf,
    part: usize,
    path: Vec<(&'a [Child], usize)>,
}

impl<'a> Cursor<'a> {
    pub fn at(root: &'a Node, byte: usize) -> Self {
        let mut start = 0;
        let mut path: Vec<(&'a [Child], usize)> = Vec::with_capacity(root.height as usize);
        let mut node = root;
        while let Kind::Branch(children) = &node.kind {
            let i = children
                .partition_point(|c| start + c.end.bytes <= byte)
                .min(children.len() - 1);
            if i > 0 {
                start += children[i - 1].end.bytes;
            }
            path.push((children, i));
            node = &children[i].node;
        }
        let Kind::Leaf(leaf) = &node.kind else {
            unreachable!()
        };
        let parts = leaf.parts();
        let last = parts.clone().rposition(|s| !s.is_empty()).unwrap_or(0);
        for (part, text) in parts.enumerate() {
            if !text.is_empty() && byte < start + text.len() || part == last {
                return Self {
                    text,
                    start,
                    leaf,
                    part,
                    path,
                };
            }
            start += text.len();
        }
        unreachable!()
    }
    pub fn advance(&mut self) -> bool {
        self.step(true)
    }
    pub fn backtrack(&mut self) -> bool {
        self.step(false)
    }
    // Keep dynamic fragment navigation out of the ordinary leaf walk's register
    // and stack requirements. Most loaded documents contain only flat leaves.
    #[inline(never)]
    fn step_part(&mut self, forward: bool) -> bool {
        let adjacent = if forward {
            (self.part + 1..self.leaf.part_count()).find(|&i| !self.leaf.part(i).is_empty())
        } else {
            (0..self.part).rfind(|&i| !self.leaf.part(i).is_empty())
        };
        if let Some(part) = adjacent {
            if forward {
                self.start += self.text.len();
            } else {
                self.start -= self.leaf.part(part).len();
            }
            self.text = self.leaf.part(part);
            self.part = part;
            return true;
        }
        false
    }
    fn step(&mut self, forward: bool) -> bool {
        if !matches!(self.leaf, Leaf::Flat(_)) && self.step_part(forward) {
            return true;
        }
        let Some(level) = self
            .path
            .iter()
            .rposition(|(c, i)| if forward { i + 1 < c.len() } else { *i > 0 })
        else {
            return false;
        };
        self.path.truncate(level + 1);
        let (children, index) = self.path.last_mut().expect("ancestor exists");
        if forward {
            *index += 1;
        } else {
            *index -= 1;
        }
        let mut node = &children[*index].node;
        if forward {
            self.start += self.text.len();
        }
        while let Kind::Branch(c) = &node.kind {
            let i = if forward { 0 } else { c.len() - 1 };
            self.path.push((c, i));
            node = &c[i].node;
        }
        let Kind::Leaf(leaf) = &node.kind else {
            unreachable!()
        };
        let (part, text) = if let Leaf::Flat(flat) = leaf {
            (0, &**flat)
        } else {
            leaf.edge_part(forward)
        };
        self.text = text;
        self.leaf = leaf;
        self.part = part;
        if !forward {
            self.start -= self.text.len();
        }
        true
    }
}

#[cfg(test)]
pub(crate) fn assert_invariants(root: &Node) {
    fn check(node: &Node, root: bool) -> Summary {
        match &node.kind {
            Kind::Leaf(s) => {
                assert_eq!(node.height, 0);
                if let Leaf::Flat(flat) = s {
                    assert_eq!(
                        (0..flat.newlines.len())
                            .map(|i| flat.newlines.at(i))
                            .collect::<Vec<_>>(),
                        memchr::memchr_iter(b'\n', flat.as_bytes()).collect::<Vec<_>>()
                    );
                }
                assert!(
                    s.len()
                        <= if matches!(s, Leaf::Spliced(_)) {
                            EDIT_CHUNK
                        } else {
                            CHUNK
                        }
                );
                // A scalar can straddle the ideal midpoint by at most three bytes.
                assert!(
                    root || s.len() >= CHUNK / 2 - 3,
                    "underfull leaf: {}",
                    s.len()
                );
                assert_eq!(
                    node.summary,
                    s.parts()
                        .fold(Summary::default(), |sum, part| sum.plus(Summary::of(part)))
                );
            }
            Kind::Branch(c) => {
                assert!(!c.is_empty() && c.len() <= FANOUT);
                assert!(if root {
                    c.len() >= 2
                } else {
                    c.len() >= FANOUT / 2
                });
                let mut sum = Summary::default();
                for child in c {
                    assert_eq!(child.node.height + 1, node.height);
                    sum = sum.plus(check(&child.node, false));
                    assert_eq!(child.end, sum);
                }
                assert_eq!(node.summary, sum);
            }
        }
        node.summary
    }
    check(root, true);
}

#[cfg(test)]
#[test]
fn merged_multibyte_leaf_split_seed_1000493() {
    // The review seed exposed a 130-byte flat leaf at batch 27. Exercise
    // every midpoint alignment as well as the 5,000-seed String model.
    for leading in 0..4 {
        for length in CHUNK + 1..=CHUNK * 2 {
            let mut content = "x".repeat(leading);
            while content.len() + 4 <= length {
                content.push('🏽');
            }
            content.push_str(&"y".repeat(length - content.len()));
            let split = content.floor_char_boundary(CHUNK / 2 - 4);
            let mut leaves = vec![
                Node::leaf(content[..split].into()),
                Node::leaf(content[split..].into()),
            ];
            repair(&mut leaves);
            for leaf in &leaves {
                assert_invariants(leaf);
                assert!(leaf.summary.bytes <= CHUNK);
            }
            let joined = leaves
                .iter()
                .flat_map(|leaf| match &leaf.kind {
                    Kind::Leaf(value) => value.parts(),
                    _ => unreachable!(),
                })
                .collect::<String>();
            assert_eq!(joined, content);
        }
    }
}
