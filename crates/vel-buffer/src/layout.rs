use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_text::Text;
/// Geometry interface supplied by layout; all offsets are UTF-8 bytes.
pub trait VerticalLayout {
    /// Refreshes cached geometry before a command, including internal macro keys.
    /// Stateless layouts need no synchronization.
    fn synchronize(&self, _buffer: &crate::Buffer, _view: crate::ViewId) {}
    /// Horizontal position of a scalar boundary.
    fn x_of(&self, text: &Text, offset: usize) -> f64;
    /// Closest grapheme boundary on another LF line at the requested position.
    fn offset_at(&self, text: &Text, line: usize, x: f64) -> usize;
    /// Number of visual lines in a page (at least one).
    fn lines_per_page(&self) -> usize;
}
/// Deterministic monospace layout for tests and headless clients.
#[derive(Clone, Copy, Debug)]
pub struct MonospaceLayout {
    /// Tab stop width, clamped to at least one.
    pub tab_size: usize,
    /// Visible LF lines, clamped to at least one.
    pub page_lines: usize,
}
impl Default for MonospaceLayout {
    fn default() -> Self {
        Self {
            tab_size: 4,
            page_lines: 30,
        }
    }
}
impl MonospaceLayout {
    fn width(self, g: &str, x: usize) -> usize {
        if g == "\t" {
            self.tab_size.max(1) - x % self.tab_size.max(1)
        } else {
            UnicodeWidthStr::width(g)
        }
    }
}
impl VerticalLayout for MonospaceLayout {
    fn x_of(&self, text: &Text, offset: usize) -> f64 {
        let start = text.line_to_byte(text.byte_to_line(offset));
        let s = text.slice(start..offset).to_string();
        let mut x = 0;
        for g in s.graphemes(true) {
            x += self.width(g, x);
        }
        x as f64
    }
    fn offset_at(&self, text: &Text, line: usize, x: f64) -> usize {
        // ST-BEHAVIOR: unverified [V1]: nearest grapheme boundary, ties choose left; nonfinite goals choose column zero.
        let start = text.line_to_byte(line);
        let s = text.line(line).to_string();
        let mut column = 0;
        let x = if x.is_finite() { x.max(0.0) } else { 0.0 };
        for (at, g) in s.grapheme_indices(true) {
            let next = column + self.width(g, column);
            if x <= (column + next) as f64 / 2.0 {
                return start + at;
            }
            column = next;
        }
        start + s.len()
    }
    fn lines_per_page(&self) -> usize {
        self.page_lines.max(1)
    }
}
