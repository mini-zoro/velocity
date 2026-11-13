use crate::{Text, TextSlice, tree};

/// A zero-copy, bidirectional regex cursor over a text or a borrowed slice.
/// Offsets are relative to the view; chunks always end on UTF-8 scalar boundaries.
#[derive(Clone, Debug)]
pub struct ChunkCursor<'a> {
    cursor: tree::Cursor<'a>,
    start: usize,
    end: usize,
}

impl<'a> ChunkCursor<'a> {
    /// Starts at the first chunk of a borrowed view.
    pub fn new(slice: TextSlice<'a>) -> Self {
        Self {
            cursor: tree::Cursor::at(&slice.text.root, slice.start.bytes),
            start: slice.start.bytes,
            end: slice.end.bytes,
        }
    }
}
impl<'a> From<&'a Text> for ChunkCursor<'a> {
    fn from(text: &'a Text) -> Self {
        Self::new(text.as_slice())
    }
}
impl<'a> From<TextSlice<'a>> for ChunkCursor<'a> {
    fn from(slice: TextSlice<'a>) -> Self {
        Self::new(slice)
    }
}

impl regex_cursor::Cursor for ChunkCursor<'_> {
    fn chunk(&self) -> &[u8] {
        let lo = self.start.max(self.cursor.start) - self.cursor.start;
        let hi = self.end.min(self.cursor.start + self.cursor.text.len()) - self.cursor.start;
        &self.cursor.text.as_bytes()[lo..hi]
    }
    fn advance(&mut self) -> bool {
        if self.cursor.start + self.cursor.text.len() >= self.end {
            false
        } else {
            self.cursor.advance()
        }
    }
    fn backtrack(&mut self) -> bool {
        if self.cursor.start <= self.start {
            false
        } else {
            self.cursor.backtrack()
        }
    }
    fn total_bytes(&self) -> Option<usize> {
        Some(self.end - self.start)
    }
    fn offset(&self) -> usize {
        self.cursor.start.saturating_sub(self.start)
    }
}
