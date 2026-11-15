//! Headless `.sublime-theme` compilation, resource inheritance and cached widget styles.
//!
//! Compile after package/User merging and after capturing the global color-scheme palette.
//! Keep one [`Resolver`] per UI thread. A warmed [`Resolver::resolve`] returns a borrowed style
//! without allocating; widget animation state and GPU texture uploads belong to the caller.
#![forbid(unsafe_code)]
mod palette;
mod properties;
mod registry;
mod textures;
use fnv::{FnvHashMap, FnvHasher};
pub use palette::Palette;
pub use properties::*;
pub use registry::{ELEMENTS, ElementInfo, WIDGET_ALIASES, luminosity};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    sync::Arc,
};
pub use textures::*;
pub use vel_color_scheme::Color;
pub use vel_config::settings::Settings;

/// Invalid source, selector, property, inheritance or PNG resource.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid JSON or typed value.
    #[error("invalid theme: {0}")]
    Format(String),
    /// Shared color syntax error.
    #[error(transparent)]
    Color(#[from] vel_color_scheme::Error),
    /// Resource read failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// PNG decode failure.
    #[error("invalid PNG: {0}")]
    Png(String),
}
/// Target operating system, part of the style cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// macOS (`osx` in theme files).
    Mac,
    /// Microsoft Windows.
    Windows,
    /// Linux.
    Linux,
}
impl Platform {
    fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "osx" => Ok(Self::Mac),
            "windows" => Ok(Self::Windows),
            "linux" => Ok(Self::Linux),
            _ => Err(Error::Format(format!("unknown platform {s}"))),
        }
    }
}
/// Appearance supplied by the OS or application preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    /// Velocity Dark.
    Dark,
    /// Velocity Light.
    Light,
}
/// Borrowed widget classes and dynamic attributes. Classes are space-separated conjunctions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Element<'a> {
    /// Original vel-ui or canonical theme classes.
    pub class: &'a str,
    /// States such as selected, dirty, focused, hover and pressed.
    pub attributes: &'a [&'a str],
}
impl<'a> Element<'a> {
    /// Creates an element without copying its class or attributes.
    pub const fn new(class: &'a str, attributes: &'a [&'a str]) -> Self {
        Self { class, attributes }
    }
    fn has_class(self, class: &str) -> bool {
        self.class.split_whitespace().any(|c| {
            c == class
                || WIDGET_ALIASES.iter().any(|(alias, target)| {
                    *alias == c && target.split_whitespace().any(|c| c == class)
                })
        })
    }
}
/// Root-to-leaf ancestor chain, including the widget being styled as the last element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementPath<'a>(pub &'a [Element<'a>]);

/// Uncompiled lenient JSON. Keep variables unresolved until every override is merged.
#[derive(Debug, Clone)]
pub struct ThemeSource(Value);
impl ThemeSource {
    /// Accepts modern objects and legacy rule arrays, comments, BOM and trailing commas.
    pub fn parse(source: &str) -> Result<Self, Error> {
        let v = vel_config::json::Document::parse(source)
            .map_err(Error::Format)?
            .value;
        let v = if v.is_array() { json!({"rules":v}) } else { v };
        if !v.is_object() {
            return Err(Error::Format("root must be object or rule array".into()));
        }
        if let Some(v) = v.get("rules")
            && !v.is_array()
        {
            return Err(Error::Format("rules must be array".into()));
        }
        if let Some(v) = v.get("variables")
            && !v.is_object()
        {
            return Err(Error::Format("variables must be object".into()));
        }
        if let Some(v) = v.get("extends")
            && !v.is_string()
        {
            return Err(Error::Format("extends must be string".into()));
        }
        Ok(Self(v))
    }
    /// Appends rules and replaces variables by name; scalar metadata uses the last source.
    pub fn merge(&mut self, overlay: &Self) {
        for (k, v) in overlay.0.as_object().expect("validated object") {
            match k.as_str() {
                "rules" => self
                    .0
                    .as_object_mut()
                    .unwrap()
                    .entry(k.clone())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap()
                    .extend(v.as_array().unwrap().iter().cloned()),
                "variables" => self
                    .0
                    .as_object_mut()
                    .unwrap()
                    .entry(k.clone())
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .unwrap()
                    .extend(v.as_object().unwrap().clone()),
                _ => {
                    self.0[k] = v.clone();
                }
            }
        }
    }
    /// Compiles a flattened source. Use [`load_resources`] to resolve `extends` first.
    pub fn compile(&self, palette: &Palette) -> Result<Theme, Error> {
        if self.0.get("extends").is_some() {
            return Err(Error::Format("extends needs load_resources".into()));
        }
        let vars: BTreeMap<String, Value> = self
            .0
            .get("variables")
            .and_then(Value::as_object)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        for v in vars.values() {
            properties::resolve_variable(v, &vars, 0)?;
        }
        let mut rules = Vec::new();
        let mut setting_keys = Vec::new();
        let mut diagnostics = Vec::new();
        for (index, rule) in self
            .0
            .get("rules")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let selector = Selector::parse(rule)?;
            for s in &selector.settings {
                setting_keys.push(s.key.clone());
            }
            let mut values = BTreeMap::new();
            for (name, value) in rule
                .as_object()
                .ok_or_else(|| Error::Format("rule must be object".into()))?
            {
                if ["class", "attributes", "parents", "settings", "platforms"]
                    .contains(&name.as_str())
                {
                    continue;
                }
                let property = Property::named(name);
                if matches!(property, Property::Extension(_)) {
                    diagnostics.push(format!("rule {index}: unrecognized property {name}"));
                }
                let value = properties::parse_value(&property, value, &vars, palette)
                    .map_err(|e| Error::Format(format!("rule {index}, {name}: {e}")))?;
                values.insert(property, value);
            }
            rules.push(Rule { selector, values });
        }
        setting_keys.sort();
        setting_keys.dedup();
        Ok(Theme {
            rules,
            setting_keys,
            diagnostics,
        })
    }
}
/// One package resource. Providers enumerate installed packages; this engine performs no I/O.
#[derive(Debug, Clone, Copy)]
pub struct Resource<'a> {
    /// `Packages/<package>/<name>.sublime-theme` or a package-relative resource name.
    pub path: &'a str,
    /// UTF-8 lenient JSON.
    pub contents: &'a str,
}
/// Loads package fragments in lexical package order, User last, then flattens inheritance.
/// Qualified `extends` references select a resource path; bare names merge matching fragments.
pub fn load_resources(
    resources: &[Resource<'_>],
    name: &str,
    palette: &Palette,
) -> Result<Theme, Error> {
    fn source(
        resources: &[Resource<'_>],
        name: &str,
        active: &mut Vec<String>,
    ) -> Result<ThemeSource, Error> {
        if active.len() >= 64 || active.iter().any(|s| s == name) {
            return Err(Error::Format(format!("cyclic/deep inheritance: {name}")));
        }
        active.push(name.into());
        let mut found: Vec<_> = resources
            .iter()
            .filter(|r| {
                if name.contains('/') {
                    r.path.trim_start_matches("Packages/") == name.trim_start_matches("Packages/")
                } else {
                    r.path.rsplit('/').next() == Some(name)
                }
            })
            .collect();
        found.sort_by_key(|r| {
            (
                r.path.trim_start_matches("Packages/").starts_with("User/"),
                r.path,
            )
        });
        if found.is_empty() {
            return Err(Error::Format(format!("theme resource not found: {name}")));
        }
        let mut out = ThemeSource::parse("{}")?;
        for r in found {
            out.merge(&ThemeSource::parse(r.contents)?);
        }
        if let Some(base) = out
            .0
            .get("extends")
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            let mut inherited = source(resources, &base, active)?;
            out.0.as_object_mut().unwrap().remove("extends");
            inherited.merge(&out);
            out = inherited;
        }
        active.pop();
        Ok(out)
    }
    source(resources, name, &mut Vec::new())?.compile(palette)
}
#[derive(Debug, Clone)]
struct MatchElement {
    classes: Vec<String>,
    attributes: Vec<String>,
}
fn strings(v: &Value) -> Result<Vec<String>, Error> {
    v.as_array()
        .ok_or_else(|| Error::Format("expected string array".into()))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::Format("expected string".into()))
        })
        .collect()
}
impl MatchElement {
    fn parse(v: &Value) -> Result<Self, Error> {
        let classes: Vec<_> = v
            .get("class")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Format("selector class missing".into()))?
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if classes.is_empty() {
            return Err(Error::Format("empty class selector".into()));
        }
        let attributes = v
            .get("attributes")
            .map(strings)
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            classes,
            attributes,
        })
    }
    fn matches(&self, e: Element<'_>) -> bool {
        self.classes.iter().all(|c| e.has_class(c))
            && self.attributes.iter().all(|a| {
                if let Some(a) = a.strip_prefix('!') {
                    !e.attributes.contains(&a)
                } else {
                    e.attributes.contains(&a.as_str())
                }
            })
    }
}
#[derive(Debug, Clone)]
struct Setting {
    key: String,
    value: SettingValue,
}
#[derive(Debug, Clone)]
enum SettingValue {
    Bool(bool),
    Strings(Vec<String>),
}
impl Setting {
    fn matches(&self, settings: &Settings) -> bool {
        match &self.value {
            SettingValue::Bool(b) => settings.bool(&self.key, false) == *b,
            SettingValue::Strings(values) => {
                values.iter().any(|v| v == settings.string(&self.key, ""))
            }
        }
    }
}
#[derive(Debug, Clone)]
struct Selector {
    element: MatchElement,
    parents: Vec<MatchElement>,
    settings: Vec<Setting>,
    platforms: Vec<Platform>,
}
impl Selector {
    fn parse(v: &Value) -> Result<Self, Error> {
        let element = MatchElement::parse(v)?;
        let parents = v
            .get("parents")
            .map(|v| {
                v.as_array()
                    .ok_or_else(|| Error::Format("parents must be array".into()))?
                    .iter()
                    .map(MatchElement::parse)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        let platforms = v
            .get("platforms")
            .map(strings)
            .transpose()?
            .unwrap_or_default()
            .iter()
            .map(|p| Platform::parse(p))
            .collect::<Result<_, _>>()?;
        let mut settings = Vec::new();
        if let Some(v) = v.get("settings") {
            if v.is_array() {
                for s in strings(v)? {
                    let (key, b) = s
                        .strip_prefix('!')
                        .map_or((s.as_str(), true), |s| (s, false));
                    settings.push(Setting {
                        key: key.into(),
                        value: SettingValue::Bool(b),
                    });
                }
            } else if let Some(v) = v.as_object() {
                for (key, value) in v {
                    let value = if let Some(b) = value.as_bool() {
                        SettingValue::Bool(b)
                    } else if let Some(s) = value.as_str() {
                        SettingValue::Strings(vec![s.into()])
                    } else {
                        SettingValue::Strings(strings(value)?)
                    };
                    settings.push(Setting {
                        key: key.clone(),
                        value,
                    });
                }
            } else {
                return Err(Error::Format("settings must be object or array".into()));
            }
        }
        Ok(Self {
            element,
            parents,
            settings,
            platforms,
        })
    }
    fn matches(&self, path: ElementPath<'_>, settings: &Settings, platform: Platform) -> bool {
        let Some((leaf, ancestors)) = path.0.split_last() else {
            return false;
        };
        if !self.element.matches(*leaf)
            || !self.settings.iter().all(|s| s.matches(settings))
            || (!self.platforms.is_empty() && !self.platforms.contains(&platform))
        {
            return false;
        }
        let mut next = 0;
        for parent in &self.parents {
            let Some(at) = ancestors[next..].iter().position(|e| parent.matches(*e)) else {
                return false;
            };
            next += at + 1;
        }
        true
    }
}
#[derive(Debug, Clone)]
struct Rule {
    selector: Selector,
    values: BTreeMap<Property, PropertyValue>,
}
/// Immutable compiled rule set. No CSS specificity is applied: later properties win.
#[derive(Debug, Clone)]
pub struct Theme {
    rules: Vec<Rule>,
    setting_keys: Vec<String>,
    /// Unsupported extension keys are retained with diagnostics, rather than discarded.
    pub diagnostics: Vec<String>,
}
impl Theme {
    /// Compiles a standalone theme with no inheritance.
    pub fn parse(source: &str, palette: &Palette) -> Result<Self, Error> {
        ThemeSource::parse(source)?.compile(palette)
    }
    /// Original Velocity theme, following an appearance chosen by the caller.
    pub fn velocity(appearance: Appearance, palette: &Palette) -> Self {
        let resources = [
            Resource {
                path: "Theme - Velocity/Velocity.sublime-theme",
                contents: include_str!(
                    "../../velocity/resources/Packages/Theme - Velocity/Velocity.sublime-theme"
                ),
            },
            Resource {
                path: "Theme - Velocity/Velocity Light.sublime-theme",
                contents: include_str!(
                    "../../velocity/resources/Packages/Theme - Velocity/Velocity Light.sublime-theme"
                ),
            },
        ];
        load_resources(
            &resources,
            if appearance == Appearance::Dark {
                "Velocity.sublime-theme"
            } else {
                "Velocity Light.sublime-theme"
            },
            palette,
        )
        .expect("validated original theme")
    }
    /// Number of compiled rules, useful for fixture coverage reports.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
    fn resolve(
        &self,
        path: ElementPath<'_>,
        settings: &Settings,
        platform: Platform,
    ) -> ResolvedStyle {
        let mut out = ResolvedStyle::default();
        for rule in &self.rules {
            if rule.selector.matches(path, settings, platform) {
                for (key, value) in &rule.values {
                    if matches!(value, PropertyValue::Reset) {
                        out.values.remove(key);
                    } else {
                        out.values.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        out
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    path: u64,
    settings: u64,
    revision: u64,
    platform: Platform,
}
#[derive(Debug)]
struct Cached {
    path: Vec<(String, Vec<String>)>,
    settings: Vec<Option<Value>>,
    style: ResolvedStyle,
}
/// Bounded style cache. Hash collisions are checked against full paths and relevant settings.
#[derive(Debug)]
pub struct Resolver {
    theme: Arc<Theme>,
    cache: FnvHashMap<CacheKey, Vec<Cached>>,
    capacity: usize,
    entries: usize,
}
impl Resolver {
    /// Creates a cache for 4096 distinct widget/state/configuration combinations.
    pub fn new(theme: Arc<Theme>) -> Self {
        Self::with_capacity(theme, 4096)
    }
    /// Creates a bounded cache. At capacity, the next miss clears old entries.
    pub fn with_capacity(theme: Arc<Theme>, capacity: usize) -> Self {
        Self {
            theme,
            cache: FnvHashMap::default(),
            capacity: capacity.max(1),
            entries: 0,
        }
    }
    /// Invalidates all styles after a theme/palette reload.
    pub fn replace_theme(&mut self, theme: Arc<Theme>) {
        self.theme = theme;
        self.clear();
    }
    /// Clears cached path and settings snapshots.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.entries = 0;
    }
    /// Resolves a root-to-leaf path. Repeated lookups allocate nothing and borrow the cached style.
    /// Settings revisions, relevant setting values and platform are all part of cache identity.
    pub fn resolve(
        &mut self,
        path: &ElementPath<'_>,
        settings: &Settings,
        platform: Platform,
    ) -> &ResolvedStyle {
        let mut path_hasher = FnvHasher::default();
        path.hash(&mut path_hasher);
        let mut settings_hasher = FnvHasher::default();
        for key in &self.theme.setting_keys {
            settings.get(key).hash(&mut settings_hasher);
        }
        let key = CacheKey {
            path: path_hasher.finish(),
            settings: settings_hasher.finish(),
            revision: settings.revision(),
            platform,
        };
        let index = self.cache.get(&key).and_then(|bucket| {
            bucket.iter().position(|e| {
                e.path.len() == path.0.len()
                    && e.path.iter().zip(path.0).all(|((class, attrs), e)| {
                        class == e.class
                            && attrs.len() == e.attributes.len()
                            && attrs.iter().zip(e.attributes).all(|(a, b)| a == b)
                    })
                    && e.settings
                        .iter()
                        .zip(&self.theme.setting_keys)
                        .all(|(v, k)| v.as_ref() == settings.get(k))
            })
        });
        let index = if let Some(index) = index {
            index
        } else {
            if self.entries >= self.capacity {
                self.clear();
            }
            let style = self.theme.resolve(*path, settings, platform);
            let entry = Cached {
                path: path
                    .0
                    .iter()
                    .map(|e| {
                        (
                            e.class.into(),
                            e.attributes.iter().map(|s| (*s).into()).collect(),
                        )
                    })
                    .collect(),
                settings: self
                    .theme
                    .setting_keys
                    .iter()
                    .map(|k| settings.get(k).cloned())
                    .collect(),
                style,
            };
            let bucket = self.cache.entry(key).or_default();
            bucket.push(entry);
            self.entries += 1;
            bucket.len() - 1
        };
        &self.cache.get(&key).expect("inserted cache entry")[index].style
    }
}
