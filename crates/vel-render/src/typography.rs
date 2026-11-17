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
