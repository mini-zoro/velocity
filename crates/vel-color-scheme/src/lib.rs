//! Clean-room Sublime color schemes, overrides, and cached scope style resolution.
#![forbid(unsafe_code)]
mod color;
mod globals;
pub use globals::{CornerStyle, DecorationOptions};
#[cfg(test)]
mod resource_colors;
pub use color::Color;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
};
use vel_scope::{Scope, ScopeStack, Selector};

/// Invalid scheme or color expression, including malformed overrides.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid JSON/plist structure.
    #[error("invalid color scheme: {0}")]
    Format(String),
    /// Invalid color or cyclic variable reference.
    #[error("invalid color: {0}")]
    Color(String),
    /// Invalid scope selector.
    #[error(transparent)]
    Selector(#[from] vel_scope::ParseScopeError),
    /// Resource read failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
/// Complete font decorations; the empty value explicitly resets previous decorations.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontStyle(u8);
impl FontStyle {
    /// Bold weight.
    pub const BOLD: Self = Self(1);
    /// Italic face.
    pub const ITALIC: Self = Self(2);
    /// Solid underline.
    pub const UNDERLINE: Self = Self(4);
    /// Dotted underline.
    pub const STIPPLED_UNDERLINE: Self = Self(8);
    /// Wavy underline.
    pub const SQUIGGLY_UNDERLINE: Self = Self(16);
    /// Glow decoration.
    pub const GLOW: Self = Self(32);
    /// Tests one or more decoration bits.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    /// Parses all documented space-separated font styles.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut bits = 0;
        for part in text.split_whitespace() {
            bits |= match part {
                "bold" => 1,
                "italic" => 2,
                "underline" => 4,
                "stippled_underline" => 8,
                "squiggly_underline" => 16,
                "glow" => 32,
                _ => return Err(Error::Format(format!("unknown font style {part}"))),
            };
        }
        Ok(Self(bits))
    }
}
/// Fully resolved, cheap-to-copy rendering style. Gradient IDs belong to the owning scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// Text color before optional token hashing.
    pub foreground: Color,
    /// Background color.
    pub background: Color,
    /// Optional selected-text override.
    pub selection_foreground: Option<Color>,
    /// Decorations, including underline variants and glow.
    pub font_style: FontStyle,
    /// Optional scheme-local hashed gradient.
    pub hashed_foreground: Option<usize>,
    adjustment: Option<usize>,
}
/// Every global is preserved; color-valued globals are available without reparsing.
#[derive(Debug, Clone, Default)]
pub struct Globals {
    colors: BTreeMap<String, Color>,
    values: BTreeMap<String, Value>,
    numbers: BTreeMap<String, f64>,
    corners: BTreeMap<String, CornerStyle>,
    options: BTreeMap<String, DecorationOptions>,
}
impl Globals {
    /// A resolved color global (caret, gutter, selection, guides, diffs, brackets, etc.).
    pub fn color(&self, key: &str) -> Option<Color> {
        self.colors.get(key).copied()
    }
    /// Parsed nonnegative finite width or radius, accepting numeric strings.
    pub fn number(&self, key: &str) -> Option<f64> {
        self.numbers.get(key).copied()
    }
    /// Parsed corner style.
    pub fn corner_style(&self, key: &str) -> Option<CornerStyle> {
        self.corners.get(key).copied()
    }
    /// Parsed bracket/tag drawing options.
    pub fn decoration_options(&self, key: &str) -> Option<DecorationOptions> {
        self.options.get(key).copied()
    }
    /// Original scalar, CSS, or options value, including future global keys.
    pub fn value(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }
    /// All globals in deterministic key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v))
    }
}
#[derive(Debug, Clone)]
struct Rule {
    selector: Selector,
    foreground: Option<Color>,
    background: Option<Color>,
    selection_foreground: Option<Color>,
    font_style: Option<FontStyle>,
    gradient: Option<usize>,
    adjustment: Option<usize>,
}
/// Immutable compiled scheme, shareable between document workers.
#[derive(Debug, Clone)]
pub struct ColorScheme {
    /// Display name.
    pub name: String,
    /// Hidden schemes may be assigned but omitted from pickers.
    pub hidden: bool,
    /// All documented globals and any extension keys.
    pub globals: Globals,
    rules: Vec<Rule>,
    gradients: Vec<[Color; 256]>,
    adjustments: Vec<String>,
    variables: BTreeMap<String, String>,
    warnings: Vec<String>,
}
/// Unresolved scheme. Merge overrides before evaluating variables or compiling selectors.
#[derive(Debug, Clone)]
pub struct SchemeSource(Value, Vec<String>);
impl SchemeSource {
    /// Lenient JSON accepts comments, BOM, and trailing commas.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let value = vel_config::json::Document::parse(text)
            .map_err(Error::Format)?
            .value;
        if !value.is_object() {
            return Ok(Self(json!({}), vec!["root must be an object".into()]));
        }
        Ok(Self(value, Vec::new()))
    }
    /// Parses a legacy plist theme into the same unresolved model.
    pub fn from_tm_theme(xml: &str) -> Result<Self, Error> {
        let mut normalized = String::with_capacity(xml.len());
        let mut rest = xml;
        while let Some(at) = rest.find("<![CDATA[") {
            normalized.push_str(&rest[..at]);
            rest = &rest[at + 9..];
            let end = rest
                .find("]]>")
                .ok_or_else(|| Error::Format("unterminated CDATA".into()))?;
            normalized.push_str(
                &rest[..end]
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;"),
            );
            rest = &rest[end + 3..];
        }
        normalized.push_str(rest);
        let plist = plist::Value::from_reader_xml(normalized.as_bytes())
            .map_err(|e| Error::Format(e.to_string()))?;
        let dict = plist
            .as_dictionary()
            .ok_or_else(|| Error::Format("theme must be a dictionary".into()))?;
        let mut root = json!({"name":dict.get("name").and_then(plist::Value::as_string).unwrap_or("Untitled"),"globals":{},"rules":[]});
        let settings = dict
            .get("settings")
            .and_then(plist::Value::as_array)
            .ok_or_else(|| Error::Format("theme settings missing".into()))?;
        let mut warnings = Vec::new();
        for entry in settings {
            let Some(d) = entry.as_dictionary() else {
                warnings.push("invalid theme entry".into());
                continue;
            };
            let Some(s) = d.get("settings").and_then(plist::Value::as_dictionary) else {
                continue;
            };
            let mut obj = serde_json::Map::new();
            for (k, v) in s {
                let mut key = String::new();
                for ch in k.chars() {
                    if ch.is_ascii_uppercase() {
                        if !key.is_empty() {
                            key.push('_');
                        }
                        key.push(ch.to_ascii_lowercase());
                    } else {
                        key.push(ch);
                    }
                }
                if let Some(v) = v.as_string() {
                    obj.insert(key, Value::String(v.into()));
                } else if let Some(v) = v.as_signed_integer() {
                    obj.insert(key, json!(v));
                } else if let Some(v) = v.as_real() {
                    obj.insert(key, json!(v));
                }
            }
            if let Some(scope) = d
                .get("scope")
                .and_then(plist::Value::as_string)
                .filter(|s| !s.is_empty())
            {
                obj.insert("scope".into(), json!(scope));
                root["rules"]
                    .as_array_mut()
                    .unwrap()
                    .push(Value::Object(obj));
            } else {
                root["globals"].as_object_mut().unwrap().extend(obj);
            }
        }
        Ok(Self(root, warnings))
    }
    /// Variables/globals overwrite by key, rules append, and scalar metadata overwrites.
    pub fn merge(&mut self, overlay: &Self) -> Result<(), Error> {
        self.1.extend(overlay.1.iter().cloned());
        for (key, value) in overlay.0.as_object().unwrap() {
            match key.as_str() {
                "variables" | "globals" => {
                    let Some(map) = value.as_object() else {
                        self.1.push(format!("{key} must be an object"));
                        continue;
                    };
                    if !self.0[key].is_object() {
                        if !self.0[key].is_null() {
                            self.1.push(format!("{key} must be an object"));
                        }
                        self.0[key] = json!({});
                    }
                    self.0[key].as_object_mut().unwrap().extend(map.clone());
                }
                "rules" => {
                    let Some(rules) = value.as_array() else {
                        self.1.push("rules must be an array".into());
                        continue;
                    };
                    if !self.0[key].is_array() {
                        if !self.0[key].is_null() {
                            self.1.push(format!("{key} must be an array"));
                        }
                        self.0[key] = json!([]);
                    }
                    self.0[key].as_array_mut().unwrap().extend(rules.clone());
                }
                _ => {
                    self.0[key] = value.clone();
                }
            }
        }
        Ok(())
    }
    /// Resolves variables after all package/User overrides have been merged.
    pub fn compile(&self) -> Result<ColorScheme, Error> {
        let mut scheme = ColorScheme::compile(&self.0)?;
        scheme.warnings.extend(self.1.iter().cloned());
        Ok(scheme)
    }
}
fn string<'a>(v: &'a Value, key: &str) -> Result<Option<&'a str>, Error> {
    v.get(key)
        .map(|v| {
            v.as_str()
                .ok_or_else(|| Error::Format(format!("{key} must be a string")))
        })
        .transpose()
}
impl ColorScheme {
    /// Loads one JSON scheme or plist theme, including hidden scheme files.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        let source = if path
            .extension()
            .is_some_and(|s| s == "tmTheme" || s == "hidden-tmTheme")
        {
            SchemeSource::from_tm_theme(&text)?
        } else {
            SchemeSource::parse(&text)?
        };
        let mut scheme = source.compile()?;
        scheme.hidden |= path
            .extension()
            .is_some_and(|s| s == "hidden-color-scheme" || s == "hidden-tmTheme");
        Ok(scheme)
    }
    /// Compiles one lenient JSON scheme.
    pub fn parse(text: &str) -> Result<Self, Error> {
        SchemeSource::parse(text)?.compile()
    }
    /// Original Velocity dark palette, embedded from the shipped resource.
    pub fn velocity_dark() -> Self {
        Self::parse(include_str!("../../velocity/resources/Packages/Color Scheme - Velocity/Velocity Dark.sublime-color-scheme")).expect("validated bundled scheme")
    }
    /// Original Velocity light palette, embedded from the shipped resource.
    pub fn velocity_light() -> Self {
        Self::parse(include_str!("../../velocity/resources/Packages/Color Scheme - Velocity/Velocity Light.sublime-color-scheme")).expect("validated bundled scheme")
    }
    fn compile(root: &Value) -> Result<Self, Error> {
        let mut out = Self {
            name: root
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Untitled")
                .into(),
            hidden: root.get("hidden").and_then(Value::as_bool).unwrap_or(false),
            globals: Globals::default(),
            rules: Vec::new(),
            gradients: Vec::new(),
            adjustments: Vec::new(),
            variables: BTreeMap::new(),
            warnings: Vec::new(),
        };
        for key in ["name", "author"] {
            if let Some(value) = root.get(key)
                && !value.is_string()
            {
                out.warnings.push(format!("{key} must be a string"));
            }
        }
        if let Some(vars) = root.get("variables") {
            if let Some(vars) = vars.as_object() {
                for (key, value) in vars {
                    if let Some(value) = value.as_str() {
                        // Variables may contain CSS or other non-color text. Validate only uses.
                        out.variables.insert(key.clone(), value.into());
                    } else {
                        out.warnings
                            .push(format!("variables.{key}: expected a string"));
                    }
                }
            } else {
                out.warnings.push("variables must be an object".into());
            }
        }
        if let Some(globals) = root.get("globals") {
            if let Some(globals) = globals.as_object() {
                for (key, value) in globals {
                    out.globals
                        .compile_value(key, value, &out.variables, &mut out.warnings);
                }
            } else {
                out.warnings.push("globals must be an object".into());
            }
        }
        if let Some(rules) = root.get("rules") {
            if let Some(rules) = rules.as_array() {
                for (index, rule) in rules.iter().enumerate() {
                    if let Some(rule) = out.compile_rule(rule, index) {
                        out.rules.push(rule);
                    }
                }
            } else {
                out.warnings.push("rules must be an array".into());
            }
        }
        out.globals.derive_defaults();
        Ok(out)
    }
    fn compile_rule(&mut self, rule: &Value, index: usize) -> Option<Rule> {
        let prefix = format!("rules[{index}]");
        let selector = recover(
            &mut self.warnings,
            &format!("{prefix}.scope"),
            string(rule, "scope").and_then(|s| {
                Selector::parse(s.ok_or_else(|| Error::Format("missing scope".into()))?)
                    .map_err(Error::from)
            }),
        )?;
        let mut color = |key: &str| {
            recover(
                &mut self.warnings,
                &format!("{prefix}.{key}"),
                string(rule, key)
                    .and_then(|s| s.map(|s| Color::parse(s, &self.variables)).transpose()),
            )
            .flatten()
        };
        let background = color("background");
        let selection_foreground = color("selection_foreground");
        let gradient_value = rule
            .get("hashed_foreground")
            .or_else(|| rule.get("foreground").filter(|v| v.is_array()));
        let (foreground, gradient) = if let Some(value) = gradient_value {
            let colors = (|| {
                let array = value
                    .as_array()
                    .filter(|a| a.len() >= 2)
                    .ok_or_else(|| Error::Format("gradient needs at least two colors".into()))?;
                array
                    .iter()
                    .map(|v| {
                        Color::parse(
                            v.as_str().ok_or_else(|| {
                                Error::Format("gradient colors must be strings".into())
                            })?,
                            &self.variables,
                        )
                    })
                    .collect::<Result<Vec<_>, Error>>()
            })();
            if let Some(colors) =
                recover(&mut self.warnings, &format!("{prefix}.foreground"), colors)
            {
                let table = std::array::from_fn(|i| {
                    let at = i as f64 / 255. * (colors.len() - 1) as f64;
                    let index = (at as usize).min(colors.len() - 2);
                    colors[index].lerp(colors[index + 1], at - index as f64, true, true)
                });
                let id = self.gradients.len();
                self.gradients.push(table);
                (Some(colors[0]), Some(id))
            } else {
                (None, None)
            }
        } else {
            (color("foreground"), None)
        };
        let adjustment = (|| -> Result<Option<usize>, Error> {
            let Some(text) = string(rule, "foreground_adjust")? else {
                return Ok(None);
            };
            if background.is_none() {
                return Err(Error::Format(
                    "foreground_adjust requires background".into(),
                ));
            }
            Color::parse(&format!("color(white {text})"), &self.variables)?;
            let id = self.adjustments.len();
            self.adjustments.push(text.into());
            Ok(Some(id))
        })();
        let adjustment = recover(
            &mut self.warnings,
            &format!("{prefix}.foreground_adjust"),
            adjustment,
        )
        .flatten();
        let font_style = recover(
            &mut self.warnings,
            &format!("{prefix}.font_style"),
            string(rule, "font_style").and_then(|s| s.map(FontStyle::parse).transpose()),
        )
        .flatten();
        Some(Rule {
            selector,
            foreground,
            background,
            selection_foreground,
            font_style,
            gradient,
            adjustment,
        })
    }
    /// Recoverable per-key/per-rule diagnostics. Valid neighboring items remain usable.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    /// Selection corner geometry, defaulting to the documented round style.
    pub fn selection_corner_style(&self) -> CornerStyle {
        self.globals
            .corner_style("selection_corner_style")
            .unwrap_or_default()
    }
    /// Selection radius in pixels (Velocity's fallback is 2).
    pub fn selection_corner_radius(&self) -> f64 {
        self.globals.number("selection_corner_radius").unwrap_or(2.)
    }
    /// Block caret corner geometry.
    pub fn block_caret_corner_style(&self) -> CornerStyle {
        self.globals
            .corner_style("block_caret_corner_style")
            .unwrap_or_default()
    }
    /// Block caret radius in pixels (Velocity's fallback is 2).
    pub fn block_caret_corner_radius(&self) -> f64 {
        self.globals
            .number("block_caret_corner_radius")
            .unwrap_or(2.)
    }
    /// Explicit bracket decorations; absence lets the renderer retain its default.
    pub fn brackets_options(&self) -> Option<DecorationOptions> {
        self.globals.decoration_options("brackets_options")
    }
    /// Explicit bracket-content decorations.
    pub fn bracket_contents_options(&self) -> Option<DecorationOptions> {
        self.globals.decoration_options("bracket_contents_options")
    }
    /// Explicit matching-tag decorations.
    pub fn tags_options(&self) -> Option<DecorationOptions> {
        self.globals.decoration_options("tags_options")
    }
    /// Base document style used for unstyled/approximate lines.
    pub fn default_style(&self) -> Style {
        Style {
            foreground: self
                .globals
                .color("foreground")
                .unwrap_or(Color::rgb(0, 0, 0)),
            background: self
                .globals
                .color("background")
                .unwrap_or(Color::rgb(255, 255, 255)),
            selection_foreground: self.globals.color("selection_foreground"),
            font_style: FontStyle::default(),
            hashed_foreground: None,
            adjustment: None,
        }
    }
    fn resolve(&self, stack: &[Scope]) -> Style {
        let mut matching: Vec<_> = self
            .rules
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.selector.does_match(stack).map(|s| (s, i, r)))
            .collect();
        matching.sort_by_key(|(s, i, _)| (*s, *i));
        let mut style = self.default_style();
        for (_, _, rule) in matching {
            if let Some(c) = rule.foreground {
                style.foreground = c;
                style.hashed_foreground = rule.gradient;
            }
            if let Some(c) = rule.background {
                style.background = c;
                style.adjustment = rule.adjustment;
            }
            if let Some(c) = rule.selection_foreground {
                style.selection_foreground = Some(c);
            }
            if let Some(f) = rule.font_style {
                style.font_style = f;
            }
        }
        self.adjust(style)
    }
    fn adjust(&self, mut style: Style) -> Style {
        if let Some(i) = style.adjustment {
            for part in color::parts(&self.adjustments[i]) {
                style.foreground = color::adjust(style.foreground, part, &self.variables, 0)
                    .expect("validated adjustment");
            }
        }
        style
    }
    /// Resolves a token's deterministic gradient color, retaining its other attributes.
    /// Token hashes use FNV-1a; their exact distribution is Velocity-defined.
    pub fn style_for_token(&self, mut style: Style, token: &str) -> Style {
        if let Some(id) = style.hashed_foreground {
            let mut hash = 2166136261u32;
            for byte in token.bytes() {
                hash = (hash ^ u32::from(byte)).wrapping_mul(16777619);
            }
            style.foreground = self.gradients[id][(hash & 255) as usize];
            style = self.adjust(style);
        }
        style
    }
}
/// Scheme-local interned scope-stack identity; only use with the creating highlighter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeStackId(usize);
/// Thread-local style resolver. Warm lookups allocate nothing and take no global locks.
#[derive(Debug)]
pub struct Highlighter {
    scheme: Arc<ColorScheme>,
    stacks: HashMap<Vec<Scope>, ScopeStackId, fnv::FnvBuildHasher>,
    styles: Vec<Style>,
}
impl Highlighter {
    /// Creates an empty cache; swap the resolver when a scheme changes.
    pub fn new(scheme: Arc<ColorScheme>) -> Self {
        Self {
            scheme,
            stacks: HashMap::default(),
            styles: Vec::new(),
        }
    }
    /// Interns active scopes (pending clears do not affect the style).
    pub fn intern(&mut self, stack: &ScopeStack) -> ScopeStackId {
        if let Some(id) = self.stacks.get(stack.as_slice()) {
            return *id;
        }
        let id = ScopeStackId(self.styles.len());
        self.styles.push(self.scheme.resolve(stack.as_slice()));
        self.stacks.insert(stack.as_slice().to_vec(), id);
        id
    }
    /// Resolves/caches a style, allocation-free on cache hits.
    pub fn style_for(&mut self, stack: &ScopeStack) -> Style {
        let id = self.intern(stack);
        self.style_for_id(id)
    }
    /// Direct indexed cache hit, useful when render spans already store interned IDs.
    pub fn style_for_id(&self, id: ScopeStackId) -> Style {
        self.styles[id.0]
    }
    /// Owning immutable scheme.
    pub fn scheme(&self) -> &Arc<ColorScheme> {
        &self.scheme
    }
}
/// Resolves explicit or OS-dependent automatic pair selection from editor settings.
pub fn selected_scheme(settings: &Value, dark: bool) -> &str {
    match settings.get("color_scheme").and_then(Value::as_str) {
        Some("auto") | None => settings
            .get(if dark {
                "dark_color_scheme"
            } else {
                "light_color_scheme"
            })
            .and_then(Value::as_str)
            .unwrap_or(if dark {
                "Velocity Dark.sublime-color-scheme"
            } else {
                "Velocity Light.sublime-color-scheme"
            }),
        Some(s) => s,
    }
}
/// Loads matching resources in package precedence order, merging User overrides by filename.
/// Pass only the resource text already obtained by vel-config; this performs no disk scans.
pub fn load_resources<'a>(
    resources: impl IntoIterator<Item = (&'a str, &'a str)>,
    filename: &str,
) -> Result<ColorScheme, Error> {
    let basename = Path::new(filename)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let mut source: Option<SchemeSource> = None;
    let mut hidden = false;
    for (path, text) in resources {
        if Path::new(path).file_name().and_then(|s| s.to_str()) != Some(basename) {
            continue;
        }
        let overlay = if path.ends_with(".tmTheme") || path.ends_with(".hidden-tmTheme") {
            SchemeSource::from_tm_theme(text)?
        } else {
            SchemeSource::parse(text)?
        };
        if let Some(base) = &mut source {
            base.merge(&overlay)?;
        } else {
            source = Some(overlay);
        }
        hidden |= path.ends_with(".hidden-color-scheme") || path.ends_with(".hidden-tmTheme");
    }
    let mut scheme = source
        .ok_or_else(|| Error::Format(format!("scheme resource not found: {filename}")))?
        .compile()?;
    scheme.hidden |= hidden;
    Ok(scheme)
}

fn recover<T>(warnings: &mut Vec<String>, key: &str, result: Result<T, Error>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            warnings.push(format!("{key}: {error}"));
            None
        }
    }
}
