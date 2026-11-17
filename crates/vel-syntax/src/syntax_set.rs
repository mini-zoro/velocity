// Adapted from syntect PR #705 (MIT); see README.md and LICENSE-syntect.
use super::ParsingError;
use super::syntax_definition::*;
use vel_scope::*;

use crate::LoadingError;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::ops::DerefMut;
use std::path::Path;

use crate::syntax_definition::ContextId;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};
use vel_regex::Regex;
use vel_regex::{MatcherBackend, MultiDfa, PatternSet, PatternSpec, Region};

fn default_backend() -> Arc<dyn MatcherBackend> {
    Arc::new(MultiDfa::default())
}
#[derive(Debug)]
pub(crate) struct ContextPatterns {
    pub rules: Vec<(ContextId, usize)>,
    pub matcher: PatternSet,
}
type ContextMatcherCache =
    Mutex<HashMap<(ContextId, Option<(Region, String)>, Option<ContextId>), Arc<ContextPatterns>>>;

/// A syntax set holds multiple syntaxes that have been linked together.
///
/// Use a [`SyntaxSetBuilder`] to load syntax definitions and build a syntax set.
///
/// After building, the syntax set is immutable and can no longer be modified, but you can convert
/// it back into a builder by using the [`into_builder`] method.
///
/// [`SyntaxSetBuilder`]: struct.SyntaxSetBuilder.html
/// [`into_builder`]: #method.into_builder
#[derive(Debug, Serialize, Deserialize)]
pub struct SyntaxSet {
    #[serde(skip, default = "default_backend")]
    matcher_backend: Arc<dyn MatcherBackend>,
    #[serde(skip)]
    pattern_contexts: ContextMatcherCache,
    syntaxes: Vec<SyntaxReference>,
    /// Stores the syntax index for every path that was loaded
    path_syntaxes: Vec<(String, usize)>,
    #[serde(skip)]
    physical_paths: HashMap<String, usize>,

    /// Warnings collected during syntax loading and linking.
    warnings: Vec<String>,

    #[serde(skip_serializing, skip_deserializing, default = "OnceLock::new")]
    first_line_cache: OnceLock<FirstLineCache>,
}

/// A linked version of a [`SyntaxDefinition`] that is only useful as part of the
/// [`SyntaxSet`] that contains it. See docs for [`SyntaxSetBuilder::build`] for
/// more info.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyntaxReference {
    /// Definition source path, used to disambiguate package references.
    pub path: String,
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
    /// The version of the sublime-syntax format (1 or 2). Default is 1.
    #[serde(default = "default_syntax_version")]
    pub version: u32,
    #[serde(skip)]
    pub(crate) lazy_contexts: OnceLock<LazyContexts>,
    #[serde(with = "serde_bytes")]
    pub(crate) serialized_lazy_contexts: Vec<u8>,
}

fn default_syntax_version() -> u32 {
    1
}

/// The lazy-loaded parts of a [`SyntaxReference`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LazyContexts {
    #[serde(serialize_with = "ordered_map")]
    pub(crate) context_ids: HashMap<String, ContextId>,
    pub(crate) contexts: Vec<Context>,
}

/// A syntax set builder is used for loading syntax definitions from the file
/// system or by adding [`SyntaxDefinition`] objects.
///
/// Once all the syntaxes have been added, call [`build`] to turn the builder into
/// a [`SyntaxSet`] that can be used for parsing or highlighting.
///
/// [`SyntaxDefinition`]: syntax_definition/struct.SyntaxDefinition.html
/// [`build`]: #method.build
/// [`SyntaxSet`]: struct.SyntaxSet.html
#[derive(Debug, Clone, Default)]
pub struct SyntaxSetBuilder {
    syntaxes: Vec<SyntaxDefinition>,
    path_syntaxes: Vec<(String, usize)>,
    warnings: Vec<String>,
    /// Tracks the `lines_include_newline` flag from the most recent
    /// `add_from_folder` call. Used by `resolve_extends` to re-resolve
    /// regexes with the correct newline mode after merging parent variables.
    lines_include_newline: bool,
}

fn load_syntax_file(
    p: &Path,
    lines_include_newline: bool,
) -> Result<SyntaxDefinition, LoadingError> {
    let s = std::fs::read_to_string(p)?;
    if crate::tm_language::is_tm_language(p) {
        return SyntaxDefinition::load_tm_language(&s);
    }

    SyntaxDefinition::load_from_str(
        &s,
        lines_include_newline,
        p.file_stem().and_then(|x| x.to_str()),
    )
    .map_err(|e| LoadingError::ParseSyntax(e, format!("{}", p.display())))
}

impl Clone for SyntaxSet {
    fn clone(&self) -> SyntaxSet {
        SyntaxSet {
            matcher_backend: self.matcher_backend.clone(),
            pattern_contexts: Mutex::new(HashMap::new()),
            syntaxes: self.syntaxes.clone(),
            path_syntaxes: self.path_syntaxes.clone(),
            physical_paths: self.physical_paths.clone(),
            warnings: self.warnings.clone(),
            // Will need to be re-initialized
            first_line_cache: OnceLock::new(),
        }
    }
}

impl Default for SyntaxSet {
    fn default() -> Self {
        SyntaxSet {
            matcher_backend: default_backend(),
            pattern_contexts: Mutex::new(HashMap::new()),
            syntaxes: Vec::new(),
            path_syntaxes: Vec::new(),
            physical_paths: HashMap::new(),
            warnings: Vec::new(),
            first_line_cache: OnceLock::new(),
        }
    }
}

impl SyntaxSet {
    pub(crate) fn bind_resource_paths<'a>(
        &mut self,
        resources: impl Iterator<Item = (&'a str, &'a str)>,
    ) {
        self.physical_paths.clear();
        for (virtual_path, physical) in resources {
            if let Some((_, index)) = self
                .path_syntaxes
                .iter()
                .find(|(path, _)| path == virtual_path)
            {
                self.physical_paths.insert(physical.into(), *index);
            }
        }
    }
    /// Creates an empty syntax collection.
    pub fn new() -> SyntaxSet {
        SyntaxSet::default()
    }

    /// Convenience constructor for creating a builder, then loading syntax
    /// definitions from a folder and then building the syntax set.
    ///
    /// Note that this uses `lines_include_newline` set to `false`, see the
    /// [`add_from_folder`] method docs on [`SyntaxSetBuilder`] for an explanation
    /// as to why this might not be the best.
    ///
    /// [`add_from_folder`]: struct.SyntaxSetBuilder.html#method.add_from_folder
    /// [`SyntaxSetBuilder`]: struct.SyntaxSetBuilder.html
    pub fn load_from_folder<P: AsRef<Path>>(folder: P) -> Result<SyntaxSet, LoadingError> {
        let mut builder = SyntaxSetBuilder::new();
        builder.add_from_folder(folder, false)?;
        Ok(builder.build())
    }

    /// Warnings collected during syntax loading and linking.
    ///
    /// These include issues like skipped files, version mismatches, and
    /// unresolved `extends` references.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The list of syntaxes in the set
    pub fn syntaxes(&self) -> &[SyntaxReference] {
        &self.syntaxes[..]
    }

    /// Finds a syntax by its default scope, for example `source.regexp` finds the regex syntax.
    ///
    /// This and all similar methods below do a linear search of syntaxes, this should be fast
    /// because there aren't many syntaxes, but don't think you can call it a bajillion times per
    /// second.
    pub fn find_syntax_by_scope(&self, scope: Scope) -> Option<&SyntaxReference> {
        self.syntaxes.iter().rev().find(|&s| s.scope == scope)
    }

    /// Finds the last registered syntax with this exact display name.
    pub fn find_syntax_by_name<'a>(&'a self, name: &str) -> Option<&'a SyntaxReference> {
        self.syntaxes.iter().rev().find(|&s| name == s.name)
    }

    /// Finds the last registered syntax accepting this case-insensitive extension.
    pub fn find_syntax_by_extension<'a>(&'a self, extension: &str) -> Option<&'a SyntaxReference> {
        self.syntaxes.iter().rev().find(|&s| {
            s.file_extensions
                .iter()
                .any(|e| e.eq_ignore_ascii_case(extension))
        })
    }

    /// Searches for a syntax first by extension and then by case-insensitive name
    ///
    /// This is useful for things like Github-flavoured-markdown code block highlighting where all
    /// you have to go on is a short token given by the user
    pub fn find_syntax_by_token<'a>(&'a self, s: &str) -> Option<&'a SyntaxReference> {
        {
            let ext_res = self.find_syntax_by_extension(s);
            if ext_res.is_some() {
                return ext_res;
            }
        }
        self.syntaxes
            .iter()
            .rev()
            .find(|&syntax| syntax.name.eq_ignore_ascii_case(s))
    }

    /// Try to find the syntax for a file based on its first line
    ///
    /// This uses regexes that come with some sublime syntax grammars for matching things like
    /// shebangs and mode lines like `-*- Mode: C -*-`
    pub fn find_syntax_by_first_line<'a>(&'a self, s: &str) -> Option<&'a SyntaxReference> {
        let s = s.strip_prefix("\u{feff}").unwrap_or(s); // Strip UTF-8 BOM
        let cache = self.first_line_cache();
        for &(ref reg, i) in cache.regexes.iter().rev() {
            if reg.search(s, 0, s.len(), None, true) {
                return Some(&self.syntaxes[i]);
            }
        }
        None
    }

    /// Searches for a syntax by it's original file path when it was first loaded from disk
    ///
    /// This is primarily useful for syntax tests. Some may specify a
    /// `Packages/PackageName/SyntaxName.sublime-syntax` path, and others may just have
    /// `SyntaxName.sublime-syntax`. This caters for these by matching the end of the path of the
    /// loaded syntax definition files
    // however, if a syntax name is provided without a folder, make sure we don't accidentally match the end of a different syntax definition's name - by checking a / comes before it or it is the full path
    pub fn find_syntax_by_path<'a>(&'a self, path: &str) -> Option<&'a SyntaxReference> {
        if let Some(&index) = self.physical_paths.get(path) {
            return self.syntaxes.get(index);
        }
        let path = path.strip_prefix("Packages/").unwrap_or(path);
        let mut slash_path = "/".to_string();
        slash_path.push_str(path);
        self.path_syntaxes
            .iter()
            .rev()
            .find(|t| t.0.ends_with(&slash_path) || t.0 == path)
            .map(|&(_, i)| &self.syntaxes[i])
    }

    /// Convenience method that tries to find the syntax for a file path, first by extension/name
    /// and then by first line of the file if that doesn't work.
    ///
    /// May IO Error because it sometimes tries to read the first line of the file.
    ///
    /// # Examples
    ///
    /// When determining how to highlight a file, use this in combination with a fallback to plain
    /// text:
    ///
    /// ```
    /// use vel_syntax::{SyntaxDefinition, SyntaxSetBuilder};
    /// let mut builder = SyntaxSetBuilder::new();
    /// builder.add_plain_text_syntax();
    /// builder.add(SyntaxDefinition::load_from_str(
    ///     "name: Fixture\nfile_extensions: [fixture]\nscope: source.fixture\ncontexts: {main: []}",
    ///     true, None).unwrap());
    /// let set = builder.build();
    /// let syntax = set.find_syntax_for_file("sample.fixture").unwrap()
    ///     .unwrap_or_else(|| set.find_syntax_plain_text());
    /// assert_eq!(syntax.name, "Fixture");
    /// ```
    pub fn find_syntax_for_file<P: AsRef<Path>>(
        &self,
        path_obj: P,
    ) -> io::Result<Option<&SyntaxReference>> {
        let path: &Path = path_obj.as_ref();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let extension = path.extension().and_then(|x| x.to_str()).unwrap_or("");
        let ext_syntax = self
            .find_syntax_by_extension(file_name)
            .or_else(|| self.find_syntax_by_extension(extension));
        let line_syntax = if ext_syntax.is_none() {
            let mut line = String::new();
            let f = File::open(path)?;
            let mut line_reader = BufReader::new(&f);
            line_reader.read_line(&mut line)?;
            self.find_syntax_by_first_line(&line)
        } else {
            None
        };
        let syntax = ext_syntax.or(line_syntax);
        Ok(syntax)
    }

    /// Finds a syntax for plain text, which usually has no highlighting rules.
    ///
    /// This is good as a fallback when you can't find another syntax but you still want to use the
    /// same highlighting pipeline code.
    ///
    /// This syntax should always be present, if not this method will panic. If the way you load
    /// syntaxes doesn't create one, use [`add_plain_text_syntax`].
    ///
    /// # Examples
    /// ```
    /// use vel_syntax::SyntaxSetBuilder;
    /// let mut builder = SyntaxSetBuilder::new();
    /// builder.add_plain_text_syntax();
    /// let ss = builder.build();
    /// let syntax = ss.find_syntax_by_token("rs").unwrap_or_else(|| ss.find_syntax_plain_text());
    /// assert_eq!(syntax.name, "Plain Text");
    /// ```
    ///
    /// [`add_plain_text_syntax`]: struct.SyntaxSetBuilder.html#method.add_plain_text_syntax
    pub fn find_syntax_plain_text(&self) -> &SyntaxReference {
        self.find_syntax_by_name("Plain Text")
            .expect("All syntax sets ought to have a plain text syntax")
    }

    /// Converts this syntax set into a builder so that more syntaxes can be
    /// added to it.
    ///
    /// Note that newly added syntaxes can have references to existing syntaxes
    /// in the set, but not the other way around.
    pub fn into_builder(self) -> SyntaxSetBuilder {
        let SyntaxSet {
            syntaxes,
            path_syntaxes,
            ..
        } = self;

        let mut context_map = HashMap::new();
        for (syntax_index, syntax) in syntaxes.iter().enumerate() {
            for (context_index, context) in syntax.contexts().iter().enumerate() {
                context_map.insert(
                    ContextId {
                        syntax_index,
                        context_index,
                    },
                    context.clone(),
                );
            }
        }

        let mut builder_syntaxes = Vec::with_capacity(syntaxes.len());

        for syntax in syntaxes {
            let SyntaxReference {
                name,
                file_extensions,
                scope,
                first_line_match,
                hidden,
                variables,
                version,
                serialized_lazy_contexts,
                ..
            } = syntax;

            let lazy_contexts = LazyContexts::deserialize(&serialized_lazy_contexts[..]);
            let mut builder_contexts = HashMap::with_capacity(lazy_contexts.context_ids.len());
            for (name, context_id) in lazy_contexts.context_ids {
                if let Some(context) = context_map.remove(&context_id) {
                    builder_contexts.insert(name, context);
                }
            }

            let syntax_definition = SyntaxDefinition {
                authoring_warnings: Vec::new(),
                raw_first_line_match: None,
                name,
                file_extensions,
                scope,
                first_line_match,
                hidden,
                variables,
                contexts: builder_contexts,
                extends: vec![],
                version,
            };
            builder_syntaxes.push(syntax_definition);
        }

        SyntaxSetBuilder {
            syntaxes: builder_syntaxes,
            path_syntaxes,
            warnings: Vec::new(),
            lines_include_newline: false,
        }
    }

    #[inline(always)]
    pub(crate) fn get_context(&self, context_id: &ContextId) -> Result<&Context, ParsingError> {
        let syntax = &self
            .syntaxes
            .get(context_id.syntax_index)
            .ok_or(ParsingError::MissingContext(*context_id))?;
        syntax
            .contexts()
            .get(context_id.context_index)
            .ok_or(ParsingError::MissingContext(*context_id))
    }

    fn first_line_cache(&self) -> &FirstLineCache {
        self.first_line_cache
            .get_or_init(|| FirstLineCache::new(self.syntaxes()))
    }

    /// Explicitly traverses contexts and reports unresolved references; materializes lazy syntax bodies.
    pub fn find_unlinked_contexts(&self) -> BTreeSet<String> {
        let SyntaxSet { syntaxes, .. } = self;

        let mut unlinked_contexts = BTreeSet::new();

        for syntax in syntaxes {
            let SyntaxReference { name, scope, .. } = syntax;

            for context in syntax.contexts() {
                self.find_unlinked_contexts_in_context(
                    name,
                    scope,
                    context,
                    &mut unlinked_contexts,
                );
            }
        }
        unlinked_contexts
    }

    fn find_unlinked_contexts_in_context(
        &self,
        name: &str,
        scope: &Scope,
        context: &Context,
        unlinked_contexts: &mut BTreeSet<String>,
    ) {
        for pattern in context.patterns.iter() {
            let maybe_refs_to_check = match pattern {
                Pattern::Match(match_pat) => match &match_pat.operation {
                    MatchOperation::Push {
                        ctx_refs: context_refs,
                        ..
                    }
                    | MatchOperation::Set {
                        ctx_refs: context_refs,
                        ..
                    }
                    | MatchOperation::Branch {
                        alternatives: context_refs,
                        ..
                    } => Some(context_refs),
                    MatchOperation::Embed { contexts, .. } => Some(contexts),
                    MatchOperation::Pop(_) | MatchOperation::None | MatchOperation::Fail(_) => None,
                },
                _ => None,
            };
            for context_ref in maybe_refs_to_check.into_iter().flatten() {
                if self.resolve_reference(context_ref).is_err() {
                    unlinked_contexts.insert(format!(
                        "Syntax '{}' with scope '{}' has unresolved context reference {:?}",
                        name, scope, context_ref
                    ));
                }
            }
        }
    }
}

impl SyntaxReference {
    /// Whether this syntax's contexts have been materialized from the parsed cache.
    pub fn contexts_are_loaded(&self) -> bool {
        self.lazy_contexts.get().is_some()
    }
    pub(crate) fn context_ids(&self) -> &HashMap<String, ContextId> {
        &self.lazy_contexts().context_ids
    }

    fn contexts(&self) -> &[Context] {
        &self.lazy_contexts().contexts
    }

    fn lazy_contexts(&self) -> &LazyContexts {
        self.lazy_contexts
            .get_or_init(|| LazyContexts::deserialize(&self.serialized_lazy_contexts[..]))
    }
}

impl LazyContexts {
    fn deserialize(data: &[u8]) -> LazyContexts {
        crate::dumps::from_reader(data).expect("data is not corrupt or out of sync with the code")
    }
}

impl SyntaxSetBuilder {
    /// Creates an empty package-definition builder.
    pub fn new() -> SyntaxSetBuilder {
        SyntaxSetBuilder::default()
    }

    /// Add a syntax to the set.
    /// Adds a parsed syntax associated with a package path.
    pub fn add_with_path(&mut self, syntax: SyntaxDefinition, path: impl Into<String>) {
        self.path_syntaxes.push((path.into(), self.syntaxes.len()));
        self.syntaxes.push(syntax);
        self.lines_include_newline = true;
    }

    pub(crate) fn add_resource(&mut self, path: &str, bytes: &[u8]) {
        let source = std::str::from_utf8(bytes).map_err(std::io::Error::other);
        let syntax = source.map_err(LoadingError::from).and_then(|source| {
            if crate::tm_language::is_tm_language(Path::new(path)) {
                SyntaxDefinition::load_tm_language(source)
            } else {
                SyntaxDefinition::load_from_str(
                    source,
                    true,
                    Path::new(path).file_stem().and_then(|s| s.to_str()),
                )
                .map_err(|e| LoadingError::ParseSyntax(e, path.into()))
            }
        });
        match syntax {
            Ok(syntax) => self.add_with_path(syntax, path),
            Err(e) => self.warnings.push(format!("skipping {path:?}: {e}")),
        }
    }

    /// Adds an in-memory definition; use add_with_path when package references are needed.
    pub fn add(&mut self, syntax: SyntaxDefinition) {
        self.syntaxes.push(syntax);
    }

    /// The list of syntaxes added so far.
    pub fn syntaxes(&self) -> &[SyntaxDefinition] {
        &self.syntaxes[..]
    }

    /// Warnings collected during syntax loading and linking.
    ///
    /// These include issues like skipped files, version mismatches, and
    /// unresolved `extends` references that were previously printed to stderr.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// A rarely useful method that loads in a syntax with no highlighting rules for plain text
    ///
    /// Exists mainly for adding the plain text syntax to syntax set dumps, because for some reason
    /// the default Sublime plain text syntax is still in `.tmLanguage` format.
    pub fn add_plain_text_syntax(&mut self) {
        let s = "---\nname: Plain Text\nfile_extensions: [txt]\nscope: text.plain\ncontexts: \
                 {main: []}";
        let syn = SyntaxDefinition::load_from_str(s, false, None).unwrap();
        self.syntaxes.push(syn);
    }

    /// Loads all the `.sublime-syntax` files in a folder into this builder.
    ///
    /// The `lines_include_newline` parameter is used to work around the fact that Sublime Text
    /// normally passes line strings including newline characters (`\n`) to its regex engine. This
    /// results in many syntaxes having regexes matching `\n`, which doesn't work if you don't pass
    /// in newlines. It is recommended that if you can you pass in lines with newlines if you can
    /// and pass `true` for this parameter. If that is inconvenient pass `false` and the loader
    /// will do some hacky find and replaces on the match regexes that seem to work for the default
    /// syntax set, but may not work for any other syntaxes.
    ///
    /// In the future I might include a "slow mode" that copies the lines passed in and appends a
    /// newline if there isn't one, but in the interest of performance currently this hacky fix will
    /// have to do.
    pub fn add_from_folder<P: AsRef<Path>>(
        &mut self,
        folder: P,
        lines_include_newline: bool,
    ) -> Result<(), LoadingError> {
        self.lines_include_newline = lines_include_newline;
        for entry in walkdir::WalkDir::new(folder).sort_by(|a, b| a.file_name().cmp(b.file_name()))
        {
            let entry = entry.map_err(|e| LoadingError::WalkDir(Box::new(e)))?;
            if crate::tm_language::is_file_entry(&entry)
                && (entry
                    .path()
                    .extension()
                    .is_some_and(|e| e == "sublime-syntax")
                    || crate::tm_language::is_tm_language(entry.path()))
            {
                match load_syntax_file(entry.path(), lines_include_newline) {
                    Ok(syntax) => {
                        if let Some(path_str) = entry.path().to_str() {
                            // Split the path up and rejoin with slashes so that syntaxes loaded on Windows
                            // can still be loaded the same way.
                            let path = Path::new(path_str);
                            let path_parts: Vec<_> =
                                path.iter().map(|c| c.to_str().unwrap()).collect();
                            self.path_syntaxes
                                .push((path_parts.join("/").to_string(), self.syntaxes.len()));
                        }
                        self.syntaxes.push(syntax);
                    }
                    Err(err) => {
                        self.warnings
                            .push(format!("skipping {:?}: {}", entry.path(), err));
                    }
                }
            }
        }

        Ok(())
    }

    /// Build a [`SyntaxSet`] from the syntaxes that have been added to this
    /// builder.
    ///
    /// ### Linking
    ///
    /// The contexts in syntaxes can reference other contexts in the same syntax
    /// or even other syntaxes. For example, a HTML syntax can reference a CSS
    /// syntax so that CSS blocks in HTML work as expected.
    ///
    /// Those references work in various ways and involve one or two lookups.
    /// To avoid having to do these lookups during parsing/highlighting, the
    /// references are changed to directly reference contexts via index. That's
    /// called linking.
    ///
    /// Linking is done in this build step. So in order to get the best
    /// performance, you should try to avoid calling this too much. Ideally,
    /// create a [`SyntaxSet`] once and then use it many times. If you can,
    /// serialize a [`SyntaxSet`] for your program and when you run the program,
    /// directly load the [`SyntaxSet`].
    ///
    /// [`SyntaxSet`]: struct.SyntaxSet.html
    pub fn build(self) -> SyntaxSet {
        let SyntaxSetBuilder {
            syntaxes: mut syntax_definitions,
            path_syntaxes,
            mut warnings,
            lines_include_newline,
        } = self;

        // Anonymous names are local implementation details, never override slots.
        // Namespace every file before inheriting (including embed and prototype wrappers).
        for (index, syntax) in syntax_definitions.iter_mut().enumerate() {
            namespace_anonymous_contexts(syntax, index);
        }
        // Extends resolution phase: merge parent contexts/variables into children
        let (mut syntax_definitions, extends_warnings) =
            Self::resolve_extends(syntax_definitions, &path_syntaxes, lines_include_newline);
        warnings.extend(extends_warnings);

        // And only now are the regexes final, so only now can
        // a `captures:` mapping be narrowed to the groups that consume. Doing
        // it while reading the YAML measures a child's regex before its
        // parent's variables exist — see `parse_captures`.
        for (index, syntax) in syntax_definitions.iter_mut().enumerate() {
            let path = path_syntaxes
                .iter()
                .find(|(_, i)| *i == index)
                .map_or(syntax.name.as_str(), |(path, _)| path.as_str());
            warnings.extend(
                syntax
                    .authoring_warnings
                    .iter()
                    .cloned()
                    .chain(crate::yaml_load::regex_authoring_warnings(syntax))
                    .map(|w| format!("{path}: {w}")),
            );
            // One sentinel per syntax keeps missing references cheap and isolated.
            syntax
                .contexts
                .insert("#velocity_missing".into(), Context::new(Some(false)));
            crate::yaml_load::filter_captures_to_consuming_groups(syntax);
        }

        let mut syntaxes = Vec::with_capacity(syntax_definitions.len());
        let mut all_context_ids = Vec::new();
        let mut all_contexts = vec![Vec::new(); syntax_definitions.len()];

        for (syntax_index, syntax_definition) in syntax_definitions.into_iter().enumerate() {
            let SyntaxDefinition {
                authoring_warnings: _,
                name,
                file_extensions,
                scope,
                first_line_match,
                hidden,
                variables,
                contexts,
                extends: _,
                raw_first_line_match: _,
                version,
            } = syntax_definition;

            let mut context_ids = HashMap::new();

            let mut contexts: Vec<(String, Context)> = contexts.into_iter().collect();
            // Sort the values of the HashMap so that the contexts in the
            // resulting SyntaxSet have a deterministic order for serializing.
            // Because we're sorting by the keys which are unique, we can use
            // an unstable sort.
            contexts.sort_unstable_by(|(name_a, _), (name_b, _)| name_a.cmp(name_b));
            for (name, mut context) in contexts {
                let context_index = all_contexts[syntax_index].len();
                context_ids.insert(
                    name,
                    ContextId {
                        syntax_index,
                        context_index,
                    },
                );
                context.id = ContextId {
                    syntax_index,
                    context_index,
                };
                all_contexts[syntax_index].push(context);
            }

            let syntax = SyntaxReference {
                path: path_syntaxes
                    .iter()
                    .find(|(_, i)| *i == syntax_index)
                    .map_or_else(|| format!("{name}.sublime-syntax"), |(p, _)| p.clone()),
                name,
                file_extensions,
                scope,
                first_line_match,
                hidden,
                variables,
                version,
                lazy_contexts: OnceLock::new(),
                serialized_lazy_contexts: Vec::new(), // initialized in the last step
            };
            syntaxes.push(syntax);
            all_context_ids.push(context_ids);
        }

        let mut found_more_backref_includes = true;
        for (syntax_index, _syntax) in syntaxes.iter().enumerate() {
            let mut no_prototype = HashSet::new();
            let prototype = all_context_ids[syntax_index].get("prototype");
            if let Some(prototype_id) = prototype {
                // TODO: We could do this after parsing YAML, instead of here?
                Self::recursively_mark_no_prototype(
                    prototype_id,
                    &all_context_ids[syntax_index],
                    &all_contexts,
                    &mut no_prototype,
                );
            }

            for context_id in all_context_ids[syntax_index].values() {
                let context = &mut all_contexts[context_id.syntax_index][context_id.context_index];
                if let Some(prototype_id) = prototype
                    && context.meta_include_prototype.unwrap_or(true)
                    && !no_prototype.contains(context_id)
                {
                    context.prototype = Some(*prototype_id);
                }
                Self::link_context(
                    context,
                    syntax_index,
                    &all_context_ids,
                    &syntaxes,
                    &mut warnings,
                );

                if context.uses_backrefs {
                    found_more_backref_includes = true;
                }
            }
        }

        // We need to recursively mark contexts that include contexts which
        // use backreferences as using backreferences. In theory we could use
        // a more efficient method here like doing a toposort or constructing
        // a representation with reversed edges and then tracing in the
        // opposite direction, but I benchmarked this and it adds <2% to link
        // time on the default syntax set, and linking doesn't even happen
        // when loading from a binary dump.
        while found_more_backref_includes {
            found_more_backref_includes = false;
            // find any contexts which include a context which uses backrefs
            // and mark those as using backrefs - to support nested includes
            for syntax_index in 0..syntaxes.len() {
                for context_index in 0..all_contexts[syntax_index].len() {
                    let context = &all_contexts[syntax_index][context_index];
                    if !context.uses_backrefs && context.patterns.iter().any(|pattern| {
                        matches!(pattern, Pattern::Include(ContextReference::Direct(id)) | Pattern::IncludeWithPrototype(ContextReference::Direct(id)) if all_contexts[id.syntax_index][id.context_index].uses_backrefs)
                    }) {
                        let context = &mut all_contexts[syntax_index][context_index];
                        context.uses_backrefs = true;
                        // look for contexts including this context
                        found_more_backref_includes = true;
                    }
                }
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            for si in 0..all_contexts.len() {
                for ci in 0..all_contexts[si].len() {
                    let context = &all_contexts[si][ci];
                    if !context.uses_base
                        && context.patterns.iter().any(|p| match p {
                            Pattern::Include(ContextReference::Base) => true,
                            Pattern::Include(ContextReference::Direct(id))
                            | Pattern::IncludeWithPrototype(ContextReference::Direct(id)) => {
                                all_contexts[id.syntax_index][id.context_index].uses_base
                            }
                            _ => false,
                        })
                    {
                        all_contexts[si][ci].uses_base = true;
                        changed = true;
                    }
                }
            }
        }

        // The combination of
        //  * the algorithms above
        //  * the borrow checker
        // makes it necessary to set these up as the last step.
        for syntax in &mut syntaxes {
            let lazy_contexts = LazyContexts {
                context_ids: all_context_ids.remove(0),
                contexts: all_contexts.remove(0),
            };

            syntax.serialized_lazy_contexts = crate::dumps::dump_binary(&lazy_contexts);
        }

        SyntaxSet {
            matcher_backend: default_backend(),
            pattern_contexts: Mutex::new(HashMap::new()),
            syntaxes,
            path_syntaxes,
            physical_paths: HashMap::new(),
            warnings,
            first_line_cache: OnceLock::new(),
        }
    }

    /// Resolve `extends` relationships between syntax definitions.
    ///
    /// For each child syntax that extends a parent, merge the parent's contexts and variables
    /// into the child. Respects `meta_prepend`/`meta_append` merge modes.
    fn resolve_extends(
        mut syntax_definitions: Vec<SyntaxDefinition>,
        path_syntaxes: &[(String, usize)],
        lines_include_newline: bool,
    ) -> (Vec<SyntaxDefinition>, Vec<String>) {
        let mut warnings = Vec::new();
        let origin = |index: usize| {
            path_syntaxes
                .iter()
                .find(|(_, i)| *i == index)
                .map_or("<memory>", |(path, _)| path.as_str())
        };

        // Build lookup maps: name -> index and path-suffix -> index
        let mut name_to_index: HashMap<String, usize> = HashMap::new();
        for (i, sd) in syntax_definitions.iter().enumerate() {
            name_to_index.insert(sd.name.clone(), i);
        }

        // Track which syntaxes need extends resolution
        let mut unresolved: HashSet<usize> = HashSet::new();
        // — the syntaxes whose `extends` was
        // refused for a reason other than a missing parent. See the pass at
        // the end that gives them a `main`.
        let mut broken: HashSet<usize> = HashSet::new();
        for (i, sd) in syntax_definitions.iter().enumerate() {
            if !sd.extends.is_empty() {
                unresolved.insert(i);
            }
        }

        if unresolved.is_empty() {
            return (syntax_definitions, warnings);
        }

        // How far away each syntax's variables were declared.
        //
        // A syntax not in here declares everything it has, at depth 0; a
        // resolved child records, per variable, how many `extends` hops from it
        // the declaration it ended up with was written. That is what decides a
        // clash between two parents — see the merge below. Kept beside the
        // definitions rather than on them: it is true only while this function
        // runs, and a field on `SyntaxDefinition` would be one more thing to
        // serialise and keep honest.
        let mut variable_depth: HashMap<usize, HashMap<String, usize>> = HashMap::new();
        let mut context_depth: HashMap<usize, HashMap<String, usize>> = HashMap::new();

        // Track root ancestor for each syntax (syntaxes with no extends are their own root)
        let mut syntax_roots: HashMap<usize, usize> = HashMap::new();
        for (i, sd) in syntax_definitions.iter().enumerate() {
            if sd.extends.is_empty() {
                syntax_roots.insert(i, i);
            }
        }

        // Fixed-point loop: resolve extends iteratively (to handle chains and multiple parents)
        let mut made_progress = true;
        while made_progress && !unresolved.is_empty() {
            made_progress = false;

            let still_unresolved: Vec<usize> = unresolved.iter().copied().collect();
            for child_idx in still_unresolved {
                let extends_paths = syntax_definitions[child_idx].extends.clone();
                if extends_paths.is_empty() {
                    continue;
                }

                // Find all parent indices; skip if any parent is not found or unresolved
                let mut parent_indices = Vec::with_capacity(extends_paths.len());
                let mut all_parents_ready = true;
                for extends_path in &extends_paths {
                    let parent_idx = Self::find_parent_index(
                        extends_path,
                        path_syntaxes,
                        &syntax_definitions,
                        &name_to_index,
                    );
                    match parent_idx {
                        Some(idx) if !unresolved.contains(&idx) => {
                            parent_indices.push(idx);
                        }
                        _ => {
                            all_parents_ready = false;
                            break;
                        }
                    }
                }

                if !all_parents_ready {
                    continue;
                }

                // H & I: all parents must share the same version as the child
                let child_version = syntax_definitions[child_idx].version;
                let version_ok = parent_indices
                    .iter()
                    .all(|&pi| syntax_definitions[pi].version == child_version);
                if !version_ok {
                    warnings.push(format!(
                        "{}: syntax '{}' has a version mismatch with one or more parents; \
                         extends will not be applied",
                        origin(child_idx),
                        syntax_definitions[child_idx].name
                    ));
                    // A refused `extends` leaves a
                    // syntax that cannot be parsed, and this said so and walked
                    // away. See `broken` below.
                    broken.insert(child_idx);
                    unresolved.remove(&child_idx);
                    syntax_roots.insert(child_idx, child_idx);
                    made_progress = true;
                    continue;
                }

                // G: for multiple parents, all must share the same root ancestor
                let parent_roots: Vec<usize> = parent_indices
                    .iter()
                    .map(|&pi| *syntax_roots.get(&pi).unwrap_or(&pi))
                    .collect();
                let common_root = parent_roots[0];
                if !parent_roots.iter().all(|&r| r == common_root) {
                    warnings.push(format!(
                        "{}: syntax '{}' extends parents that derive from different base syntaxes; \
                         extends will not be applied",
                        origin(child_idx),
                        syntax_definitions[child_idx].name
                    ));
                    // The other refusal.
                    broken.insert(child_idx);
                    unresolved.remove(&child_idx);
                    syntax_roots.insert(child_idx, child_idx);
                    made_progress = true;
                    continue;
                }

                // Merge all parents left-to-right: later parent overrides earlier
                //
                // Only among declarations the same distance
                // away. A parent that *declares* a variable or a context beats
                // one that merely inherited it, whichever order they are listed
                // in, and "later wins" decides only a tie.
                //
                // Asked of Sublime Text rather than argued —
                // the `extends_variable_precedence.probe` probe
                // and `…_reversed.probe` are a grandchild of two parents, one
                // declaring the variable and one inheriting the base's, in both
                // orders; ST takes the declared one both times.
                //
                // `Zsh (for Markdown)` is the real file, and it is both halves
                // of this. It extends `Bash (for Markdown)`, which sets
                // `no_indent: ^\s*` so that a shebang inside an *indented*
                // Markdown code fence is still a shebang and declares a `main`
                // that recognises a `$ ` interactive prompt, and then `Zsh`,
                // which extends `Bash` and declares neither. Flat "later wins"
                // handed both of `Bash`'s back: a fenced `#!/usr/bin/env zsh`
                // came out `comment.line.number-sign`, and `$ ls` came out as a
                // command called `$`.
                //
                // The context half is probed the same way,
                // `extends_context_precedence.probe` and `…_reversed.probe`.
                let mut merged_variables: HashMap<String, (usize, String)> = HashMap::new();
                let mut merged_contexts: HashMap<String, (usize, Context)> = HashMap::new();

                for &parent_idx in &parent_indices {
                    let parent_variables = syntax_definitions[parent_idx].variables.clone();
                    let parent_contexts = syntax_definitions[parent_idx].contexts.clone();

                    // Merge variables: nearest declaration wins, then later parent
                    for (k, v) in parent_variables {
                        let depth = variable_depth
                            .get(&parent_idx)
                            .and_then(|known| known.get(&k))
                            .copied()
                            .unwrap_or(0)
                            + 1;
                        match merged_variables.get(&k) {
                            Some((nearer, _)) if *nearer < depth => {}
                            _ => {
                                merged_variables.insert(k, (depth, v));
                            }
                        }
                    }

                    // Merge contexts: nearest declaration wins, then later parent
                    for (ctx_name, parent_ctx) in parent_contexts {
                        let depth = context_depth
                            .get(&parent_idx)
                            .and_then(|known| known.get(&ctx_name))
                            .copied()
                            .unwrap_or(0)
                            + 1;
                        match merged_contexts.get(&ctx_name) {
                            Some((nearer, _)) if *nearer < depth => {}
                            _ => {
                                merged_contexts.insert(ctx_name, (depth, parent_ctx));
                            }
                        }
                    }
                }

                // The scopes of the parents this child is about to inherit
                // `main` from — read before the borrow below, and used to
                // strip the parent's file scope out of it. See the comment at
                // the regeneration of `__start`/`__main`.
                let parent_scopes: Vec<Scope> = parent_indices
                    .iter()
                    .map(|&pi| syntax_definitions[pi].scope)
                    .collect();

                let child = &mut syntax_definitions[child_idx];

                // Child variables override merged parent variables: nothing is
                // nearer than declared here.
                let child_variables: HashMap<String, String> = std::mem::take(&mut child.variables);
                for (k, v) in child_variables {
                    merged_variables.insert(k, (0, v));
                }
                variable_depth.insert(
                    child_idx,
                    merged_variables
                        .iter()
                        .map(|(name, (depth, _))| (name.clone(), *depth))
                        .collect(),
                );
                child.variables = merged_variables
                    .into_iter()
                    .map(|(name, (_, value))| (name, value))
                    .collect();

                // Merge contexts: child applies merge_mode against merged parent result
                let mut inherited_at: HashMap<String, usize> = HashMap::new();
                for (ctx_name, (depth, parent_ctx)) in merged_contexts {
                    if let Some(child_ctx) = child.contexts.get_mut(&ctx_name) {
                        match child_ctx.merge_mode {
                            ContextMergeMode::Replace => {
                                // Child's version wins, keep as-is
                            }
                            ContextMergeMode::Prepend => {
                                // child patterns + parent patterns
                                let mut merged_patterns = child_ctx.patterns.clone();
                                merged_patterns.extend(parent_ctx.patterns);
                                child_ctx.patterns = merged_patterns;
                                if child_ctx.meta_scope.is_empty() {
                                    child_ctx.meta_scope = parent_ctx.meta_scope;
                                }
                                if child_ctx.meta_content_scope.is_empty() {
                                    child_ctx.meta_content_scope = parent_ctx.meta_content_scope;
                                }
                                if child_ctx.clear_scopes.is_none() {
                                    child_ctx.clear_scopes = parent_ctx.clear_scopes;
                                }
                                if child_ctx.meta_include_prototype.is_none() {
                                    child_ctx.meta_include_prototype =
                                        parent_ctx.meta_include_prototype;
                                }
                            }
                            ContextMergeMode::Append => {
                                // parent patterns + child patterns
                                let child_patterns = child_ctx.patterns.clone();
                                child_ctx.patterns = parent_ctx.patterns;
                                child_ctx.patterns.extend(child_patterns);
                                if child_ctx.meta_scope.is_empty() {
                                    child_ctx.meta_scope = parent_ctx.meta_scope;
                                }
                                if child_ctx.meta_content_scope.is_empty() {
                                    child_ctx.meta_content_scope = parent_ctx.meta_content_scope;
                                }
                                if child_ctx.clear_scopes.is_none() {
                                    child_ctx.clear_scopes = parent_ctx.clear_scopes;
                                }
                                if child_ctx.meta_include_prototype.is_none() {
                                    child_ctx.meta_include_prototype =
                                        parent_ctx.meta_include_prototype;
                                }
                            }
                        }
                        // Whatever the merge mode, the child has a say in this
                        // one: nothing is nearer than declared here.
                        inherited_at.insert(ctx_name, 0);
                    } else {
                        // Parent context not in child: inherit it, and remember
                        // how far away it was written.
                        inherited_at.insert(ctx_name.clone(), depth);
                        child.contexts.insert(ctx_name, parent_ctx);
                    }
                }
                context_depth.insert(
                    child_idx,
                    child
                        .contexts
                        .keys()
                        .map(|name| (name.clone(), inherited_at.get(name).copied().unwrap_or(0)))
                        .collect(),
                );

                // Regenerate __start/__main for the child's own scope.
                // The parent's __start carries the parent's meta_scope (e.g.
                // text.html.plain), which is wrong for the child (e.g.
                // text.html.rails). Always regenerate so the child's scope
                // is used.
                if child.contexts.contains_key("main") {
                    let mut scope_repo = vel_scope::lock_global_scope_repo();
                    let top_level_scope = child.scope;

                    // A child that does not declare `main`
                    // inherits the parent's, and the parent's already carries
                    // the parent's file scope at position 0 — put there by the
                    // parent's own `add_initial_contexts`. Regenerating below
                    // inserts the *child's* in front of it rather than in place
                    // of it, so `Zsh` came out `source.shell.zsh
                    // source.shell.bash` and `Regular Expressions`
                    // `source.regexp source.regexp.basic`, where Sublime Text
                    // pushes the child's alone — asked directly,
                    // the `regexp_modes.probe` probe and
                    // `shellscript.probe`.
                    //
                    // It cost no assertion either way — the atom was being
                    // over-popped a few columns later, at the close of a
                    // `(?ix: … )` mode group, so it was *absorbing* a pop
                    // rather than causing one. That over-pop is a separate bug
                    // and still open: it is the corpus' last scope-stack
                    // divergence (`syntax_test_regexp.re`, 4 ops) and Python's
                    // last failing assertion, and taking the spare atom away
                    // made the divergence one op larger rather than smaller.
                    if let Some(main) = child.contexts.get_mut("main")
                        && main
                            .meta_content_scope
                            .first()
                            .is_some_and(|first| parent_scopes.contains(first))
                    {
                        main.meta_content_scope.remove(0);
                    }

                    SyntaxDefinition::add_initial_contexts(
                        &mut child.contexts,
                        &mut crate::yaml_load::ParserState {
                            scope_repo: scope_repo.deref_mut(),
                            variables: child.variables.clone(),
                            variable_regex: Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}".into()),
                            backref_regex: Regex::new(r"\\\d".into()),
                            lines_include_newline,
                            version: child.version,
                            context_depth: 0,
                        },
                        top_level_scope,
                    );
                }

                if let Err(e) =
                    crate::yaml_load::re_resolve_all_regexes(child, lines_include_newline)
                {
                    warnings.push(format!(
                        "{}: failed to re-resolve regexes for '{}' after extends: {}",
                        origin(child_idx),
                        child.name,
                        e
                    ));
                }

                syntax_roots.insert(child_idx, common_root);
                unresolved.remove(&child_idx);
                made_progress = true;
            }
        }

        // Every syntax whose `extends` was
        // refused, not only the ones whose parent could not be found.
        //
        // The two version/base refusals above take the child out of
        // `unresolved` and carry on, so it reached the set with whatever
        // contexts it declared itself — which for a definition that is
        // *nothing but* an `extends` is none at all, no `main` and no
        // `__start`. `ParseState::new` indexes `context_ids()["__start"]`, so
        // the first line of the first file that resolved to it took the
        // process down.
        //
        // It is a file an end user writes: a two-line `.sublime-syntax`
        // extending Sublime's Java without repeating its `version: 2` is the
        // whole of it. The reference says the versions must match; it does not
        // say the library may fall over when they do not.
        // `extends_refused_for_a_version_mismatch_still_parses` is the
        // regression test.
        let broken: BTreeSet<usize> = broken.union(&unresolved).copied().collect();
        if !broken.is_empty() {
            for idx in &broken {
                let syntax = &mut syntax_definitions[*idx];
                let e = &syntax.extends;
                let extends_str = if e.is_empty() {
                    "?".to_string()
                } else {
                    e.join(", ")
                };
                if unresolved.contains(idx) {
                    warnings.push(format!(
                        "{}: syntax '{}' extends '{}' but parent was not found or has circular dependency",
                        origin(*idx), syntax.name, extends_str,
                    ));
                }
                // Mark broken syntaxes as hidden so they won't be found by
                // name/extension lookups and won't cause panics when used.
                syntax.hidden = true;

                // Ensure the syntax has __start and __main contexts so that
                // ParseState::new won't panic if this syntax is still accessed.
                if !syntax.contexts.contains_key("main") {
                    syntax
                        .contexts
                        .insert("main".to_string(), Context::new(None));
                }
                if !syntax.contexts.contains_key("__start") {
                    let mut scope_repo = vel_scope::lock_global_scope_repo();
                    let top_level_scope = syntax.scope;
                    SyntaxDefinition::add_initial_contexts(
                        &mut syntax.contexts,
                        &mut crate::yaml_load::ParserState {
                            scope_repo: scope_repo.deref_mut(),
                            variables: syntax.variables.clone(),
                            variable_regex: Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}".into()),
                            backref_regex: Regex::new(r"\\\d".into()),
                            lines_include_newline,
                            version: syntax.version,
                            context_depth: 0,
                        },
                        top_level_scope,
                    );
                }
            }
        }

        (syntax_definitions, warnings)
    }

    /// Find the index of a parent syntax by matching the extends path.
    fn find_parent_index(
        extends_path: &str,
        path_syntaxes: &[(String, usize)],
        syntax_definitions: &[SyntaxDefinition],
        name_to_index: &HashMap<String, usize>,
    ) -> Option<usize> {
        // Normalize separators for matching
        let normalized = extends_path.replace('\\', "/");
        let normalized = normalized
            .strip_prefix("Packages/")
            .unwrap_or(&normalized)
            .to_owned();

        // First try matching against path_syntaxes (path ends with extends value)
        //
        // Last match, not first.
        //
        // Everything else in `syntect` resolves a reference to the *last*
        // definition that answers — `find_id` does, and the `name_to_index`
        // fallback below does, because it is built by inserting every name in
        // order. Only this loop took the first, and the difference shows the
        // moment one builder holds two copies of a package — the supported way
        // to let a user's `Packages` override the one that shipped — so `TSX`
        // in the second copy would find the *first* copy's `JSX` for one of its
        // parents and the second's for another.
        // Upstream's own check then sees parents deriving from different base
        // syntaxes and drops the inheritance entirely — "syntax 'TSX' extends
        // parents that derive from different base syntaxes; extends will not
        // be applied" — and every `jsx-*` context TSX inherits goes
        // unresolved.
        let slash_normalized = format!("/{}", normalized);
        for (path, idx) in path_syntaxes.iter().rev() {
            let path_normalized = path.replace('\\', "/");
            if path_normalized.ends_with(&slash_normalized) || path_normalized == normalized {
                return Some(*idx);
            }
        }

        // Try matching by file stem (e.g., "Packages/C/C.sublime-syntax" -> look for syntax named "C")
        if let Some(file_name) = std::path::Path::new(&normalized).file_stem()
            && let Some(name_str) = file_name.to_str()
        {
            if let Some(&idx) = name_to_index.get(name_str) {
                return Some(idx);
            }
            // Try case-insensitive match
            for (i, sd) in syntax_definitions.iter().enumerate() {
                if sd.name.eq_ignore_ascii_case(name_str) {
                    return Some(i);
                }
            }
        }

        None
    }

    /// Anything recursively included by the prototype shouldn't include the prototype.
    /// This marks them as such.
    fn recursively_mark_no_prototype(
        context_id: &ContextId,
        syntax_context_ids: &HashMap<String, ContextId>,
        all_contexts: &[Vec<Context>],
        no_prototype: &mut HashSet<ContextId>,
    ) {
        let first_time = no_prototype.insert(*context_id);
        if !first_time {
            return;
        }

        for pattern in &all_contexts[context_id.syntax_index][context_id.context_index].patterns {
            match *pattern {
                // Apparently inline blocks also don't include the prototype when within the prototype.
                // This is really weird, but necessary to run the YAML syntax.
                Pattern::Match(ref match_pat) => {
                    let maybe_context_refs = match match_pat.operation {
                        MatchOperation::Push {
                            ctx_refs: ref context_refs,
                            ..
                        }
                        | MatchOperation::Set {
                            ctx_refs: ref context_refs,
                            ..
                        }
                        | MatchOperation::Branch {
                            alternatives: ref context_refs,
                            ..
                        } => Some(context_refs),
                        // An `embed:` is where the walk
                        // stops. Sublime was asked, with four probes rather
                        // than one, because two rules explain the same file:
                        // `prototype_in_pushed` says a context *pushed* from
                        // within the prototype's reach is prototype-free (ST
                        // agrees with upstream, and its control says the
                        // prototype is otherwise applied), while
                        // `prototype_in_embedded` — push, then embed — says
                        // that one keeps it, and `…_plain` says the
                        // `meta_include_prototype: false` in between is not
                        // what makes the difference.
                        //
                        // Haskell is the file. Its prototype includes
                        // `preprocessor-pragmas`, which pushes
                        // `preprocessor-pragma-body`, which *embeds*
                        // `preprocessor-pragma-signature-value`, which includes
                        // `functions` — so following the embed marked the
                        // branch every function declaration in the language is
                        // read by, and `--` stopped being a comment inside one.
                        MatchOperation::Embed { .. }
                        | MatchOperation::Pop(_)
                        | MatchOperation::None
                        | MatchOperation::Fail(_) => None,
                    };
                    if let Some(context_refs) = maybe_context_refs {
                        for context_ref in context_refs.iter() {
                            match context_ref {
                                ContextReference::Inline(s) | ContextReference::Named(s) => {
                                    if let Some(i) = syntax_context_ids.get(s) {
                                        Self::recursively_mark_no_prototype(
                                            i,
                                            syntax_context_ids,
                                            all_contexts,
                                            no_prototype,
                                        );
                                    }
                                }
                                ContextReference::Direct(id) => {
                                    Self::recursively_mark_no_prototype(
                                        id,
                                        syntax_context_ids,
                                        all_contexts,
                                        no_prototype,
                                    );
                                }
                                _ => (),
                            }
                        }
                    }
                }
                Pattern::Include(ref reference) | Pattern::IncludeWithPrototype(ref reference) => {
                    match reference {
                        ContextReference::Named(s) => {
                            if let Some(id) = syntax_context_ids.get(s) {
                                Self::recursively_mark_no_prototype(
                                    id,
                                    syntax_context_ids,
                                    all_contexts,
                                    no_prototype,
                                );
                            }
                        }
                        ContextReference::Direct(id) => {
                            Self::recursively_mark_no_prototype(
                                id,
                                syntax_context_ids,
                                all_contexts,
                                no_prototype,
                            );
                        }
                        _ => (),
                    }
                }
            }
        }
    }

    fn link_context(
        context: &mut Context,
        syntax_index: usize,
        all_context_ids: &[HashMap<String, ContextId>],
        syntaxes: &[SyntaxReference],
        warnings: &mut Vec<String>,
    ) {
        for pattern in &mut context.patterns {
            match *pattern {
                Pattern::Match(ref mut match_pat) => Self::link_match_pat(
                    match_pat,
                    syntax_index,
                    all_context_ids,
                    syntaxes,
                    warnings,
                ),
                Pattern::Include(ref mut context_ref)
                | Pattern::IncludeWithPrototype(ref mut context_ref) => Self::link_ref(
                    context_ref,
                    syntax_index,
                    all_context_ids,
                    syntaxes,
                    warnings,
                ),
            }
        }
    }

    fn link_ref(
        context_ref: &mut ContextReference,
        syntax_index: usize,
        all_context_ids: &[HashMap<String, ContextId>],
        syntaxes: &[SyntaxReference],
        warnings: &mut Vec<String>,
    ) {
        // println!("{:?}", context_ref);
        use super::syntax_definition::ContextReference::*;
        let linked_context_id = match *context_ref {
            Named(ref s) | Inline(ref s) => {
                // This isn't actually correct, but it is better than nothing/crashing.
                // This is being phased out anyhow, see https://github.com/sublimehq/Packages/issues/73
                // Fixes issue #30
                if s == "$top_level_main" {
                    all_context_ids[syntax_index].get("main")
                } else {
                    all_context_ids[syntax_index].get(s)
                }
            }
            ByScope {
                scope,
                ref sub_context,
                with_escape: _,
            } => Self::find_id(sub_context, all_context_ids, syntaxes, |index_and_syntax| {
                index_and_syntax.1.scope == scope
            }),
            File {
                ref name,
                ref sub_context,
                with_escape: _,
            } => Self::find_id(sub_context, all_context_ids, syntaxes, |index_and_syntax| {
                file_reference_matches(name, index_and_syntax.1)
            }),
            Direct(_) | Base => None,
        };
        if let Some(context_id) = linked_context_id {
            *context_ref = Direct(*context_id);
        } else if !matches!(context_ref, Direct(_) | Base) {
            warnings.push(format!(
                "{}: unresolved context reference {context_ref:?}",
                syntaxes[syntax_index].path
            ));
            // Missing embedded languages retain the legacy Plain Text scope. A typo in
            // an existing language's context must stay empty, rather than change language.
            let missing_syntax = match context_ref {
                ByScope {
                    scope,
                    with_escape: true,
                    ..
                } => !syntaxes.iter().any(|s| s.scope == *scope),
                File {
                    name,
                    with_escape: true,
                    ..
                } => !syntaxes.iter().any(|s| file_reference_matches(name, s)),
                _ => false,
            };
            let fallback = missing_syntax
                .then(|| {
                    Self::find_id(&None, all_context_ids, syntaxes, |(_, s)| {
                        s.scope.build_string() == "text.plain"
                    })
                })
                .flatten()
                .copied();
            *context_ref =
                Direct(fallback.unwrap_or(all_context_ids[syntax_index]["#velocity_missing"]));
        }
    }

    fn find_id<'a>(
        sub_context: &Option<String>,
        all_context_ids: &'a [HashMap<String, ContextId>],
        syntaxes: &'a [SyntaxReference],
        predicate: impl FnMut(&(usize, &SyntaxReference)) -> bool,
    ) -> Option<&'a ContextId> {
        let context_name = sub_context.as_ref().map_or("main", |x| &**x);
        syntaxes
            .iter()
            .enumerate()
            .rev()
            .find(predicate)
            .and_then(|index_and_syntax| all_context_ids[index_and_syntax.0].get(context_name))
    }

    fn link_match_pat(
        match_pat: &mut MatchPattern,
        syntax_index: usize,
        all_context_ids: &[HashMap<String, ContextId>],
        syntaxes: &[SyntaxReference],
        warnings: &mut Vec<String>,
    ) {
        let maybe_context_refs = match match_pat.operation {
            MatchOperation::Push {
                ctx_refs: ref mut context_refs,
                ..
            }
            | MatchOperation::Set {
                ctx_refs: ref mut context_refs,
                ..
            }
            | MatchOperation::Branch {
                alternatives: ref mut context_refs,
                ..
            }
            | MatchOperation::Embed {
                contexts: ref mut context_refs,
                ..
            } => Some(context_refs),
            MatchOperation::Pop(_) | MatchOperation::None | MatchOperation::Fail(_) => None,
        };
        if let Some(context_refs) = maybe_context_refs {
            for context_ref in context_refs.iter_mut() {
                Self::link_ref(
                    context_ref,
                    syntax_index,
                    all_context_ids,
                    syntaxes,
                    warnings,
                );
            }
        }
        if let Some(ref mut context_ref) = match_pat.with_prototype {
            Self::link_ref(
                context_ref,
                syntax_index,
                all_context_ids,
                syntaxes,
                warnings,
            );
        }
    }
}

#[derive(Debug)]
struct FirstLineCache {
    /// (first line regex, syntax index) pairs for all syntaxes with a first line regex
    regexes: Vec<(Regex, usize)>,
}

impl FirstLineCache {
    fn new(syntaxes: &[SyntaxReference]) -> FirstLineCache {
        let mut regexes = Vec::new();
        for (i, syntax) in syntaxes.iter().enumerate() {
            if let Some(ref reg_str) = syntax.first_line_match {
                let reg = Regex::new(reg_str.into());
                regexes.push((reg, i));
            }
        }
        FirstLineCache { regexes }
    }
}

impl SyntaxSet {
    /// Returns a flattened static context matcher for diagnostics and microbenchmarks.
    /// `syntax` must belong to this set. Runtime backreferences require a live parser
    /// binding and remain unresolved here; ordinary parsing supplies those bindings.
    pub fn context_matcher(
        &self,
        syntax: &SyntaxReference,
        name: &str,
    ) -> Result<Option<PatternSet>, ParsingError> {
        let Some(id) = syntax.context_ids().get(name) else {
            return Ok(None);
        };
        self.context_patterns(
            *id,
            None,
            *syntax.context_ids().get("main").expect("main context"),
        )
        .map(|context| Some(context.matcher.clone()))
    }
    /// Chooses the ordered-context matching backend. Configure before creating parser states.
    pub fn with_matcher_backend(mut self, backend: Arc<dyn MatcherBackend>) -> Self {
        self.matcher_backend = backend;
        self.pattern_contexts = Mutex::new(HashMap::new());
        for syntax in &mut self.syntaxes {
            if let Some(lazy) = syntax.lazy_contexts.get_mut() {
                for context in &mut lazy.contexts {
                    context.compiled = Default::default();
                }
            }
        }
        self
    }
    pub(crate) fn context_patterns(
        &self,
        id: ContextId,
        captures: Option<&(Region, String)>,
        base: ContextId,
    ) -> Result<Arc<ContextPatterns>, ParsingError> {
        let context = self.get_context(&id)?;
        let is_static = (!context.uses_backrefs || captures.is_none()) && !context.uses_base;
        if is_static && let Some(compiled) = context.compiled.0.get() {
            return Ok(compiled.clone());
        }
        // Store the full capture binding so hash collisions cannot reuse a different pattern.
        let key = (
            id,
            if context.uses_backrefs {
                captures.cloned()
            } else {
                None
            },
            context.uses_base.then_some(base),
        );
        if !is_static
            && let Some(cached) = self
                .pattern_contexts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
        {
            return Ok(cached.clone());
        }
        let mut rules = Vec::new();
        let mut patterns = Vec::new();
        for (ctx, index) in context_iter_with_base(self, context, base) {
            let pattern = ctx.match_at(index)?;
            let regex = match (pattern.has_captures, captures) {
                (true, Some((r, line))) => pattern.regex_with_refs(r, line),
                _ => pattern.regex.clone(),
            };
            rules.push((ctx.id, index));
            patterns.push(PatternSpec {
                regex,
                allow_empty: !matches!(pattern.operation, MatchOperation::None),
            });
        }
        let compiled = Arc::new(ContextPatterns {
            rules,
            matcher: {
                let syntax = &self.syntaxes[id.syntax_index];
                let name = syntax
                    .context_ids()
                    .iter()
                    .find_map(|(name, context)| (*context == id).then_some(name.as_str()))
                    .unwrap_or("<anonymous>");
                self.matcher_backend
                    .build_named(patterns, &format!("{}::{name}", syntax.name))
            },
        });
        if is_static {
            let _ = context.compiled.0.set(compiled.clone());
            return Ok(compiled);
        }
        let mut cache = self
            .pattern_contexts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if cache.len() > 8192 {
            cache.clear();
        }
        cache.insert(key, compiled.clone());
        Ok(compiled)
    }
}

fn file_reference_matches(reference: &str, syntax: &SyntaxReference) -> bool {
    let reference = reference.strip_prefix("Packages/").unwrap_or(reference);
    syntax.path == reference
        || syntax
            .path
            .strip_suffix(reference)
            .is_some_and(|prefix| prefix.ends_with('/'))
        || syntax.name == reference
}
impl SyntaxSet {
    pub(crate) fn resolve_reference(
        &self,
        reference: &ContextReference,
    ) -> Result<ContextId, ParsingError> {
        let (target, sub, with_escape) = match reference {
            ContextReference::Direct(id) => return Ok(*id),
            ContextReference::ByScope {
                scope,
                sub_context,
                with_escape,
            } => (self.find_syntax_by_scope(*scope), sub_context, *with_escape),
            ContextReference::File {
                name,
                sub_context,
                with_escape,
            } => (
                self.syntaxes
                    .iter()
                    .rev()
                    .find(|s| file_reference_matches(name, s)),
                sub_context,
                *with_escape,
            ),
            _ => return Err(ParsingError::UnresolvedContextReference(reference.clone())),
        };
        let target = target.or_else(|| {
            with_escape
                .then(|| self.find_syntax_by_name("Plain Text"))
                .flatten()
        });
        target
            .and_then(|s| s.context_ids().get(sub.as_deref().unwrap_or("main")))
            .copied()
            .ok_or_else(|| ParsingError::UnresolvedContextReference(reference.clone()))
    }
}

/// Give compiler-created contexts file-local identities before `extends` merges names.
fn namespace_anonymous_contexts(syntax: &mut SyntaxDefinition, file: usize) {
    let names: HashMap<_, _> = syntax
        .contexts
        .keys()
        .filter(|n| n.starts_with("#anon_"))
        .map(|name| (name.clone(), format!("#anon_file{file}_{name}")))
        .collect();
    let rename = |reference: &mut ContextReference| {
        if let ContextReference::Inline(name) | ContextReference::Named(name) = reference
            && let Some(new) = names.get(name)
        {
            *name = new.clone();
        }
    };
    for context in syntax.contexts.values_mut() {
        for pattern in &mut context.patterns {
            match pattern {
                Pattern::Include(reference) | Pattern::IncludeWithPrototype(reference) => {
                    rename(reference)
                }
                Pattern::Match(pattern) => {
                    let references = match &mut pattern.operation {
                        MatchOperation::Push { ctx_refs, .. }
                        | MatchOperation::Set { ctx_refs, .. } => Some(ctx_refs),
                        MatchOperation::Branch { alternatives, .. } => Some(alternatives),
                        MatchOperation::Embed { contexts, .. } => Some(contexts),
                        _ => None,
                    };
                    if let Some(references) = references {
                        for reference in references {
                            rename(reference);
                        }
                    }
                    if let Some(reference) = &mut pattern.with_prototype {
                        rename(reference);
                    }
                }
            }
        }
    }
    for (old, new) in names {
        let context = syntax
            .contexts
            .remove(&old)
            .expect("collected context name");
        syntax.contexts.insert(new, context);
    }
}
