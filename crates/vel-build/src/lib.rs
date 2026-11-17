//! Original build-system parsing, variable expansion, streaming exec and result navigation.
#![warn(missing_docs)]
mod environment;
/// Warms the cached login environment in the background when build systems are selected.
pub fn warm_login_environment() {
    environment::warm_login_environment();
}

mod output;
pub use output::{Events, OutputStats, Results};
mod process;
pub mod variables;
pub use process::{Event, Stream, Task};
use regex::Regex;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
pub use variables::Variables;
/// Named build resource or inline project definition.
#[derive(Clone, Debug)]
pub struct BuildSystem {
    /// Display name, independent of a resource path.
    pub name: String,
    /// Effective platform-specific options, retaining unknown fields.
    pub options: Map<String, Value>,
}
impl BuildSystem {
    /// Parses a `.sublime-build` using the shared comment/trailing-comma JSON reader.
    pub fn parse(name: &str, source: &str) -> Result<Self, String> {
        Self::from_value(name, vel_config::json::Document::parse(source)?.value)
    }
    /// Loads a project-inline definition and merges the current platform overlay.
    pub fn from_value(name: &str, value: Value) -> Result<Self, String> {
        let mut options = value.as_object().ok_or("build must be an object")?.clone();
        let platform = if cfg!(target_os = "macos") {
            "osx"
        } else if cfg!(windows) {
            "windows"
        } else {
            "linux"
        };
        if let Some(overrides) = options.get(platform).and_then(Value::as_object).cloned() {
            options.extend(overrides);
        }
        let name = options
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_owned();
        Ok(Self { name, options })
    }
    /// Variant names in source order, excluding the base build.
    pub fn variants(&self) -> Vec<String> {
        self.options
            .get("variants")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v["name"].as_str().map(str::to_owned))
            .collect()
    }
    /// Merges one named variant over base options and platform settings.
    pub fn variant(&self, name: &str) -> Result<Self, String> {
        let variant = self
            .options
            .get("variants")
            .and_then(Value::as_array)
            .and_then(|v| v.iter().find(|v| v["name"] == name))
            .ok_or("unknown build variant")?;
        let mut options = self.options.clone();
        options.remove("variants");
        // The base is already platform-resolved; a variant's own platform options win.
        // https://www.sublimetext.com/docs/build_systems.html#variants
        for platform in ["osx", "windows", "linux"] {
            options.remove(platform);
        }
        options.extend(
            variant
                .as_object()
                .ok_or("variant must be an object")?
                .clone(),
        );
        Self::from_value(&self.name, Value::Object(options))
    }
    /// Automatic eligibility: any matching syntax, filename pattern, or folder keyfile.
    pub fn matches(&self, scope: &str, file: Option<&Path>, folders: &[PathBuf]) -> bool {
        let scopes = scope
            .split_whitespace()
            .filter_map(|s| vel_scope::Scope::new(s).ok())
            .collect::<Vec<_>>();
        self.match_score(&scopes, file, folders).is_some()
    }
    /// Eligibility and specificity using the full caret stack, including embedded languages.
    pub fn match_score(
        &self,
        scopes: &[vel_scope::Scope],
        file: Option<&Path>,
        folders: &[PathBuf],
    ) -> Option<vel_scope::MatchScore> {
        // https://www.sublimetext.com/docs/build_systems.html#selector
        let selector = self
            .options
            .get("selector")
            .and_then(Value::as_str)
            .and_then(|s| vel_scope::Selector::parse(s).ok())
            .and_then(|s| s.does_match(scopes));
        let pattern = self
            .options
            .get("file_patterns")
            .and_then(Value::as_array)
            .is_some_and(|patterns| {
                patterns.iter().filter_map(Value::as_str).any(|p| {
                    globset::Glob::new(p).is_ok_and(|g| {
                        file.is_some_and(|f| {
                            g.compile_matcher().is_match(f)
                                || f.file_name()
                                    .is_some_and(|n| g.compile_matcher().is_match(Path::new(n)))
                        })
                    })
                })
            });
        let keys = self
            .options
            .get("keyfiles")
            .and_then(Value::as_array)
            .map(|v| v.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_else(|| {
                self.options
                    .get("keyfile")
                    .and_then(Value::as_str)
                    .into_iter()
                    .collect()
            });
        selector.or_else(|| {
            (pattern
                || keys
                    .iter()
                    .any(|key| folders.iter().any(|root| root.join(key).is_file())))
            .then_some(vel_scope::MatchScore(0))
        })
    }
    /// Resolves all exec fields, rejecting unsupported custom targets before process creation.
    pub fn resolve(&self, variables: &Variables) -> Result<Exec, String> {
        let string = |key: &str| self.options.get(key).and_then(Value::as_str);
        if string("target").is_some_and(|s| s != "exec") {
            return Err("only the built-in exec target is supported".into());
        }
        let cmd = match self.options.get("cmd") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(v)) => v
                .iter()
                .map(|v| variables.expand(v.as_str().ok_or("cmd arguments must be strings")?))
                .collect::<Result<Vec<_>, String>>()?,
            _ => return Err("cmd must be an array of strings".into()),
        };
        let shell_cmd = string("shell_cmd")
            .map(|s| variables.expand(s))
            .transpose()?;
        if shell_cmd.is_none() && cmd.is_empty() {
            return Err("build needs cmd or shell_cmd".into());
        }
        if self.options.get("interactive").and_then(Value::as_bool) == Some(true) {
            return Err("Interactive builds are not supported (stdin is closed)".into());
        }
        let working_dir = string("working_dir")
            .map(|s| variables.expand(s))
            .transpose()?
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            // https://www.sublimetext.com/docs/build_systems.html#working_dir
            .or_else(|| {
                variables
                    .0
                    .get("file_path")
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
        // path/env are expanded again against the cached login environment at launch.
        // https://www.sublimetext.com/docs/build_systems.html#env
        let mut env = std::collections::BTreeMap::new();
        if let Some(values) = self.options.get("env") {
            for (key, value) in values.as_object().ok_or("env must be an object")? {
                env.insert(
                    key.clone(),
                    value
                        .as_str()
                        .ok_or("env values must be strings")?
                        .to_owned(),
                );
            }
        }
        if let Some(path) = string("path") {
            env.insert("PATH".into(), path.to_owned());
        }
        let env_source = env.clone();
        let base = std::env::vars().collect();
        let expanded = environment::expand_environment(&base, &env_source, variables)?;
        for (key, value) in &mut env {
            *value = expanded[key].clone();
        }
        let parser = ResultParser::new(
            string("file_regex"),
            string("line_regex"),
            working_dir.clone(),
        )?;
        Ok(Exec {
            cmd,
            shell_cmd,
            working_dir,
            env,
            env_source,
            variables: variables.clone(),
            shell: self
                .options
                .get("shell")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            quiet: self
                .options
                .get("quiet")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            // Public docs describe word_wrap without a default. Follow the round-18
            // output-panel contract: long compiler lines wrap unless explicitly disabled.
            // https://www.sublimetext.com/docs/build_systems.html#word_wrap
            word_wrap: self
                .options
                .get("word_wrap")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            syntax: string("syntax").map(str::to_owned),
            parser,
        })
    }
}
/// Resolved invocation, ready to launch without any filesystem writes by Velocity.
#[derive(Clone, Debug)]
pub struct Exec {
    /// Executable and argv.
    pub cmd: Vec<String>,
    /// Explicit shell source; takes precedence over argv.
    pub shell_cmd: Option<String>,
    /// Process working directory; output paths resolve against it.
    pub working_dir: PathBuf,
    /// Additional environment variables.
    pub env: std::collections::BTreeMap<String, String>,
    /// File/project expansion context, merged over the launch environment.
    pub variables: Variables,
    /// Original environment expressions, resolved against the cached login environment.
    pub env_source: std::collections::BTreeMap<String, String>,
    /// Run argv through the system shell.
    pub shell: bool,
    /// Suppress invocation/working-directory banner.
    pub quiet: bool,
    /// Output panel wrap preference.
    pub word_wrap: bool,
    /// Output syntax resource.
    pub syntax: Option<String>,
    /// Stateful diagnostic parser.
    pub parser: ResultParser,
}
/// A navigable compiler diagnostic / inline badge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Resolved file path.
    pub path: PathBuf,
    /// One-based line.
    pub line: usize,
    /// One-based column, default 1.
    pub column: usize,
    /// Error/warning text.
    pub message: String,
    /// Zero-based output line for double-click navigation.
    pub output_line: usize,
}
/// Stateful `file_regex`/`line_regex` parsing, including filename-only headers.
#[derive(Clone, Debug)]
pub struct ResultParser {
    file: Option<Regex>,
    line: Option<Regex>,
    base: PathBuf,
    original_base: PathBuf,
    previous: Option<PathBuf>,
    row: usize,
}
impl ResultParser {
    /// Compiles regexes once. Syntax is Rust regex (no lookarounds/backreferences).
    pub fn new(file: Option<&str>, line: Option<&str>, base: PathBuf) -> Result<Self, String> {
        let compile = |s: Option<&str>| {
            s.filter(|s| !s.is_empty())
                .map(Regex::new)
                .transpose()
                .map_err(|e| e.to_string())
        };
        Ok(Self {
            file: compile(file)?,
            line: compile(line)?,
            original_base: base.clone(),
            base: std::fs::canonicalize(&base).unwrap_or(base),
            previous: None,
            row: 0,
        })
    }
    /// Consumes one complete line; line-only matches use the latest file header.
    pub fn parse(&mut self, text: &str) -> Option<Diagnostic> {
        let row = self.row;
        self.row += 1;
        if self.file.is_none() && self.line.is_none() {
            return None;
        }
        let clean = strip_ansi(text.trim_end_matches(['\r', '\n']));
        if let Some(c) = self.file.as_ref().and_then(|r| r.captures(&clean)) {
            let raw = c.get(1)?.as_str();
            let path = PathBuf::from(raw);
            let path = if path.is_absolute() {
                path
            } else {
                self.base.join(path)
            };
            // Parsing streamed output must never issue filesystem I/O on the UI thread.
            // Resolve the working-directory alias once, then normalize compiler paths lexically.
            // Opening a result performs the usual filesystem/symlink resolution later.
            let path = if path.is_absolute() {
                path.strip_prefix(&self.original_base)
                    .map_or_else(|_| path.clone(), |relative| self.base.join(relative))
            } else {
                path
            };
            let mut normalized = PathBuf::new();
            for part in path.components() {
                match part {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        normalized.pop();
                    }
                    _ => normalized.push(part.as_os_str()),
                }
            }
            let path = normalized;
            self.previous = Some(path.clone());
            if let Some(line) = c
                .get(2)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .filter(|n| *n > 0)
            {
                return Some(Diagnostic {
                    path,
                    line,
                    column: c
                        .get(3)
                        .and_then(|m| m.as_str().parse().ok())
                        .unwrap_or(1)
                        .max(1),
                    message: c.get(4).map_or(text, |m| m.as_str()).to_owned(),
                    output_line: row,
                });
            }
        }
        let c = self.line.as_ref()?.captures(&clean)?;
        Some(Diagnostic {
            path: self.previous.clone()?,
            line: c.get(1)?.as_str().parse::<usize>().ok()?.max(1),
            column: c
                .get(2)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(1)
                .max(1),
            message: c.get(3).map_or(text, |m| m.as_str()).to_owned(),
            output_line: row,
        })
    }
}
fn strip_ansi(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\u{1b}') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}
