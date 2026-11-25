//! Pure settings-to-feature decisions; no file scans or worker creation.

/// Measurements accumulated by loading or another background scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DocumentMetrics {
    /// Decoded UTF-8 byte size (a loaded prefix is a valid input).
    pub bytes: u64,
    /// LF-separated lines, including an empty final line.
    pub lines: u64,
    /// Longest line body in UTF-8 bytes, excluding LF.
    pub longest_line_bytes: u64,
}
/// Syntax work permitted by the document policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightMode {
    /// Ordinary full-document background highlighting.
    Full,
    /// Bounded visible-line requests; exact repair runs only on a worker.
    Viewport,
}
/// Effective feature switches. Threshold comparisons are inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeaturePolicy {
    /// Full-document or viewport syntax work.
    pub highlighting: HighlightMode,
    /// Permit project/document symbol indexing.
    pub indexing: bool,
    /// Initial wrapping preference; an explicit user override belongs to the caller.
    pub word_wrap: bool,
    /// Draw one minimap line per this many document lines (at least one).
    pub minimap_stride: u64,
    /// Permit spelling analysis.
    pub spell_check: bool,
    /// Permit a diff against the saved disk snapshot.
    pub diff_against_disk: bool,
}
/// Configurable huge-document settings; sizes are bytes and counts are lines.
/// See README for the corresponding setting names and defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeDocumentPolicy {
    /// Switch to viewport highlighting at this decoded size.
    pub viewport_bytes: u64,
    /// Also switch at this line count.
    pub viewport_lines: u64,
    /// Also switch when a line reaches this byte length.
    pub long_line_bytes: u64,
    /// Disable indexing at this decoded size.
    pub indexing_bytes: u64,
    /// Disable wrapping at this decoded size.
    pub wrap_bytes: u64,
    /// Disable spelling at this decoded size.
    pub spell_bytes: u64,
    /// Disable disk diff at this decoded size.
    pub diff_bytes: u64,
    /// Maximum sampled minimap lines; zero samples at most one line with a maximal stride.
    pub minimap_lines: u64,
}
impl Default for LargeDocumentPolicy {
    fn default() -> Self {
        Self {
            viewport_bytes: 64 * 1024 * 1024,
            viewport_lines: 1_000_000,
            long_line_bytes: 16 * 1024,
            indexing_bytes: 64 * 1024 * 1024,
            wrap_bytes: 16 * 1024 * 1024,
            spell_bytes: 16 * 1024 * 1024,
            diff_bytes: 64 * 1024 * 1024,
            minimap_lines: 10_000,
        }
    }
}
impl LargeDocumentPolicy {
    /// Makes a deterministic decision without inspecting document text.
    pub fn evaluate(&self, m: DocumentMetrics) -> FeaturePolicy {
        let huge = m.bytes >= self.viewport_bytes || m.lines >= self.viewport_lines;
        let long = m.longest_line_bytes >= self.long_line_bytes;
        FeaturePolicy {
            highlighting: if huge || long {
                HighlightMode::Viewport
            } else {
                HighlightMode::Full
            },
            indexing: !huge && !long && m.bytes < self.indexing_bytes,
            // A long line does not disable wrapping: `word_wrap: auto` exists precisely to
            // wrap lines wider than the viewport, and an unwrapped 100k-column line leaves
            // the viewport at its far end with nothing visible. Wrap cost is bounded by the
            // document bytes, and `huge` documents still keep wrapping off.
            word_wrap: !huge && m.bytes < self.wrap_bytes,
            minimap_stride: if self.minimap_lines == 0 {
                u64::MAX
            } else {
                m.lines.div_ceil(self.minimap_lines).max(1)
            },
            spell_check: !huge && !long && m.bytes < self.spell_bytes,
            diff_against_disk: !huge && !long && m.bytes < self.diff_bytes,
        }
    }
}
