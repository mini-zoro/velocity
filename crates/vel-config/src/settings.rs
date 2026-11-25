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
    /// A snapshot of overrides for this window, supplied by New Window.
    Window,
    /// Project values. Sublime consults them before syntax-specific settings, so a syntax file
    /// (e.g. `Packages/User/Python.sublime-settings`) overrides the project.
    Project,
    /// Syntax-specific values.
    Syntax,
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
    layers: [Map<String, Value>; 8],
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
        result.ruler_entries = self.get("rulers").map(parse_rulers).unwrap_or_default();
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
    /// Extra delimiters for sub-word motion and deletion.
    pub sub_word_separators: String,
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
    #[serde(deserialize_with = "deserialize_rulers")]
    pub rulers: Vec<usize>,
    /// Parsed drawing metadata, sharing the validated columns above.
    #[serde(skip)]
    pub ruler_entries: Vec<Ruler>,
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
    #[serde(deserialize_with = "deserialize_scroll_past_end")]
    pub scroll_past_end: f64,
    /// Caret rendering style.
    pub caret_style: String,
    /// Highlight each caret line.
    pub highlight_line: bool,
    /// Enable context-based bracket/quote pairing.
    pub auto_match_enabled: bool,
    /// Suppress built-in HTML and CSS completion candidates.
    pub disable_default_completions: bool,
    /// Override the built-in completion scope selector (a string or a list of fragments).
    pub default_completions_selector: Option<Value>,
    /// Replace existing files using temp + rename when that keeps their identity.
    pub atomic_save: bool,
    /// Tab / Backspace with spaces move to tab stops instead of a fixed `tab_size` / one space.
    pub use_tab_stops: bool,
    /// Copy / cut with only empty selections take the caret lines.
    pub copy_with_empty_selection: bool,
}
/// Rejects malformed values of known keys before they can mask lower layers.
/// Unknown keys are retained for packages and forward compatibility.
/// <https://www.sublimetext.com/docs/settings.html>
pub fn validate_layer(
    settings: &mut serde_json::Map<String, Value>,
    defaults: &serde_json::Map<String, Value>,
) -> Vec<String> {
    fn kind(value: &Value) -> &'static str {
        match value {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        }
    }
    let mut issues = Vec::new();
    settings.retain(|key, value| {
        let actual = kind(value);
        let Some(default) = defaults.get(key) else {
            return true;
        };
        let primary = [kind(default)];
        let expected: &[&str] = match key.as_str() {
            // These readers deliberately accept the documented legacy forms.
            "word_wrap"
            | "auto_find_in_selection"
            | "open_files_in_new_window"
            | "hot_exit"
            | "remember_layout"
            | "show_side_bar"
            | "auto_complete_preserve_order"
            | "preview_on_click"
            | "trim_trailing_white_space_on_save" => &["a boolean", "a string"],
            "terminal_command" => &["an array", "a string"],
            "scroll_past_end" => &["a boolean", "a number"],
            "rulers" => &["an array", "a number"],
            "draw_white_space" | "default_completions_selector" | "auto_complete_selector" => {
                &["a string", "an array", "null"]
            }
            "vintage_ctrl_keys" => &["a boolean", "null"],
            "package_index_url" => &["a string", "null"],
            _ => &primary,
        };
        if expected.contains(&actual) {
            true
        } else {
            issues.push(format!(
                "{key}: expected {}, got {actual}",
                expected.join(" or ")
            ));
            false
        }
    });
    issues
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: 4,
            translate_tabs_to_spaces: false,
            word_separators: "./\\()\"'-:,.;<>~!@#$%^&*|+=[]{}`~?".into(),
            // Build 4142 made this configurable; retain Velocity's underscore delimiter.
            // https://www.sublimetext.com/download
            sub_word_separators: "_".into(),
            font_face: if cfg!(windows) { "Consolas" } else { "Menlo" }.into(),
            font_size: 13.,
            line_numbers: true,
            gutter: true,
            draw_white_space: "none".into(),
            rulers: vec![],
            ruler_entries: vec![],
            ensure_newline_at_eof_on_save: false,
            trim_trailing_white_space_on_save: Value::Bool(false),
            // Build 4107: trimming newly inserted whitespace is the default.
            // https://www.sublimetext.com/download
            trim_only_modified_white_space: true,
            default_line_ending: "system".into(),
            default_encoding: "UTF-8".into(),
            fallback_encoding: "Western (Windows 1252)".into(),
            scroll_past_end: 0.,
            caret_style: "blink".into(),
            highlight_line: true,
            auto_match_enabled: true,
            // HTML/CSS defaults are enabled unless the syntax or user opts out.
            // https://www.sublimetext.com/3 (HTML and CSS default completion opt-out).
            disable_default_completions: false,
            default_completions_selector: None,
            // ST3 disabled atomic_save by default: https://www.sublimetext.com/3
            atomic_save: false,
            use_tab_stops: true,
            copy_with_empty_selection: true,
        }
    }
}

/// Stroke pattern for column rulers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RulerStyle {
    /// Continuous stroke.
    Solid,
    /// Short dots.
    Dotted,
    /// Spaced dashes.
    Stippled,
}
/// Independently validated ruler entry; absent style inherits `ruler_style`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Ruler {
    /// Column measured in editor cells.
    pub column: usize,
    /// Optional per-entry stroke pattern.
    pub style: Option<RulerStyle>,
}
fn deserialize_rulers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<usize>, D::Error> {
    let value = Value::deserialize(d)?;
    Ok(parse_rulers(&value).into_iter().map(|r| r.column).collect())
}
fn parse_rulers(value: &Value) -> Vec<Ruler> {
    // A single column is also documented; malformed list entries remain isolated.
    // https://docs.sublimetext.io/reference/settings.html#visual-settings
    let entries = value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(value));
    // Build 4142: https://www.sublimetext.com/download (ruler_style).
    // Invalid entries are isolated, just as independent setting fields are above.
    entries
        .iter()
        .filter_map(|entry| {
            let (column, style) = if let Some(parts) = entry.as_array() {
                if parts.len() != 2 {
                    return None;
                }
                (
                    parts[0].as_u64()?,
                    Some(serde_json::from_value::<RulerStyle>(parts[1].clone()).ok()?),
                )
            } else {
                (entry.as_u64()?, None)
            };
            Some(Ruler {
                column: usize::try_from(column).ok()?,
                style,
            })
        })
        .collect()
}
fn deserialize_scroll_past_end<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let value = Value::deserialize(d)?;
    // Build 4107: https://www.sublimetext.com/download documents 0.0 through 1.0.
    Ok(match value {
        Value::Bool(on) => f64::from(u8::from(on)),
        _ => value
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| serde::de::Error::custom("expected boolean or finite number"))?
            .clamp(0., 1.),
    })
}
#[cfg(test)]
mod round17_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn scroll_distance_and_rulers_load_independently() {
        for (value, expected) in [
            (json!(true), 1.),
            (json!(false), 0.),
            (json!(0.5), 0.5),
            (json!(-4.), 0.),
            (json!(2.), 1.),
        ] {
            let mut settings = Settings::default();
            settings.replace(Layer::User, json!({"scroll_past_end":value,"rulers":[80,[90,"dotted"],"bad",[1,"invalid"],-2,[100,"stippled"],120],"font_size":15})).unwrap();
            let prefs = settings.editor();
            assert_eq!(prefs.scroll_past_end, expected);
            assert_eq!(prefs.rulers, [80, 90, 100, 120]);
            assert_eq!(prefs.ruler_entries[1].style, Some(RulerStyle::Dotted));
            assert_eq!(prefs.font_size, 15.);
        }
    }
}
