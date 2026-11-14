//! Scope-selected public tmPreferences metadata, symbol transformations, and symbol extraction.
use plist::Value;
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    path::Path,
    sync::{Arc, Mutex},
};
use vel_regex::{Regex, Region};
use vel_scope::{Scope, ScopeStack, Selector};

/// Invalid plist, selector, or transformation program.
#[derive(Debug, thiserror::Error)]
#[error("invalid tmPreferences: {0}")]
pub struct MetadataError(pub String);
/// Effective metadata. Unknown plist keys remain available for future integrations.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    /// Comment delimiters and related TM_COMMENT_* settings, merged by variable name.
    pub shell_variables: BTreeMap<String, String>,
    /// All other settings, merged independently by selector score and resource order.
    pub settings: BTreeMap<String, Value>,
}
impl Metadata {
    /// Reads a string setting, such as an indent pattern or transformation.
    pub fn string(&self, key: &str) -> Option<&str> {
        self.settings.get(key).and_then(Value::as_string)
    }
    /// Accepts plist booleans and the integer/string forms used by public packages.
    pub fn boolean(&self, key: &str) -> Option<bool> {
        self.settings.get(key).and_then(|v| {
            v.as_boolean()
                .or_else(|| v.as_signed_integer().map(|i| i != 0))
                .or_else(|| {
                    v.as_string().and_then(|s| match s {
                        "1" | "true" => Some(true),
                        "0" | "false" => Some(false),
                        _ => None,
                    })
                })
        })
    }
    fn merge(&mut self, other: &Self) {
        self.shell_variables.extend(other.shell_variables.clone());
        self.settings.extend(other.settings.clone());
    }
}
#[derive(Debug)]
struct Entry {
    selector: Selector,
    metadata: Metadata,
}
/// Immutable metadata rules with shared effective-setting and regex caches.
#[derive(Debug, Default)]
pub struct MetadataSet {
    entries: Vec<Entry>,
    cache: Mutex<HashMap<Vec<Scope>, Arc<Metadata>>>,
    patterns: Mutex<HashMap<String, Regex>>,
    transforms: Mutex<HashMap<String, Result<SymbolTransformation, MetadataError>>>,
}
impl Clone for MetadataError {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl MetadataSet {
    pub(crate) fn for_stacks(&self, stacks: &[&[Scope]]) -> Arc<Metadata> {
        if let [scopes] = stacks {
            return self.shared_for(scopes);
        }
        let mut matches: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| {
                stacks
                    .iter()
                    .filter_map(|s| entry.selector.does_match(s))
                    .max()
                    .map(|score| (score, i, &entry.metadata))
            })
            .collect();
        matches.sort_by_key(|(score, i, _)| (*score, *i));
        let mut metadata = Metadata::default();
        for (_, _, value) in matches {
            metadata.merge(value);
        }
        Arc::new(metadata)
    }

    /// Clean-room base behavior from the public scope naming conventions.
    /// No application-bundled Default preferences are read or copied.
    pub fn new() -> Self {
        let mut set = Self::default();
        for (selector, local, index, reference) in [
            (
                "entity.name.function, entity.name.class, entity.name.struct, entity.name.enum, entity.name.union, entity.name.trait, entity.name.interface, entity.name.type",
                true,
                true,
                false,
            ),
            ("meta.toc-list", true, false, false),
            (
                "variable.function - entity.name.function, source support.function - entity.name.function, entity.other.inherited-class, meta.function-call.identifier - entity.name.function",
                false,
                false,
                true,
            ),
            (
                "entity.name.class.forward-decl, entity.name.struct.forward-decl, entity.name.union.forward-decl, entity.name.enum.forward-decl",
                false,
                false,
                false,
            ),
        ] {
            let mut metadata = Metadata::default();
            for (key, b) in [
                ("showInSymbolList", local),
                ("showInIndexedSymbolList", index),
                ("showInIndexedReferenceList", reference),
            ] {
                metadata.settings.insert(key.into(), Value::Boolean(b));
            }
            set.entries.push(Entry {
                selector: Selector::parse(selector).unwrap(),
                metadata,
            });
        }
        set
    }
    /// Loads public preferences in deterministic resource order; pass User directories last.
    pub fn load_directories<P: AsRef<Path>>(roots: &[P]) -> Result<Self, MetadataError> {
        let mut set = Self::new();
        for root in roots {
            let mut paths = Vec::new();
            for entry in walkdir::WalkDir::new(root) {
                let e = entry.map_err(|e| MetadataError(e.to_string()))?;
                if e.file_type().is_file()
                    && e.path().extension().is_some_and(|s| s == "tmPreferences")
                {
                    paths.push(e.into_path());
                }
            }
            paths.sort();
            for path in paths {
                let bytes = std::fs::read(&path)
                    .map_err(|e| MetadataError(format!("{}: {e}", path.display())))?;
                set.add_plist(&bytes)
                    .map_err(|e| MetadataError(format!("{}: {e}", path.display())))?;
            }
        }
        Ok(set)
    }
    /// Adds one XML or binary plist; call before sharing the set with workers.
    pub fn add_plist(&mut self, bytes: &[u8]) -> Result<(), MetadataError> {
        let normalized = preserve_cdata(bytes)?;
        let value = Value::from_reader(std::io::Cursor::new(normalized))
            .map_err(|e| MetadataError(e.to_string()))?;
        let d = value
            .as_dictionary()
            .ok_or_else(|| MetadataError("root must be a dictionary".into()))?;
        let scope = d.get("scope").and_then(Value::as_string).unwrap_or("");
        let settings = d
            .get("settings")
            .and_then(Value::as_dictionary)
            .ok_or_else(|| MetadataError("settings missing".into()))?;
        let mut metadata = Metadata::default();
        for (k, v) in settings {
            if k == "shellVariables" {
                let a = v
                    .as_array()
                    .ok_or_else(|| MetadataError("shellVariables must be an array".into()))?;
                for variable in a {
                    let d = variable
                        .as_dictionary()
                        .ok_or_else(|| MetadataError("invalid shell variable".into()))?;
                    let k = d
                        .get("name")
                        .and_then(Value::as_string)
                        .ok_or_else(|| MetadataError("variable name missing".into()))?;
                    let v = d
                        .get("value")
                        .and_then(Value::as_string)
                        .ok_or_else(|| MetadataError("variable value missing".into()))?;
                    metadata.shell_variables.insert(k.into(), v.into());
                }
            } else {
                metadata.settings.insert(k.clone(), v.clone());
            }
        }
        self.entries.push(Entry {
            selector: Selector::parse(scope).map_err(|e| MetadataError(e.to_string()))?,
            metadata,
        });
        self.cache.get_mut().unwrap().clear();
        Ok(())
    }
    /// Merges settings by specificity, with later resources winning equal scores.
    pub fn metadata_for(&self, scopes: &ScopeStack) -> Metadata {
        (*self.shared_for(scopes.as_slice())).clone()
    }
    pub(crate) fn shared_for(&self, scopes: &[Scope]) -> Arc<Metadata> {
        if let Some(m) = self.cache.lock().unwrap().get(scopes) {
            return m.clone();
        }
        let mut matches: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.selector.does_match(scopes).map(|s| (s, i, &e.metadata)))
            .collect();
        matches.sort_by_key(|(s, i, _)| (*s, *i));
        let mut metadata = Metadata::default();
        for (_, _, m) in matches {
            metadata.merge(m);
        }
        let metadata = Arc::new(metadata);
        self.cache
            .lock()
            .unwrap()
            .insert(scopes.to_vec(), metadata.clone());
        metadata
    }
    /// Tests a cached Oniguruma indentation pattern. Empty/missing patterns do not match.
    pub fn matches_pattern(&self, metadata: &Metadata, key: &str, line: &str) -> bool {
        let Some(pattern) = metadata.string(key).filter(|s| !s.is_empty()) else {
            return false;
        };
        let regex = self
            .patterns
            .lock()
            .unwrap()
            .entry(pattern.into())
            .or_insert_with(|| Regex::new(pattern.into()))
            .clone();
        regex.is_match(line)
    }
    /// Applies the selected symbol/index transformation; failures remain explicit.
    pub fn transform(
        &self,
        metadata: &Metadata,
        index: bool,
        text: &str,
    ) -> Result<String, MetadataError> {
        let key = if index {
            "symbolIndexTransformation"
        } else {
            "symbolTransformation"
        };
        let Some(script) = metadata.string(key) else {
            return Ok(text.into());
        };
        let transform = self
            .transforms
            .lock()
            .unwrap()
            .entry(script.into())
            .or_insert_with(|| SymbolTransformation::parse(script))
            .clone()?;
        Ok(transform.apply(text))
    }
}
#[derive(Debug, Clone)]
struct Substitution {
    regex: Regex,
    replacement: String,
    global: bool,
}
/// Sed-like substitution programs used by symbolTransformation and symbolIndexTransformation.
#[derive(Debug, Clone, Default)]
pub struct SymbolTransformation {
    steps: Vec<Substitution>,
}
fn delimited(text: &str, at: &mut usize, delimiter: u8) -> Result<String, MetadataError> {
    let mut out = String::new();
    while *at < text.len() {
        let c = text.as_bytes()[*at];
        *at += 1;
        if c == delimiter {
            return Ok(out);
        }
        if c == b'\\' {
            if *at >= text.len() {
                break;
            }
            let next = text.as_bytes()[*at];
            if next == delimiter {
                out.push(delimiter as char);
                *at += 1;
                continue;
            }
            out.push('\\');
            let ch = text[*at..].chars().next().unwrap();
            out.push(ch);
            *at += ch.len_utf8();
        } else {
            let ch = text[*at - 1..].chars().next().unwrap();
            out.push(ch);
            *at += ch.len_utf8() - 1;
        }
    }
    Err(MetadataError("unterminated substitution".into()))
}
impl SymbolTransformation {
    /// Parses `/pattern/replacement/flags` or `s/pattern/replacement/flags`, comments and newlines.
    pub fn parse(script: &str) -> Result<Self, MetadataError> {
        let mut out = Self::default();
        let mut at = 0;
        while at < script.len() {
            let rest = &script[at..];
            let c = rest.chars().next().unwrap();
            if c.is_whitespace() || c == ';' {
                at += c.len_utf8();
                continue;
            }
            if c == '#' {
                at += rest.find('\n').unwrap_or(rest.len());
                continue;
            }
            if rest.starts_with("(?#") {
                let n = rest
                    .find(')')
                    .ok_or_else(|| MetadataError("unterminated transformation comment".into()))?;
                at += n + 1;
                continue;
            }
            if c == 's' {
                at += 1;
            }
            let delimiter = *script
                .as_bytes()
                .get(at)
                .ok_or_else(|| MetadataError("missing substitution delimiter".into()))?;
            if delimiter.is_ascii_alphanumeric() {
                return Err(MetadataError("invalid substitution delimiter".into()));
            }
            at += 1;
            let pattern = delimited(script, &mut at, delimiter)?;
            let replacement = delimited(script, &mut at, delimiter)?;
            let start = at;
            while script
                .as_bytes()
                .get(at)
                .is_some_and(u8::is_ascii_alphabetic)
            {
                at += 1;
            }
            let flags = &script[start..at];
            if flags.chars().any(|c| !"gimx".contains(c)) {
                return Err(MetadataError(format!(
                    "unsupported substitution flags {flags}"
                )));
            }
            let options = flags.replace('g', "");
            let pattern = if options.is_empty() {
                pattern
            } else {
                format!("(?{options}){pattern}")
            };
            let regex = Regex::new(pattern);
            if let Some(e) = regex.compile_error() {
                return Err(MetadataError(e.to_string()));
            }
            out.steps.push(Substitution {
                regex,
                replacement,
                global: flags.contains('g'),
            });
        }
        Ok(out)
    }
    /// Executes substitutions, supporting $n, backslash-n captures, and case conversion escapes.
    pub fn apply(&self, text: &str) -> String {
        let mut text = text.to_string();
        for step in &self.steps {
            let mut output = String::new();
            let mut at = 0;
            let mut copied = 0;
            let mut region = Region::new();
            let mut retry_nonempty = false;
            loop {
                let found =
                    step.regex
                        .search(&text, at, text.len(), Some(&mut region), !retry_nonempty);
                let position = if found { region.pos(0) } else { None };
                if retry_nonempty && position.is_none_or(|(a, _)| a != at) {
                    let Some(ch) = text[at..].chars().next() else {
                        break;
                    };
                    at += ch.len_utf8();
                    retry_nonempty = false;
                    continue;
                }
                let Some((a, b)) = position else {
                    break;
                };
                output.push_str(&text[copied..a]);
                output.push_str(&replacement(&step.replacement, &text, &region));
                copied = b;
                if !step.global {
                    break;
                }
                at = b;
                retry_nonempty = b == a;
            }
            output.push_str(&text[copied..]);
            text = output;
        }
        text
    }
}
fn replacement(template: &str, text: &str, region: &Region) -> String {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    let mut case = None;
    let mut next_case = None;
    while let Some(ch) = chars.next() {
        let mut piece = String::new();
        if (ch == '$' || ch == '\\') && chars.peek().is_some_and(char::is_ascii_digit) {
            let mut n = 0;
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                n = n * 10 + d as usize;
                chars.next();
            }
            if let Some((a, b)) = region.pos(n) {
                piece.push_str(&text[a..b]);
            }
        } else if ch == '\\' {
            match chars.next() {
                Some('u') => {
                    next_case = Some(true);
                    continue;
                }
                Some('l') => {
                    next_case = Some(false);
                    continue;
                }
                Some('U') => {
                    case = Some(true);
                    continue;
                }
                Some('L') => {
                    case = Some(false);
                    continue;
                }
                Some('E') => {
                    case = None;
                    continue;
                }
                Some('n') => piece.push('\n'),
                Some('t') => piece.push('\t'),
                Some(c) => piece.push(c),
                None => piece.push('\\'),
            }
        } else {
            piece.push(ch);
        }
        for c in piece.chars() {
            match next_case.take().or(case) {
                Some(true) => out.extend(c.to_uppercase()),
                Some(false) => out.extend(c.to_lowercase()),
                None => out.push(c),
            }
        }
    }
    out
}
/// A byte range and its active scope stack, after all branch corrections.
#[derive(Debug, Clone)]
pub struct ScopedSpan {
    /// UTF-8 byte range in the document.
    pub range: Range<usize>,
    /// Active scopes, outermost first.
    pub scopes: Vec<Scope>,
}
/// A definition or reference extracted from scope-selected metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// Original UTF-8 byte range, before name transformations.
    pub range: Range<usize>,
    /// Name displayed by the document symbol list.
    pub name: String,
    /// Name stored in the cross-file index.
    pub index_name: String,
    /// Included in the document symbol list.
    pub local: bool,
    /// Included in the definition index.
    pub indexed: bool,
    /// Included in the reference index.
    pub reference: bool,
}
impl MetadataSet {
    /// Extracts contiguous symbol regions, including nested scopes and multiline names.
    pub fn symbols(&self, text: &str, spans: &[ScopedSpan]) -> Result<Vec<Symbol>, MetadataError> {
        let mut groups: Vec<(Range<usize>, Arc<Metadata>)> = Vec::new();
        for span in spans {
            let m = self.shared_for(&span.scopes);
            let flags = |m: &Metadata| {
                [
                    m.boolean("showInSymbolList").unwrap_or(false),
                    m.boolean("showInIndexedSymbolList").unwrap_or(false),
                    m.boolean("showInIndexedReferenceList").unwrap_or(false),
                ]
            };
            if !flags(&m).iter().any(|b| *b) {
                continue;
            }
            if let Some((range, old)) = groups.last_mut()
                && range.end == span.range.start
                && flags(old) == flags(&m)
                && old.string("symbolTransformation") == m.string("symbolTransformation")
                && old.string("symbolIndexTransformation") == m.string("symbolIndexTransformation")
            {
                range.end = span.range.end;
                continue;
            }
            groups.push((span.range.clone(), m));
        }
        groups
            .into_iter()
            .map(|(range, m)| {
                let source = text
                    .get(range.clone())
                    .ok_or_else(|| MetadataError("symbol range outside UTF-8 text".into()))?;
                Ok(Symbol {
                    name: self.transform(&m, false, source)?,
                    index_name: self.transform(&m, true, source)?,
                    range,
                    local: m.boolean("showInSymbolList").unwrap_or(false),
                    indexed: m.boolean("showInIndexedSymbolList").unwrap_or(false),
                    reference: m.boolean("showInIndexedReferenceList").unwrap_or(false),
                })
            })
            .collect()
    }
}

// plist's XML reader currently ignores CDATA events. Public preferences use them for
// regexes and selectors. Escape their contents as equivalent XML character data first.
fn preserve_cdata(bytes: &[u8]) -> Result<Vec<u8>, MetadataError> {
    if bytes.starts_with(b"bplist") {
        return Ok(bytes.to_vec());
    }
    let text = std::str::from_utf8(bytes).map_err(|e| MetadataError(e.to_string()))?;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("<![CDATA[") {
        out.push_str(&rest[..at]);
        rest = &rest[at + 9..];
        let end = rest
            .find("]]>")
            .ok_or_else(|| MetadataError("unterminated CDATA".into()))?;
        out.push_str(
            &rest[..end]
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;"),
        );
        rest = &rest[end + 3..];
    }
    out.push_str(rest);
    Ok(out.into_bytes())
}
