//! Scheme colors shared by themes and minihtml.
use crate::Error;
use std::collections::BTreeMap;
use vel_color_scheme::{Color, ColorScheme, Highlighter};

/// Color environment captured when the application changes its global color scheme.
#[derive(Debug, Clone)]
pub struct Palette {
    /// CSS custom properties, including all eleven documented predefined variables.
    pub variables: BTreeMap<String, String>,
}
impl Default for Palette {
    fn default() -> Self {
        Self::from_scheme(&ColorScheme::velocity_dark())
    }
}
impl Palette {
    /// Derives semantic colors from scheme styles; explicit globals override the heuristics.
    /// Use [`Self::set`] for scheme-variable overrides not exposed by the scheme's globals API.
    pub fn from_scheme(scheme: &ColorScheme) -> Self {
        let mut variables = BTreeMap::new();
        let base = scheme.default_style();
        let mut highlighter = Highlighter::new(std::sync::Arc::new(scheme.clone()));
        for (name, scope) in [
            ("redish", "invalid.illegal"),
            ("orangish", "constant.character"),
            ("yellowish", "constant.numeric"),
            ("greenish", "string"),
            ("cyanish", "entity.name.class"),
            ("bluish", "entity.name.function"),
            ("purplish", "keyword"),
            ("pinkish", "variable.parameter"),
        ] {
            let color = scheme
                .globals
                .color(&format!("--{name}"))
                .unwrap_or_else(|| {
                    highlighter
                        .style_for(&scope.parse().expect("constant scope"))
                        .foreground
                });
            variables.insert(format!("--{name}"), css(color));
        }
        for (name, fallback) in [
            ("background", base.background),
            ("foreground", base.foreground),
            ("accent", base.foreground),
        ] {
            variables.insert(
                format!("--{name}"),
                css(scheme.globals.color(name).unwrap_or(fallback)),
            );
        }
        Self { variables }
    }
    /// Builds a palette from lenient scheme JSON, honoring explicitly named predefined variables.
    /// This also supports ordinary scheme variables referenced by those overrides.
    pub fn from_scheme_json(source: &str) -> Result<Self, Error> {
        let scheme = ColorScheme::parse(source)?;
        let mut palette = Self::from_scheme(&scheme);
        let document = vel_config::json::Document::parse(source).map_err(Error::Format)?;
        let vars: BTreeMap<String, String> = document
            .value
            .get("variables")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.into())))
            .collect();
        for key in palette.variables.keys().cloned().collect::<Vec<_>>() {
            if let Some(value) = vars
                .get(&key)
                .or_else(|| vars.get(key.trim_start_matches("--")))
            {
                palette
                    .variables
                    .insert(key, css(Color::parse(value, &vars)?));
            }
        }
        Ok(palette)
    }
    /// Sets a custom property to a literal color.
    pub fn set(&mut self, name: &str, color: Color) {
        self.variables.insert(name.into(), css(color));
    }
    /// Resolves a CSS color, variable reference or color-mod expression.
    pub fn color(&self, value: &str) -> Result<Color, Error> {
        Ok(Color::parse(value, &self.variables)?)
    }
    /// HSL background lightness determines minihtml's implicit `html.dark`/`html.light`.
    pub fn is_dark(&self) -> bool {
        let c = self
            .color("var(--background)")
            .unwrap_or(Color::rgb(0, 0, 0));
        u16::from(c.r.max(c.g).max(c.b)) + u16::from(c.r.min(c.g).min(c.b)) < 255
    }
}
pub(crate) fn css(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
}
