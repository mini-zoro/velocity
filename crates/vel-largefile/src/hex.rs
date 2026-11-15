//! No eager binary-file allocation; row reads touch only the requested mapped pages.
use memmap2::Mmap;
use std::{fs::File, io, path::Path};

/// Routing result for a prefix (normally the first 8 KiB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    /// UTF-8 or an undecided, possibly incomplete UTF-8 suffix.
    Utf8,
    /// UTF-16 BOM, or invalid UTF-8: use the encoding-aware full decoder.
    DecodeRequired,
    /// NUL outside a UTF-16 BOM: offer a read-only hex view.
    Binary,
}
/// Detects BOMs before NULs; an incomplete last scalar is allowed in a sample.
/// This is a text/binary heuristic, not a MIME classifier.
pub fn detect_content(prefix: &[u8]) -> ContentKind {
    if prefix.starts_with(&[0xff, 0xfe]) || prefix.starts_with(&[0xfe, 0xff]) {
        return ContentKind::DecodeRequired;
    }
    if prefix.contains(&0) {
        return ContentKind::Binary;
    }
    match std::str::from_utf8(prefix) {
        Ok(_) => ContentKind::Utf8,
        Err(e) if e.error_len().is_none() => ContentKind::Utf8,
        Err(_) => ContentKind::DecodeRequired,
    }
}
/// A borrowed hex row, including the actual byte count of a partial last row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexRow<'a> {
    /// Absolute byte offset.
    pub offset: u64,
    /// Up to 16 original bytes; formatting can use `format!("{byte:02x}")`.
    pub bytes: &'a [u8],
    /// Printable ASCII; other bytes become '.', unused cells are spaces.
    pub ascii: [u8; 16],
}
/// Read-only memory mapping. No complete file copy, including for multi-GB files.
#[derive(Debug)]
pub struct HexView {
    map: Option<Mmap>,
}
impl HexView {
    /// Maps a stable file read-only; an empty file needs no OS mapping.
    ///
    /// # Safety
    /// The caller must ensure that the file's contents and length cannot be
    /// modified externally for the mapping's entire lifetime. An editor must
    /// release this view before permitting writes/truncation, including reloads.
    pub unsafe fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let map = if file.metadata()?.len() == 0 {
            None
        } else {
            // SAFETY: the caller guarantees a stable file for this map's lifetime.
            Some(unsafe { Mmap::map(&file)? })
        };
        Ok(Self { map })
    }
    /// Original mapped byte count.
    pub fn len_bytes(&self) -> u64 {
        self.map.as_ref().map_or(0, |map| map.len() as u64)
    }
    /// Whether there are no bytes or rows.
    pub fn is_empty(&self) -> bool {
        self.len_bytes() == 0
    }
    /// Number of 16-byte rows, rounding a partial last row up.
    pub fn len_rows(&self) -> u64 {
        self.len_bytes().div_ceil(16)
    }
    /// Reads one row; out-of-range requests return None without touching the map.
    pub fn row(&self, row: u64) -> Option<HexRow<'_>> {
        if row >= self.len_rows() {
            return None;
        }
        let offset = row.checked_mul(16)?;
        let map = self.map.as_ref()?;
        let start = usize::try_from(offset).ok()?;
        let bytes = &map[start..start.saturating_add(16).min(map.len())];
        let mut ascii = [b' '; 16];
        for (out, &byte) in ascii.iter_mut().zip(bytes) {
            *out = if (0x20..=0x7e).contains(&byte) {
                byte
            } else {
                b'.'
            };
        }
        Some(HexRow {
            offset,
            bytes,
            ascii,
        })
    }
}
