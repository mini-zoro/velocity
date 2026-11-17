//! Load-time global types and Velocity's conservative fallback palette.
use super::*;

/// Documented geometry for selection and block-caret corners.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CornerStyle {
    /// Rounded corners (the documented default).
    #[default]
    Round,
    /// Diagonally clipped corners.
    Cut,
    /// Square corners.
    Square,
}

/// Compiled bracket and matching-tag decorations; no string parsing during painting.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DecorationOptions(u8);
impl DecorationOptions {
    /// Recolor matching glyphs.
    pub const FOREGROUND: Self = Self(1);
    /// Solid underline.
    pub const UNDERLINE: Self = Self(2);
    /// Dotted underline.
    pub const STIPPLED_UNDERLINE: Self = Self(4);
    /// Wavy underline.
    pub const SQUIGGLY_UNDERLINE: Self = Self(8);
    /// Bold matching brackets.
    pub const BOLD: Self = Self(16);
    /// Italic matching brackets.
    pub const ITALIC: Self = Self(32);
    /// Tests one or more options.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl Globals {
    pub(super) fn compile_value(
        &mut self,
        key: &str,
        value: &Value,
        vars: &BTreeMap<String, String>,
        warnings: &mut Vec<String>,
    ) {
        // Public type table: https://www.sublimetext.com/docs/color_schemes.html
        let result = (|| -> Result<(), Error> {
            match key {
                "selection_corner_style" | "block_caret_corner_style" => {
                    let style = match value.as_str() {
                        Some("round") => CornerStyle::Round,
                        Some("cut") => CornerStyle::Cut,
                        Some("square") => CornerStyle::Square,
                        _ => return Err(Error::Format("expected round, cut or square".into())),
                    };
                    self.corners.insert(key.into(), style);
                }
                "selection_corner_radius"
                | "block_caret_corner_radius"
                | "line_diff_width"
                | "shadow_width" => {
                    let number = value
                        .as_f64()
                        .or_else(|| value.as_str()?.parse().ok())
                        .filter(|n| n.is_finite() && *n >= 0.)
                        .ok_or_else(|| {
                            Error::Format("expected a nonnegative finite number".into())
                        })?;
                    self.numbers.insert(key.into(), number);
                }
                "brackets_options" | "bracket_contents_options" | "tags_options" => {
                    let text = value
                        .as_str()
                        .ok_or_else(|| Error::Format("expected options string".into()))?;
                    let mut options = DecorationOptions::default();
                    for word in text.split_whitespace() {
                        let bit = match word {
                            "foreground" => DecorationOptions::FOREGROUND,
                            "underline" => DecorationOptions::UNDERLINE,
                            "stippled_underline" => DecorationOptions::STIPPLED_UNDERLINE,
                            "squiggly_underline" => DecorationOptions::SQUIGGLY_UNDERLINE,
                            "bold" if key != "tags_options" => DecorationOptions::BOLD,
                            "italic" if key != "tags_options" => DecorationOptions::ITALIC,
                            _ => {
                                warnings.push(format!("globals.{key}: unknown option {word}"));
                                continue;
                            }
                        };
                        options.0 |= bit.0;
                    }
                    self.options.insert(key.into(), options);
                }
                "font_style" => {
                    FontStyle::parse(
                        value
                            .as_str()
                            .ok_or_else(|| Error::Format("expected font style string".into()))?,
                    )?;
                }
                "font_face" => {
                    if !value.is_string() {
                        return Err(Error::Format("expected font face string".into()));
                    }
                }
                key if key.ends_with("_css") || key.ends_with("_options") => {
                    if !value.is_string() {
                        return Err(Error::Format("expected string".into()));
                    }
                }
                _ => {
                    let color = Color::parse(
                        value
                            .as_str()
                            .ok_or_else(|| Error::Format("expected color string".into()))?,
                        vars,
                    )?;
                    self.colors.insert(key.into(), color);
                }
            }
            self.values.insert(key.into(), value.clone());
            Ok(())
        })();
        recover(warnings, &format!("globals.{key}"), result);
    }

    pub(super) fn derive_defaults(&mut self) {
        let fg = self.color("foreground").unwrap_or(Color::rgb(0, 0, 0));
        let bg = self
            .color("background")
            .unwrap_or(Color::rgb(255, 255, 255));
        let transparent = Color { a: 0, ..fg };
        let alpha = |a: f64| Color {
            a: (a * 255.).round() as u8,
            ..fg
        };
        let mix = |amount| bg.lerp(fg, amount, false, true);
        for (key, color) in [
            ("foreground", fg),
            ("background", bg),
            // Documented: https://www.sublimetext.com/docs/color_schemes.html#invisibles
            ("invisibles", alpha(0.35)),
            // Each remaining fallback is our own palette choice, not a compatibility claim.
            ("gutter", bg),                    // Velocity: continuous background.
            ("gutter_foreground", mix(0.5)),   // Velocity: quieter line numbers.
            ("line_highlight", alpha(0.05)),   // Velocity: faint caret-line overlay.
            ("rulers", alpha(0.15)),           // Velocity: unobtrusive column markers.
            ("guide", alpha(0.15)),            // Velocity: passive indentation.
            ("active_guide", alpha(0.4)),      // Velocity: active indentation.
            ("stack_guide", alpha(0.25)),      // Velocity: enclosing indentation.
            ("find_highlight", mix(0.3)),      // Velocity: visible in light/dark schemes.
            ("find_highlight_foreground", fg), // Velocity: retain readable match text.
            ("misspelling", alpha(0.7)),       // Velocity: visible diagnostic underline.
            ("line_diff_added", mix(0.6)),     // Velocity: neutral fallback diff markers.
            ("line_diff_modified", mix(0.45)), // Velocity: distinct neutral intensity.
            ("line_diff_deleted", mix(0.8)),   // Velocity: strongest neutral intensity.
            ("caret", fg),
            ("block_caret", fg),               // Velocity: use the document ink.
            ("fold_marker", fg),               // Velocity's choice: keep folded text visible.
            ("selection", mix(0.25)),          // Velocity: readable selected background.
            ("inactive_selection", mix(0.15)), // Velocity: quieter unfocused selection.
            ("selection_border", transparent), // Velocity: opt-in borders/markers.
            ("inactive_selection_border", transparent),
            ("minimap_border", transparent),
            ("block_caret_border", transparent),
            ("block_caret_underline", fg), // Velocity: an underline caret is visible ink.
            ("shadow", transparent),
        ] {
            self.colors.entry(key.into()).or_insert(color);
        }
    }
}
