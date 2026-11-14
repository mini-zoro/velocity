//! Indexed key sequences and per-selection context predicates.
use crate::{Value, json::Document, settings::Settings};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;

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
    compiled: Option<Regex>,
}
fn equal() -> String {
    "equal".into()
}
fn truth() -> Value {
    Value::Bool(true)
}
impl Context {
    /// Validates an operator and compiles its regex once at resource load time.
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
                self.compiled = Some(Regex::new(&pattern).map_err(|e| e.to_string())?);
            }
            _ => return Err(format!("unknown context operator {}", self.operator)),
        }
        Ok(())
    }
    /// Evaluates a predicate using any/all semantics over the current selections.
    pub fn matches(&self, provider: &dyn ContextProvider) -> bool {
        if self.key == "selector" {
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
                let matched = self
                    .compiled
                    .as_ref()
                    .is_some_and(|r| r.is_match(v.as_str().unwrap_or("")));
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
/// Deserialized binding with compiled context regular expressions.
#[derive(Clone, Debug)]
pub struct Binding {
    /// Parsed sequence.
    pub keys: Vec<Key>,
    /// Command and arguments.
    pub invocation: Invocation,
    /// Contexts are ANDed.
    pub context: Vec<Context>,
}
#[derive(Deserialize)]
struct RawBinding {
    keys: Vec<String>,
    #[serde(flatten)]
    invocation: Invocation,
    #[serde(default)]
    context: Vec<Context>,
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
#[derive(Default, Debug)]
struct Node {
    children: HashMap<Key, usize>,
    candidates: Vec<usize>,
}
/// Trie lookup only visits bindings sharing this prefix. Later appended entries win.
#[derive(Default, Debug)]
pub struct Keymap {
    bindings: Vec<Binding>,
    nodes: Vec<Node>,
    pending: usize,
}
impl Keymap {
    /// Most recent unconditional binding hint for an invocation (including arguments).
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
            .map(|(_, b)| {
                b.keys
                    .iter()
                    .map(|key| {
                        let mut parts = Vec::new();
                        for (bit, name) in [(1, "Ctrl"), (2, "Alt"), (4, "Shift"), (8, "Cmd")] {
                            if key.modifiers & bit != 0 {
                                parts.push(name.to_owned());
                            }
                        }
                        parts.push(key.name.to_uppercase());
                        parts.join("+")
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
    }
    /// Appends one resource; subsequent resources have higher precedence.
    pub fn append(&mut self, source: &str) -> Result<(), String> {
        let doc = Document::parse(source)?;
        let raw: Vec<RawBinding> = serde_json::from_value(doc.value).map_err(|e| e.to_string())?;
        let parsed: Vec<_> = raw
            .into_iter()
            .map(|r| {
                if r.keys.is_empty() {
                    return Err("key sequence may not be empty".into());
                }
                let mut context = r.context;
                for c in &mut context {
                    c.prepare()?;
                }
                Ok(Binding {
                    keys: r
                        .keys
                        .iter()
                        .map(|k| Key::parse(k))
                        .collect::<Result<_, _>>()?,
                    invocation: r.invocation,
                    context,
                })
            })
            .collect::<Result<_, String>>()?;
        if self.nodes.is_empty() {
            self.nodes.push(Node::default());
        }
        for binding in parsed {
            let index = self.bindings.len();
            let mut node = 0;
            for key in &binding.keys {
                let next = if let Some(next) = self.nodes[node].children.get(key) {
                    *next
                } else {
                    let next = self.nodes.len();
                    self.nodes.push(Node::default());
                    self.nodes[node].children.insert(key.clone(), next);
                    next
                };
                node = next;
                self.nodes[node].candidates.push(index);
            }
            self.bindings.push(binding);
        }
        self.pending = 0;
        Ok(())
    }
    /// Cancels a pending prefix (for focus/tab changes or explicit escape).
    pub fn cancel(&mut self) {
        self.pending = 0;
    }
    /// Resolves a key using the latest matching entry, preserving valid prefixes.
    pub fn resolve(&mut self, key: &Key, context: &dyn ContextProvider) -> Resolution {
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
