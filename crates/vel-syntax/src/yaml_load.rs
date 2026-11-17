// Adapted from syntect PR #705 (MIT); see README.md and LICENSE-syntect.
use super::syntax_definition::*;
use std::collections::HashMap;
use std::error::Error;
use std::ops::DerefMut;
use std::path::Path;
use vel_regex::{Regex, Region};
use vel_scope::*;
use yaml_rust2::yaml::Hash;
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
/// Recoverable YAML, structure, variable-expansion or scope errors in a syntax definition.
pub enum ParseSyntaxError {
    /// Invalid YAML file syntax, or at least something yaml_rust2 can't handle
    #[error("Invalid YAML file syntax: {0}")]
    InvalidYaml(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The file must contain at least one YAML document
    #[error("The file must contain at least one YAML document")]
    EmptyFile,
    /// Some keys are required for something to be a valid `.sublime-syntax`
    #[error("Missing mandatory key in YAML file: {0}")]
    MissingMandatoryKey(&'static str),
    /// Recursive variables cannot expand to a finite regex.
    #[error("recursive syntax variable: {0}")]
    VariableCycle(String),
    /// Expansion is bounded to keep malformed user definitions recoverable.
    #[error("variable expansion exceeds 128 levels or 1 MiB")]
    VariableExpansionLimit,
    /// Invalid regex
    #[error("Error while compiling regex '{0}': {1}")]
    RegexCompileError(String, #[source] Box<dyn Error + Send + Sync + 'static>),
    /// A scope that syntect's scope implementation can't handle
    #[error("Invalid scope: {0}")]
    InvalidScope(ParseScopeError),
    /// A reference to another file that is invalid
    #[error("Invalid file reference")]
    BadFileRef,
    /// Syntaxes must have a context named "main"
    #[error("Context 'main' is missing")]
    MainMissing,
    /// Some part of the YAML file is the wrong type (e.g a string but should be a list)
    /// Sorry this doesn't give you any way to narrow down where this is.
    /// Maybe use Sublime Text to figure it out.
    #[error("Type mismatch")]
    TypeMismatch,
    /// Invalid optional field, including its authoring location.
    #[error("{0}")]
    InvalidValue(String),
}

fn get_key<'a, R, F: FnOnce(&'a Yaml) -> Option<R>>(
    map: &'a Hash,
    key: &'static str,
    f: F,
) -> Result<R, ParseSyntaxError> {
    map.get(&Yaml::String(key.to_owned()))
        .ok_or(ParseSyntaxError::MissingMandatoryKey(key))
        .and_then(|x| f(x).ok_or(ParseSyntaxError::TypeMismatch))
}

fn str_to_scopes(s: &str, repo: &mut ScopeRepository) -> Result<Vec<Scope>, ParseSyntaxError> {
    s.split_whitespace()
        .map(|scope| repo.build(scope).map_err(ParseSyntaxError::InvalidScope))
        .collect()
}

pub(crate) struct ParserState<'a> {
    pub(crate) scope_repo: &'a mut ScopeRepository,
    pub(crate) variables: HashMap<String, String>,
    pub(crate) variable_regex: Regex,
    pub(crate) backref_regex: Regex,
    pub(crate) lines_include_newline: bool,
    pub(crate) version: u32,
    pub(crate) context_depth: usize,
}

// `__start` must not include prototypes from the actual syntax definition,
// otherwise it's possible that a prototype makes us pop out of `__start`.
static START_CONTEXT: &str = "
__start:
    - meta_include_prototype: false
    - match: ''
      push: __main
__main:
    - include: main
";

impl SyntaxDefinition {
    /// In case you want to create your own SyntaxDefinition's in memory from strings.
    ///
    /// Generally you should use a [`SyntaxSet`].
    ///
    /// `fallback_name` is an optional name to use when the YAML doesn't provide a `name` key.
    ///
    /// [`SyntaxSet`]: ../struct.SyntaxSet.html
    pub fn load_from_str(
        s: &str,
        lines_include_newline: bool,
        fallback_name: Option<&str>,
    ) -> Result<SyntaxDefinition, ParseSyntaxError> {
        let mut docs = match YamlLoader::load_from_str(s) {
            Ok(x) => x,
            Err(e) => return Err(ParseSyntaxError::InvalidYaml(Box::new(e))),
        };
        if docs.is_empty() {
            return Err(ParseSyntaxError::EmptyFile);
        }
        let warnings = normalize_authoring_types(&mut docs[0]);
        let doc = &docs[0];
        let mut scope_repo = lock_global_scope_repo();
        let mut syntax = SyntaxDefinition::parse_top_level(
            doc,
            scope_repo.deref_mut(),
            lines_include_newline,
            fallback_name,
        )?;
        syntax.authoring_warnings = warnings;
        Ok(syntax)
    }

    fn parse_top_level(
        doc: &Yaml,
        scope_repo: &mut ScopeRepository,
        lines_include_newline: bool,
        fallback_name: Option<&str>,
    ) -> Result<SyntaxDefinition, ParseSyntaxError> {
        let h = doc.as_hash().ok_or(ParseSyntaxError::TypeMismatch)?;

        let mut variables = HashMap::new();
        if let Ok(map) = get_key(h, "variables", |x| x.as_hash()) {
            for (key, value) in map.iter() {
                if let (Some(key_str), Some(val_str)) = (key.as_str(), value.as_str()) {
                    variables.insert(key_str.to_owned(), val_str.to_owned());
                }
            }
        }
        validate_variables(&variables)?;
        let has_extends = get_key(h, "extends", Some).is_ok();
        let empty_contexts = Hash::new();
        let contexts_hash = match get_key(h, "contexts", |x| x.as_hash()) {
            Ok(hash) => hash,
            // extends-only syntaxes (e.g. "Batch File (Compound)") inherit
            // all contexts from their parent and have no `contexts:` key.
            Err(_) if has_extends => &empty_contexts,
            Err(e) => return Err(e),
        };
        let top_level_scope = scope_repo
            .build(get_key(h, "scope", |x| x.as_str())?)
            .map_err(ParseSyntaxError::InvalidScope)?;
        let version = get_key(h, "version", |x| x.as_i64()).unwrap_or(1) as u32;

        let mut state = ParserState {
            scope_repo,
            variables,
            variable_regex: Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}".into()),
            backref_regex: Regex::new(r"\\\d".into()),
            lines_include_newline,
            version,
            context_depth: 0,
        };

        let mut contexts = SyntaxDefinition::parse_contexts(contexts_hash, &mut state)?;
        if !contexts.contains_key("main") && !has_extends {
            return Err(ParseSyntaxError::MainMissing);
        }

        if contexts.contains_key("main") {
            SyntaxDefinition::add_initial_contexts(&mut contexts, &mut state, top_level_scope);
        }

        let mut file_extensions = Vec::new();
        for extension_key in &["file_extensions", "hidden_file_extensions"] {
            if let Ok(v) = get_key(h, extension_key, |x| x.as_vec()) {
                file_extensions.extend(v.iter().filter_map(|y| y.as_str().map(|s| s.to_owned())))
            }
        }

        let extends = get_key(h, "extends", Some)
            .ok()
            .map(|y| {
                if let Some(s) = y.as_str() {
                    vec![s.to_owned()]
                } else if let Some(seq) = y.as_vec() {
                    seq.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                        .collect()
                } else {
                    vec![]
                }
            })
            .unwrap_or_default();

        let defn = SyntaxDefinition {
            authoring_warnings: Vec::new(),
            raw_first_line_match: get_key(h, "first_line_match", |x| x.as_str())
                .ok()
                .map(str::to_owned),
            name: get_key(h, "name", |x| x.as_str())
                .unwrap_or_else(|_| fallback_name.unwrap_or("Unnamed"))
                .to_owned(),
            scope: top_level_scope,
            file_extensions,
            // TODO maybe cache a compiled version of this Regex
            first_line_match: get_key(h, "first_line_match", |x| x.as_str())
                .ok()
                .map(|s| Self::resolve_variables(s, &state)),
            hidden: get_key(h, "hidden", |x| x.as_bool()).unwrap_or(false),

            variables: state.variables,
            contexts,
            extends,
            version,
        };
        Ok(defn)
    }

    fn parse_contexts(
        map: &Hash,
        state: &mut ParserState<'_>,
    ) -> Result<HashMap<String, Context>, ParseSyntaxError> {
        let mut contexts = HashMap::new();
        for (key, value) in map.iter() {
            if let (Some(name), Some(val_vec)) = (key.as_str(), value.as_vec()) {
                let mut namer = ContextNamer::new(name);
                SyntaxDefinition::parse_context(val_vec, state, &mut contexts, &mut namer)?;
            }
        }

        Ok(contexts)
    }

    fn parse_context(
        vec: &[Yaml],
        // TODO: Maybe just pass the scope repo if that's all that's needed?
        state: &mut ParserState<'_>,
        contexts: &mut HashMap<String, Context>,
        namer: &mut ContextNamer,
    ) -> Result<String, ParseSyntaxError> {
        // Every parsed context starts with `meta_include_prototype = None`
        // (unset). YAML-explicit `meta_include_prototype: <bool>` later
        // upgrades it to `Some(<bool>)`. The prototype context's own
        // self-attachment is suppressed by the `no_prototype` set in
        // `SyntaxSetBuilder::link_syntaxes`, so we don't need a distinct
        // initial value for prototype vs. non-prototype contexts.
        if state.context_depth >= 256 {
            return Err(ParseSyntaxError::InvalidValue(
                "anonymous context nesting exceeds 256".into(),
            ));
        }
        state.context_depth += 1;
        let mut context = Context::new(None);
        let name = namer.next();

        for y in vec.iter() {
            let map = y.as_hash().ok_or(ParseSyntaxError::TypeMismatch)?;

            let mut is_special = false;
            if let Ok(x) = get_key(map, "meta_scope", |x| x.as_str()) {
                context.meta_scope = str_to_scopes(x, state.scope_repo)?;
                is_special = true;
            }
            if let Ok(x) = get_key(map, "meta_content_scope", |x| x.as_str()) {
                context.meta_content_scope = str_to_scopes(x, state.scope_repo)?;
                is_special = true;
            }
            if let Ok(x) = get_key(map, "meta_include_prototype", |x| x.as_bool()) {
                context.meta_include_prototype = Some(x);
                is_special = true;
            }
            if let Ok(true) = get_key(map, "meta_prepend", |x| x.as_bool()) {
                context.merge_mode = ContextMergeMode::Prepend;
                is_special = true;
            }
            if let Ok(true) = get_key(map, "meta_append", |x| x.as_bool()) {
                context.merge_mode = ContextMergeMode::Append;
                is_special = true;
            }
            if let Ok(true) = get_key(map, "clear_scopes", |x| x.as_bool()) {
                context.clear_scopes = Some(ClearAmount::All);
                is_special = true;
            }
            if let Ok(x) = get_key(map, "clear_scopes", |x| x.as_i64()) {
                context.clear_scopes = Some(ClearAmount::TopN(x as usize));
                is_special = true;
            }
            if !is_special {
                if let Ok(x) = get_key(map, "include", Some) {
                    let reference =
                        SyntaxDefinition::parse_reference(x, state, contexts, namer, false)?;
                    let apply_prototype =
                        get_key(map, "apply_prototype", |x| x.as_bool()).unwrap_or(false);
                    if apply_prototype {
                        context
                            .patterns
                            .push(Pattern::IncludeWithPrototype(reference));
                    } else {
                        context.patterns.push(Pattern::Include(reference));
                    }
                } else {
                    let pattern =
                        SyntaxDefinition::parse_match_pattern(map, state, contexts, namer)?;
                    if pattern.has_captures {
                        context.uses_backrefs = true;
                    }
                    context.patterns.push(Pattern::Match(pattern));
                }
            }
        }

        state.context_depth -= 1;
        contexts.insert(name.clone(), context);
        Ok(name)
    }

    fn parse_reference(
        y: &Yaml,
        state: &mut ParserState<'_>,
        contexts: &mut HashMap<String, Context>,
        namer: &mut ContextNamer,
        with_escape: bool,
    ) -> Result<ContextReference, ParseSyntaxError> {
        if let Some(s) = y.as_str() {
            let parts: Vec<&str> = s.split('#').collect();
            let sub_context = if parts.len() > 1 {
                Some(parts[1].to_owned())
            } else {
                None
            };
            if parts[0].starts_with("scope:") {
                Ok(ContextReference::ByScope {
                    scope: state
                        .scope_repo
                        .build(&parts[0][6..])
                        .map_err(ParseSyntaxError::InvalidScope)?,
                    sub_context,
                    with_escape,
                })
            } else if parts[0].ends_with(".sublime-syntax") {
                if Path::new(parts[0]).file_stem().is_none() {
                    return Err(ParseSyntaxError::BadFileRef);
                }
                Ok(ContextReference::File {
                    name: parts[0].to_owned(),
                    sub_context,
                    with_escape,
                })
            } else {
                Ok(ContextReference::Named(parts[0].to_owned()))
            }
        } else if let Some(v) = y.as_vec() {
            let subname = SyntaxDefinition::parse_context(v, state, contexts, namer)?;
            Ok(ContextReference::Inline(subname))
        } else {
            Err(ParseSyntaxError::TypeMismatch)
        }
    }

    fn parse_match_pattern(
        map: &Hash,
        state: &mut ParserState<'_>,
        contexts: &mut HashMap<String, Context>,
        namer: &mut ContextNamer,
    ) -> Result<MatchPattern, ParseSyntaxError> {
        let raw_regex = get_key(map, "match", |x| x.as_str())?;
        let raw_regex_owned = raw_regex.to_owned();
        let regex_str = Self::parse_regex(raw_regex, state)?;

        let scope = get_key(map, "scope", |x| x.as_str())
            .ok()
            .map(|s| str_to_scopes(s, state.scope_repo))
            .unwrap_or_else(|| Ok(vec![]))?;

        let captures = if let Ok(map) = get_key(map, "captures", |x| x.as_hash()) {
            Some(Self::parse_captures(map, &regex_str, state)?)
        } else {
            None
        };

        let mut has_captures = false;
        let operation = if let Ok(y) = get_key(map, "pop", |y| {
            y.as_i64().or(match y.as_bool() {
                Some(true) => Some(1),
                _ => None,
            })
        }) {
            // Thanks @wbond for letting me know this is the correct way to check for captures
            has_captures = state
                .backref_regex
                .search(&regex_str, 0, regex_str.len(), None, true);
            // In Sublime Text, `pop: N` + `embed:` pops N contexts then pushes
            // the embedded syntax with escape priority.
            if get_key(map, "embed", Some).is_ok() {
                Self::parse_embed_op(map, state, contexts, namer, y as usize)?
            } else if let Ok(b) = get_key(map, "branch", |x| x.as_vec()) {
                let branch_point = get_key(map, "branch_point", |x| x.as_str())?;
                let alternatives: Vec<ContextReference> = b
                    .iter()
                    .map(|item| {
                        SyntaxDefinition::parse_reference(item, state, contexts, namer, false)
                    })
                    .collect::<Result<_, _>>()?;
                MatchOperation::Branch {
                    name: branch_point.to_owned(),
                    alternatives,
                    pop_count: y as usize,
                }
            } else if let Ok(s) = get_key(map, "set", Some) {
                // `pop: N + set: X` pops N contexts then pushes X.
                MatchOperation::Set {
                    ctx_refs: SyntaxDefinition::parse_pushargs(s, state, contexts, namer)?,
                    pop_count: y as usize,
                }
            } else if let Ok(s) = get_key(map, "push", Some) {
                // `pop: N + push: X`, which was read as a bare
                // `Pop(N)` — the push was **silently dropped**, leaving the parser
                // on the outer context instead of the one the rule names. Four
                // rules in the definitions we ship do this: Java's
                // `pop: 2 + push: annotation-parameters-body` and
                // `pop: 1 + push: case-label-expression`, and Python's two
                // `pop: 2 + push:` parameter lists. Upstream's #667.
                MatchOperation::Push {
                    ctx_refs: SyntaxDefinition::parse_pushargs(s, state, contexts, namer)?,
                    pop_count: y as usize,
                }
            } else {
                MatchOperation::Pop(y as usize)
            }
        } else if let Ok(y) = get_key(map, "push", Some) {
            MatchOperation::Push {
                ctx_refs: SyntaxDefinition::parse_pushargs(y, state, contexts, namer)?,
                pop_count: 0,
            }
        } else if let Ok(y) = get_key(map, "set", Some) {
            MatchOperation::Set {
                ctx_refs: SyntaxDefinition::parse_pushargs(y, state, contexts, namer)?,
                pop_count: 1,
            }
        } else if let Ok(y) = get_key(map, "branch", |x| x.as_vec()) {
            let branch_point = get_key(map, "branch_point", |x| x.as_str())?;
            let alternatives: Vec<ContextReference> = y
                .iter()
                .map(|item| SyntaxDefinition::parse_reference(item, state, contexts, namer, false))
                .collect::<Result<_, _>>()?;
            MatchOperation::Branch {
                name: branch_point.to_owned(),
                alternatives,
                pop_count: 0,
            }
        } else if let Ok(y) = get_key(map, "fail", |x| x.as_str()) {
            MatchOperation::Fail(y.to_owned())
        } else if get_key(map, "embed", Some).is_ok() {
            Self::parse_embed_op(map, state, contexts, namer, 0)?
        } else {
            MatchOperation::None
        };

        let with_prototype = if let Ok(v) = get_key(map, "with_prototype", |x| x.as_vec()) {
            // should a with_prototype include the prototype? I don't think so.
            let subname = Self::parse_context(v, state, contexts, namer)?;
            Some(ContextReference::Inline(subname))
        } else {
            None
        };

        let pattern = MatchPattern::new_with_raw(
            has_captures,
            regex_str,
            raw_regex_owned,
            scope,
            captures,
            operation,
            with_prototype,
        );

        Ok(pattern)
    }

    fn parse_embed_op(
        map: &Hash,
        state: &mut ParserState<'_>,
        contexts: &mut HashMap<String, Context>,
        namer: &mut ContextNamer,
        pop_count: usize,
    ) -> Result<MatchOperation, ParseSyntaxError> {
        let y = get_key(map, "embed", Some)?;
        let v = get_key(map, "escape", Some)
            .map_err(|_| ParseSyntaxError::MissingMandatoryKey("escape"))?;

        let escape_raw = v.as_str().ok_or(ParseSyntaxError::TypeMismatch)?;
        let escape_regex_str = Self::parse_regex(escape_raw, state)?;
        let escape_has_captures =
            state
                .backref_regex
                .search(&escape_regex_str, 0, escape_regex_str.len(), None, true);

        let escape_captures = if let Ok(cap_map) = get_key(map, "escape_captures", |x| x.as_hash())
        {
            Some(Self::parse_captures(cap_map, &escape_regex_str, state)?)
        } else {
            None
        };

        let escape_info = EscapeInfo {
            escape_regex: Regex::new(escape_regex_str),
            has_captures: escape_has_captures,
            escape_captures,
            raw_escape_regex_str: Some(escape_raw.to_owned()),
        };

        // Build the wrapper context for embed_scope (meta_content_scope)
        // and the embedded context reference
        let mut embed_contexts = Vec::new();

        // Create wrapper context with embed_scope if present
        let has_embed_scope = get_key(map, "embed_scope", Some).is_ok();
        if has_embed_scope {
            let mut embed_scope_context_yaml = vec![];
            let mut commands = Hash::new();
            commands.insert(
                Yaml::String("meta_include_prototype".to_string()),
                Yaml::Boolean(false),
            );
            embed_scope_context_yaml.push(Yaml::Hash(commands));
            if let Ok(s) = get_key(map, "embed_scope", Some) {
                let mut commands2 = Hash::new();
                commands2.insert(Yaml::String("meta_content_scope".to_string()), s.clone());
                embed_scope_context_yaml.push(Yaml::Hash(commands2));
            }
            // Add a match-all to pass through to next context
            let mut match_map = Hash::new();
            match_map.insert(
                Yaml::String("match".to_string()),
                Yaml::String(String::new()),
            );
            match_map.insert(Yaml::String("pop".to_string()), Yaml::Boolean(true));
            embed_scope_context_yaml.push(Yaml::Hash(match_map));
            let scope_ctx_name =
                SyntaxDefinition::parse_context(&embed_scope_context_yaml, state, contexts, namer)?;
            // In v2, embed_scope replaces the embedded syntax's scope
            if state.version >= 2
                && let Some(ctx) = contexts.get_mut(&scope_ctx_name)
            {
                ctx.embed_scope_replaces = true;
            }
            embed_contexts.push(ContextReference::Inline(scope_ctx_name));
        }

        embed_contexts.push(SyntaxDefinition::parse_reference(
            y, state, contexts, namer, true,
        )?);

        Ok(MatchOperation::Embed {
            contexts: embed_contexts,
            escape: escape_info,
            pop_count,
        })
    }

    fn parse_pushargs(
        y: &Yaml,
        state: &mut ParserState<'_>,
        contexts: &mut HashMap<String, Context>,
        namer: &mut ContextNamer,
    ) -> Result<Vec<ContextReference>, ParseSyntaxError> {
        // check for a push of multiple items
        if y.as_vec().is_some_and(|v| {
            !v.is_empty()
                && (v[0].as_str().is_some()
                    || v[0]
                        .as_vec()
                        .and_then(|v| v.first())
                        .is_some_and(|v| v.as_hash().is_some()))
        }) {
            // this works because Result implements FromIterator to handle the errors
            y.as_vec()
                .unwrap()
                .iter()
                .map(|x| SyntaxDefinition::parse_reference(x, state, contexts, namer, false))
                .collect()
        } else {
            let reference = SyntaxDefinition::parse_reference(y, state, contexts, namer, false)?;
            Ok(vec![reference])
        }
    }

    fn parse_regex(raw_regex: &str, state: &ParserState<'_>) -> Result<String, ParseSyntaxError> {
        let regex = Self::resolve_variables(raw_regex, state);
        let regex = replace_posix_char_classes(regex);
        let regex = if state.lines_include_newline {
            regex_for_newlines(regex)
        } else {
            // If the passed in strings don't include newlines (unlike Sublime) we can't match on
            // them using the original regex. So this tries to rewrite the regex in a way that
            // allows matching against lines without newlines (essentially replacing `\n` with `$`).
            regex_for_no_newlines(regex)
        };
        // Regex compilation is lazy; errors belong to individual rules.
        Ok(regex)
    }

    fn resolve_variables(raw_regex: &str, state: &ParserState<'_>) -> String {
        let mut result = String::new();
        let mut index = 0;
        let mut region = Region::new();
        while state.variable_regex.search(
            raw_regex,
            index,
            raw_regex.len(),
            Some(&mut region),
            true,
        ) {
            let (begin, end) = region.pos(0).unwrap();

            result.push_str(&raw_regex[index..begin]);

            let var_pos = region.pos(1).unwrap();
            let var_name = &raw_regex[var_pos.0..var_pos.1];
            let var_raw = state
                .variables
                .get(var_name)
                .map(String::as_ref)
                .unwrap_or("");
            let var_resolved = Self::resolve_variables(var_raw, state);
            result.push_str(&var_resolved);

            index = end;
        }
        if index < raw_regex.len() {
            result.push_str(&raw_regex[index..]);
        }
        result
    }

    // The captures are kept whole here and narrowed to the
    // groups that consume once, at the end of the build, by
    // `filter_captures_to_consuming_groups`. Narrowing them here reads the
    // regex *before* `extends` has merged the parent's variables in, and a
    // variable this syntax has never heard of resolves to the empty string
    // (`resolve_variables`) — so `escape: '{{script_content_end}}'` in
    // `HTML (Rails)` was measured against `""`, found no capture groups at
    // all, and every one of its `escape_captures` was dropped. The regex is
    // put right afterwards by `re_resolve_all_regexes`; the captures were not,
    // and could not be, because the mapping they were narrowed from was gone.
    #[allow(clippy::unnecessary_wraps)]
    fn parse_captures(
        map: &Hash,
        _regex_str: &str,
        state: &mut ParserState<'_>,
    ) -> Result<CaptureMapping, ParseSyntaxError> {
        let mut captures = Vec::new();
        for (key, value) in map.iter() {
            if let (Some(key_int), Some(val_str)) = (key.as_i64(), value.as_str()) {
                captures.push((key_int as usize, str_to_scopes(val_str, state.scope_repo)?));
            }
        }
        Ok(captures)
    }

    /// Sublime treats the top level context slightly differently from
    /// including the main context from other syntaxes. When main is popped
    /// it is immediately re-added and when it is `set` over the file level
    /// scope remains. This behaviour is emulated through some added contexts
    /// that are the actual top level contexts used in parsing.
    /// See <https://github.com/trishume/syntect/issues/58> for more.
    pub(crate) fn add_initial_contexts(
        contexts: &mut HashMap<String, Context>,
        state: &mut ParserState<'_>,
        top_level_scope: Scope,
    ) {
        let yaml_docs = YamlLoader::load_from_str(START_CONTEXT).unwrap();
        let yaml = &yaml_docs[0];

        let start_yaml: &[Yaml] = yaml["__start"].as_vec().unwrap();
        SyntaxDefinition::parse_context(
            start_yaml,
            state,
            contexts,
            &mut ContextNamer::new("__start"),
        )
        .unwrap();
        if let Some(start) = contexts.get_mut("__start") {
            start.meta_content_scope = vec![top_level_scope];
        }

        let main_yaml: &[Yaml] = yaml["__main"].as_vec().unwrap();
        SyntaxDefinition::parse_context(
            main_yaml,
            state,
            contexts,
            &mut ContextNamer::new("__main"),
        )
        .unwrap();

        let meta_include_prototype = contexts["main"].meta_include_prototype;
        let meta_scope = contexts["main"].meta_scope.clone();
        // Copy `main`'s meta_content_scope to `__main`, but strip the
        // auto-inserted `top_level_scope` at position 0 if present.
        // On a fresh load `main.meta_content_scope` is still pre-insert
        // here, so this is a no-op. On a re-run (from `resolve_extends`
        // after a child inherits its parent's contexts), `main` already
        // carries `top_level_scope` from the first call; copying it to
        // `__main` would make both push the file scope at runtime,
        // producing duplicates like `[source.diff.git, source.diff.git]`
        // for Git Diff.
        let mut meta_content_scope = contexts["main"].meta_content_scope.clone();
        if meta_content_scope.first() == Some(&top_level_scope) {
            meta_content_scope.remove(0);
        }

        if let Some(outer_main) = contexts.get_mut("__main") {
            outer_main.meta_include_prototype = meta_include_prototype;
            outer_main.meta_scope = meta_scope;
            outer_main.meta_content_scope = meta_content_scope;
        }

        // add the top_level_scope as a meta_content_scope to main so
        // pushes from other syntaxes add the file scope.
        // Idempotent so a re-run (from `resolve_extends`) doesn't
        // double-insert — see the comment on the copy above.
        // TODO: this order is not quite correct if main also has a meta_scope
        if let Some(main) = contexts.get_mut("main")
            && main.meta_content_scope.first() != Some(&top_level_scope)
        {
            main.meta_content_scope.insert(0, top_level_scope);
        }
    }
}

/// Re-resolve a raw regex string with the given variables and newline mode.
/// Expands variables and normalizes character classes/newlines; regex machines remain lazy.
pub(crate) fn re_resolve_regex(
    raw: &str,
    variables: &HashMap<String, String>,
    lines_include_newline: bool,
) -> Result<String, ParseSyntaxError> {
    static VARIABLE_REGEX: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}".into()));
    let state = ReResolveState {
        variables,
        variable_regex: &VARIABLE_REGEX,
    };
    let regex = re_resolve_variables(raw, &state);
    let regex = replace_posix_char_classes(regex);
    let regex = if lines_include_newline {
        regex_for_newlines(regex)
    } else {
        regex_for_no_newlines(regex)
    };
    // Regex compilation is lazy.
    Ok(regex)
}

struct ReResolveState<'a> {
    variables: &'a HashMap<String, String>,
    variable_regex: &'a Regex,
}

fn re_resolve_variables(raw_regex: &str, state: &ReResolveState<'_>) -> String {
    let mut result = String::new();
    let mut index = 0;
    let mut region = Region::new();
    while state
        .variable_regex
        .search(raw_regex, index, raw_regex.len(), Some(&mut region), true)
    {
        let (begin, end) = region.pos(0).unwrap();
        result.push_str(&raw_regex[index..begin]);

        let var_pos = region.pos(1).unwrap();
        let var_name = &raw_regex[var_pos.0..var_pos.1];
        let var_raw = state
            .variables
            .get(var_name)
            .map(String::as_ref)
            .unwrap_or("");
        let var_resolved = re_resolve_variables(var_raw, state);
        result.push_str(&var_resolved);

        index = end;
    }
    if index < raw_regex.len() {
        result.push_str(&raw_regex[index..]);
    }
    result
}

/// Drop the captures naming a group that does not consume.
///
/// A scope on a group inside a lookaround is never applied, so it is not worth
/// carrying; that is upstream's reason for narrowing a `captures:` mapping to
/// the consuming groups, and this does the same thing at the one moment it can
/// be right about — after `resolve_extends` has merged the parent's variables
/// and `re_resolve_all_regexes` has rebuilt every regex from them. See
/// `parse_captures` for what doing it earlier costs.
pub(crate) fn filter_captures_to_consuming_groups(syntax: &mut SyntaxDefinition) {
    fn narrow(captures: &mut Option<CaptureMapping>, regex_str: &str) {
        let Some(mapping) = captures else { return };
        let valid = get_consuming_capture_indexes(regex_str);
        mapping.retain(|(index, _)| valid.contains(index));
    }
    for context in syntax.contexts.values_mut() {
        for pattern in &mut context.patterns {
            let Pattern::Match(match_pat) = pattern else {
                continue;
            };
            let regex_str = match_pat.regex.regex_str().to_owned();
            narrow(&mut match_pat.captures, &regex_str);
            if let MatchOperation::Embed { ref mut escape, .. } = match_pat.operation {
                let escape_str = escape.escape_regex.regex_str().to_owned();
                narrow(&mut escape.escape_captures, &escape_str);
            }
        }
    }
}

/// Re-resolve all regexes in a SyntaxDefinition that have a stored raw_regex_str.
/// This is used after merging variables during extends resolution.
pub(crate) fn re_resolve_all_regexes(
    syntax: &mut SyntaxDefinition,
    lines_include_newline: bool,
) -> Result<(), ParseSyntaxError> {
    validate_variables(&syntax.variables)?;
    if let Some(raw) = &syntax.raw_first_line_match {
        syntax.first_line_match = Some(re_resolve_regex(
            raw,
            &syntax.variables,
            lines_include_newline,
        )?);
    }
    for context in syntax.contexts.values_mut() {
        for pattern in &mut context.patterns {
            if let Pattern::Match(match_pat) = pattern {
                if let Some(ref raw) = match_pat.raw_regex_str {
                    let new_regex_str =
                        re_resolve_regex(raw, &syntax.variables, lines_include_newline)?;
                    match_pat.regex = Regex::new(new_regex_str);
                }
                // Also re-resolve the escape regex for embed operations
                if let MatchOperation::Embed { ref mut escape, .. } = match_pat.operation
                    && let Some(ref raw) = escape.raw_escape_regex_str
                {
                    let new_regex_str =
                        re_resolve_regex(raw, &syntax.variables, lines_include_newline)?;
                    escape.escape_regex = Regex::new(new_regex_str);
                }
            }
        }
    }
    Ok(())
}

struct ContextNamer {
    name: String,
    anonymous_index: Option<usize>,
}

impl ContextNamer {
    fn new(name: &str) -> ContextNamer {
        ContextNamer {
            name: name.to_string(),
            anonymous_index: None,
        }
    }

    fn next(&mut self) -> String {
        let name = if let Some(index) = self.anonymous_index {
            format!("#anon_{}_{}", self.name, index)
        } else {
            self.name.clone()
        };

        self.anonymous_index = Some(self.anonymous_index.map(|i| i + 1).unwrap_or(0));
        name
    }
}

/// In fancy-regex, POSIX character classes only match ASCII characters.
///
/// Sublime's syntaxes expect them to match Unicode characters as well, so transform them to
/// corresponding Unicode character classes.
fn replace_posix_char_classes(regex: String) -> String {
    regex
        .replace("[:alpha:]", r"\p{L}")
        .replace("[:alnum:]", r"\p{L}\p{N}")
        .replace("[:lower:]", r"\p{Ll}")
        .replace("[:upper:]", r"\p{Lu}")
        .replace("[:digit:]", r"\p{Nd}")
}

/// Some of the regexes include `$` and expect it to match end of line,
/// e.g. *before* the `\n` in `test\n`.
///
/// In fancy-regex, `$` means end of text by default, so that would
/// match *after* `\n`. Using `(?m:$)` instead means it matches end of line.
///
/// Note that we don't want to add a `(?m)` in the beginning to change the
/// whole regex because that would also change the meaning of `^`. In
/// fancy-regex, that also matches at the end of e.g. `test\n` which is
/// different from onig. It would also change `.` to match more.
fn regex_for_newlines(regex: String) -> String {
    if !regex.contains('$') {
        return regex;
    }

    let rewriter = RegexRewriterForNewlines {
        parser: Parser::new(regex.as_bytes()),
    };
    rewriter.rewrite()
}

struct RegexRewriterForNewlines<'a> {
    parser: Parser<'a>,
}

impl RegexRewriterForNewlines<'_> {
    fn rewrite(mut self) -> String {
        let mut result = Vec::new();

        while let Some(c) = self.parser.peek() {
            match c {
                b'$' => {
                    self.parser.next();
                    result.extend_from_slice(br"(?m:$)");
                }
                b'\\' => {
                    self.parser.next();
                    result.push(c);
                    if let Some(c2) = self.parser.peek() {
                        self.parser.next();
                        result.push(c2);
                    }
                }
                b'[' => {
                    let (mut content, _) = self.parser.parse_character_class();
                    result.append(&mut content);
                }
                _ => {
                    self.parser.next();
                    result.push(c);
                }
            }
        }
        String::from_utf8(result).unwrap()
    }
}

/// Rewrite a regex that matches `\n` to one that matches `$` (end of line) instead.
/// That allows the regex to be used to match lines that don't include a trailing newline character.
///
/// The reason we're doing this is because the regexes in the syntax definitions assume that the
/// lines that are being matched on include a trailing newline.
///
/// Note that the rewrite is just an approximation and there's a couple of cases it can not handle,
/// due to `$` being an anchor whereas `\n` matches a character.
fn regex_for_no_newlines(regex: String) -> String {
    if !regex.contains(r"\n") {
        return regex;
    }

    // A special fix to rewrite a pattern from the `Rd` syntax that the RegexRewriter can not
    // handle properly.
    let regex = regex.replace("(?:\\n)?", "(?:$|)");

    let rewriter = RegexRewriterForNoNewlines {
        parser: Parser::new(regex.as_bytes()),
    };
    rewriter.rewrite()
}

struct RegexRewriterForNoNewlines<'a> {
    parser: Parser<'a>,
}

impl RegexRewriterForNoNewlines<'_> {
    fn rewrite(mut self) -> String {
        let mut result = Vec::new();
        while let Some(c) = self.parser.peek() {
            match c {
                b'\\' => {
                    self.parser.next();
                    if let Some(c2) = self.parser.peek() {
                        self.parser.next();
                        // Replacing `\n` with `$` in `\n?` or `\n+` would make parsing later fail
                        // with "target of repeat operator is invalid"
                        let c3 = self.parser.peek();
                        if c2 == b'n' && c3 != Some(b'?') && c3 != Some(b'+') && c3 != Some(b'*') {
                            result.extend_from_slice(b"$");
                        } else {
                            result.push(c);
                            result.push(c2);
                        }
                    } else {
                        result.push(c);
                    }
                }
                b'[' => {
                    let (mut content, matches_newline) = self.parser.parse_character_class();
                    if matches_newline && self.parser.peek() != Some(b'?') {
                        result.extend_from_slice(b"(?:");
                        result.append(&mut content);
                        result.extend_from_slice(br"|$)");
                    } else {
                        result.append(&mut content);
                    }
                }
                _ => {
                    self.parser.next();
                    result.push(c);
                }
            }
        }
        String::from_utf8(result).unwrap()
    }
}

fn get_consuming_capture_indexes(regex: &str) -> Vec<usize> {
    let parser = ConsumingCaptureIndexParser {
        parser: Parser::new(regex.as_bytes()),
    };
    parser.get_consuming_capture_indexes()
}

struct ConsumingCaptureIndexParser<'a> {
    parser: Parser<'a>,
}

impl ConsumingCaptureIndexParser<'_> {
    /// Find capture groups which are not inside lookarounds.
    ///
    /// If, in a YAML syntax definition, a scope stack is applied to a capture group inside a
    /// lookaround, (i.e. "captures:\n x: scope.stack goes.here", where "x" is the number of a
    /// capture group in a lookahead/behind), those those scopes are not applied, so no need to
    /// even parse them.
    fn get_consuming_capture_indexes(mut self) -> Vec<usize> {
        let mut result = Vec::new();
        let mut stack = Vec::new();
        let mut cap_num = 0;
        let mut in_lookaround = false;
        stack.push(in_lookaround);
        result.push(cap_num);

        while let Some(c) = self.parser.peek() {
            match c {
                b'\\' => {
                    self.parser.next();
                    self.parser.next();
                }
                b'[' => {
                    self.parser.parse_character_class();
                }
                b'(' => {
                    self.parser.next();
                    // add the current lookaround state to the stack so we can just pop at a closing paren
                    stack.push(in_lookaround);
                    if let Some(c2) = self.parser.peek() {
                        if c2 != b'?' {
                            // simple numbered capture group
                            cap_num += 1;
                            // if we are not currently in a lookaround,
                            // add this capture group number to the valid ones
                            if !in_lookaround {
                                result.push(cap_num);
                            }
                        } else {
                            self.parser.next();
                            if let Some(c3) = self.parser.peek() {
                                self.parser.next();
                                if c3 == b'=' || c3 == b'!' {
                                    // lookahead
                                    in_lookaround = true;
                                } else if c3 == b'<' {
                                    if let Some(c4) = self.parser.peek()
                                        && (c4 == b'=' || c4 == b'!')
                                    {
                                        self.parser.next();
                                        // lookbehind
                                        in_lookaround = true;
                                    }
                                } else if c3 == b'P'
                                    && let Some(c4) = self.parser.peek()
                                    && c4 == b'<'
                                {
                                    // named capture group
                                    cap_num += 1;
                                    // if we are not currently in a lookaround,
                                    // add this capture group number to the valid ones
                                    if !in_lookaround {
                                        result.push(cap_num);
                                    }
                                }
                            }
                        }
                    }
                }
                b')' => {
                    if let Some(value) = stack.pop() {
                        in_lookaround = value;
                    }
                    self.parser.next();
                }
                _ => {
                    self.parser.next();
                }
            }
        }
        result
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl Parser<'_> {
    fn new(bytes: &[u8]) -> Parser<'_> {
        Parser { bytes, index: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn next(&mut self) {
        self.index += 1;
    }

    fn parse_character_class(&mut self) -> (Vec<u8>, bool) {
        let mut content = Vec::new();
        let mut negated = false;
        let mut nesting = 0;
        let mut matches_newline = false;

        self.next();
        content.push(b'[');
        if let Some(b'^') = self.peek() {
            self.next();
            content.push(b'^');
            negated = true;
        }

        // An unescaped `]` is allowed after `[` or `[^` and doesn't mean the end of the class.
        if let Some(b']') = self.peek() {
            self.next();
            content.push(b']');
        }

        while let Some(c) = self.peek() {
            match c {
                b'\\' => {
                    self.next();
                    content.push(c);
                    if let Some(c2) = self.peek() {
                        self.next();
                        if c2 == b'n' && !negated && nesting == 0 {
                            matches_newline = true;
                        }
                        content.push(c2);
                    }
                }
                b'[' => {
                    self.next();
                    content.push(b'[');
                    nesting += 1;
                }
                b']' => {
                    self.next();
                    content.push(b']');
                    if nesting == 0 {
                        break;
                    }
                    nesting -= 1;
                }
                _ => {
                    self.next();
                    content.push(c);
                }
            }
        }

        (content, matches_newline)
    }
}

fn validate_variables(variables: &HashMap<String, String>) -> Result<(), ParseSyntaxError> {
    static REFS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}").unwrap());
    fn visit(
        name: &str,
        vars: &HashMap<String, String>,
        marks: &mut HashMap<String, Option<usize>>,
        depth: usize,
    ) -> Result<usize, ParseSyntaxError> {
        if depth > 128 {
            return Err(ParseSyntaxError::VariableExpansionLimit);
        }
        if let Some(mark) = marks.get(name) {
            return mark.ok_or_else(|| ParseSyntaxError::VariableCycle(name.into()));
        }
        let Some(raw) = vars.get(name) else {
            return Ok(0);
        };
        marks.insert(name.into(), None);
        let mut length = raw.len();
        for captures in REFS.captures_iter(raw) {
            length = length
                .saturating_sub(captures[0].len())
                .saturating_add(visit(&captures[1], vars, marks, depth + 1)?);
            if length > 1024 * 1024 {
                return Err(ParseSyntaxError::VariableExpansionLimit);
            }
        }
        marks.insert(name.into(), Some(length));
        Ok(length)
    }
    let mut marks = HashMap::new();
    for name in variables.keys() {
        visit(name, variables, &mut marks, 0)?;
    }
    Ok(())
}

/// Normalize legacy scalar forms and discard only mistyped optional keys/rules.
/// Diagnostics are retained on the definition, then attributed to its package by the linker.
fn normalize_authoring_types(doc: &mut Yaml) -> Vec<String> {
    fn mapping(map: &mut Hash, path: &str, kind: &str, warnings: &mut Vec<String>) {
        for (key, mut value) in std::mem::take(map) {
            let Some(name) = key.as_str() else {
                warnings.push(format!("{path}: ignored non-string key"));
                continue;
            };
            // YAML empty scopes mean no scopes; quoted capture indices and numeric
            // extensions are compatible scalar spellings, not discarded authoring data.
            if name == "scope" && value == Yaml::Null {
                value = Yaml::String(String::new());
            }
            if kind == "top"
                && matches!(name, "file_extensions" | "hidden_file_extensions")
                && let Yaml::Array(items) = &mut value
            {
                items.retain_mut(|item| {
                    if let Yaml::Integer(n) = item {
                        *item = Yaml::String(n.to_string());
                    }
                    if item.as_str().is_some() {
                        true
                    } else {
                        warnings.push(format!("{path}.{name}: ignored invalid extension"));
                        false
                    }
                });
            }
            if kind == "rule"
                && matches!(name, "captures" | "escape_captures")
                && let Yaml::Hash(captures) = &mut value
            {
                for (index, scope) in std::mem::take(captures) {
                    let number = index.as_i64().or_else(|| index.as_str()?.parse().ok());
                    if let Some(number) = number.filter(|n| *n >= 0)
                        && scope.as_str().is_some()
                    {
                        captures.insert(Yaml::Integer(number), scope);
                    } else {
                        warnings.push(format!("{path}.{name}: ignored invalid capture {index:?}"));
                    }
                }
            }
            let valid = match (kind, name) {
                ("top", "name" | "scope" | "first_line_match") => value.as_str().is_some(),
                ("top", "version") => matches!(value.as_i64(), Some(1 | 2)),
                ("top", "hidden") => value.as_bool().is_some(),
                ("top", "variables" | "contexts") => value.as_hash().is_some(),
                ("top", "file_extensions" | "hidden_file_extensions") => value.as_vec().is_some(),
                ("top", "extends") => {
                    value.as_str().is_some()
                        || value
                            .as_vec()
                            .is_some_and(|v| v.iter().all(|x| x.as_str().is_some()))
                }
                ("variables", _) => value.as_str().is_some(),
                (
                    "rule",
                    "match" | "scope" | "meta_scope" | "meta_content_scope" | "embed_scope"
                    | "escape" | "branch_point" | "fail" | "include" | "embed",
                ) => value.as_str().is_some(),
                (
                    "rule",
                    "meta_include_prototype" | "meta_prepend" | "meta_append" | "apply_prototype",
                ) => value.as_bool().is_some(),
                ("rule", "pop" | "clear_scopes") => {
                    value.as_bool().is_some() || value.as_i64().is_some_and(|n| n >= 0)
                }
                ("rule", "captures" | "escape_captures") => value.as_hash().is_some(),
                ("rule", "branch" | "with_prototype") => value.as_vec().is_some(),
                ("rule", "push" | "set") => value.as_str().is_some() || value.as_vec().is_some(),
                _ => true,
            };
            if valid {
                map.insert(key, value);
            } else {
                warnings.push(format!("{path}.{name}: ignored wrong value type or range"));
            }
        }
    }
    fn context(value: &mut Yaml, path: &str, depth: usize, warnings: &mut Vec<String>) {
        if depth > 256 {
            return;
        } // The parser reports its bounded nesting error.
        let Yaml::Array(rules) = value else {
            warnings.push(format!("{path}: ignored non-array context"));
            *value = Yaml::Array(Vec::new());
            return;
        };
        rules.retain_mut(|value| {
            let Yaml::Hash(map) = value else {
                warnings.push(format!("{path}: ignored non-mapping rule"));
                return false;
            };
            mapping(map, path, "rule", warnings);
            for name in ["push", "set", "with_prototype", "branch"] {
                if let Some(value) = map.get_mut(&Yaml::String(name.into()))
                    && let Yaml::Array(items) = value
                {
                    if items.first().is_some_and(|x| x.as_hash().is_some())
                        || name == "with_prototype"
                    {
                        context(value, &format!("{path}.{name}"), depth + 1, warnings);
                    } else {
                        for item in items {
                            if item.as_vec().is_some() {
                                context(item, &format!("{path}.{name}"), depth + 1, warnings);
                            }
                        }
                    }
                }
            }
            // A dropped match/include cannot turn a malformed rule into a fatal file error.
            map.keys().any(|key| {
                matches!(
                    key.as_str(),
                    Some(
                        "match"
                            | "include"
                            | "meta_scope"
                            | "meta_content_scope"
                            | "meta_include_prototype"
                            | "meta_prepend"
                            | "meta_append"
                            | "clear_scopes"
                    )
                )
            })
        });
    }
    let mut warnings = Vec::new();
    if let Yaml::Hash(top) = doc {
        mapping(top, "syntax", "top", &mut warnings);
        if let Some(Yaml::Hash(vars)) = top.get_mut(&Yaml::String("variables".into())) {
            mapping(vars, "variables", "variables", &mut warnings);
        }
        if let Some(Yaml::Hash(contexts)) = top.get_mut(&Yaml::String("contexts".into())) {
            for (name, value) in contexts {
                context(
                    value,
                    &format!("contexts.{}", name.as_str().unwrap_or("<invalid name>")),
                    0,
                    &mut warnings,
                );
            }
        }
    }
    warnings
}

/// Inspect final inherited regex text without building a regex machine. Oniguruma-only
/// features are left to its lazy compiler; only unambiguous structural errors are diagnosed.
pub(crate) fn regex_authoring_warnings(syntax: &SyntaxDefinition) -> Vec<String> {
    fn inspect(raw: &str, vars: &HashMap<String, String>, path: &str, warnings: &mut Vec<String>) {
        static VARIABLES: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}").unwrap());
        for capture in VARIABLES.captures_iter(raw) {
            if !vars.contains_key(&capture[1]) {
                warnings.push(format!("{path}: unknown variable {{{{{}}}}}", &capture[1]));
            }
        }
        let Ok(expanded) = re_resolve_regex(raw, vars, true) else {
            return;
        };
        if let Some(error) = regex_structure_error(&expanded) {
            warnings.push(format!("{path}: invalid regex {raw:?}: {error}"));
        }
    }
    let mut warnings = Vec::new();
    for (name, value) in &syntax.variables {
        // Variables are regex fragments, and need not balance until assembled in a rule.
        static VARIABLES: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}").unwrap());
        for capture in VARIABLES.captures_iter(value) {
            if !syntax.variables.contains_key(&capture[1]) {
                warnings.push(format!(
                    "variables.{name}: unknown variable {{{{{}}}}}",
                    &capture[1]
                ));
            }
        }
    }
    if let Some(raw) = &syntax.raw_first_line_match {
        inspect(raw, &syntax.variables, "first_line_match", &mut warnings);
    }
    for (name, context) in &syntax.contexts {
        for (index, pattern) in context.patterns.iter().enumerate() {
            if let Pattern::Match(pattern) = pattern {
                let path = format!("contexts.{name}[{index}]");
                if let Some(raw) = &pattern.raw_regex_str {
                    inspect(raw, &syntax.variables, &path, &mut warnings);
                }
                if let MatchOperation::Embed { escape, .. } = &pattern.operation
                    && let Some(raw) = &escape.raw_escape_regex_str
                {
                    inspect(
                        raw,
                        &syntax.variables,
                        &format!("{path}.escape"),
                        &mut warnings,
                    );
                }
            }
        }
    }
    warnings.sort();
    warnings.dedup();
    warnings
}

/// Structural validation for the Oniguruma dialect. In particular, literal `{` is
/// legal and x-mode whitespace/comments differ from Rust regex syntax. This scans
/// delimiters only; it does not compile machines or reject dialect-specific escapes.
fn regex_structure_error(regex: &str) -> Option<&'static str> {
    // The AST parser is machine-free too. Diagnose only errors shared with Onig;
    // e.g. Rust rejects a literal `{`, Onig accepts it, so that is not an error here.
    if let Err(error) = regex_syntax::ast::parse::Parser::new().parse(regex) {
        use regex_syntax::ast::ErrorKind;
        match error.kind() {
            ErrorKind::ClassRangeInvalid => return Some("reversed character-class range"),
            ErrorKind::RepetitionCountInvalid => return Some("reversed repetition range"),
            ErrorKind::RepetitionMissing
                if matches!(
                    regex.as_bytes().get(error.span().start.offset),
                    Some(b'*' | b'+' | b'?')
                ) =>
            {
                return Some("repetition without a target");
            }
            _ => {}
        }
    }
    let bytes = regex.as_bytes();
    let mut groups = Vec::new();
    let mut extended = false;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => {
                at += 1;
                if at == bytes.len() {
                    return Some("unfinished escape");
                }
            }
            b'#' if extended => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
                continue;
            }
            b'[' => {
                let mut depth = 1usize;
                at += 1;
                if bytes.get(at) == Some(&b'^') {
                    at += 1;
                }
                if bytes.get(at) == Some(&b']') {
                    at += 1;
                }
                while at < bytes.len() && depth > 0 {
                    match bytes[at] {
                        b'\\' => {
                            at += 1;
                        }
                        b'[' if bytes.get(at + 1) != Some(&b']') => {
                            depth += 1;
                        }
                        b']' => {
                            depth -= 1;
                        }
                        _ => {}
                    }
                    at += 1;
                }
                if depth > 0 {
                    return Some("unclosed character class");
                }
                continue;
            }
            b'(' if bytes.get(at + 1) == Some(&b'?') && bytes.get(at + 2) == Some(&b'#') => {
                at += 3;
                while at < bytes.len() && bytes[at] != b')' {
                    if bytes[at] == b'\\' {
                        at += 1;
                    }
                    at += 1;
                }
                if at == bytes.len() {
                    return Some("unclosed comment group");
                }
            }
            b'(' => {
                let old = extended;
                if bytes.get(at + 1) == Some(&b'?') {
                    let mut flag = at + 2;
                    let mut enable = true;
                    while let Some(byte) = bytes.get(flag) {
                        match byte {
                            b'-' => enable = false,
                            b'x' => extended = enable,
                            b'i' | b'm' | b's' | b'a' | b'u' | b'D' | b'S' | b'W' | b'P' | b'y' => {
                            }
                            _ => break,
                        }
                        flag += 1;
                    }
                    if flag > at + 2 && bytes.get(flag) == Some(&b')') {
                        at = flag + 1;
                        continue;
                    }
                }
                groups.push(old);
            }
            b')' => {
                if let Some(old) = groups.pop() {
                    extended = old;
                } else {
                    return Some("unopened group");
                }
            }
            _ => {}
        }
        at += 1;
    }
    if groups.is_empty() {
        None
    } else {
        Some("unclosed group")
    }
}
