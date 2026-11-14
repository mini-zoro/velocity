//! Clean-room Sublime color schemes, overrides, and cached scope style resolution.
#![forbid(unsafe_code)]
mod color;
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
}
impl Globals {
    /// A resolved color global (caret, gutter, selection, guides, diffs, brackets, etc.).
    pub fn color(&self, key: &str) -> Option<Color> {
        self.colors.get(key).copied()
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
}
/// Unresolved scheme. Merge overrides before evaluating variables or compiling selectors.
#[derive(Debug, Clone)]
pub struct SchemeSource(Value);
impl SchemeSource {
    /// Lenient JSON accepts comments, BOM, and trailing commas.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let value = vel_config::json::Document::parse(text)
            .map_err(Error::Format)?
            .value;
        if !value.is_object() {
            return Err(Error::Format("root must be an object".into()));
        }
        Ok(Self(value))
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
        for entry in settings {
            let Some(d) = entry.as_dictionary() else {
                return Err(Error::Format("invalid theme entry".into()));
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
        Ok(Self(root))
    }
    /// Variables/globals overwrite by key, rules append, and scalar metadata overwrites.
    pub fn merge(&mut self, overlay: &Self) -> Result<(), Error> {
        for (key, value) in overlay.0.as_object().unwrap() {
            match key.as_str() {
                "variables" | "globals" => {
                    let map = value
                        .as_object()
                        .ok_or_else(|| Error::Format(format!("{key} must be an object")))?;
                    let dst = self
                        .0
                        .as_object_mut()
                        .unwrap()
                        .entry(key)
                        .or_insert_with(|| json!({}))
                        .as_object_mut()
                        .ok_or_else(|| Error::Format(format!("{key} must be an object")))?;
                    dst.extend(map.clone());
                }
                "rules" => {
                    let rules = value
                        .as_array()
                        .ok_or_else(|| Error::Format("rules must be an array".into()))?;
                    self.0
                        .as_object_mut()
                        .unwrap()
                        .entry(key)
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .ok_or_else(|| Error::Format("rules must be an array".into()))?
                        .extend(rules.clone());
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
        ColorScheme::compile(&self.0)
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
        let source = if path.extension().is_some_and(|s| s == "tmTheme") {
            SchemeSource::from_tm_theme(&text)?
        } else {
            SchemeSource::parse(&text)?
        };
        let mut scheme = source.compile()?;
        scheme.hidden |= path.extension().is_some_and(|s| s == "hidden-color-scheme");
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
            name: string(root, "name")?.unwrap_or("Untitled").into(),
            hidden: root.get("hidden").and_then(Value::as_bool).unwrap_or(false),
            globals: Globals::default(),
            rules: Vec::new(),
            gradients: Vec::new(),
            adjustments: Vec::new(),
            variables: BTreeMap::new(),
        };
        if let Some(vars) = root.get("variables") {
            for (k, v) in vars
                .as_object()
                .ok_or_else(|| Error::Format("variables must be an object".into()))?
            {
                out.variables.insert(
                    k.clone(),
                    v.as_str()
                        .ok_or_else(|| Error::Format("variable must be a string".into()))?
                        .into(),
                );
            }
        }
        for value in out.variables.values() {
            Color::parse(value, &out.variables)?;
        }
        if let Some(globals) = root.get("globals") {
            for (k, v) in globals
                .as_object()
                .ok_or_else(|| Error::Format("globals must be an object".into()))?
            {
                out.globals.values.insert(k.clone(), v.clone());
                if let Some(s) = v.as_str()
                    && !k.ends_with("_options")
                    && !k.ends_with("_css")
                    && !["font_face", "font_style"].contains(&k.as_str())
                {
                    out.globals
                        .colors
                        .insert(k.clone(), Color::parse(s, &out.variables)?);
                }
            }
        }
        if let Some(rules) = root.get("rules") {
            for rule in rules
                .as_array()
                .ok_or_else(|| Error::Format("rules must be an array".into()))?
            {
                let color = |key: &str| -> Result<Option<Color>, Error> {
                    string(rule, key)?
                        .map(|s| Color::parse(s, &out.variables))
                        .transpose()
                };
                let background = color("background")?;
                let selection_foreground = color("selection_foreground")?;
                let gradient_value = rule
                    .get("hashed_foreground")
                    .or_else(|| rule.get("foreground").filter(|v| v.is_array()));
                let (foreground, gradient) = if let Some(g) = gradient_value {
                    let a = g
                        .as_array()
                        .ok_or_else(|| Error::Format("gradient must be an array".into()))?;
                    if a.len() < 2 {
                        return Err(Error::Format("gradient needs two colors".into()));
                    }
                    let colors: Vec<_> = a
                        .iter()
                        .map(|v| {
                            Color::parse(
                                v.as_str().ok_or_else(|| {
                                    Error::Format("gradient colors must be strings".into())
                                })?,
                                &out.variables,
                            )
                        })
                        .collect::<Result<_, Error>>()?;
                    let table = std::array::from_fn(|i| {
                        let at = i as f64 / 255. * (colors.len() - 1) as f64;
                        let index = (at as usize).min(colors.len() - 2);
                        colors[index].lerp(colors[index + 1], at - index as f64, true, true)
                    });
                    let id = out.gradients.len();
                    out.gradients.push(table);
                    (Some(colors[0]), Some(id))
                } else {
                    (color("foreground")?, None)
                };
                let adjustment = if let Some(text) = string(rule, "foreground_adjust")? {
                    if background.is_none() {
                        return Err(Error::Format(
                            "foreground_adjust requires background".into(),
                        ));
                    }
                    Color::parse(&format!("color(white {text})"), &out.variables)?;
                    let i = out.adjustments.len();
                    out.adjustments.push(text.into());
                    Some(i)
                } else {
                    None
                };
                out.rules.push(Rule {
                    selector: Selector::parse(
                        string(rule, "scope")?
                            .ok_or_else(|| Error::Format("rule requires scope".into()))?,
                    )?,
                    foreground,
                    background,
                    selection_foreground,
                    font_style: string(rule, "font_style")?
                        .map(FontStyle::parse)
                        .transpose()?,
                    gradient,
                    adjustment,
                });
            }
        }
        Ok(out)
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
        let overlay = if path.ends_with(".tmTheme") {
            SchemeSource::from_tm_theme(text)?
        } else {
            SchemeSource::parse(text)?
        };
        if let Some(base) = &mut source {
            base.merge(&overlay)?;
        } else {
            source = Some(overlay);
        }
        hidden |= path.ends_with(".hidden-color-scheme");
    }
    let mut scheme = source
        .ok_or_else(|| Error::Format(format!("scheme resource not found: {filename}")))?
        .compile()?;
    scheme.hidden |= hidden;
    Ok(scheme)
}
