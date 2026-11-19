//! Indexed key sequences and per-selection context predicates.
use crate::{Value, json::Document, settings::Settings};
use regex::Regex;
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

// Keep menu chords and their displayed hints in the same macOS modifier order.
// https://developer.apple.com/design/human-interface-guidelines/keyboards
const MODIFIERS: [(u8, &str, char, &str); 4] = [
    (1, "ctrl", '⌃', "Ctrl"),
    (2, "alt", '⌥', "Alt"),
    (4, "shift", '⇧', "Shift"),
    (8, "super", '⌘', "Cmd"),
];

/// A normalized chord, including modifier-only taps and named/keypad keys.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// Canonical key name, or a printable Unicode character.
    pub name: String,
    /// Bitset: control=1, alt=2, shift=4, super=8.
    pub modifiers: u8,
}
impl Key {
    /// Parses public Sublime key syntax without hardware-specific keycodes.
    pub fn parse(input: &str) -> Result<Self, String> {
        let input = input.to_lowercase();
        let mut modifiers = 0;
        let (prefix, name) = if input == "+" {
            ("", "+")
        } else if let Some(prefix) = input.strip_suffix("++") {
            (prefix, "+")
        } else {
            input.rsplit_once('+').unwrap_or(("", input.as_str()))
        };
        if name.is_empty() {
            return Err(format!("empty key in {input}"));
        }
        for modifier in prefix.split('+').filter(|s| !s.is_empty()) {
            modifiers |= match modifier {
                "ctrl" | "control" => 1,
                "alt" | "option" => 2,
                "shift" => 4,
                "super" | "command" | "cmd" => 8,
                "primary" => {
                    if cfg!(target_os = "macos") {
                        8
                    } else {
                        1
                    }
                }
                _ => return Err(format!("unknown modifier {modifier}")),
            };
        }
        Ok(Self::from_parts(name, modifiers))
    }
    /// Normalizes native shifted punctuation to its base key plus Shift. Literal
    /// printable aliases in keymaps resolve to the same chord as `shift+<base>`.
    pub fn from_parts(name: &str, mut modifiers: u8) -> Self {
        let shifted = match name {
            "~" => Some("`"),
            "!" => Some("1"),
            "@" => Some("2"),
            "#" => Some("3"),
            "$" => Some("4"),
            "%" => Some("5"),
            "^" => Some("6"),
            "&" => Some("7"),
            "*" => Some("8"),
            "(" => Some("9"),
            ")" => Some("0"),
            "_" => Some("-"),
            "+" => Some("="),
            "{" => Some("["),
            "}" => Some("]"),
            "|" => Some("\\"),
            ":" => Some(";"),
            "\"" => Some("'"),
            "<" => Some(","),
            ">" => Some("."),
            "?" => Some("/"),
            _ => None,
        };
        let name = if let Some(base) = shifted {
            modifiers |= 4;
            base
        } else {
            name
        };
        let name = match name {
            " " => "space",
            "return" => "enter",
            "keypad_0" => "keypad0",
            "keypad_1" => "keypad1",
            "keypad_2" => "keypad2",
            "keypad_3" => "keypad3",
            "keypad_4" => "keypad4",
            "keypad_5" => "keypad5",
            "keypad_6" => "keypad6",
            "keypad_7" => "keypad7",
            "keypad_8" => "keypad8",
            "keypad_9" => "keypad9",
            "esc" => "escape",
            "backspace" => "backspace",
            _ => name,
        }
        .to_string();
        Self { name, modifiers }
    }
}
/// JSON command name and arguments shared by keymaps, menus and macros.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Invocation {
    /// Registry name.
    pub command: String,
    /// Keyword arguments, defaulting to an empty object.
    #[serde(default = "empty_args")]
    pub args: Value,
}
/// Empty keyword-argument object.
pub fn empty_args() -> Value {
    serde_json::json!({})
}
/// Predicate from a key binding's `context` array.
#[derive(Clone, Debug, Deserialize)]
pub struct Context {
    /// Context name.
    pub key: String,
    /// Comparison operator.
    #[serde(default = "equal")]
    pub operator: String,
    /// Expected value (true by default).
    #[serde(default = "truth")]
    pub operand: Value,
    /// Require every selection instead of at least one.
    #[serde(default)]
    pub match_all: bool,
    #[serde(skip)]
    regex: Option<Arc<ContextRegex>>,
}
#[derive(Debug)]
struct ContextRegex {
    pattern: String,
    compiled: OnceLock<Option<Regex>>,
    onig: Option<vel_regex::Regex>,
}
impl ContextRegex {
    fn get(&self) -> Option<&Regex> {
        self.compiled
            .get_or_init(|| Regex::new(&self.pattern).ok())
            .as_ref()
    }
    fn matches(&self, value: &str) -> Option<bool> {
        if let Some(onig) = &self.onig {
            return Some(onig.is_match(value));
        }
        self.get().map(|regex| regex.is_match(value))
    }
}
fn equal() -> String {
    "equal".into()
}
fn truth() -> Value {
    Value::Bool(true)
}
impl Context {
    /// Validates the context at load time, including lookaround and backreferences.
    pub fn prepare(&mut self) -> Result<(), String> {
        match self.operator.as_str() {
            "equal" | "not_equal" => {}
            "regex_match" | "not_regex_match" | "regex_contains" | "not_regex_contains" => {
                let pattern = self
                    .operand
                    .as_str()
                    .ok_or("regex operand must be a string")?;
                let pattern = if self.operator.ends_with("regex_match") {
                    format!("\\A(?:{pattern})\\z")
                } else {
                    pattern.into()
                };
                let onig = if regex_syntax::Parser::new().parse(&pattern).is_ok() {
                    None
                } else {
                    let regex = vel_regex::Regex::new(pattern.clone());
                    if let Some(error) = regex.compile_error() {
                        return Err(error.to_string());
                    }
                    Some(regex)
                };
                self.regex = Some(Arc::new(ContextRegex {
                    pattern,
                    compiled: OnceLock::new(),
                    onig,
                }));
            }
            _ => return Err(format!("unknown context operator {}", self.operator)),
        }
        Ok(())
    }
    /// Evaluates a predicate using any/all semantics over the current selections.
    pub fn matches(&self, provider: &dyn ContextProvider) -> bool {
        if self.key == "selector" {
            if !provider.scopes_ready() {
                return false;
            }
            let selector = self.operand.as_str().unwrap_or("");
            let values = provider.selector_matches(selector);
            let predicate = |v: bool| if self.operator == "not_equal" { !v } else { v };
            return if self.match_all {
                !values.is_empty() && values.into_iter().all(predicate)
            } else {
                values.into_iter().any(predicate)
            };
        }
        let values = provider.values(&self.key);
        let predicate = |v: &Value| match self.operator.as_str() {
            "equal" => v == &self.operand,
            "not_equal" => v != &self.operand,
            op => {
                let Some(matched) = self
                    .regex
                    .as_ref()
                    .and_then(|r| r.matches(v.as_str().unwrap_or("")))
                else {
                    return false;
                };
                if op.starts_with("not_") {
                    !matched
                } else {
                    matched
                }
            }
        };
        if self.match_all {
            !values.is_empty() && values.iter().all(predicate)
        } else {
            values.iter().any(predicate)
        }
    }
}
/// Supplies lazy context values; text predicates may return one value per selection.
pub trait ContextProvider {
    /// Whether current-version scopes are available. Unknown scopes cannot satisfy
    /// positive or negative selector predicates when no scopes have been published. Providers
    /// may use the last published scopes mapped through edits while parsing catches up.
    fn scopes_ready(&self) -> bool {
        true
    }
    /// Current value(s), or an empty vector for an unknown key.
    fn values(&self, key: &str) -> Vec<Value>;
    /// Scope selector results, one per selection.
    fn selector_matches(&self, selector: &str) -> Vec<bool>;
}
/// Scope evaluation seam for lane B's future syntax provider.
pub trait ScopeProvider {
    /// Tests a selector at a byte offset.
    fn matches(&self, selector: &str, byte: usize) -> bool;
}
/// Plain text provider used until syntax highlighting is integrated.
#[derive(Debug, Default)]
pub struct PlainScope;
impl ScopeProvider for PlainScope {
    fn matches(&self, selector: &str, _: usize) -> bool {
        selector.split(',').any(|s| {
            let mut parts = s.trim().split(" - ");
            let positive = parts.next().unwrap_or("").trim();
            let test = |s: &str| s == "*" || s == "text" || s == "text.plain";
            test(positive) && !parts.any(|s| test(s.trim()))
        })
    }
}
/// Context provider for settings-only clients and benchmarks.
#[derive(Debug)]
pub struct SettingsContext<'a>(pub &'a Settings);
impl ContextProvider for SettingsContext<'_> {
    fn values(&self, key: &str) -> Vec<Value> {
        key.strip_prefix("setting.")
            .and_then(|k| self.0.get(k))
            .cloned()
            .into_iter()
            .collect()
    }
    fn selector_matches(&self, selector: &str) -> Vec<bool> {
        vec![PlainScope.matches(selector, 0)]
    }
}
/// Deserialized binding with validated, lazily compiled context regexes.
#[derive(Clone, Debug)]
pub struct Binding {
    /// Parsed sequence.
    pub keys: Vec<Key>,
    /// Command and arguments.
    pub invocation: Invocation,
    /// Contexts are ANDed.
    pub context: Vec<Context>,
    /// Package resource path for diagnostics, absent for direct in-memory bindings.
    pub origin: Option<Arc<str>>,
}
#[derive(Deserialize)]
struct RawBinding {
    keys: Vec<String>,
    #[serde(flatten)]
    invocation: Invocation,
    #[serde(default)]
    context: Vec<Context>,
}
fn parse_binding(mut raw: RawBinding, origin: Option<Arc<str>>) -> Result<Binding, String> {
    if raw.keys.is_empty() {
        return Err("key sequence may not be empty".into());
    }
    for context in &mut raw.context {
        context.prepare()?;
    }
    Ok(Binding {
        keys: raw
            .keys
            .iter()
            .map(|k| Key::parse(k))
            .collect::<Result<_, _>>()?,
        invocation: raw.invocation,
        context: raw.context,
        origin,
    })
}
/// Result of advancing a key sequence.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// No binding accepted this key.
    Unbound,
    /// A valid sequence prefix consumed the key; no timeout.
    Pending,
    /// An enabled binding consumed the key.
    Command(Invocation),
}
#[derive(Clone, Default, Debug)]
struct Node {
    children: HashMap<Key, usize>,
    candidates: Vec<usize>,
}
/// Trie lookup only visits bindings sharing this prefix. Later appended entries win.
/// Clones share immutable tables and keep independent pending chords. Appending a resource
/// copies shared tables before mutation, so other views retain their existing bindings.
#[derive(Clone, Default, Debug)]
pub struct Keymap {
    bindings: Arc<Vec<Binding>>,
    layers: Arc<Vec<usize>>,
    nodes: Arc<Vec<Node>>,
    pending: usize,
    app_context: HashMap<String, Value>,
}
impl Keymap {
    /// Supplies an app-owned context without changing any preference revision.
    pub fn set_app_context(&mut self, key: &str, value: Value) {
        if self.app_context.get(key) != Some(&value) {
            self.app_context.insert(key.into(), value);
        }
    }
    /// Most recent unconditional binding hint for an invocation (including arguments).
    /// Uses native key glyphs on macOS and spelled-out modifiers elsewhere.
    pub fn hint(&self, invocation: &Invocation) -> Option<String> {
        self.bindings
            .iter()
            .enumerate()
            .rev()
            .find(|(index, b)| {
                b.context.is_empty()
                    && b.invocation == *invocation
                    && !self.bindings[index + 1..]
                        .iter()
                        .any(|later| later.context.is_empty() && later.keys == b.keys)
            })
            .map(|(_, b)| format_hint(&b.keys, cfg!(target_os = "macos")))
    }
    /// Display an effective single Command chord. Unlike the palette, a native
    /// menu cannot describe a sequence. Resource and argument precedence follows
    /// <https://www.sublimetext.com/docs/key_bindings.html>.
    pub fn menu_shortcut(&self, command: &str, args: &Value) -> Option<String> {
        let matches = |binding: &Binding| {
            binding.invocation.command == command && binding.invocation.args == *args
        };
        let mut end = self.bindings.len();
        let mut conditional = None;
        let mut count = 0;
        // Prefer a User/package override over a default, then the first
        // unconditional binding within the effective resource.
        for &start in self.layers.iter().rev() {
            for index in start..end {
                let binding = &self.bindings[index];
                if !matches(binding) {
                    continue;
                }
                count += 1;
                let [key] = binding.keys.as_slice() else {
                    continue;
                };
                if key.modifiers & 8 == 0
                    || self.bindings[index + 1..].iter().any(|later| {
                        later.context.is_empty() && later.keys == binding.keys && !matches(later)
                    })
                {
                    continue;
                }
                if binding.context.is_empty() {
                    return Some(menu_chord(key));
                }
                conditional = Some(key);
            }
            end = start;
        }
        conditional.filter(|_| count == 1).map(menu_chord)
    }
    /// Appends one resource; subsequent resources have higher precedence.
    pub fn append(&mut self, source: &str) -> Result<(), String> {
        let doc = Document::parse(source)?;
        let raw: Vec<RawBinding> = serde_json::from_value(doc.value).map_err(|e| e.to_string())?;
        let parsed = raw
            .into_iter()
            .map(|raw| parse_binding(raw, None))
            .collect::<Result<Vec<_>, _>>()?;
        self.add(parsed);
        Ok(())
    }
    /// Loads independent bindings with origin and one-based entry diagnostics.
    /// Returns the count filtered out by the caller's command registry.
    pub fn append_resource(
        &mut self,
        source: &str,
        origin: &str,
        allowed: impl Fn(&Invocation) -> bool,
        warnings: &mut Vec<String>,
    ) -> Result<usize, String> {
        let doc = Document::parse(source)?;
        let list = doc.value.as_array().ok_or("keymap must be an array")?;
        let origin = Arc::<str>::from(origin);
        let mut entries = Vec::new();
        let mut skipped = 0;
        for (n, value) in list.iter().enumerate() {
            let binding = serde_json::from_value(value.clone())
                .map_err(|e| e.to_string())
                .and_then(|raw: RawBinding| parse_binding(raw, Some(origin.clone())));
            match binding {
                Ok(binding) if allowed(&binding.invocation) => entries.push(binding),
                Ok(_) => skipped += 1,
                Err(error) => warnings.push(format!("{origin}: binding {}: {error}", n + 1)),
            }
        }
        self.add(entries);
        Ok(skipped)
    }
    /// All accepted bindings, in dispatch precedence order.
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }
    /// Builds a keymap from already parsed bindings.
    pub fn from_bindings(bindings: Vec<Binding>) -> Self {
        let mut map = Self::default();
        map.add(bindings);
        map
    }
    fn add(&mut self, parsed: Vec<Binding>) {
        let bindings = Arc::make_mut(&mut self.bindings);
        if !parsed.is_empty() {
            Arc::make_mut(&mut self.layers).push(bindings.len());
        }
        let nodes = Arc::make_mut(&mut self.nodes);
        if nodes.is_empty() {
            nodes.push(Node::default());
        }
        for binding in parsed {
            let index = bindings.len();
            let mut node = 0;
            for key in &binding.keys {
                let next = if let Some(next) = nodes[node].children.get(key) {
                    *next
                } else {
                    let next = nodes.len();
                    nodes.push(Node::default());
                    nodes[node].children.insert(key.clone(), next);
                    next
                };
                node = next;
                nodes[node].candidates.push(index);
            }
            bindings.push(binding);
        }
        self.pending = 0;
    }
    /// Cancels a pending prefix (for focus/tab changes or explicit escape).
    pub fn cancel(&mut self) {
        self.pending = 0;
    }
    /// Resolves a key using the latest matching entry, preserving valid prefixes.
    pub fn resolve(&mut self, key: &Key, context: &dyn ContextProvider) -> Resolution {
        struct AppContext<'a> {
            app: &'a HashMap<String, Value>,
            editor: &'a dyn ContextProvider,
        }
        impl ContextProvider for AppContext<'_> {
            fn scopes_ready(&self) -> bool {
                self.editor.scopes_ready()
            }
            fn values(&self, key: &str) -> Vec<Value> {
                self.app
                    .get(key)
                    .map_or_else(|| self.editor.values(key), |v| vec![v.clone()])
            }
            fn selector_matches(&self, selector: &str) -> Vec<bool> {
                self.editor.selector_matches(selector)
            }
        }
        let context = &AppContext {
            app: &self.app_context,
            editor: context,
        };
        if self.nodes.is_empty() {
            return Resolution::Unbound;
        }
        let next = self.nodes[self.pending]
            .children
            .get(key)
            .copied()
            .or_else(|| self.nodes[0].children.get(key).copied());
        let Some(node) = next else {
            self.cancel();
            return Resolution::Unbound;
        };
        let candidate = self.nodes[node]
            .candidates
            .iter()
            .rev()
            .copied()
            .find(|i| self.bindings[*i].context.iter().all(|c| c.matches(context)));
        let Some(index) = candidate else {
            self.cancel();
            return Resolution::Unbound;
        };
        let binding = &self.bindings[index];
        // A node is terminal for this binding iff its final chord reaches this node.
        let mut terminal = 0;
        for k in &binding.keys {
            terminal = self.nodes[terminal].children[k];
        }
        if terminal == node {
            let invocation = binding.invocation.clone();
            self.cancel();
            Resolution::Command(invocation)
        } else {
            self.pending = node;
            Resolution::Pending
        }
    }
    /// Number of loaded bindings.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }
    /// Whether no bindings have been loaded.
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

fn menu_chord(key: &Key) -> String {
    let mut chord = String::new();
    for (bit, name, _, _) in MODIFIERS {
        if key.modifiers & bit != 0 {
            chord.push_str(name);
            chord.push('+');
        }
    }
    chord.push_str(&key.name);
    chord
}

fn format_hint(keys: &[Key], mac: bool) -> String {
    let mut hint = String::new();
    for (i, key) in keys.iter().enumerate() {
        if i != 0 {
            hint.push_str(if mac { " " } else { ", " });
        }
        for (bit, _, symbol, name) in MODIFIERS {
            if key.modifiers & bit != 0 {
                if mac {
                    hint.push(symbol);
                } else {
                    hint.push_str(name);
                    hint.push('+');
                }
            }
        }
        let symbol = if mac {
            match key.name.as_str() {
                "up" => "↑",
                "down" => "↓",
                "left" => "←",
                "right" => "→",
                "enter" => "↩",
                "tab" => "⇥",
                "escape" => "⎋",
                "backspace" => "⌫",
                "delete" => "⌦",
                "space" => "Space",
                _ => "",
            }
        } else {
            ""
        };
        if symbol.is_empty() {
            hint.extend(key.name.chars().flat_map(char::to_uppercase));
        } else {
            hint.push_str(symbol);
        }
    }
    hint
}

#[cfg(test)]
#[test]
fn mac_hints_share_the_menu_modifier_order() {
    for (binding, menu, hint) in [
        ("super+shift+s", "shift+super+s", "⇧⌘S"),
        ("super+alt+s", "alt+super+s", "⌥⌘S"),
        ("shift+super+ctrl+f", "ctrl+shift+super+f", "⌃⇧⌘F"),
        (
            "super+shift+alt+ctrl+tab",
            "ctrl+alt+shift+super+tab",
            "⌃⌥⇧⌘⇥",
        ),
        ("alt+super+up", "alt+super+up", "⌥⌘↑"),
        ("super+,", "super+,", "⌘,"),
        ("super++", "shift+super+=", "⇧⌘="),
        ("f5", "f5", "F5"),
    ] {
        let key = Key::parse(binding).unwrap();
        assert_eq!(menu_chord(&key), menu);
        assert_eq!(format_hint(&[key], true), hint);
    }
    let keys = [
        Key::parse("super+k").unwrap(),
        Key::parse("super+b").unwrap(),
    ];
    assert_eq!(format_hint(&keys, true), "⌘K ⌘B");
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TextContext;
    impl ContextProvider for TextContext {
        fn values(&self, key: &str) -> Vec<Value> {
            assert_eq!(key, "preceding_text");
            vec![Value::String("café".into())]
        }
        fn selector_matches(&self, _: &str) -> Vec<bool> {
            vec![]
        }
    }

    #[test]
    fn context_regex_validates_at_load_and_compiles_on_first_matching_key() {
        let mut map = Keymap::default();
        map.append(
            r#"[
          {"keys":["a"],"command":"base"},
          {"keys":["'"],"command":"word","context":[
            {"key":"preceding_text","operator":"regex_contains","operand":"[\\w]$"}]}
        ]"#,
        )
        .unwrap();
        let regex = map.bindings[1].context[0].regex.clone().unwrap();
        assert!(regex.compiled.get().is_none());
        assert!(
            matches!(map.resolve(&Key::parse("a").unwrap(), &TextContext), Resolution::Command(i) if i.command == "base")
        );
        assert!(regex.compiled.get().is_none());
        assert!(
            matches!(map.resolve(&Key::parse("'").unwrap(), &TextContext), Resolution::Command(i) if i.command == "word")
        );
        let first = regex.compiled.get().unwrap().as_ref().unwrap();
        let mut clone = map.clone();
        assert!(
            matches!(clone.resolve(&Key::parse("'").unwrap(), &TextContext), Resolution::Command(i) if i.command == "word")
        );
        assert!(std::ptr::eq(
            first,
            clone.bindings[1].context[0]
                .regex
                .as_ref()
                .unwrap()
                .get()
                .unwrap()
        ));
        assert!(
            map.append(
                r#"[{"keys":["'"],"command":"invalid","context":[
            {"key":"preceding_text","operator":"not_regex_contains","operand":"["}]}]"#
            )
            .is_err()
        );
        assert_eq!(map.len(), 2);
    }
}
