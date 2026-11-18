//! Opt-in display labels for code points with no reliably visible glyph.

/// Which non-ASCII whitespace is labeled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WhiteSpace {
    /// Draw whitespace normally.
    #[default]
    None,
    /// Label ambiguous spaces, leaving ideographic space unmarked.
    Punctuation,
    /// Label all non-ASCII whitespace.
    All,
}

/// Controls which invisible characters are represented by a hex label.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Invisible {
    /// Draw non-ASCII white space, including zero-width space and BOM.
    pub whitespace: WhiteSpace,
    /// Draw Unicode bidi control characters.
    pub bidi: bool,
    /// Draw C0 (except line separators and tabs) and DEL.
    pub control: bool,
}
impl Invisible {
    /// Size of the unshaped monospace label in cells, if enabled for this scalar.
    pub fn cells(self, c: char) -> Option<usize> {
        let cp = u32::from(c);
        let matches = (self.whitespace != WhiteSpace::None
            && !c.is_ascii()
            && (self.whitespace == WhiteSpace::All || c != '\u{3000}')
            && (c.is_whitespace() || matches!(c, '\u{200b}' | '\u{feff}')))
            || (self.bidi
                && matches!(cp, 0x061c | 0x200e..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069))
            || (self.control && matches!(cp, 0..=8 | 11..=12 | 14..=31 | 127));
        matches.then(|| 4 + ((u32::BITS - cp.leading_zeros()).div_ceil(4) as usize).max(2))
    }

    /// Whether the string has a potential marker, before grapheme segmentation.
    pub fn contains(self, text: &str) -> bool {
        (self.control || !text.is_ascii()) && text.chars().any(|c| self.cells(c).is_some())
    }

    /// Extra cells for a single-scalar grapheme drawn with a label.
    pub fn cell(self, text: &str, width: usize) -> Option<(char, usize)> {
        let mut chars = text.chars();
        let c = chars.next()?;
        let cells = self.cells(c)?;
        chars
            .next()
            .is_none()
            .then_some((c, cells.saturating_sub(width)))
    }
}

/// Formats a single code point as an ASCII hex label.
pub fn label(c: char) -> String {
    format!("<0x{:02x}>", u32::from(c))
}
