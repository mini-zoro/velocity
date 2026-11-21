//! Persistent UTF-8 text with byte, scalar, UTF-16 and LF-line coordinates.
//!
//! Snapshots share a B-tree; edits copy only affected paths and leaves. Strict
//! coordinates must be in bounds and on scalar boundaries. Points exclude LF.
//!
//! ```
//! use vel_text::{Edit, Point, Text};
//! let mut text = Text::from("hello\n世界🦀");
//! let snapshot = text.clone();
//! text.apply_edits(&[Edit { range: 0..5, text: "hi" }]);
//! assert_eq!(text.point_to_byte(Point { row: 1, column: 6 }), 9);
//! assert_eq!(snapshot.len_utf16(), 10);
//! ```

#![forbid(unsafe_code)]

#[cfg(feature = "regex-cursor")]
mod regex;
mod tree;
#[cfg(feature = "regex-cursor")]
pub use regex::ChunkCursor;

use std::{
    fmt,
    hash::{Hash, Hasher},
    io,
    ops::Range,
    sync::Arc,
};
use tree::{Metric, Node, Summary};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

/// A zero-based LF row and byte column, excluding the row's terminating LF.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    /// Zero-based line index.
    pub row: usize,
    /// Byte offset within the line, on a scalar boundary.
    pub column: usize,
}

/// A zero-based LF row and UTF-16 column, on a scalar boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PointUtf16 {
    /// Zero-based line index.
    pub row: usize,
    /// Number of UTF-16 code units preceding the position in its line.
    pub column: usize,
}

/// One replacement, expressed in the coordinates before a batch starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit<'a> {
    /// Half-open byte range to replace; endpoints must be scalar boundaries.
    pub range: Range<usize>,
    /// Valid UTF-8 replacement, possibly empty.
    pub text: &'a str,
}

/// A persistent B-tree rope containing valid UTF-8. Cloning is O(1).
#[derive(Clone)]
pub struct Text {
    root: Arc<Node>,
}

/// A borrowed, zero-copy byte range. All coordinates are relative to the view.
#[derive(Clone, Copy)]
pub struct TextSlice<'a> {
    text: &'a Text,
    start: Summary,
    end: Summary,
}

impl Text {
    /// Creates an empty text (one empty line).
    pub fn new() -> Self {
        Self::from_str("")
    }
    /// Copies UTF-8 into a balanced tree.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        let mut builder = tree::Builder::default();
        builder.push(s);
        Self {
            root: builder.finish(),
        }
    }
    /// Loads with a bounded pipeline of UTF-8 leaf builders (up to four workers).
    /// At most 8 MiB of input is in flight, never a second complete input copy.
    /// Invalid or truncated UTF-8 returns [`io::ErrorKind::InvalidData`].
    pub fn from_reader(r: impl io::Read) -> io::Result<Self> {
        Ok(Self {
            root: tree::from_reader_parallel(r)?,
        })
    }
    /// Loads through a single bounded worker pipeline, publishing immutable prefixes.
    /// The first completed 2 MiB block is published immediately; later publications
    /// follow the byte/time cadence (checked between completed blocks), plus EOF.
    /// Callback errors stop loading and join the bounded workers. Invalid/truncated
    /// UTF-8 is InvalidData, including errors after already published prefixes.
    /// Readers need not be Send; callback snapshots share their complete subtrees.
    pub fn from_reader_snapshots(
        reader: impl io::Read,
        publish_bytes: usize,
        interval: std::time::Duration,
        mut publish: impl FnMut(Self) -> io::Result<()>,
    ) -> io::Result<Self> {
        let root = tree::from_reader_snapshots(reader, publish_bytes, interval, |root| {
            publish(Self { root })
        })?;
        Ok(Self { root })
    }
    /// Loads on the calling thread, with a 1 MiB staging buffer.
    /// Suitable for small streams or callers already managing a thread pool.
    /// Invalid or truncated UTF-8 returns [`io::ErrorKind::InvalidData`].
    pub fn from_reader_serial(r: impl io::Read) -> io::Result<Self> {
        Ok(Self {
            root: tree::from_reader(r)?,
        })
    }
    /// Borrows the complete text without copying content.
    pub fn as_slice(&self) -> TextSlice<'_> {
        TextSlice {
            text: self,
            start: Summary::default(),
            end: self.root.summary,
        }
    }
    /// Estimates reachable heap storage, counting shared allocations once.
    /// Includes spare String/Vec capacity, LF indexes, retained original leaves
    /// and Arc headers. Excludes allocator bookkeeping, size-class rounding and
    /// temporary accounting memory. Runs in O(nodes), for diagnostics only.
    pub fn estimated_storage_bytes(&self) -> usize {
        tree::storage_bytes(&self.root)
    }
    /// Heap payload of the leaf LF indexes, already included in the total
    /// storage estimate. Inline node summaries remain part of rope storage.
    pub fn line_index_bytes(&self) -> usize {
        tree::line_index_bytes(&self.root)
    }
    /// Estimates allocations retained by this snapshot but absent from `current`.
    /// Counts shared nodes (including inline edits' original leaves) only once.
    /// Like [`Self::estimated_storage_bytes`], excludes allocator bookkeeping.
    /// Equal roots return immediately; otherwise runs in O(nodes) with scratch
    /// storage. Intended for history accounting outside ordinary typing paths.
    pub fn estimated_retained_bytes(&self, current: &Self) -> usize {
        tree::storage_excluding(&self.root, Some(&current.root))
    }
    /// Replaces a scalar-aligned byte range; invalid ranges panic before mutation.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        self.apply_edits(&[Edit { range, text }]);
    }
    /// Inserts UTF-8 at a scalar boundary.
    pub fn insert(&mut self, at: usize, text: &str) {
        self.replace(at..at, text);
    }
    /// Deletes a scalar-aligned byte range.
    pub fn delete(&mut self, range: Range<usize>) {
        self.replace(range, "");
    }
    /// Applies sorted, non-overlapping edits in original coordinates in one traversal.
    /// Touching ranges and repeated insertion positions are allowed; equal insertion
    /// positions retain input order. Invalid input panics before mutation.
    pub fn apply_edits(&mut self, edits: &[Edit<'_>]) {
        let mut previous_end = 0;
        let ascii = self.root.summary.bytes == self.root.summary.chars;
        for edit in edits {
            assert!(
                edit.range.start >= previous_end,
                "edits must be sorted and non-overlapping"
            );
            assert!(
                edit.range.start <= edit.range.end && edit.range.end <= self.len_bytes(),
                "edit byte range is reversed or out of bounds"
            );
            previous_end = edit.range.end;
        }
        if !ascii {
            tree::validate_boundaries(&self.root, edits);
        }
        tree::apply(&mut self.root, edits);
    }
}

impl<'a> TextSlice<'a> {
    fn absolute(&self, byte: usize) -> usize {
        assert!(
            byte <= self.len_bytes(),
            "byte offset {byte} exceeds slice length {}",
            self.len_bytes()
        );
        self.start.bytes + byte
    }
    fn prefix(&self, byte: usize) -> Summary {
        tree::prefix(&self.text.root, self.absolute(byte))
    }
    /// Number of UTF-8 bytes.
    pub fn len_bytes(&self) -> usize {
        self.end.bytes - self.start.bytes
    }
    /// Number of Unicode scalar values.
    pub fn len_chars(&self) -> usize {
        self.end.chars - self.start.chars
    }
    /// Number of UTF-16 code units.
    pub fn len_utf16(&self) -> usize {
        self.end.utf16 - self.start.utf16
    }
    /// Number of LF separators plus one, including an empty final line.
    pub fn len_lines(&self) -> usize {
        self.end.newlines - self.start.newlines + 1
    }
    /// Whether the view is empty.
    pub fn is_empty(&self) -> bool {
        self.len_bytes() == 0
    }
    /// Converts a scalar-aligned byte offset to a scalar offset in O(log n).
    pub fn byte_to_char(&self, byte: usize) -> usize {
        let absolute = self.absolute(byte);
        if self.len_bytes() == self.len_chars() {
            return byte;
        }
        let (s, before) = tree::seek(&self.text.root, absolute, Metric::Byte);
        before.chars
            + s.get(..absolute - before.bytes)
                .expect("byte offset is not a scalar boundary")
                .chars()
                .count()
            - self.start.chars
    }
    /// Converts a scalar offset (including EOF) to bytes in O(log n).
    pub fn char_to_byte(&self, ch: usize) -> usize {
        assert!(ch <= self.len_chars(), "scalar offset exceeds slice length");
        if self.len_bytes() == self.len_chars() {
            return ch;
        }
        tree::metric_to_byte(&self.text.root, self.start.chars + ch, Metric::Char)
            - self.start.bytes
    }
    /// Converts scalar-aligned bytes to UTF-16 in O(log n).
    pub fn byte_to_utf16(&self, byte: usize) -> usize {
        let absolute = self.absolute(byte);
        if self.len_bytes() == self.len_chars() {
            return byte;
        }
        let (s, before) = tree::seek(&self.text.root, absolute, Metric::Byte);
        before.utf16
            + Summary::fragment(
                s.get(..absolute - before.bytes)
                    .expect("byte offset is not a scalar boundary"),
            )
            .utf16
            - self.start.utf16
    }
    /// Converts UTF-16 to bytes; panics inside a surrogate pair or out of bounds.
    pub fn utf16_to_byte(&self, u: usize) -> usize {
        assert!(u <= self.len_utf16(), "UTF-16 offset exceeds slice length");
        if self.len_bytes() == self.len_chars() {
            return u;
        }
        tree::metric_to_byte(&self.text.root, self.start.utf16 + u, Metric::Utf16)
            - self.start.bytes
    }
    /// Finds the LF row containing a scalar-aligned byte offset in O(log n).
    pub fn byte_to_line(&self, byte: usize) -> usize {
        tree::byte_line(
            &self.text.root,
            self.absolute(byte),
            self.len_bytes() == self.len_chars(),
        ) - self.start.newlines
    }
    /// Start of an existing line; `len_lines()` is out of bounds.
    pub fn line_to_byte(&self, line: usize) -> usize {
        assert!(
            line < self.len_lines(),
            "line index exceeds slice's last line"
        );
        if line == 0 {
            0
        } else {
            tree::line_to_byte(&self.text.root, self.start.newlines + line) - self.start.bytes
        }
    }
    /// Converts scalar-aligned bytes to an LF row and byte column.
    pub fn byte_to_point(&self, byte: usize) -> Point {
        let absolute = self.absolute(byte);
        let (row, column) = tree::byte_point(
            &self.text.root,
            absolute,
            self.len_bytes() == self.len_chars(),
        );
        Point {
            row: row - self.start.newlines,
            column: absolute - (absolute - column).max(self.start.bytes),
        }
    }
    /// Converts an LF/byte point to bytes; columns beyond line content panic.
    pub fn point_to_byte(&self, p: Point) -> usize {
        if self.len_bytes() == self.len_chars() {
            let (start, end) = self.line_bounds(p.row);
            assert!(p.column <= end - start, "byte column exceeds line content");
            return start + p.column;
        }
        let (start, window) = self.line_window(p.row);
        let len = memchr::memchr(b'\n', window.as_bytes()).unwrap_or_else(|| {
            if p.row + 1 == self.len_lines() {
                self.len_bytes() - start
            } else {
                self.line_to_byte(p.row + 1) - start - 1
            }
        });
        assert!(p.column <= len, "byte column exceeds line content");
        assert!(
            if p.column <= window.len() {
                window.is_char_boundary(p.column)
            } else {
                self.is_char_boundary(start + p.column)
            },
            "byte column is not a UTF-8 scalar boundary"
        );
        start + p.column
    }
    /// Converts scalar-aligned bytes to an LF row and UTF-16 column.
    pub fn byte_to_point_utf16(&self, byte: usize) -> PointUtf16 {
        let p = self.byte_to_point(byte);
        if self.len_bytes() == self.len_chars() {
            return PointUtf16 {
                row: p.row,
                column: p.column,
            };
        }
        PointUtf16 {
            row: p.row,
            column: self.byte_to_utf16(byte) - self.byte_to_utf16(byte - p.column),
        }
    }
    /// Converts an LF/UTF-16 point to bytes; invalid rows or columns panic.
    pub fn point_utf16_to_byte(&self, p: PointUtf16) -> usize {
        if self.len_bytes() == self.len_chars() {
            return self.point_to_byte(Point {
                row: p.row,
                column: p.column,
            });
        }
        let start = self.line_to_byte(p.row);
        let base = self.byte_to_utf16(start);
        let end = start + self.line_len_bytes(p.row);
        assert!(
            p.column <= self.byte_to_utf16(end) - base,
            "UTF-16 column exceeds line content"
        );
        self.utf16_to_byte(base + p.column)
    }
    /// Byte length of a line, excluding LF; CR remains ordinary content.
    pub fn line_len_bytes(&self, line: usize) -> usize {
        let (start, end) = self.line_bounds(line);
        end - start
    }
    fn line_bounds(&self, line: usize) -> (usize, usize) {
        assert!(
            line < self.len_lines(),
            "line index exceeds slice's last line"
        );
        let (start, end) = tree::line_range(&self.text.root, self.start.newlines + line);
        (
            start.max(self.start.bytes) - self.start.bytes,
            end.min(self.end.bytes) - self.start.bytes,
        )
    }
    fn line_window(&self, line: usize) -> (usize, &'a str) {
        assert!(
            line < self.len_lines(),
            "line index exceeds slice's last line"
        );
        let (mut s, mut before, mut local) = if line == 0 {
            let (s, before) = tree::seek(&self.text.root, self.start.bytes, Metric::Byte);
            (s, before, self.start.bytes - before.bytes)
        } else {
            tree::line_start_chunk(&self.text.root, self.start.newlines + line)
        };
        let start = before.bytes + local;
        if local == s.len() && start < self.end.bytes {
            (s, before) = tree::seek(&self.text.root, start, Metric::Byte);
            local = 0;
        }
        (
            start - self.start.bytes,
            &s[local..s.len().min(self.end.bytes - before.bytes)],
        )
    }
    /// Whether a byte is a scalar boundary; false outside the view.
    pub fn is_char_boundary(&self, byte: usize) -> bool {
        if byte > self.len_bytes() {
            return false;
        }
        if byte == self.len_bytes() {
            return true;
        }
        if self.len_bytes() == self.len_chars() {
            return true;
        }
        let absolute = self.start.bytes + byte;
        tree::is_char_boundary(&self.text.root, absolute)
    }
    /// Clamps to EOF and rounds down to a scalar boundary.
    pub fn floor_char_boundary(&self, byte: usize) -> usize {
        let mut byte = byte.min(self.len_bytes());
        while !self.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }
    /// Clamps to EOF and rounds up to a scalar boundary.
    pub fn ceil_char_boundary(&self, byte: usize) -> usize {
        let mut byte = byte.min(self.len_bytes());
        while !self.is_char_boundary(byte) {
            byte += 1;
        }
        byte
    }
    /// Next extended grapheme boundary (or EOF), with context across leaf edges.
    pub fn next_grapheme_boundary(&self, byte: usize) -> usize {
        self.grapheme(byte, true)
    }
    /// Previous extended grapheme boundary (or zero), with context across leaf edges.
    pub fn prev_grapheme_boundary(&self, byte: usize) -> usize {
        self.grapheme(byte, false)
    }
    fn grapheme(&self, byte: usize, forward: bool) -> usize {
        assert!(
            self.is_char_boundary(byte),
            "grapheme offset is out of bounds or not a scalar boundary"
        );
        let mut cursor = GraphemeCursor::new(byte, self.len_bytes(), true);
        let (mut chunk, mut start) = self.chunk_at_byte(byte);
        loop {
            let result = if forward {
                cursor.next_boundary(chunk, start)
            } else {
                cursor.prev_boundary(chunk, start)
            };
            match result {
                Ok(boundary) => {
                    return boundary.unwrap_or(if forward { self.len_bytes() } else { 0 });
                }
                Err(GraphemeIncomplete::NextChunk) => {
                    (chunk, start) = self.chunk_at_byte(start + chunk.len());
                }
                Err(GraphemeIncomplete::PrevChunk) => {
                    (chunk, start) = self.chunk_at_byte(start - 1);
                }
                Err(GraphemeIncomplete::PreContext(end)) => {
                    let (context, at) = self.chunk_at_byte(end - 1);
                    cursor.provide_context(&context[..end - at], at);
                }
                Err(GraphemeIncomplete::InvalidOffset) => {
                    unreachable!("validated grapheme cursor position")
                }
            }
        }
    }
    /// Borrows a scalar-aligned subrange; all resulting coordinates remain relative.
    pub fn slice(&self, range: Range<usize>) -> Self {
        assert!(range.start <= range.end, "slice byte range is reversed");
        Self {
            text: self.text,
            start: self.prefix(range.start),
            end: self.prefix(range.end),
        }
    }
    /// Borrows one line without LF.
    pub fn line(&self, line: usize) -> Self {
        let start = self.line_to_byte(line);
        self.slice(start..start + self.line_len_bytes(line))
    }
    /// Reads a scalar; EOF and non-boundaries panic.
    pub fn char_at(&self, byte: usize) -> char {
        assert!(
            byte < self.len_bytes(),
            "char_at byte offset is out of bounds"
        );
        let (s, start) = self.chunk_at_byte(byte);
        s.get(byte - start..)
            .expect("char_at byte offset is not a scalar boundary")
            .chars()
            .next()
            .expect("nonempty suffix")
    }
    /// Reads one UTF-8 byte; EOF is out of bounds.
    pub fn byte_at(&self, byte: usize) -> u8 {
        assert!(byte < self.len_bytes(), "byte_at offset is out of bounds");
        let (s, start) = self.chunk_at_byte(byte);
        s.as_bytes()[byte - start]
    }
    /// Iterates borrowed chunks in either direction, without copying content.
    pub fn chunks(&self) -> Chunks<'a> {
        Chunks::new(*self)
    }
    /// Iterates scalars in either direction.
    pub fn chars(&self) -> impl DoubleEndedIterator<Item = char> + use<'a> {
        self.chunks().flat_map(str::chars)
    }
    /// Iterates UTF-8 bytes in either direction.
    pub fn bytes(&self) -> impl DoubleEndedIterator<Item = u8> + use<'a> {
        self.chunks().flat_map(str::bytes)
    }
    /// Iterates lines without LF, including the possibly empty final line.
    pub fn lines(&self) -> Lines<'a> {
        Lines::new(*self)
    }
    /// Returns a chunk and its relative byte start; EOF returns the last chunk.
    /// Interior byte positions need not be scalar-aligned. Empty views return `("", 0)`.
    pub fn chunk_at_byte(&self, byte: usize) -> (&'a str, usize) {
        let absolute = self.absolute(byte);
        if self.is_empty() {
            return ("", 0);
        }
        let at = if byte == self.len_bytes() {
            absolute - 1
        } else {
            absolute
        };
        let (s, before) = tree::seek(&self.text.root, at, Metric::Byte);
        let low = before.bytes.max(self.start.bytes);
        let high = (before.bytes + s.len()).min(self.end.bytes);
        (
            &s[low - before.bytes..high - before.bytes],
            low - self.start.bytes,
        )
    }
    /// Clamps bytes to EOF and rounds down to a scalar boundary.
    pub fn clip_byte(&self, byte: usize) -> usize {
        self.floor_char_boundary(byte)
    }
    /// Clamps a scalar offset to EOF.
    pub fn clip_char(&self, ch: usize) -> usize {
        ch.min(self.len_chars())
    }
    /// Clamps UTF-16 to EOF, rounding down inside a surrogate pair.
    pub fn clip_utf16(&self, u: usize) -> usize {
        let u = u.min(self.len_utf16());
        let absolute = self.start.utf16 + u;
        let (s, before) = tree::seek(&self.text.root, absolute, Metric::Utf16);
        if s.is_ascii() {
            return u;
        }
        let mut units = before.utf16;
        for ch in s.chars() {
            if units + ch.len_utf16() > absolute {
                break;
            }
            units += ch.len_utf16();
        }
        units - self.start.utf16
    }
    /// Clamps a row to the last line.
    pub fn clip_line(&self, row: usize) -> usize {
        row.min(self.len_lines() - 1)
    }
    /// Clamps a byte point to line content, rounding its column down.
    pub fn clip_point(&self, p: Point) -> Point {
        let row = self.clip_line(p.row);
        let start = self.line_to_byte(row);
        Point {
            row,
            column: self.floor_char_boundary(start + p.column.min(self.line_len_bytes(row)))
                - start,
        }
    }
    /// Clamps a UTF-16 point to line content, rounding its column down.
    pub fn clip_point_utf16(&self, p: PointUtf16) -> PointUtf16 {
        let row = self.clip_line(p.row);
        let line = self.line(row);
        PointUtf16 {
            row,
            column: line.clip_utf16(p.column),
        }
    }
}

/// A double-ended, fused iterator over borrowed UTF-8 chunks.
#[derive(Clone, Debug)]
pub struct Chunks<'a> {
    front: tree::Cursor<'a>,
    back: tree::Cursor<'a>,
    low: usize,
    high: usize,
}
impl<'a> Chunks<'a> {
    fn new(slice: TextSlice<'a>) -> Self {
        Self {
            front: tree::Cursor::at(&slice.text.root, slice.start.bytes),
            back: tree::Cursor::at(&slice.text.root, slice.end.bytes.saturating_sub(1)),
            low: slice.start.bytes,
            high: slice.end.bytes,
        }
    }
}
impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<Self::Item> {
        if self.low >= self.high {
            return None;
        }
        let end = (self.front.start + self.front.text.len()).min(self.high);
        let s = &self.front.text[self.low - self.front.start..end - self.front.start];
        self.low = end;
        if self.low < self.high {
            self.front.advance();
        }
        Some(s)
    }
}
impl DoubleEndedIterator for Chunks<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.low >= self.high {
            return None;
        }
        let start = self.back.start.max(self.low);
        let s = &self.back.text[start - self.back.start..self.high - self.back.start];
        self.high = start;
        if self.low < self.high {
            self.back.backtrack();
        }
        Some(s)
    }
}
impl std::iter::FusedIterator for Chunks<'_> {}

/// A fused streaming line iterator. Line slices exclude LF; EOF yields a final line.
#[derive(Clone, Debug)]
pub struct Lines<'a> {
    text: &'a Text,
    chunks: Chunks<'a>,
    current: &'a str,
    ascii: bool,
    position: Summary,
    done: bool,
}
impl<'a> Lines<'a> {
    fn new(slice: TextSlice<'a>) -> Self {
        let mut chunks = slice.chunks();
        let current = chunks.next().unwrap_or("");
        Self {
            text: slice.text,
            chunks,
            current,
            ascii: current.is_ascii(),
            position: slice.start,
            done: false,
        }
    }
    fn fragment(&self, s: &str) -> Summary {
        if self.ascii {
            Summary {
                bytes: s.len(),
                chars: s.len(),
                utf16: s.len(),
                newlines: 0,
            }
        } else {
            Summary::fragment(s)
        }
    }
}
impl<'a> Iterator for Lines<'a> {
    type Item = TextSlice<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let start = self.position;
        loop {
            if let Some(i) = memchr::memchr(b'\n', self.current.as_bytes()) {
                let end = self.position.plus(self.fragment(&self.current[..i]));
                self.position = end.plus(Summary {
                    bytes: 1,
                    chars: 1,
                    utf16: 1,
                    newlines: 1,
                });
                self.current = &self.current[i + 1..];
                return Some(TextSlice {
                    text: self.text,
                    start,
                    end,
                });
            }
            self.position = self.position.plus(self.fragment(self.current));
            match self.chunks.next() {
                Some(chunk) => {
                    self.current = chunk;
                    self.ascii = chunk.is_ascii();
                }
                None => {
                    self.done = true;
                    return Some(TextSlice {
                        text: self.text,
                        start,
                        end: self.position,
                    });
                }
            }
        }
    }
}
impl std::iter::FusedIterator for Lines<'_> {}

impl Default for Text {
    fn default() -> Self {
        Self::new()
    }
}
impl From<&str> for Text {
    fn from(s: &str) -> Self {
        Self::from_str(s)
    }
}
impl From<String> for Text {
    fn from(s: String) -> Self {
        Self::from_str(&s)
    }
}
impl std::str::FromStr for Text {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(s))
    }
}
impl fmt::Display for TextSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for s in self.chunks() {
            f.write_str(s)?;
        }
        Ok(())
    }
}
impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}
impl fmt::Debug for TextSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let preview: String = self.chars().take(64).collect();
        f.debug_struct("TextSlice")
            .field("bytes", &self.len_bytes())
            .field("preview", &preview)
            .field("truncated", &(self.len_chars() > 64))
            .finish()
    }
}
impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let preview: String = self.chars().take(64).collect();
        f.debug_struct("Text")
            .field("bytes", &self.len_bytes())
            .field("preview", &preview)
            .field("truncated", &(self.len_chars() > 64))
            .finish()
    }
}
impl PartialEq for Text {
    fn eq(&self, rhs: &Self) -> bool {
        tree::equal(&self.root, &rhs.root)
    }
}
impl Eq for Text {}
impl PartialEq for TextSlice<'_> {
    fn eq(&self, rhs: &Self) -> bool {
        self.len_bytes() == rhs.len_bytes()
            && ((std::ptr::eq(self.text, rhs.text) && self.start.bytes == rhs.start.bytes)
                || self.bytes().eq(rhs.bytes()))
    }
}
impl Eq for TextSlice<'_> {}
impl PartialEq<str> for TextSlice<'_> {
    fn eq(&self, rhs: &str) -> bool {
        self.len_bytes() == rhs.len() && self.bytes().eq(rhs.bytes())
    }
}
impl PartialEq<str> for Text {
    fn eq(&self, rhs: &str) -> bool {
        self.as_slice() == *rhs
    }
}
impl PartialEq<&str> for Text {
    fn eq(&self, rhs: &&str) -> bool {
        self == *rhs
    }
}
impl PartialEq<&str> for TextSlice<'_> {
    fn eq(&self, rhs: &&str) -> bool {
        self == *rhs
    }
}
impl Hash for TextSlice<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // A canonical byte stream gives equal hashes for every tree partition and
        // every Hasher, including hashers sensitive to individual write calls.
        for byte in self.bytes() {
            state.write_u8(byte);
        }
        state.write_u8(0xff);
    }
}
impl Hash for Text {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

#[cfg(test)]
mod tests;

impl Text {
    /// Number of UTF-8 bytes.
    pub fn len_bytes(&self) -> usize {
        self.as_slice().len_bytes()
    }
    /// Number of Unicode scalars.
    pub fn len_chars(&self) -> usize {
        self.as_slice().len_chars()
    }
    /// Number of UTF-16 code units.
    pub fn len_utf16(&self) -> usize {
        self.as_slice().len_utf16()
    }
    /// Number of LF separators plus one.
    pub fn len_lines(&self) -> usize {
        self.as_slice().len_lines()
    }
    /// Whether the text is empty.
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
    /// Converts scalar-aligned bytes to scalars in O(log n).
    pub fn byte_to_char(&self, byte: usize) -> usize {
        self.as_slice().byte_to_char(byte)
    }
    /// Converts a scalar offset to bytes in O(log n).
    pub fn char_to_byte(&self, ch: usize) -> usize {
        self.as_slice().char_to_byte(ch)
    }
    /// Converts scalar-aligned bytes to UTF-16 in O(log n).
    pub fn byte_to_utf16(&self, byte: usize) -> usize {
        self.as_slice().byte_to_utf16(byte)
    }
    /// Converts UTF-16 to bytes; panics inside a surrogate pair.
    pub fn utf16_to_byte(&self, u: usize) -> usize {
        self.as_slice().utf16_to_byte(u)
    }
    /// Finds the LF row of a scalar-aligned byte offset.
    pub fn byte_to_line(&self, byte: usize) -> usize {
        self.as_slice().byte_to_line(byte)
    }
    /// Start of an existing LF line; len_lines() is out of bounds.
    pub fn line_to_byte(&self, line: usize) -> usize {
        self.as_slice().line_to_byte(line)
    }
    /// Converts scalar-aligned bytes to an LF/byte point.
    pub fn byte_to_point(&self, byte: usize) -> Point {
        self.as_slice().byte_to_point(byte)
    }
    /// Converts an LF/byte point to bytes; invalid coordinates panic.
    pub fn point_to_byte(&self, p: Point) -> usize {
        self.as_slice().point_to_byte(p)
    }
    /// Converts scalar-aligned bytes to an LF/UTF-16 point.
    pub fn byte_to_point_utf16(&self, byte: usize) -> PointUtf16 {
        self.as_slice().byte_to_point_utf16(byte)
    }
    /// Converts an LF/UTF-16 point to bytes; invalid coordinates panic.
    pub fn point_utf16_to_byte(&self, p: PointUtf16) -> usize {
        self.as_slice().point_utf16_to_byte(p)
    }
    /// Line length in bytes, excluding LF.
    pub fn line_len_bytes(&self, line: usize) -> usize {
        self.as_slice().line_len_bytes(line)
    }
    /// Whether a byte is a scalar boundary; false out of bounds.
    pub fn is_char_boundary(&self, byte: usize) -> bool {
        self.as_slice().is_char_boundary(byte)
    }
    /// Clamps to EOF and rounds down to a scalar boundary.
    pub fn floor_char_boundary(&self, byte: usize) -> usize {
        self.as_slice().floor_char_boundary(byte)
    }
    /// Clamps to EOF and rounds up to a scalar boundary.
    pub fn ceil_char_boundary(&self, byte: usize) -> usize {
        self.as_slice().ceil_char_boundary(byte)
    }
    /// Next extended grapheme boundary or EOF, across leaf edges.
    pub fn next_grapheme_boundary(&self, byte: usize) -> usize {
        self.as_slice().next_grapheme_boundary(byte)
    }
    /// Previous extended grapheme boundary or zero, across leaf edges.
    pub fn prev_grapheme_boundary(&self, byte: usize) -> usize {
        self.as_slice().prev_grapheme_boundary(byte)
    }
    /// Borrows a scalar-aligned byte range; invalid ranges panic.
    pub fn slice(&self, range: Range<usize>) -> TextSlice<'_> {
        self.as_slice().slice(range)
    }
    /// Borrows one line without LF.
    pub fn line(&self, line: usize) -> TextSlice<'_> {
        self.as_slice().line(line)
    }
    /// Reads a scalar; EOF and non-boundaries panic.
    pub fn char_at(&self, byte: usize) -> char {
        self.as_slice().char_at(byte)
    }
    /// Reads a byte; EOF is out of bounds.
    pub fn byte_at(&self, byte: usize) -> u8 {
        self.as_slice().byte_at(byte)
    }
    /// Iterates borrowed chunks in either direction.
    pub fn chunks(&self) -> Chunks<'_> {
        self.as_slice().chunks()
    }
    /// Iterates scalars in either direction.
    pub fn chars(&self) -> impl DoubleEndedIterator<Item = char> + '_ {
        self.as_slice().chars()
    }
    /// Iterates bytes in either direction.
    pub fn bytes(&self) -> impl DoubleEndedIterator<Item = u8> + '_ {
        self.as_slice().bytes()
    }
    /// Iterates lines without LF, including an empty final line.
    pub fn lines(&self) -> Lines<'_> {
        self.as_slice().lines()
    }
    /// Chunk and its start; EOF returns the last chunk; empty text returns an empty chunk.
    pub fn chunk_at_byte(&self, byte: usize) -> (&str, usize) {
        self.as_slice().chunk_at_byte(byte)
    }
    /// Clamps bytes to EOF and rounds down to a scalar boundary.
    pub fn clip_byte(&self, byte: usize) -> usize {
        self.as_slice().clip_byte(byte)
    }
    /// Clamps a scalar offset to EOF.
    pub fn clip_char(&self, ch: usize) -> usize {
        self.as_slice().clip_char(ch)
    }
    /// Clamps UTF-16 to EOF, rounding down inside a surrogate pair.
    pub fn clip_utf16(&self, u: usize) -> usize {
        self.as_slice().clip_utf16(u)
    }
    /// Clamps a row to the last line.
    pub fn clip_line(&self, line: usize) -> usize {
        self.as_slice().clip_line(line)
    }
    /// Clamps a byte point to line content and rounds down.
    pub fn clip_point(&self, p: Point) -> Point {
        self.as_slice().clip_point(p)
    }
    /// Clamps a UTF-16 point to line content and rounds down.
    pub fn clip_point_utf16(&self, p: PointUtf16) -> PointUtf16 {
        self.as_slice().clip_point_utf16(p)
    }
}
