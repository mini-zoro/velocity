//! TextMate plist/JSON conversion; no editor assets or private implementation.
use crate::{LoadingError, SyntaxDefinition, syntax_definition::*};
use serde_json::{Map, Value};
use std::{collections::HashMap, path::Path};
use vel_scope::Scope;

/// Invalid TextMate input or a construct which cannot be represented faithfully.
#[derive(Debug, thiserror::Error)]
#[error("TextMate grammar: {0}")]
pub struct TmLanguageError(
    /// Human-readable source or unsupported-construct diagnostic.
    pub String,
);
type Result<T> = std::result::Result<T, TmLanguageError>;

pub(crate) fn is_tm_language(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    name.ends_with(".tmLanguage") || name.ends_with(".tmLanguage.json")
}

impl SyntaxDefinition {
    /// Converts XML plist or JSON TextMate grammar text. Lines must retain `\n`.
    /// Unsupported injections, capture retokenization and interpolated scope names
    /// return a diagnostic instead of silently changing their meaning.
    pub fn load_tm_language(source: &str) -> std::result::Result<Self, LoadingError> {
        load(source).map_err(LoadingError::from)
    }
}
fn error(message: impl Into<String>) -> TmLanguageError {
    TmLanguageError(message.into())
}
fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| error("expected dictionary"))
}
fn string<'a>(map: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>> {
    map.get(key)
        .map(|v| {
            v.as_str()
                .ok_or_else(|| error(format!("{key} must be a string")))
        })
        .transpose()
}
fn load(source: &str) -> Result<SyntaxDefinition> {
    let source = source.trim_start_matches('\u{feff}');
    let value: Value = if source.trim_start().starts_with('{') {
        serde_json::from_str(source).map_err(|e| error(e.to_string()))?
    } else {
        let bytes =
            crate::metadata::preserve_cdata(source.as_bytes()).map_err(|e| error(e.to_string()))?;
        let plist =
            plist::Value::from_reader_xml(bytes.as_slice()).map_err(|e| error(e.to_string()))?;
        serde_json::to_value(plist).map_err(|e| error(e.to_string()))?
    };
    let root = object(&value)?;
    if root.contains_key("injections") || root.contains_key("injectionSelector") {
        return Err(error("injections are not supported"));
    }
    let scope = string(root, "scopeName")?.ok_or_else(|| error("missing scopeName"))?;
    let skeleton = serde_json::json!({
        "name": string(root, "name")?.unwrap_or(scope), "scope": scope, "version": 2,
        "hidden": root.get("hideFromUser").and_then(Value::as_bool).unwrap_or(false),
        "file_extensions": root.get("fileTypes").cloned().unwrap_or_else(|| serde_json::json!([])),
        "first_line_match": string(root, "firstLineMatch")?.unwrap_or("(?!)"),
        "contexts": {"main": [{"meta_include_prototype": false}]}
    });
    let mut syntax = SyntaxDefinition::load_from_str(&skeleton.to_string(), true, None)
        .map_err(|e| error(e.to_string()))?;
    if !root.contains_key("firstLineMatch") {
        syntax.first_line_match = None;
        syntax.raw_first_line_match = None;
    }
    let mut converter = Converter {
        contexts: HashMap::new(),
        serial: 0,
        rules: 0,
    };
    let env = converter.repository(root, &HashMap::new(), true, 0)?;
    let patterns = converter.patterns(root, &env, 0)?;
    syntax.contexts.get_mut("main").unwrap().patterns = patterns;
    syntax.contexts.extend(converter.contexts);
    Ok(syntax)
}
struct Converter {
    contexts: HashMap<String, Context>,
    serial: usize,
    rules: usize,
}
impl Converter {
    fn fresh(&mut self) -> String {
        self.serial += 1;
        format!("tm.anonymous.{}", self.serial)
    }
    fn repository(
        &mut self,
        rule: &Map<String, Value>,
        parent: &HashMap<String, String>,
        root: bool,
        depth: usize,
    ) -> Result<HashMap<String, String>> {
        self.check_depth(depth)?;
        let mut env = parent.clone();
        let Some(repository) = rule.get("repository") else {
            return Ok(env);
        };
        let repository = object(repository)?;
        for key in repository.keys() {
            let name = if root {
                format!("tm.repository.{key}")
            } else {
                self.fresh()
            };
            env.insert(key.clone(), name);
        }
        for (key, value) in repository {
            let patterns = self.rule(object(value)?, &env, depth + 1)?;
            let mut context = Context::new(Some(false));
            context.patterns = patterns;
            self.contexts.insert(env[key].clone(), context);
        }
        Ok(env)
    }
    fn check_depth(&self, depth: usize) -> Result<()> {
        if depth > 128 {
            Err(error("nesting exceeds 128 levels"))
        } else {
            Ok(())
        }
    }
    fn patterns(
        &mut self,
        rule: &Map<String, Value>,
        env: &HashMap<String, String>,
        depth: usize,
    ) -> Result<Vec<Pattern>> {
        self.check_depth(depth)?;
        let Some(patterns) = rule.get("patterns") else {
            return Ok(Vec::new());
        };
        let patterns = patterns
            .as_array()
            .ok_or_else(|| error("patterns must be an array"))?;
        let mut output = Vec::new();
        for pattern in patterns {
            output.extend(self.rule(object(pattern)?, env, depth + 1)?);
        }
        Ok(output)
    }
    fn rule(
        &mut self,
        rule: &Map<String, Value>,
        parent: &HashMap<String, String>,
        depth: usize,
    ) -> Result<Vec<Pattern>> {
        self.check_depth(depth)?;
        self.rules += 1;
        if self.rules > 100_000 {
            return Err(error("more than 100000 patterns"));
        }
        if rule
            .get("disabled")
            .is_some_and(|v| v == &Value::Bool(true) || v == &Value::from(1))
        {
            return Ok(Vec::new());
        }
        let env = self.repository(rule, parent, false, depth + 1)?;
        if let Some(include) = string(rule, "include")? {
            let reference = match include {
                "$self" => ContextReference::Named("main".into()),
                "$base" => ContextReference::Base,
                name if name.starts_with('#') => ContextReference::Named(
                    env.get(&name[1..])
                        .cloned()
                        .ok_or_else(|| error(format!("unknown repository include {name}")))?,
                ),
                name => {
                    let (scope, sub) = name.split_once('#').map_or((name, None), |(s, n)| {
                        (s, Some(format!("tm.repository.{n}")))
                    });
                    ContextReference::ByScope {
                        scope: scope
                            .parse()
                            .map_err(|e| error(format!("invalid include scope: {e}")))?,
                        sub_context: sub,
                        with_escape: false,
                    }
                }
            };
            return Ok(vec![Pattern::Include(reference)]);
        }
        let captures = capture_map(rule.get("captures"))?;
        if let Some(regex) = string(rule, "match")? {
            return Ok(vec![Pattern::Match(MatchPattern::new(
                false,
                regex.into(),
                scopes(string(rule, "name")?)?,
                captures,
                MatchOperation::None,
                None,
            ))]);
        }
        if let Some(begin) = string(rule, "begin")? {
            let name = self.fresh();
            let mut context = Context::new(Some(false));
            context.meta_scope = scopes(string(rule, "name")?)?;
            context.meta_content_scope = scopes(string(rule, "contentName")?)?;
            let mut patterns = self.patterns(rule, &env, depth + 1)?;
            let while_regex = string(rule, "while")?;
            let end = string(rule, "end")?;
            if while_regex.is_some() && end.is_some() {
                return Err(error("rule has both end and while"));
            }
            if let Some(regex) = while_regex {
                let pattern = MatchPattern::new(
                    backrefs(regex),
                    regex.into(),
                    Vec::new(),
                    capture_map(rule.get("whileCaptures"))?.or_else(|| captures.clone()),
                    MatchOperation::None,
                    None,
                );
                context.uses_backrefs = pattern.has_captures;
                context.while_pattern = Some(pattern);
            } else {
                let regex = end.unwrap_or("(?!)");
                let pattern = MatchPattern::new(
                    backrefs(regex),
                    regex.into(),
                    Vec::new(),
                    capture_map(rule.get("endCaptures"))?.or_else(|| captures.clone()),
                    MatchOperation::Pop(1),
                    None,
                );
                context.uses_backrefs = pattern.has_captures;
                let last = rule
                    .get("applyEndPatternLast")
                    .is_some_and(|v| v == &Value::Bool(true) || v == &Value::from(1));
                if last {
                    patterns.push(Pattern::Match(pattern));
                } else {
                    patterns.insert(0, Pattern::Match(pattern));
                }
            }
            context.patterns = patterns;
            self.contexts.insert(name.clone(), context);
            return Ok(vec![Pattern::Match(MatchPattern::new(
                false,
                begin.into(),
                Vec::new(),
                capture_map(rule.get("beginCaptures"))?.or(captures),
                MatchOperation::Push {
                    ctx_refs: vec![ContextReference::Named(name)],
                    pop_count: 0,
                },
                None,
            ))]);
        }
        self.patterns(rule, &env, depth + 1)
    }
}
fn scopes(text: Option<&str>) -> Result<Vec<Scope>> {
    text.unwrap_or("")
        .split_whitespace()
        .map(|s| {
            if s.contains('$') {
                return Err(error("interpolated scope names are not supported"));
            }
            s.parse().map_err(|e| error(format!("invalid scope: {e}")))
        })
        .collect()
}
fn capture_map(value: Option<&Value>) -> Result<Option<CaptureMapping>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let mut captures = Vec::new();
    for (number, value) in object(value)? {
        let map = object(value)?;
        if map.contains_key("patterns") {
            return Err(error("capture retokenization is not supported"));
        }
        captures.push((
            number
                .parse()
                .map_err(|_| error("capture key is not numeric"))?,
            scopes(string(map, "name")?)?,
        ));
    }
    captures.sort_by_key(|c| c.0);
    Ok(Some(captures))
}
fn backrefs(regex: &str) -> bool {
    let mut chars = regex.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.next().is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}
