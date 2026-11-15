//! Observable settings, in increasing precedence order.
use serde::Deserialize;
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static VALUE_ID: AtomicU64 = AtomicU64::new(1);

/// Setting source, from lowest to highest priority.
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Layer {
    /// Bundled common values.
    Default,
    /// Bundled OS-specific values.
    Platform,
    /// User preferences.
    User,
    /// Syntax-specific values.
    Syntax,
    /// Project values.
    Project,
    /// Mode-specific editor overrides from Distraction Free.sublime-settings.
    DistractionFree,
    /// View overrides.
    View,
}
type Observer = Arc<dyn Fn(&str, Option<&Value>) + Send + Sync>;
/// Effective settings. Observers fire once per changed effective key.
#[derive(Clone, Default)]
pub struct Settings {
    revision: u64,
    cache_identity: u64,
    single_layer: Option<usize>,
    layers: [Map<String, Value>; 7],
    observers: BTreeMap<String, Observer>,
}
impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("layers", &self.layers)
            .field("observer_count", &self.observers.len())
            .finish()
    }
}
impl Settings {
    /// Monotonic configuration revision, for cached typed accessors.
    #[inline]
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Opaque process-local identity of these values. Clones share it until either mutates.
    /// Unlike the local revision counter, independent changed settings never share an identity.
    #[inline]
    pub fn cache_identity(&self) -> u64 {
        self.cache_identity
    }
    /// Copies selected effective keys into one lookup layer with a distinct value identity.
    /// The local revision is retained. Observers are not copied.
    pub fn project<'a>(&self, keys: impl IntoIterator<Item = &'a str>) -> Self {
        let mut projected = Self {
            revision: self.revision,
            cache_identity: VALUE_ID.fetch_add(1, Ordering::Relaxed),
            single_layer: Some(Layer::View as usize),
            ..Default::default()
        };
        projected.layers[Layer::View as usize] = keys
            .into_iter()
            .filter_map(|key| self.get(key).map(|value| (key.into(), value.clone())))
            .collect();
        projected
    }
    /// Returns one unmerged source layer.
    pub fn layer(&self, layer: Layer) -> &Map<String, Value> {
        &self.layers[layer as usize]
    }
    /// Looks up a value, resolving per-view through default values.
    #[inline]
    pub fn get(&self, key: &str) -> Option<&Value> {
        if let Some(layer) = self.single_layer {
            return self.layers[layer].get(key);
        }
        self.layers.iter().rev().find_map(|m| m.get(key))
    }
    /// Replaces a whole source, notifying only on effective value changes.
    pub fn replace(&mut self, layer: Layer, values: Value) -> Result<(), String> {
        let values = values
            .as_object()
            .ok_or("settings must be an object")?
            .clone();
        self.revision += 1;
        self.cache_identity = VALUE_ID.fetch_add(1, Ordering::Relaxed);
        let before = self.effective();
        self.layers[layer as usize] = values;
        let mut nonempty = self
            .layers
            .iter()
            .enumerate()
            .filter_map(|(i, m)| (!m.is_empty()).then_some(i));
        self.single_layer = nonempty.next().filter(|_| nonempty.next().is_none());
        let after = self.effective();
        let mut keys: Vec<_> = before.keys().chain(after.keys()).collect();
        keys.sort();
        keys.dedup();
        for key in keys {
            if before.get(key) != after.get(key) {
                for observer in self.observers.values() {
                    observer(key, after.get(key));
                }
            }
        }
        Ok(())
    }
    /// Changes one setting in a layer.
    pub fn set(&mut self, layer: Layer, key: &str, value: Value) {
        let mut values = self.layers[layer as usize].clone();
        values.insert(key.into(), value);
        self.replace(layer, Value::Object(values)).expect("object");
    }
    /// Registers/replaces a named observer.
    pub fn observe(
        &mut self,
        name: &str,
        callback: impl Fn(&str, Option<&Value>) + Send + Sync + 'static,
    ) {
        self.observers.insert(name.into(), Arc::new(callback));
    }
    /// Removes an observer.
    pub fn unobserve(&mut self, name: &str) {
        self.observers.remove(name);
    }
    /// Merged object, useful for persistence and diagnostics.
    pub fn effective(&self) -> Map<String, Value> {
        let mut out = Map::new();
        for layer in &self.layers {
            out.extend(layer.clone());
        }
        out
    }
    /// Boolean accessor, retaining the caller's default on wrong types.
    pub fn bool(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(default)
    }
    /// Nonnegative integer accessor.
    pub fn usize(&self, key: &str, default: usize) -> usize {
        self.get(key)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(default)
    }
    /// Finite number accessor.
    pub fn number(&self, key: &str, default: f64) -> f64 {
        self.get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .unwrap_or(default)
    }
    /// String accessor.
    pub fn string<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).and_then(Value::as_str).unwrap_or(default)
    }
    /// Typed settings used by the editor and file layer.
    pub fn editor(&self) -> EditorSettings {
        // Deserialize each field independently so one malformed user value does not reset others.
        let defaults = serde_json::to_value(EditorSettings::default()).expect("settings serialize");
        let mut values = defaults.as_object().expect("object").clone();
        for (key, value) in self.effective() {
            if values.contains_key(&key) {
                let mut candidate = values.clone();
                candidate.insert(key.clone(), value);
                if serde_json::from_value::<EditorSettings>(Value::Object(candidate.clone()))
                    .is_ok()
                {
                    values = candidate;
                }
            }
        }
        let mut result: EditorSettings =
            serde_json::from_value(Value::Object(values)).unwrap_or_default();
        result.tab_size = result.tab_size.clamp(1, 32);
        result.font_size = result.font_size.clamp(6., 96.);
        result
    }
}
/// Supported editor preferences; unknown settings remain accessible through `get`.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct EditorSettings {
    /// Tab stops in columns.
    pub tab_size: usize,
    /// Insert spaces for a tab.
    pub translate_tabs_to_spaces: bool,
    /// Characters that delimit words.
    pub word_separators: String,
    /// Requested primary font.
    pub font_face: String,
    /// Font size in logical points.
    pub font_size: f64,
    /// Show line numbers.
    pub line_numbers: bool,
    /// Show gutter.
    pub gutter: bool,
    /// Whitespace rendering mode.
    pub draw_white_space: String,
    /// Vertical column guides.
    pub rulers: Vec<usize>,
    /// Add final LF when saving.
    pub ensure_newline_at_eof_on_save: bool,
    /// False, true, or "not_on_caret".
    pub trim_trailing_white_space_on_save: Value,
    /// Trim only lines changed since save/load.
    pub trim_only_modified_white_space: bool,
    /// New-file line endings.
    pub default_line_ending: String,
    /// New-file encoding.
    pub default_encoding: String,
    /// Decoder for non-UTF-8 input.
    pub fallback_encoding: String,
    /// Allow scrolling beyond the last line.
    pub scroll_past_end: bool,
    /// Caret rendering style.
    pub caret_style: String,
    /// Highlight each caret line.
    pub highlight_line: bool,
    /// Enable context-based bracket/quote pairing.
    pub auto_match_enabled: bool,
    /// Replace files using temp + rename.
    pub atomic_save: bool,
}
impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: 4,
            translate_tabs_to_spaces: false,
            word_separators: "./\\()\"'-:,.;<>~!@#$%^&*|+=[]{}?".into(),
            font_face: "Menlo".into(),
            font_size: 13.,
            line_numbers: true,
            gutter: true,
            draw_white_space: "none".into(),
            rulers: vec![],
            ensure_newline_at_eof_on_save: false,
            trim_trailing_white_space_on_save: Value::Bool(false),
            trim_only_modified_white_space: false,
            default_line_ending: "unix".into(),
            default_encoding: "UTF-8".into(),
            fallback_encoding: "Western (Windows 1252)".into(),
            scroll_past_end: false,
            caret_style: "solid".into(),
            highlight_line: true,
            auto_match_enabled: true,
            atomic_save: true,
        }
    }
}
