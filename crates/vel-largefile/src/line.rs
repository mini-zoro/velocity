//! Snapshot-bound, chunked long-line metrics. Index construction belongs off the UI thread.
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_text::Text;

const BLOCK: usize = 4096;
/// Editor word classes. Combining sequences use the class of their first scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordClass {
    /// Unicode alphanumeric or underscore.
    Word,
    /// Unicode whitespace.
    Space,
    /// Other punctuation/symbols.
    Punctuation,
}
fn class(grapheme: &str) -> WordClass {
    let c = grapheme.chars().next().expect("nonempty grapheme");
    if c.is_alphanumeric() || c == '_' {
        WordClass::Word
    } else if c.is_whitespace() {
        WordClass::Space
    } else {
        WordClass::Punctuation
    }
}
fn advance(column: usize, grapheme: &str, tab: usize) -> usize {
    if grapheme == "\t" {
        column + tab - column % tab
    } else {
        column + UnicodeWidthStr::width(grapheme)
    }
}
#[derive(Debug)]
struct Block {
    bytes: usize,
    graphemes: usize,
    // Width transform: a tab rounds the input column once. Everything after the
    // first tab is independent of that input's residue.
    first_tab: Option<usize>,
    width: usize,
    simple_ascii: bool,
    uniform: Option<WordClass>,
    byte: usize,
    grapheme: usize,
    column: usize,
    end_column: usize,
    run_start: usize,
    run_end: usize,
}
impl Block {
    fn end_column(&self, column: usize, tab: usize) -> usize {
        match self.first_tab {
            Some(before) => {
                let n = column + before;
                n + tab - n % tab + self.width
            }
            None => column + self.width,
        }
    }
}
/// Optional index for one LF line of a persistent Text snapshot (LF excluded).
///
/// Queries binary-search ~4 KiB blocks then inspect at most one block. Bytes and
/// grapheme columns are line-local; display columns use unicode-width (ambiguous
/// width 1) and configurable tab stops. This is terminal-cell geometry, not font
/// shaping or bidi layout. No per-character array or second full text copy is kept.
/// Updates rebuild nearby blocks and their compact prefix summaries. Unbounded
/// Unicode clusters/RI context may require rebuilding a larger affected region.
#[derive(Debug)]
pub struct LongLineIndex {
    text: Text,
    row: usize,
    base: usize,
    bytes: usize,
    graphemes: usize,
    columns: usize,
    tab: usize,
    blocks: Vec<Block>,
}
impl LongLineIndex {
    /// Builds metrics for an existing line. Tab size is clamped to at least one.
    /// Scans the line on a worker; unbounded Unicode context can add work.
    pub fn new(text: Text, row: usize, tab_size: usize) -> Self {
        let base = text.line_to_byte(row);
        let bytes = text.line_len_bytes(row);
        let tab = tab_size.max(1);
        let blocks = Self::build(&text, base, 0..bytes, tab);
        let mut result = Self {
            text,
            row,
            base,
            bytes,
            graphemes: 0,
            columns: 0,
            tab,
            blocks,
        };
        result.summarize();
        result
    }
    fn build(text: &Text, base: usize, range: Range<usize>, tab: usize) -> Vec<Block> {
        let view = text.slice(base + range.start..base + range.end);
        let mut at = 0;
        let mut blocks = Vec::new();
        while at < view.len_bytes() {
            let target = view.floor_char_boundary((at + BLOCK).min(view.len_bytes()));
            let mut end = if target == view.len_bytes() {
                target
            } else {
                view.next_grapheme_boundary(target)
            };
            // Isolate an exceptionally large cluster, so every multi-cluster
            // block remains bounded and queries never copy that cluster.
            if end - at > 2 * BLOCK {
                let before = view.prev_grapheme_boundary(target);
                if before > at {
                    end = before;
                }
            }
            let chunk = view.slice(at..end).to_string();
            let simple_ascii = chunk.bytes().all(|b| (0x20..=0x7e).contains(&b));
            let mut first_tab = None;
            let mut width = 0;
            let mut uniform = chunk.graphemes(true).next().map(class);
            let mut graphemes = 0;
            if simple_ascii {
                graphemes = chunk.len();
                width = chunk.len();
                if !chunk.graphemes(true).all(|g| Some(class(g)) == uniform) {
                    uniform = None;
                }
            } else {
                for g in chunk.graphemes(true) {
                    graphemes += 1;
                    if Some(class(g)) != uniform {
                        uniform = None;
                    }
                    if g == "\t" && first_tab.is_none() {
                        first_tab = Some(width);
                        width = 0;
                    } else {
                        width = advance(width, g, tab);
                    }
                }
            }
            blocks.push(Block {
                bytes: end - at,
                graphemes,
                first_tab,
                width,
                simple_ascii,
                uniform,
                byte: 0,
                grapheme: 0,
                column: 0,
                end_column: 0,
                run_start: 0,
                run_end: 0,
            });
            at = end;
        }
        blocks
    }
    fn summarize(&mut self) {
        let (mut byte, mut grapheme, mut column) = (0, 0, 0);
        for i in 0..self.blocks.len() {
            let same = i != 0
                && self.blocks[i].uniform.is_some()
                && self.blocks[i].uniform == self.blocks[i - 1].uniform;
            let run_start = if same {
                self.blocks[i - 1].run_start
            } else {
                i
            };
            let b = &mut self.blocks[i];
            b.byte = byte;
            b.grapheme = grapheme;
            b.column = column;
            byte += b.bytes;
            grapheme += b.graphemes;
            column = b.end_column(column, self.tab);
            b.end_column = column;
            b.run_start = run_start;
        }
        for i in (0..self.blocks.len()).rev() {
            let same = i + 1 < self.blocks.len()
                && self.blocks[i].uniform.is_some()
                && self.blocks[i].uniform == self.blocks[i + 1].uniform;
            self.blocks[i].run_end = if same {
                self.blocks[i + 1].run_end
            } else {
                i + 1
            };
        }
        self.bytes = byte;
        self.graphemes = grapheme;
        self.columns = column;
    }
    fn block_at(&self, byte: usize) -> usize {
        self.blocks
            .partition_point(|b| b.byte <= byte)
            .saturating_sub(1)
    }
    fn check(&self, byte: usize) {
        assert!(
            byte <= self.bytes && self.text.is_char_boundary(self.base + byte),
            "line byte is out of range or not a scalar boundary"
        );
    }
    fn contents(&self, b: &Block) -> String {
        self.text
            .slice(self.base + b.byte..self.base + b.byte + b.bytes)
            .to_string()
    }
    /// Snapshot corresponding to the index. Returned edits can be installed into Buffer.
    pub fn text(&self) -> &Text {
        &self.text
    }
    /// Indexed LF row.
    pub fn row(&self) -> usize {
        self.row
    }
    /// Absolute start byte; add it to line-local byte results.
    pub fn line_start(&self) -> usize {
        self.base
    }
    /// Line end in bytes, in O(1).
    pub fn len_bytes(&self) -> usize {
        self.bytes
    }
    /// Whether the line body is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes == 0
    }
    /// Number of extended grapheme clusters, in O(1).
    pub fn len_graphemes(&self) -> usize {
        self.graphemes
    }
    /// Display width in cells with configured tab stops, in O(1).
    pub fn display_width(&self) -> usize {
        self.columns
    }
    /// Grapheme column at a scalar-aligned byte; rounds a cluster's interior down.
    pub fn byte_to_grapheme(&self, byte: usize) -> usize {
        self.check(byte);
        if byte == self.bytes {
            return self.graphemes;
        }
        let b = &self.blocks[self.block_at(byte)];
        if b.graphemes == 1 {
            return b.grapheme;
        }
        if b.simple_ascii {
            return b.grapheme + byte - b.byte;
        }
        b.grapheme
            + self
                .contents(b)
                .grapheme_indices(true)
                .take_while(|(i, g)| b.byte + i + g.len() <= byte)
                .count()
    }
    /// Byte at a grapheme column (including EOF); columns beyond EOF are clipped.
    pub fn grapheme_to_byte(&self, column: usize) -> usize {
        if column >= self.graphemes {
            return self.bytes;
        }
        let i = self
            .blocks
            .partition_point(|b| b.grapheme <= column)
            .saturating_sub(1);
        let b = &self.blocks[i];
        if b.graphemes == 1 {
            return b.byte;
        }
        if b.simple_ascii {
            return b.byte + column - b.grapheme;
        }
        b.byte
            + self
                .contents(b)
                .grapheme_indices(true)
                .nth(column - b.grapheme)
                .expect("grapheme summary")
                .0
    }
    /// Display column before the grapheme containing this byte.
    pub fn byte_to_column(&self, byte: usize) -> usize {
        self.check(byte);
        if byte == self.bytes {
            return self.columns;
        }
        let b = &self.blocks[self.block_at(byte)];
        if b.graphemes == 1 {
            return b.column;
        }
        if b.simple_ascii {
            return b.column + byte - b.byte;
        }
        let mut column = b.column;
        for (i, g) in self.contents(b).grapheme_indices(true) {
            if b.byte + i + g.len() > byte {
                break;
            }
            column = advance(column, g, self.tab);
        }
        column
    }
    /// Byte at/before a display column; tabs/wide clusters round to their start.
    pub fn column_to_byte(&self, column: usize) -> usize {
        if column >= self.columns {
            return self.bytes;
        }
        let b = &self.blocks[self.blocks.partition_point(|b| b.end_column <= column)];
        if b.graphemes == 1 {
            return b.byte;
        }
        if b.simple_ascii {
            return b.byte + column - b.column;
        }
        let mut at = b.column;
        for (i, g) in self.contents(b).grapheme_indices(true) {
            let end = advance(at, g, self.tab);
            if column < end {
                return b.byte + i;
            }
            at = end;
        }
        b.byte + b.bytes
    }
    /// Grapheme-aligned byte window covering the requested horizontal cells.
    pub fn visible_window(&self, first_column: usize, columns: usize) -> Range<usize> {
        let start = self.column_to_byte(first_column);
        if columns == 0 {
            return start..start;
        }
        let last = first_column.saturating_add(columns);
        let mut end = self.column_to_byte(last);
        if end < self.bytes && self.byte_to_column(end) < last {
            end = self.next_grapheme_boundary(end);
        }
        start..end
    }
    /// Next indexed grapheme boundary or line end.
    pub fn next_grapheme_boundary(&self, byte: usize) -> usize {
        self.grapheme_to_byte(self.byte_to_grapheme(byte).saturating_add(1))
    }
    /// Previous indexed grapheme boundary or line start.
    pub fn prev_grapheme_boundary(&self, byte: usize) -> usize {
        let g = self.byte_to_grapheme(byte);
        let start = self.grapheme_to_byte(g);
        if start < byte {
            start
        } else {
            self.grapheme_to_byte(g.saturating_sub(1))
        }
    }
    /// End of the current word-class run. Uniform blocks are skipped in O(1).
    pub fn next_word_boundary(&self, byte: usize) -> usize {
        self.check(byte);
        if byte == self.bytes {
            return byte;
        }
        let mut i = self.block_at(byte);
        let mut wanted = None;
        while i < self.blocks.len() {
            let b = &self.blocks[i];
            if let Some(kind) = b.uniform {
                if wanted.is_some_and(|w| w != kind) {
                    return b.byte;
                }
                wanted = Some(kind);
                i = b.run_end;
                continue;
            }
            for (at, g) in self.contents(b).grapheme_indices(true) {
                if b.byte + at + g.len() <= byte {
                    continue;
                }
                let kind = class(g);
                if wanted.is_some_and(|w| w != kind) {
                    return b.byte + at;
                }
                wanted = Some(kind);
            }
            i += 1;
        }
        self.bytes
    }
    /// Start of the preceding/current word-class run; uniform blocks are skipped.
    pub fn prev_word_boundary(&self, byte: usize) -> usize {
        self.check(byte);
        if byte == 0 {
            return 0;
        }
        let mut i = self.block_at(byte - 1);
        let mut wanted = None;
        let mut result = byte;
        loop {
            let b = &self.blocks[i];
            if let Some(kind) = b.uniform {
                if wanted.is_some_and(|w| w != kind) {
                    return result;
                }
                wanted = Some(kind);
                result = self.blocks[b.run_start].byte;
                if b.run_start == 0 {
                    return 0;
                }
                i = b.run_start - 1;
                continue;
            }
            for (at, g) in self.contents(b).grapheme_indices(true).rev() {
                if b.byte + at >= byte {
                    continue;
                }
                let kind = class(g);
                if wanted.is_some_and(|w| w != kind) {
                    return result;
                }
                wanted = Some(kind);
                result = b.byte + at;
            }
            if i == 0 {
                return 0;
            }
            i -= 1;
        }
    }
    /// Applies a scalar-aligned line-local replacement to both snapshot and index.
    /// LF insertions are rejected before mutation. Ordinary edits rebuild ~3 blocks
    /// and O(block count) prefix metadata; Unicode context is extended until an
    /// unchanged grapheme boundary is reached, including long regional-indicator runs.
    /// Keep this object on the buffer's edit owner; it is tied to its Text snapshot.
    pub fn replace(&mut self, range: Range<usize>, inserted: &str) {
        self.check(range.start);
        self.check(range.end);
        assert!(
            range.start <= range.end && !inserted.contains('\n'),
            "reversed edit or LF in a single-line edit"
        );
        let mut after = self.text.clone();
        after.replace(self.base + range.start..self.base + range.end, inserted);
        self.update_after_edit(after, range, inserted.len());
    }
    /// Updates metrics after the edit owner has already committed one line-local
    /// replacement. `after` must be the result of exactly that edit on text().
    /// This avoids performing the Buffer transaction twice. Newline changes,
    /// edits before this line and tab-size changes require constructing a new index.
    /// Panics on inconsistent sizes/line boundaries before changing the index.
    pub fn update_after_edit(&mut self, after: Text, range: Range<usize>, inserted_bytes: usize) {
        self.check(range.start);
        self.check(range.end);
        assert!(range.start <= range.end, "reversed edit");
        assert_eq!(
            after.len_bytes(),
            self.text.len_bytes() - range.len() + inserted_bytes,
            "snapshot length mismatch"
        );
        assert_eq!(
            after.len_lines(),
            self.text.len_lines(),
            "line count changed"
        );
        assert_eq!(
            after.line_to_byte(self.row),
            self.base,
            "indexed line moved"
        );
        assert_eq!(
            after.line_len_bytes(self.row),
            self.bytes - range.len() + inserted_bytes,
            "indexed line length mismatch"
        );
        let lo = self.block_at(range.start).saturating_sub(1);
        let mut hi = (self.block_at(range.end) + 2).min(self.blocks.len());
        let start = self.blocks.get(lo).map_or(0, |b| b.byte);
        let old_end = |blocks: &[Block], hi: usize| blocks.get(hi).map_or(self.bytes, |b| b.byte);
        let mut end = old_end(&self.blocks, hi) - range.len() + inserted_bytes;
        self.text = after;
        let new_bytes = self.bytes - range.len() + inserted_bytes;
        // A boundary inside an RI run can shift arbitrarily far after one insertion.
        while end < new_bytes {
            let after = self.text.ceil_char_boundary(self.base + end + 1);
            if self.text.prev_grapheme_boundary(after) == self.base + end {
                break;
            }
            hi += 1;
            end = old_end(&self.blocks, hi) - range.len() + inserted_bytes;
        }
        let blocks = Self::build(&self.text, self.base, start..end, self.tab);
        self.blocks.splice(lo..hi, blocks);
        self.summarize();
    }
}
