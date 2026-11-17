// Adapted from syntect PR #705 (MIT); see README.md and LICENSE-syntect.
//! Data structures for representing syntax definitions
//!
//! Everything here is public becaues I want this library to be useful in super integrated cases
//! like text editors and I have no idea what kind of monkeying you might want to do with the data.
//! Perhaps parsing your own syntax format into this data structure?

use crate::ParsingError;
use crate::syntax_set::SyntaxSet;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use vel_regex::escape_literal as escape;
use vel_regex::{Regex, Region};
use vel_scope::*;

/// Capture-group numbers paired with the scopes applied to each participating span.
pub type CaptureMapping = Vec<(usize, Vec<Scope>)>;

/// Information about the escape pattern for an `embed` operation.
/// The escape regex takes strict precedence over all other patterns,
/// unlike `with_prototype` where patterns compete equally.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EscapeInfo {
    /// Escape pattern, instantiated with the entering match when required.
    pub escape_regex: Regex,
    /// Whether numeric capture references need runtime substitution.
    pub has_captures: bool,
    /// Scopes applied to capture groups of the escape match.
    pub escape_captures: Option<CaptureMapping>,
    /// Raw escape regex string as written in YAML, before variable resolution.
    /// Stored to enable re-resolution when variables are merged via extends.
    #[serde(skip)]
    pub(crate) raw_escape_regex_str: Option<String>,
}

/// An opaque ID for a [`Context`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ContextId {
    /// Index into [`SyntaxSet::syntaxes`]
    pub(crate) syntax_index: usize,

    /// Index into [`crate::LazyContexts::contexts`] for the [`Self::syntax_index`] syntax
    pub(crate) context_index: usize,
}

/// The main data structure representing a syntax definition loaded from a
/// `.sublime-syntax` file
///
/// You'll probably only need these as references to be passed around to parsing code.
///
/// Some useful public fields are the `name` field which is a human readable name to display in
/// syntax lists, and the `hidden` field which means hide this syntax from any lists because it is
/// for internal use.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyntaxDefinition {
    #[serde(skip)]
    pub(crate) authoring_warnings: Vec<String>,
    #[serde(skip)]
    pub(crate) raw_first_line_match: Option<String>,
    /// Human-readable syntax name or the identifier carried by this reference.
    pub name: String,
    /// Recognized extensions, including hidden file extensions, without leading dots.
    pub file_extensions: Vec<String>,
    /// Scope names applied to the match, or the top-level scope of a syntax.
    pub scope: Scope,
    /// Optional expanded pattern for detecting shebangs and mode lines.
    pub first_line_match: Option<String>,
    /// Whether syntax selection lists should hide this definition.
    pub hidden: bool,
    #[serde(serialize_with = "ordered_map")]
    /// Named regex fragments, merged before inherited patterns are expanded.
    pub variables: HashMap<String, String>,
    #[serde(serialize_with = "ordered_map")]
    /// Named contexts, or the ordered contexts entered by this operation.
    pub contexts: HashMap<String, Context>,
    /// The syntax(es) this definition extends (e.g., "Packages/C/C.sublime-syntax").
    /// Can be a single path or a list of paths for multiple inheritance.
    #[serde(default)]
    pub extends: Vec<String>,
    /// The version of the sublime-syntax format (1 or 2). Default is 1.
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_version() -> u32 {
    1
}

fn one() -> usize {
    1
}

/// How a child context should merge with its parent during extends resolution.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ContextMergeMode {
    /// Replace the parent context entirely (default behavior).
    #[default]
    Replace,
    /// Prepend child patterns before parent patterns.
    Prepend,
    /// Append child patterns after parent patterns.
    Append,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// A context with metadata, ordered match/include rules and a linked prototype.
pub struct Context {
    #[serde(skip)]
    pub(crate) compiled: ContextCache,
    /// Assigned by the syntax-set linker; not meaningful on standalone definitions.
    pub(crate) id: ContextId,
    /// Scopes covering both delimiters and the body of an entered context.
    pub meta_scope: Vec<Scope>,
    /// Scopes covering the context body after its entering match.
    pub meta_content_scope: Vec<Scope>,
    /// Whether this context includes its syntax's prototype. `Some(bool)` carries the value
    /// set by the YAML (or inherited from a parent on an `extends:` / `meta_append` /
    /// `meta_prepend` merge). `None` means the field was never set explicitly and callers
    /// should treat it as `true` (Sublime's default — use `.unwrap_or(true)` at consumption
    /// points). Tracking unset separately lets an `extends` merge inherit the parent's
    /// value when the child doesn't restate it — the case that was miscompiling TSQL's
    /// `inside-like-single-quoted-string` `meta_append` against the SQL base that sets
    /// `meta_include_prototype: false`.
    pub meta_include_prototype: Option<bool>,
    /// Scopes temporarily removed when this context is entered.
    pub clear_scopes: Option<ClearAmount>,
    /// This is filled in by the linker at link time
    /// for contexts that have `meta_include_prototype==true`
    /// and are not included from the prototype.
    pub prototype: Option<ContextId>,
    /// Whether this context or an included context requires entering-match captures.
    pub uses_backrefs: bool,
    /// Whether this context includes the outer TextMate grammar.
    #[serde(default)]
    pub(crate) uses_base: bool,
    /// TextMate continuation tested at the start of each subsequent line.
    #[serde(default)]
    pub(crate) while_pattern: Option<MatchPattern>,

    /// Rules in source-priority order.
    pub patterns: Vec<Pattern>,

    /// How this context should be merged with a parent context during extends resolution.
    #[serde(skip)]
    pub(crate) merge_mode: ContextMergeMode,

    /// When true, this context's `meta_content_scope` (from embed_scope) should replace
    /// the embedded syntax's top-level scope rather than stacking with it. (v2 behavior)
    #[serde(default)]
    pub(crate) embed_scope_replaces: bool,
}

impl Context {
    /// Creates an empty context with the specified prototype policy.
    pub fn new(meta_include_prototype: Option<bool>) -> Context {
        Context {
            compiled: ContextCache::default(),
            id: ContextId {
                syntax_index: 0,
                context_index: 0,
            },
            meta_scope: Vec::new(),
            meta_content_scope: Vec::new(),
            meta_include_prototype,
            clear_scopes: None,
            uses_backrefs: false,
            uses_base: false,
            while_pattern: None,
            patterns: Vec::new(),
            prototype: None,
            merge_mode: ContextMergeMode::default(),
            embed_scope_replaces: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// A match rule or a context included at this priority position.
pub enum Pattern {
    /// An executable regex rule.
    Match(MatchPattern),
    /// Includes another context without implicitly adding its prototype.
    Include(ContextReference),
    /// Like `Include`, but also applies the target syntax's `prototype` context.
    /// Created when `apply_prototype: true` is used with an `include` directive.
    IncludeWithPrototype(ContextReference),
}

/// Used to iterate over all the match patterns in a context
///
/// Basically walks the tree of patterns and include directives in the correct order.
#[derive(Debug)]
pub struct MatchIter<'a> {
    syntax_set: &'a SyntaxSet,
    base: ContextId,
    ctx_stack: Vec<&'a Context>,
    index_stack: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// An expanded regex, its scope assignments, and its context transition.
pub struct MatchPattern {
    /// Whether numeric capture references need runtime substitution.
    pub has_captures: bool,
    /// Lazy expanded regex used to search the current line.
    pub regex: Regex,
    /// Scope names applied to the match, or the top-level scope of a syntax.
    pub scope: Vec<Scope>,
    /// Scope assignments for participating capture groups of the match.
    pub captures: Option<CaptureMapping>,
    /// Context transition performed when this rule wins.
    pub operation: MatchOperation,
    /// Additional rules propagated through contexts entered by this match.
    pub with_prototype: Option<ContextReference>,
    /// Raw regex string as written in YAML, before variable resolution and transforms.
    /// Stored to enable re-resolution when variables are overridden via extends.
    #[serde(skip)]
    pub(crate) raw_regex_str: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
/// A local, package-file, scope-based or already linked context reference.
pub enum ContextReference {
    /// The main context of the outermost TextMate grammar (`$base`).
    Base,
    #[non_exhaustive]
    /// A context in the current syntax.
    Named(String),
    #[non_exhaustive]
    /// A syntax identified by its top-level scope, optionally naming a context.
    ByScope {
        /// Scope names applied to the match, or the top-level scope of a syntax.
        scope: Scope,
        /// A named context instead of the target syntax main context.
        sub_context: Option<String>,
        /// `true` if this reference by scope is part of an `embed` for which
        /// there is an `escape`. In other words a reference for a context for
        /// which there "always is a way out". Enables falling back to `Plain
        /// Text` syntax in case the referenced scope is missing.
        with_escape: bool,
    },
    #[non_exhaustive]
    /// A syntax identified by package filename or path.
    File {
        /// Human-readable syntax name or the identifier carried by this reference.
        name: String,
        /// A named context instead of the target syntax main context.
        sub_context: Option<String>,
        /// Same semantics as for [`Self::ByScope::with_escape`].
        with_escape: bool,
    },
    #[non_exhaustive]
    /// An anonymous context synthesized while loading YAML.
    Inline(String),
    #[non_exhaustive]
    /// An already linked context identifier.
    Direct(ContextId),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// The stack or branch action performed by a winning match rule.
pub enum MatchOperation {
    /// Pushes `ctx_refs`, after first popping `pop_count` contexts.
    ///
    /// `pop_count` is new, and it is not a `Set`. Sublime
    /// documents the difference — "for `push`, `embed` and `branch` actions, the
    /// pop treats the match as if it were a lookahead, which means the match
    /// will not receive the `meta_scope` of the contexts that are popped",
    /// whereas a `set` gives the match the meta scopes of both. The stack effect
    /// is identical, so only the trigger token's scopes tell them apart, which
    /// is why they need to be different variants rather than one with a count.
    /// `pop_count == 0` is an ordinary `push:`.
    Push {
        /// Contexts pushed in order, with the final one becoming active.
        ctx_refs: Vec<ContextReference>,
        #[serde(default)]
        /// Number of contexts removed before entering the replacement contexts.
        pop_count: usize,
    },
    /// Pops `pop_count` contexts off the stack, then pushes `ctx_refs`.
    /// A plain `set:` is `pop_count == 1`; `pop: N + set:` is `pop_count == N`.
    Set {
        /// Contexts pushed in order, with the final one becoming active.
        ctx_refs: Vec<ContextReference>,
        #[serde(default = "one")]
        /// Number of contexts removed before entering the replacement contexts.
        pop_count: usize,
    },
    /// Removes the requested number of context frames.
    Pop(usize),
    /// Applies scopes without changing the context stack.
    None,
    /// Branch with backtracking.
    /// Acts like Push for the first alternative, saving a checkpoint.
    /// If a `Fail` with the same name fires later, the next alternative is tried.
    Branch {
        /// Human-readable syntax name or the identifier carried by this reference.
        name: String,
        /// Ordered interpretations retried when a matching fail action occurs.
        alternatives: Vec<ContextReference>,
        /// Number of contexts to pop before pushing the branch alternative.
        /// In Sublime Text, `pop: N` combined with `branch:` pops N contexts
        /// then sets up the branch point.
        #[serde(default)]
        pop_count: usize,
    },
    /// Trigger backtracking to the named branch point.
    Fail(String),
    /// Embed contexts with a prioritized escape pattern.
    /// Unlike Push+with_prototype, the escape regex takes strict precedence
    /// over all other patterns — it is checked first and truncates the search
    /// region for normal patterns.
    Embed {
        /// Named contexts, or the ordered contexts entered by this operation.
        contexts: Vec<ContextReference>,
        /// Prioritized escape pattern and its scope assignments.
        escape: EscapeInfo,
        /// Number of contexts to pop before pushing the embedded contexts.
        /// In Sublime Text, `pop: N` combined with `embed:` pops N contexts
        /// then pushes the embedded syntax with escape priority.
        #[serde(default)]
        pop_count: usize,
    },
}

impl<'a> Iterator for MatchIter<'a> {
    type Item = (&'a Context, usize);

    fn next(&mut self) -> Option<(&'a Context, usize)> {
        loop {
            if self.ctx_stack.is_empty() {
                return None;
            }
            // uncomment for debugging infinite recursion
            // println!("{:?}", self.index_stack);
            // use std::thread::sleep_ms;
            // sleep_ms(500);
            let last_index = self.ctx_stack.len() - 1;
            let context = self.ctx_stack[last_index];
            let index = self.index_stack[last_index];
            self.index_stack[last_index] = index + 1;
            if index < context.patterns.len() {
                match context.patterns[index] {
                    Pattern::Match(_) => {
                        return Some((context, index));
                    }
                    Pattern::Include(ref ctx_ref) => {
                        let ctx_ptr = match *ctx_ref {
                            ContextReference::Direct(ref context_id) => {
                                self.syntax_set.get_context(context_id).unwrap()
                            }
                            ContextReference::Base => {
                                self.syntax_set.get_context(&self.base).unwrap()
                            }
                            _ => return self.next(), // skip this and move onto the next one
                        };
                        if self.ctx_stack.iter().any(|ctx| std::ptr::eq(*ctx, ctx_ptr)) {
                            continue;
                        }
                        self.ctx_stack.push(ctx_ptr);
                        self.index_stack.push(0);
                    }
                    Pattern::IncludeWithPrototype(ref ctx_ref) => {
                        let context_id = match *ctx_ref {
                            ContextReference::Direct(ref id) => id,
                            _ => return self.next(),
                        };
                        let ctx_ptr = self.syntax_set.get_context(context_id).unwrap();
                        // The included context goes on first,
                        // and the prototype on top of it. `ctx_stack` is a
                        // stack — `next` reads its *last* entry — so whatever
                        // is pushed last is iterated first, and pushing the
                        // prototype first put its patterns after every one of
                        // the included context's. A prototype outranks the
                        // context it is attached to (`find_best_match` chains
                        // `cur_prototype` before `cur_context` for exactly
                        // that reason), and at an equal start position the
                        // earlier pattern wins, so the order was the whole
                        // difference: HAML's `- include:
                        // scope:source.ruby.rails.embedded.haml` with
                        // `apply_prototype: true` never saw that syntax's
                        // prototype rule for a line-ending `|`, and Ruby's
                        // bitwise-or rule took it instead.
                        //
                        // Also include the external syntax's prototype if the context allows it
                        if self.ctx_stack.iter().any(|ctx| std::ptr::eq(*ctx, ctx_ptr)) {
                            continue;
                        }
                        self.ctx_stack.push(ctx_ptr);
                        self.index_stack.push(0);
                        if ctx_ptr.meta_include_prototype.unwrap_or(true)
                            && let Some(ref proto_id) = ctx_ptr.prototype
                        {
                            let proto_ctx = self.syntax_set.get_context(proto_id).unwrap();
                            self.ctx_stack.push(proto_ctx);
                            self.index_stack.push(0);
                        }
                    }
                }
            } else {
                self.ctx_stack.pop();
                self.index_stack.pop();
            }
        }
    }
}

/// Returns an iterator over all the match patterns in this context.
///
/// It recursively follows include directives. Can only be run on contexts that have already been
/// linked up.
pub fn context_iter<'a>(syntax_set: &'a SyntaxSet, context: &'a Context) -> MatchIter<'a> {
    context_iter_with_base(syntax_set, context, context.id)
}

pub(crate) fn context_iter_with_base<'a>(
    syntax_set: &'a SyntaxSet,
    context: &'a Context,
    base: ContextId,
) -> MatchIter<'a> {
    MatchIter {
        syntax_set,
        base,
        ctx_stack: vec![context],
        index_stack: vec![0],
    }
}

impl Context {
    /// Returns the match pattern at an index
    pub fn match_at(&self, index: usize) -> Result<&MatchPattern, ParsingError> {
        match self.patterns.get(index) {
            Some(Pattern::Match(match_pat)) => Ok(match_pat),
            _ => Err(ParsingError::BadMatchIndex(index)),
        }
    }
}

impl ContextReference {
    /// find the pointed to context
    pub fn resolve<'a>(&self, syntax_set: &'a SyntaxSet) -> Result<&'a Context, ParsingError> {
        match *self {
            ContextReference::Direct(ref context_id) => syntax_set.get_context(context_id),
            _ => syntax_set.get_context(&syntax_set.resolve_reference(self)?),
        }
    }

    /// Resolves a reference on first use, including cross-syntax embeds.
    pub fn id_in(&self, syntax_set: &SyntaxSet) -> Result<ContextId, ParsingError> {
        syntax_set.resolve_reference(self)
    }

    /// get the context ID this reference points to
    pub fn id(&self) -> Result<ContextId, ParsingError> {
        match *self {
            ContextReference::Direct(ref context_id) => Ok(*context_id),
            _ => Err(ParsingError::UnresolvedContextReference(self.clone())),
        }
    }
}

pub(crate) fn substitute_backrefs_in_regex<F>(regex_str: &str, substituter: F) -> String
where
    F: Fn(usize) -> Option<String>,
{
    let mut reg_str = String::with_capacity(regex_str.len());
    let mut chars = regex_str.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            reg_str.push(c);
            continue;
        }
        match chars.next() {
            Some(d) if d.is_ascii_digit() => {
                let mut digits = d.to_string();
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    digits.push(chars.next().unwrap());
                }
                if let Ok(index) = digits.parse()
                    && let Some(value) = substituter(index)
                {
                    reg_str.push_str(&value);
                }
            }
            Some(c) => {
                reg_str.push('\\');
                reg_str.push(c);
            }
            None => reg_str.push('\\'),
        }
    }
    reg_str
}

impl MatchPattern {
    /// Creates an expanded match rule from its scopes and stack action.
    pub fn new(
        has_captures: bool,
        regex_str: String,
        scope: Vec<Scope>,
        captures: Option<CaptureMapping>,
        operation: MatchOperation,
        with_prototype: Option<ContextReference>,
    ) -> MatchPattern {
        MatchPattern {
            has_captures,
            regex: Regex::new(regex_str),
            scope,
            captures,
            operation,
            with_prototype,
            raw_regex_str: None,
        }
    }

    pub(crate) fn new_with_raw(
        has_captures: bool,
        regex_str: String,
        raw_regex_str: String,
        scope: Vec<Scope>,
        captures: Option<CaptureMapping>,
        operation: MatchOperation,
        with_prototype: Option<ContextReference>,
    ) -> MatchPattern {
        MatchPattern {
            has_captures,
            regex: Regex::new(regex_str),
            scope,
            captures,
            operation,
            with_prototype,
            raw_regex_str: Some(raw_regex_str),
        }
    }

    /// Used by the parser to compile a regex which needs to reference
    /// regions from another matched pattern.
    pub fn regex_with_refs(&self, region: &Region, text: &str) -> Regex {
        let new_regex = substitute_backrefs_in_regex(self.regex.regex_str(), |i| {
            region.pos(i).map(|(start, end)| escape(&text[start..end]))
        });

        // No prefilter. This pattern is rebuilt at every
        // token it is tried at, so working one out costs a `regex-syntax`
        // parse per token to save at most a single search.
        Regex::new_unfiltered(new_regex)
    }

    /// Returns the shared lazy regex for this rule.
    pub fn regex(&self) -> &Regex {
        &self.regex
    }
}

/// Serialize the provided map in natural key order, so that it's deterministic when dumping.
pub(crate) fn ordered_map<K, V, S>(map: &HashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    K: Eq + Hash + Ord + Serialize,
    V: Serialize,
{
    let ordered: BTreeMap<_, _> = map.iter().collect();
    ordered.serialize(serializer)
}

// Cache contents are not part of a syntax definition's value or serialized representation.
#[derive(Debug, Default)]
pub(crate) struct ContextCache(
    pub std::sync::OnceLock<std::sync::Arc<crate::syntax_set::ContextPatterns>>,
);
impl Clone for ContextCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}
impl PartialEq for ContextCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for ContextCache {}
