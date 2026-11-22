//! Shared font option and grid geometry contracts for shaping and input.
/// Font options affecting OpenType shaping and rasterization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FontOptions {
    /// Disabled default liga/clig/calt features, in that order.
    pub disabled: u8,
    /// Opt-in discretionary ligatures.
    pub discretionary: bool,
    /// Enabled stylistic sets 01 through 20.
    pub stylistic_sets: u32,
    /// Disable grayscale coverage (threshold masks instead).
    pub no_antialias: bool,
}
impl FontOptions {
    /// Documented switches only; platform-inapplicable switches preserve native coverage.
    /// <https://www.sublimetext.com/docs/ligatures.html>
    /// <https://docs.sublimetext.io/reference/settings.html#visual-settings>
    /// Stylistic sets: <https://github.com/tonsky/FiraCode/wiki/How-to-enable-stylistic-sets>
    pub fn parse<'a>(options: impl IntoIterator<Item = &'a str>) -> Self {
        let mut out = Self::default();
        for option in options {
            match option {
                "no_liga" => out.disabled |= 1,
                "no_clig" => out.disabled |= 2,
                "no_calt" => out.disabled |= 4,
                "dlig" => out.discretionary = true,
                "no_antialias" => out.no_antialias = true,
                "gray_antialias" | "subpixel_antialias" => out.no_antialias = false,
                s if s.starts_with("ss") && s.len() == 4 => {
                    if let Ok(set @ 1..=20) = s[2..].parse::<u8>() {
                        out.stylistic_sets |= 1 << (set - 1);
                    }
                }
                _ => {}
            }
        }
        out
    }
    /// Stable shaping/raster cache identity.
    pub fn identity(self) -> u64 {
        u64::from(self.disabled)
            | ((self.discretionary as u64) << 3)
            | (u64::from(self.stylistic_sets) << 4)
            | ((self.no_antialias as u64) << 24)
    }
}
/// Monospace cluster advances are divided across their occupied cells. Both caret/hit
/// geometry and shaping use this, including positions inside punctuation ligatures.
pub fn grid_x(columns: usize, cell: f64) -> f64 {
    columns as f64 * cell
}

/// Natural advances of the editor's wide fallback classes, in units of the
/// primary cell. Zoom scales both in the same proportion.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WideAdvances(Option<[f64; 8]>);

impl WideAdvances {
    /// One representative per class: Han, hiragana, katakana, hangul,
    /// full-width forms, emoji, flag sequences, and other wide symbols.
    pub const SAMPLES: [&str; 8] = ["中", "あ", "ア", "한", "Ａ", "😀", "🇯🇵", "◽"];

    /// Constructs measured advances (zero retains the grid for a missing class).
    pub fn new(cells: [f64; 8]) -> Self {
        Self(Some(cells))
    }

    /// Whether this projection uses wide advances at all.
    pub fn enabled(self) -> bool {
        self.0.is_some()
    }

    /// Geometry key for shaped text caches; the legacy grid key remains zero.
    #[cfg(any(target_os = "macos", windows))]
    pub(crate) fn identity(self) -> u64 {
        use std::hash::{Hash, Hasher};
        self.0.map_or(0, |samples| {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            samples.map(f64::to_bits).hash(&mut hash);
            hash.finish()
        })
    }

    /// The distance to add to `columns * cell`, in cell units. Tabs and virtual
    /// columns (phantoms, label padding, hanging indent) stay on the grid.
    #[inline]
    pub fn adjustment(&self, grapheme: &str, columns: usize) -> f64 {
        if columns < 2 || grapheme == "\t" {
            return 0.;
        }
        let Some(samples) = &self.0 else { return 0. };
        let Some(first) = grapheme.chars().next() else {
            return 0.;
        };
        let class = match first as u32 {
            0x3400..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x3ffff => 0,
            0x3040..=0x309f | 0x1b001..=0x1b11f => 1,
            0x30a0..=0x30ff | 0x31f0..=0x31ff | 0x1b000 => 2,
            0x1100..=0x11ff | 0x3130..=0x318f | 0xa960..=0xa97f | 0xac00..=0xd7ff => 3,
            0x3000..=0x303f | 0xfe10..=0xfe6f | 0xff01..=0xff60 | 0xffe0..=0xffe6 => 4,
            0x1f1e6..=0x1f1ff => 6,
            #[cfg(target_os = "macos")]
            _ if crate::font::emoji_presentation(grapheme) => 5,
            _ => 7,
        };
        let measured = samples[class];
        if measured.is_finite() && measured > 0. {
            f64::from((measured - columns as f64) as f32)
        } else {
            0.
        }
    }
}

/// Sparse piecewise-linear column projection. Wide graphemes keep their logical
/// cells; a partial cell range (e.g. a rectangular selection) uses the same slope.
/// An ordinary row has no allocation and projects via a single multiplication.
#[derive(Clone, Debug, Default)]
pub struct WideColumns(Vec<(usize, usize, f64)>);

impl WideColumns {
    /// Projects an ordered stream of graphemes and any grid gap before each.
    pub fn from_units<'a>(
        advances: &WideAdvances,
        indent: usize,
        units: impl IntoIterator<Item = (&'a str, usize, usize)>,
    ) -> Self {
        let mut columns = Self::default();
        let mut col = indent;
        for (text, width, gap) in units {
            col += gap;
            columns.push(col, width, advances.adjustment(text, width));
            col += width;
        }
        columns
    }
    /// Adds a wide cluster at its projected column, in ascending order.
    pub fn push(&mut self, start: usize, width: usize, adjustment: f64) {
        if adjustment != 0. {
            let total = self.0.last().map_or(0., |span| span.2);
            self.0.push((start, start + width, total + adjustment));
        }
    }

    /// Whether this row contains any measured adjustments.
    pub fn is_grid(&self) -> bool {
        self.0.is_empty()
    }

    /// The cumulative adjustment before a logical column.
    #[inline]
    pub fn adjustment(&self, column: usize) -> f64 {
        let i = self.0.partition_point(|span| span.0 < column);
        if i == 0 {
            return 0.;
        }
        let (start, end, total) = self.0[i - 1];
        if column >= end {
            return total;
        }
        let before = if i > 1 { self.0[i - 2].2 } else { 0. };
        before + (total - before) * (column - start) as f64 / (end - start) as f64
    }

    /// Projects one column to logical points (or to fractional cells if cell is 1).
    #[inline]
    pub fn x(&self, column: usize, cell: f64) -> f64 {
        if self.is_grid() {
            grid_x(column, cell)
        } else {
            (column as f64 + self.adjustment(column)) * cell
        }
    }

    /// Inverts a pixel distance expressed in cells, preserving fractional columns
    /// until the caller chooses a grapheme boundary.
    pub fn column_at(&self, x: f64) -> f64 {
        let i = self
            .0
            .partition_point(|&(_, end, after)| end as f64 + after <= x);
        let before = if i > 0 { self.0[i - 1].2 } else { 0. };
        let Some(&(start, end, after)) = self.0.get(i) else {
            return x - before;
        };
        let left = start as f64 + before;
        if x <= left {
            return x - before;
        }
        let right = end as f64 + after;
        start as f64 + (x - left) * (end - start) as f64 / (right - left)
    }
}
/// Metrics of the primary face at 13 points, before user padding.
#[derive(Clone, Copy, Debug)]
pub struct FontMetrics {
    /// Primary ascent.
    pub ascent: f64,
    /// Positive primary descent.
    pub descent: f64,
    /// Primary line gap.
    pub leading: f64,
}
impl FontMetrics {
    /// Device-pixel-rounded height and baseline offset in logical points.
    pub fn line(self, size: f64, device_scale: f64, top: f64, bottom: f64) -> (f64, f64) {
        let device = device_scale.max(1.);
        let scale = size / 13.;
        (
            ((self.ascent + self.descent + self.leading).max(1.) * scale * device).ceil() / device
                + top.max(0.)
                + bottom.max(0.),
            self.ascent * scale + top.max(0.),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn option_table_and_fractional_metrics_have_stable_cache_identities() {
        let options = FontOptions::parse([
            "no_liga",
            "no_clig",
            "no_calt",
            "dlig",
            "ss01",
            "ss20",
            "ss21",
            "ssx0",
            "no_antialias",
        ]);
        assert_eq!(options.disabled, 7);
        assert!(options.discretionary);
        assert_eq!(options.stylistic_sets, 1 | (1 << 19));
        assert!(options.no_antialias);
        assert_ne!(options.identity(), FontOptions::default().identity());
        assert!(!FontOptions::parse(["no_antialias", "gray_antialias"]).no_antialias);
        let metrics = FontMetrics {
            ascent: 12.,
            descent: 3.,
            leading: 0.3,
        };
        assert_eq!(metrics.line(13., 2., 0., 0.), (15.5, 12.));
        assert_eq!(metrics.line(13., 1., 1., 2.), (19., 13.));
    }
}
