//! Completion sources, scope-aware ranking and cancellable snapshot queries.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
mod markup;
mod worker;
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};
use vel_buffer::ChangeId;
use vel_highlight::ScopeProvider;
use vel_index::{Index, SymbolKind, fuzzy_score};
use vel_scope::Selector;
use vel_snippet::{Snippet, SnippetFile};
use vel_text::Text;
pub use worker::CompletionService;
/// Completion resource/settings error.
#[derive(Debug, thiserror::Error)]
#[error("invalid completion resource: {0}")]
pub struct Error(pub String);
/// How the insertion string is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionFormat {
    /// Insert literally.
    Text,
    /// Expand using vel-snippet.
    Snippet,
}
/// Public completion kind category, independent of renderer icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindId {
    /// Unspecified/custom.
    Ambiguous,
    /// Callable.
    Function,
    /// Language keyword.
    Keyword,
    /// Markup.
    Markup,
    /// Module/namespace.
    Namespace,
    /// Navigation item.
    Navigation,
    /// Snippet.
    Snippet,
    /// Data type.
    Type,
    /// Variable/property.
    Variable,
}
/// Kind metadata, including custom letter and tooltip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kind {
    /// Icon color/category.
    pub category: KindId,
    /// Optional Unicode icon letter.
    pub letter: String,
    /// Tooltip/description.
    pub name: String,
}
impl Default for Kind {
    fn default() -> Self {
        Self {
            category: KindId::Ambiguous,
            letter: String::new(),
            name: String::new(),
        }
    }
}
/// Completion row ready for presentation and insertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// Match/display trigger without a legacy tab annotation suffix.
    pub trigger: String,
    /// Short right-side annotation.
    pub annotation: String,
    /// Text or inline snippet inserted after replacing each list replacement range.
    pub completion: String,
    /// Interpretation of `completion`.
    pub completion_format: CompletionFormat,
    /// Public category/custom icon metadata.
    pub kind: Kind,
    /// Minihtml detail text; rendering/sanitizing belongs to the UI.
    pub details: String,
}
/// A versioned query result. Empty lists can represent trigger suppression or stale scopes.
#[derive(Debug, Clone)]
pub struct CompletionList {
    /// View identity supplied by the caller.
    pub view_id: u64,
    /// Buffer snapshot identity.
    pub version: ChangeId,
    /// Async request generation, zero for synchronous queries.
    pub generation: u64,
    /// One original UTF-8 word-prefix range per caret, in caller order.
    pub replace_ranges: Vec<Range<usize>>,
    /// Ranked completion rows.
    pub items: Vec<CompletionItem>,
    /// Whether Enter should insert a newline and Tab should commit.
    pub commit_on_tab: bool,
    /// Whether the UI should wrap navigation at list boundaries.
    pub cycle: bool,
    /// True if cancellation interrupted source collection/ranking.
    pub cancelled: bool,
}
/// Invocation context for automatic trigger filtering.
#[derive(Debug, Clone, Copy)]
pub enum Invocation {
    /// Explicit completion command ignores automatic trigger/size/delay restrictions.
    Manual,
    /// Tab completion (exact snippet triggers are prioritized).
    Tab,
    /// Automatic completion following an input character and idle delay.
    Typing {
        /// Most recently entered scalar.
        character: char,
        /// Time since last input.
        idle: Duration,
    },
}
/// Additional character trigger overriding only the main automatic scope selector.
#[derive(Debug, Clone)]
pub struct CompletionTrigger {
    /// Selector at every caret.
    pub selector: String,
    /// Characters immediately to the left of the caret that trigger completion.
    pub characters: String,
    /// Require EOF or a whitespace character immediately after the caret.
    pub rhs_empty: bool,
}
/// Reordering policies documented at <https://www.sublimetext.com/docs/completions.html#settings>.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PreserveOrder {
    /// Rank by textual match.
    None,
    /// Also consider source likelihood and proximity.
    #[default]
    Some,
    /// Keep source order.
    Strict,
}
impl PreserveOrder {
    /// Reads the documented string values; unknown/legacy values use normal ranking.
    pub fn from_setting(value: &str) -> Self {
        match value {
            "some" => Self::Some,
            "strict" => Self::Strict,
            "none" => Self::None,
            _ => Self::Some,
        }
    }
}
/// Editor settings consumed by the engine, with UI policies returned on CompletionList.
#[derive(Debug, Clone)]
pub struct CompletionSettings {
    /// Enable popup while typing.
    pub auto_complete: bool,
    /// Opt out of the built-in HTML and CSS rows; other providers are unaffected.
    pub disable_default_completions: bool,
    /// Override the selector controlling eligibility for the markup provider.
    pub default_completions_selector: Option<String>,
    /// Main automatic scope selector.
    pub auto_complete_selector: String,
    /// Explicit character/scope triggers.
    pub auto_complete_triggers: Vec<CompletionTrigger>,
    /// Required input idle delay.
    pub auto_complete_delay: Duration,
    /// Maximum buffer bytes for automatic popup.
    pub auto_complete_size_limit: usize,
    /// Complete on Tab instead of Enter.
    pub auto_complete_commit_on_tab: bool,
    /// Permit automatic completion inside active snippet fields when completing on Tab.
    pub auto_complete_with_fields: bool,
    /// Wrap UI completion navigation.
    pub auto_complete_cycle: bool,
    /// Suppress automatic suggestions without a word prefix or explicit trigger.
    pub auto_complete_when_likely: bool,
    /// Include project words and sequence/punctuation suggestions.
    pub auto_complete_use_index: bool,
    /// Use committed completion recency.
    pub auto_complete_use_history: bool,
    /// Keep stable source/resource order, retaining exact tab-trigger priority.
    pub auto_complete_preserve_order: PreserveOrder,
    /// Append punctuation inferred from indexed occurrences, never strip resource contents.
    pub auto_complete_trailing_symbols: bool,
    /// Append spaces supported by observed buffer occurrences.
    pub auto_complete_trailing_spaces: bool,
    /// Include snippet resources.
    pub auto_complete_include_snippets: bool,
    /// Include snippet resources in automatic typing queries.
    pub auto_complete_include_snippets_when_typing: bool,
    /// Maximum rows returned.
    pub max_results: usize,
}
impl Default for CompletionSettings {
    fn default() -> Self {
        Self {
            auto_complete: true,
            disable_default_completions: false,
            default_completions_selector: None,
            // Public selector example: https://www.sublimetext.com/docs/completions.html#settings
            auto_complete_selector: "meta.tag, source - comment - string.quoted.double.block - string.quoted.single.block - string.unquoted.heredoc".into(),
            auto_complete_triggers: Vec::new(),
            auto_complete_delay: Duration::from_millis(50),
            auto_complete_size_limit: 4 * 1024 * 1024,
            auto_complete_commit_on_tab: false,
            auto_complete_with_fields: false,
            auto_complete_cycle: false,
            auto_complete_when_likely: true,
            auto_complete_use_index: true,
            // The public completions/settings pages and ST4 changelog describe these
            // switches, but do not establish different shipped defaults; retain ours.
            // https://www.sublimetext.com/docs/completions.html#settings
            auto_complete_use_history: true,
            auto_complete_preserve_order: PreserveOrder::Some,
            // The public docs define these as inferred suffixes, not edits to authored snippets.
            // https://www.sublimetext.com/download (ST4 Auto Complete: symbols default off).
            auto_complete_trailing_symbols: false,
            // No default is specified by the public docs; preserve our no-space behavior.
            auto_complete_trailing_spaces: false,
            auto_complete_include_snippets: true,
            // https://www.sublimetext.com/docs/completions.html#settings documents false
            // as opting out of automatic (but not explicitly invoked) snippet suggestions.
            auto_complete_include_snippets_when_typing: true,
            max_results: 64,
        }
    }
}
#[derive(Debug, Clone, Default)]
struct Word {
    positions: Vec<usize>,
    followed_by_space: usize,
}
/// Prepared current/other-buffer word source. Build on a snapshot publisher/worker, then reuse
/// it across queries. Position lists support logarithmic nearest-caret scoring.
#[derive(Debug, Clone, Default)]
pub struct BufferWords {
    words: BTreeMap<String, Word>,
}
impl BufferWords {
    /// Tokenizes complete rope lines without flattening the buffer.
    pub fn new(text: &Text) -> Self {
        let mut words: BTreeMap<String, Word> = BTreeMap::new();
        let mut offset = 0;
        for line in text.lines() {
            let line = line.to_string();
            let mut start = None;
            for (at, c) in line
                .char_indices()
                .chain(std::iter::once((line.len(), ' ')))
            {
                if c.is_alphanumeric() || c == '_' {
                    start.get_or_insert(at);
                } else if let Some(start) = start.take() {
                    let word = words.entry(line[start..at].into()).or_default();
                    word.positions.push(offset + start);
                    word.followed_by_space += usize::from(line[at..].starts_with(' '));
                }
            }
            offset += line.len() + 1;
        }
        Self { words }
    }
    /// Number of distinct prepared words.
    pub fn len(&self) -> usize {
        self.words.len()
    }
    /// Whether this source contains no words.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}
/// Immutable view inputs. Reuse an `Arc<BufferWords>` when only the caret changes; refresh it after
/// text transactions on an editor worker. Settings are captured with the query.
#[derive(Debug, Clone)]
pub struct ViewSnapshot {
    /// Stable editor view identity, used to exclude it from other-open-buffer words.
    pub view_id: u64,
    /// Buffer transaction identity.
    pub version: ChangeId,
    /// O(1) rope snapshot.
    pub text: Text,
    /// Prepared words belonging to this exact text snapshot.
    pub words: Arc<BufferWords>,
    /// Effective editor settings.
    pub settings: CompletionSettings,
    /// Automatic/manual invocation state.
    pub invocation: Invocation,
    /// Whether snippet fields currently own Tab navigation.
    pub fields_active: bool,
}
impl ViewSnapshot {
    /// Convenience constructor; tokenization occurs here, outside the timed query path.
    pub fn new(view_id: u64, version: ChangeId, text: Text) -> Self {
        let words = Arc::new(BufferWords::new(&text));
        Self {
            view_id,
            version,
            text,
            words,
            settings: CompletionSettings::default(),
            invocation: Invocation::Manual,
            fields_active: false,
        }
    }
}
#[derive(Clone, Debug)]
struct Resource {
    selector: Selector,
    item: CompletionItem,
    snippet_resource: bool,
}

/// A shipped snippet whose trigger and selector were inventoried at build time.
#[derive(Debug)]
pub struct ShippedSnippet {
    /// Caret selector.
    pub scope: &'static str,
    /// Exact Tab trigger, if present.
    pub trigger: &'static str,
    /// Palette and completion label.
    pub description: &'static str,
    xml: &'static [u8],
    parsed: std::sync::OnceLock<Option<SnippetFile>>,
}
impl ShippedSnippet {
    /// Registers metadata without parsing or touching the XML body.
    pub fn new(
        scope: &'static str,
        trigger: &'static str,
        description: &'static str,
        xml: &'static [u8],
    ) -> Self {
        Self {
            scope,
            trigger,
            description,
            xml,
            parsed: Default::default(),
        }
    }
    /// Parses once, on an eligible request.
    pub fn snippet(&self) -> Option<&SnippetFile> {
        self.parsed
            .get_or_init(|| SnippetFile::parse(std::str::from_utf8(self.xml).ok()?).ok())
            .as_ref()
    }
    /// Whether a request has materialized the body.
    pub fn loaded(&self) -> bool {
        self.parsed.get().is_some()
    }
}

#[derive(Clone, Debug)]
enum ShippedSource {
    Completions(&'static [u8]),
    Snippet(Arc<ShippedSnippet>),
}
#[derive(Debug)]
struct Deferred {
    source: ShippedSource,
    items: std::sync::OnceLock<Vec<Resource>>,
}
impl Deferred {
    fn items(&self) -> &[Resource] {
        self.items.get_or_init(|| {
            let mut engine = CompletionEngine::new();
            let result = match &self.source {
                ShippedSource::Completions(bytes) => std::str::from_utf8(bytes)
                    .ok()
                    .map(|source| engine.add_shipped_completions(source)),
                ShippedSource::Snippet(snippet) => {
                    snippet.snippet().map(|s| engine.add_snippet(s.clone()))
                }
            };
            if let Some(Err(e)) = result {
                eprintln!("Invalid bundled completion resource: {e}");
            }
            Arc::unwrap_or_clone(engine.resources)
        })
    }
}

#[derive(Clone, Debug, Default)]
struct ScopeGroup {
    selectors: std::sync::OnceLock<Vec<Selector>>,
    sources: Vec<Arc<Deferred>>,
}

#[derive(Debug, Default)]
struct History {
    clock: u64,
    used: HashMap<String, u64>,
}
/// Prepared sources shared by synchronous or asynchronous queries. Resource order is stable.
#[derive(Debug, Default)]
pub struct CompletionEngine {
    resources: Arc<Vec<Resource>>,
    shipped: Arc<BTreeMap<Vec<&'static str>, ScopeGroup>>,
    open_buffers: RwLock<BTreeMap<u64, Arc<BufferWords>>>,
    index: Option<Arc<RwLock<Index>>>,
    history: Mutex<History>,
}
impl CompletionEngine {
    /// Empty engine; words in the supplied view are always a source.
    pub fn new() -> Self {
        Self::default()
    }
    /// Shares parsed resources, with independent word sources and history.
    pub fn with_shared_resources(&self, index: Option<Arc<RwLock<Index>>>) -> Self {
        Self {
            resources: self.resources.clone(),
            shipped: self.shipped.clone(),
            index,
            ..Self::default()
        }
    }
    fn add_deferred(&mut self, selectors: Vec<&'static str>, source: ShippedSource) {
        Arc::make_mut(&mut self.shipped)
            .entry(selectors)
            .or_default()
            .sources
            .push(Arc::new(Deferred {
                source,
                items: Default::default(),
            }));
    }
    /// Registers a bundled completion file, gated by its file and per-item selectors.
    pub fn defer_shipped_completions(&mut self, selectors: &[&'static str], source: &'static [u8]) {
        self.add_deferred(selectors.to_vec(), ShippedSource::Completions(source));
    }
    /// Registers a bundled snippet with the same lazy body used by exact Tab and the palette.
    pub fn defer_shipped_snippet(&mut self, snippet: Arc<ShippedSnippet>) {
        if !snippet.trigger.is_empty() {
            self.add_deferred(vec![snippet.scope], ShippedSource::Snippet(snippet));
        }
    }
    fn resources_at<'a>(
        &'a self,
        stacks: &'a [&[vel_scope::Scope]],
        include_snippets: bool,
    ) -> impl Iterator<Item = &'a Resource> {
        self.resources.iter().chain(
            self.shipped
                .iter()
                .filter(move |(scopes, group)| {
                    group
                        .selectors
                        .get_or_init(|| {
                            scopes
                                .iter()
                                .filter_map(|s| Selector::parse(s).ok())
                                .collect()
                        })
                        .iter()
                        .any(|selector| stacks.iter().all(|stack| selector.matches(stack)))
                })
                .flat_map(|(_, group)| &group.sources)
                .filter(move |entry| {
                    include_snippets || !matches!(entry.source, ShippedSource::Snippet(_))
                })
                .flat_map(|entry| entry.items()),
        )
    }
    /// Installs the project's live index; disabled per query by auto_complete_use_index.
    pub fn set_index(&mut self, index: Arc<RwLock<Index>>) {
        self.index = Some(index);
    }
    /// Publishes a prepared open-buffer word source (including unsaved edits).
    pub fn set_open_buffer(&self, view_id: u64, words: Arc<BufferWords>) {
        self.open_buffers.write().unwrap().insert(view_id, words);
    }
    /// Removes closed buffer words.
    pub fn remove_open_buffer(&self, view_id: u64) {
        self.open_buffers.write().unwrap().remove(&view_id);
    }
    /// Loads .sublime-completions JSON/JSONC strings or rich items, validating selectors/snippets.
    pub fn add_completions(&mut self, json: &str) -> Result<(), Error> {
        self.add_completion_file(json, false)
    }
    /// Loads bundled language completion files, accepting rich items with only a trigger.
    pub fn add_shipped_completions(&mut self, json: &str) -> Result<(), Error> {
        self.add_completion_file(json, true)
    }
    fn add_completion_file(&mut self, json: &str, shipped: bool) -> Result<(), Error> {
        let value: serde_json::Value =
            serde_json::from_str(&jsonc(json)?).map_err(|e| Error(e.to_string()))?;
        let scope = value.get("scope").and_then(|v| v.as_str()).unwrap_or("");
        let values = value
            .get("completions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error("missing completions array".into()))?;
        let mut resources = Vec::new();
        for value in values {
            let (trigger, completion, format) = if let Some(text) = value.as_str() {
                // A legacy tab introduces annotation, not inserted document text.
                let literal = text.split_once('\t').map_or(text, |(trigger, _)| trigger);
                (text, literal, CompletionFormat::Text)
            } else {
                let trigger = value
                    .get("trigger")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error("missing trigger".into()))?;
                match value.get("contents") {
                    Some(contents) => (
                        trigger,
                        contents
                            .as_str()
                            .ok_or_else(|| Error("contents must be a string".into()))?,
                        CompletionFormat::Snippet,
                    ),
                    // https://www.sublimetext.com/docs/completions.html requires
                    // contents in rich entries. The shipped packages also use
                    // an abbreviated entry that means insert the literal trigger.
                    None if shipped => (
                        trigger,
                        trigger
                            .split_once('\t')
                            .map_or(trigger, |(trigger, _)| trigger),
                        CompletionFormat::Text,
                    ),
                    None => return Err(Error("missing contents".into())),
                }
            };
            // A bundled completion body is parsed only when selected. User
            // resources still get validation and file diagnostics on reload.
            if format == CompletionFormat::Snippet && !shipped {
                Snippet::parse(completion).map_err(|e| Error(e.to_string()))?;
            }
            let (trigger, legacy) = trigger.split_once('\t').unwrap_or((trigger, ""));
            resources.push(Resource {
                selector: Selector::parse(
                    value.get("scope").and_then(|v| v.as_str()).unwrap_or(scope),
                )
                .map_err(|e| Error(e.to_string()))?,
                item: CompletionItem {
                    trigger: trigger.into(),
                    annotation: value
                        .get("annotation")
                        .and_then(|v| v.as_str())
                        .unwrap_or(legacy)
                        .into(),
                    completion: completion.into(),
                    completion_format: format,
                    kind: parse_kind(value.get("kind"))?,
                    details: value
                        .get("details")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                },
                snippet_resource: false,
            });
        }
        Arc::make_mut(&mut self.resources).extend(resources);
        Ok(())
    }
    /// Adds a parsed XML snippet as a scoped exact-Tab/interactive completion source.
    pub fn add_snippet(&mut self, snippet: SnippetFile) -> Result<(), Error> {
        if snippet.tab_trigger.is_empty() {
            return Ok(());
        }
        Arc::make_mut(&mut self.resources).push(Resource {
            selector: Selector::parse(&snippet.scope).map_err(|e| Error(e.to_string()))?,
            item: CompletionItem {
                trigger: snippet.tab_trigger,
                annotation: snippet.description,
                completion: snippet.content,
                completion_format: CompletionFormat::Snippet,
                kind: Kind {
                    category: KindId::Snippet,
                    letter: "s".into(),
                    name: "Snippet".into(),
                },
                details: String::new(),
            },
            snippet_resource: true,
        });
        Ok(())
    }
    /// Records a successful commit for subsequent history-aware ranking.
    pub fn record_commit(&self, item: &CompletionItem) {
        let mut history = self.history.lock().unwrap();
        history.clock += 1;
        let clock = history.clock;
        history.used.insert(item.trigger.clone(), clock);
        if history.used.len() > 1024 {
            let cutoff = clock.saturating_sub(1024);
            history.used.retain(|_, last| *last > cutoff);
        }
    }
    /// Queries all sources with compatible scopes at every caret. Unavailable/stale scopes
    /// suppress results; this function never invokes a syntax parser.
    pub fn query(
        &self,
        view: &ViewSnapshot,
        carets: &[usize],
        scopes: &dyn ScopeProvider,
    ) -> CompletionList {
        self.query_cancel(view, carets, scopes, || false)
    }
    pub(crate) fn query_cancel(
        &self,
        view: &ViewSnapshot,
        carets: &[usize],
        scopes: &dyn ScopeProvider,
        cancel: impl Fn() -> bool,
    ) -> CompletionList {
        let settings = &view.settings;
        let mut list = CompletionList {
            view_id: view.view_id,
            version: view.version,
            generation: 0,
            replace_ranges: Vec::new(),
            items: Vec::new(),
            commit_on_tab: settings.auto_complete_commit_on_tab,
            cycle: settings.auto_complete_cycle,
            cancelled: false,
        };
        if scopes.version() != view.version || carets.is_empty() || settings.max_results == 0 {
            return list;
        }
        let mut prefixes = Vec::new();
        let mut stacks = Vec::new();
        let mut rhs_empty = true;
        let check_rhs = matches!(view.invocation, Invocation::Typing {character, ..} if settings.auto_complete_triggers.iter().any(|trigger| trigger.rhs_empty && (trigger.characters.is_empty() || trigger.characters.contains(character))));
        for &caret in carets {
            if caret > view.text.len_bytes() || !view.text.is_char_boundary(caret) {
                return list;
            }
            // Build 4072: automatic selectors use the just-typed character.
            // https://www.sublimetext.com/dev (4072). Reuse the query's one scope
            // lookup per caret: triggers add no scope lookups to ordinary typing.
            let point = if matches!(view.invocation, Invocation::Typing { .. }) {
                view.text.floor_char_boundary(caret.saturating_sub(1))
            } else {
                caret
            };
            let Some(stack) = scopes.scopes_at(point) else {
                return list;
            };
            stacks.push(stack);
            let row = view.text.byte_to_line(caret);
            let offset = view.text.line_to_byte(row);
            let line = view.text.line(row).to_string();
            let column = caret - offset;
            if check_rhs {
                // https://docs.sublimetext.io/guide/extensibility/completions.html#auto-completion-triggers-and-selector
                rhs_empty &= line[column..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace);
            }
            let tab_trigger = matches!(view.invocation, Invocation::Tab)
                .then(|| {
                    self.resources_at(&[stack], true)
                        .filter(|resource| {
                            resource.snippet_resource
                                || resource.item.completion_format == CompletionFormat::Snippet
                        })
                        .filter(|resource| resource.selector.matches(stack))
                        .filter_map(|resource| {
                            vel_snippet::tab_trigger_range(
                                &view.text,
                                caret,
                                &resource.item.trigger,
                            )
                            .map(|r| r.start)
                        })
                        .min()
                })
                .flatten();
            let start = line[..column]
                .char_indices()
                .rev()
                .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
                .last()
                .map_or(column, |p| p.0);
            let start = tab_trigger.unwrap_or(offset + start);
            list.replace_ranges.push(start..caret);
            prefixes.push(view.text.slice(start..caret).to_string());
        }
        let matches = |selector: &Selector| stacks.iter().all(|stack| selector.matches(stack));
        // This is a worker snapshot: both the text and scopes above are from the same
        // revision. Do not ask an editor/main-thread provider to parse missing scopes.
        let mut markup_slot = None;
        let slot_at = |caret: usize, stack: &[vel_scope::Scope], selector: &Selector| {
            // A '<' immediately after the prefix belongs to the *next* tag.
            // Consult the prefix's scope for text expansion, but keep the usual
            // scope for tag attributes, CSS, and resource/snippet selectors.
            if !matches!(view.invocation, Invocation::Typing { .. })
                && caret > 0
                && caret < view.text.len_bytes()
                && view.text.byte_at(caret) == b'<'
                && let Some(left) = scopes.scopes_at(view.text.floor_char_boundary(caret - 1))
                && let Some(
                    found @ (
                        markup::Slot::Tag {
                            after_open: false, ..
                        },
                        _,
                    ),
                ) = markup::slot(&view.text, caret, left, selector)
            {
                return Some(found);
            }
            markup::slot(&view.text, caret, stack, selector)
        };
        if !settings.disable_default_completions
            && let Some(selector) =
                markup::selector(settings.default_completions_selector.as_deref())
            && let Some((slot, start)) = slot_at(carets[0], stacks[0], &selector)
        {
            let mut starts = vec![start];
            let compatible = carets[1..].iter().zip(&stacks[1..]).all(|(&caret, stack)| {
                if let Some((other, start)) = slot_at(caret, stack, &selector)
                    && other == slot
                {
                    starts.push(start);
                    true
                } else {
                    false
                }
            });
            if compatible {
                for (((range, prefix), start), &caret) in list
                    .replace_ranges
                    .iter_mut()
                    .zip(&mut prefixes)
                    .zip(starts)
                    .zip(carets)
                {
                    *range = start..caret;
                    *prefix = view.text.slice(range.clone()).to_string();
                }
                markup_slot = Some(slot);
            }
        }
        if let Invocation::Typing { character, idle } = view.invocation {
            if !settings.auto_complete
                || idle < settings.auto_complete_delay
                || view.text.len_bytes() > settings.auto_complete_size_limit
                || settings.auto_complete_commit_on_tab
                    && view.fields_active
                    && !settings.auto_complete_with_fields
            {
                return list;
            }
            let trigger = settings.auto_complete_triggers.iter().any(|trigger| {
                (trigger.characters.is_empty() || trigger.characters.contains(character))
                    && (!trigger.rhs_empty || rhs_empty)
                    && Selector::parse(&trigger.selector).is_ok_and(|selector| matches(&selector))
            });
            if !trigger
                && !Selector::parse(&settings.auto_complete_selector)
                    .is_ok_and(|selector| matches(&selector))
            {
                return list;
            }
            if settings.auto_complete_when_likely
                && !trigger
                && prefixes.iter().any(String::is_empty)
            {
                return list;
            }
        }
        let primary = &prefixes[0];
        let (clock, used) = {
            let history = self.history.lock().unwrap();
            (history.clock, history.used.clone())
        };
        let history_score = |trigger: &str| {
            if settings.auto_complete_use_history {
                used.get(trigger)
                    .map_or(0, |last| 100 - clock.saturating_sub(*last).min(100) as i64)
            } else {
                0
            }
        };
        // Decide once per query, not once per candidate × caret.
        // Only an immediate line ending permits a new space. Existing whitespace is kept
        // without duplicating it; punctuation and word continuations never get a space.
        let append_space = settings.auto_complete_trailing_spaces
            && carets.iter().all(|&at| {
                view.text
                    .slice(at..view.text.len_bytes())
                    .chars()
                    .next()
                    .is_none_or(|c| matches!(c, '\n' | '\r'))
            });
        let mut ranked: Vec<(bool, i64, usize, CompletionItem, bool)> = Vec::new();
        let mut order = 0usize;
        let mut add = |item: CompletionItem, bonus: i64, exact_trigger: bool, space: bool| {
            let score = prefixes
                .iter()
                .map(|prefix| fuzzy_score(&item.trigger, prefix))
                .collect::<Option<Vec<_>>>()
                .and_then(|values| values.into_iter().min());
            if let Some(score) = score {
                ranked.push((
                    exact_trigger && prefixes.iter().all(|prefix| prefix == &item.trigger),
                    score
                        + if settings.auto_complete_preserve_order == PreserveOrder::Some {
                            bonus
                        } else {
                            0
                        }
                        + history_score(&item.trigger),
                    order,
                    item,
                    space,
                ));
            }
            order += 1;
        };
        if let Some(slot) = markup_slot {
            for item in slot.items(primary, view.invocation) {
                add(item, 120, true, false);
            }
        }
        let include_snippets = settings.auto_complete_include_snippets
            && (!matches!(view.invocation, Invocation::Typing { .. })
                || settings.auto_complete_include_snippets_when_typing);
        for (i, resource) in self.resources_at(&stacks, include_snippets).enumerate() {
            if i % 256 == 0 && cancel() {
                list.cancelled = true;
                return list;
            }
            if resource.snippet_resource && !include_snippets {
                continue;
            }
            if matches(&resource.selector) {
                add(
                    resource.item.clone(),
                    60,
                    resource.snippet_resource
                        || resource.item.completion_format == CompletionFormat::Snippet,
                    false,
                );
            }
        }
        let mut add_words = |words: &BufferWords, current: bool| {
            for (i, (word, info)) in words.words.iter().enumerate() {
                if i % 256 == 0 && cancel() {
                    return false;
                }
                if word == primary {
                    continue;
                }
                let at = info.positions.partition_point(|p| *p < carets[0]);
                let distance = info
                    .positions
                    .get(at)
                    .into_iter()
                    .chain(at.checked_sub(1).and_then(|i| info.positions.get(i)))
                    .map(|&p| p.abs_diff(carets[0]))
                    .min()
                    .unwrap_or(usize::MAX);
                let proximity = if current {
                    40 - distance.min(4000) as i64 / 100
                } else {
                    0
                };
                // Original inference policy: at least three observations, and at least
                // three quarters followed by spaces. One assignment is not evidence.
                let space = info.followed_by_space >= 3
                    && info.followed_by_space * 4 >= info.positions.len() * 3;
                add(
                    word_item(word.clone()),
                    proximity + (info.positions.len().max(1).ilog2() as i64) * 3,
                    false,
                    space,
                );
            }
            true
        };
        if !add_words(&view.words, true) {
            list.cancelled = true;
            return list;
        }
        for (id, words) in self.open_buffers.read().unwrap().iter() {
            if *id != view.view_id && !add_words(words, false) {
                list.cancelled = true;
                return list;
            }
        }
        if settings.auto_complete_use_index
            && let Some(index) = &self.index
        {
            let preceding = preceding(&view.text, list.replace_ranges[0].start);
            let context: Vec<_> = preceding.iter().map(String::as_str).collect();
            let guard = index.read().unwrap();
            let mut queries: Vec<_> = prefixes.iter().map(String::as_str).collect();
            queries.sort_unstable();
            queries.dedup();
            let Some(words) =
                guard.suggest_words_for(&queries, &context, settings.max_results, &cancel)
            else {
                list.cancelled = true;
                return list;
            };
            for word in words {
                let mut item = word_item(word.word);
                if word.kind != SymbolKind::Other {
                    item.kind.category = word.kind.into();
                    item.kind.name = format!("{:?}", word.kind);
                    item.kind.letter = match word.kind {
                        SymbolKind::Function => "f",
                        SymbolKind::Type => "t",
                        SymbolKind::Namespace => "a",
                        SymbolKind::Markup => "m",
                        _ => "v",
                    }
                    .into();
                }
                if settings.auto_complete_trailing_symbols
                    && word.context_count > 0
                    && !word.trailing.is_empty()
                {
                    let mut prefix = word.trailing.len();
                    for &caret in carets {
                        let end = view.text.floor_char_boundary(
                            caret
                                .saturating_add(word.trailing.len())
                                .min(view.text.len_bytes()),
                        );
                        let after = view.text.slice(caret..end).to_string();
                        let keep = word
                            .trailing
                            .char_indices()
                            .map(|(at, _)| at)
                            .find(|&at| after.starts_with(&word.trailing[at..]))
                            .unwrap_or(word.trailing.len());
                        prefix = prefix.min(keep);
                    }
                    item.completion.push_str(&word.trailing[..prefix]);
                    item.annotation = "context".into();
                }
                let quality = fuzzy_score(&item.trigger, primary).unwrap_or(0);
                add(item, word.score - quality + 10, false, false);
            }
        }
        ranked.sort_unstable_by(|a, b| {
            b.0.cmp(&a.0).then_with(|| {
                if settings.auto_complete_preserve_order == PreserveOrder::Strict {
                    a.2.cmp(&b.2)
                } else {
                    b.1.cmp(&a.1).then(a.2.cmp(&b.2))
                }
            })
        });
        let mut seen = std::collections::HashSet::new();
        list.items = ranked
            .into_iter()
            .filter_map(|(_, _, _, mut item, space)| {
                // Only ranked rows considered for display reach this point; right-side
                // eligibility was computed once above. Deduplicate the final contents.
                if append_space && space {
                    item.completion.push(' ');
                }
                seen.insert((
                    item.trigger.clone(),
                    item.completion.clone(),
                    item.completion_format as u8,
                ))
                .then_some(item)
            })
            .take(settings.max_results)
            .collect();
        if cancel() {
            list.items.clear();
            list.cancelled = true;
        }
        list
    }
}
fn word_item(word: String) -> CompletionItem {
    CompletionItem {
        trigger: word.clone(),
        annotation: String::new(),
        completion: word,
        completion_format: CompletionFormat::Text,
        kind: Kind {
            category: KindId::Variable,
            letter: "v".into(),
            name: "Word".into(),
        },
        details: String::new(),
    }
}
fn preceding(text: &Text, point: usize) -> Vec<String> {
    let row = text.byte_to_line(point);
    let line = text.slice(text.line_to_byte(row)..point).to_string();
    line.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(str::to_string)
        .collect()
}
fn parse_kind(value: Option<&serde_json::Value>) -> Result<Kind, Error> {
    let Some(value) = value else {
        return Ok(Kind::default());
    };
    let (name, letter, description) = if let Some(name) = value.as_str() {
        (name, None, None)
    } else if let Some(values) = value.as_array().filter(|values| values.len() == 3) {
        (
            values[0]
                .as_str()
                .ok_or_else(|| Error("invalid kind".into()))?,
            Some(
                values[1]
                    .as_str()
                    .ok_or_else(|| Error("kind letter must be a string".into()))?,
            ),
            Some(
                values[2]
                    .as_str()
                    .ok_or_else(|| Error("kind description must be a string".into()))?,
            ),
        )
    } else {
        return Err(Error("kind must be a category or three strings".into()));
    };
    let (category, default_letter) = match name {
        "ambiguous" => (KindId::Ambiguous, ""),
        "function" => (KindId::Function, "f"),
        "keyword" => (KindId::Keyword, "k"),
        "markup" => (KindId::Markup, "m"),
        // https://www.sublimetext.com/docs/completions.html#kind-info
        "namespace" => (KindId::Namespace, "a"),
        "navigation" => (KindId::Navigation, "n"),
        "snippet" => (KindId::Snippet, "s"),
        "type" => (KindId::Type, "t"),
        "variable" => (KindId::Variable, "v"),
        _ => return Err(Error(format!("unknown kind {name}"))),
    };
    if letter.is_some_and(|letter| letter.chars().count() != 1) {
        return Err(Error("custom kind letter must be one scalar".into()));
    }
    Ok(Kind {
        category,
        letter: letter.unwrap_or(default_letter).into(),
        name: description.unwrap_or(name).into(),
    })
}
impl From<SymbolKind> for KindId {
    fn from(kind: SymbolKind) -> Self {
        match kind {
            SymbolKind::Function => Self::Function,
            SymbolKind::Type => Self::Type,
            SymbolKind::Namespace => Self::Namespace,
            SymbolKind::Variable => Self::Variable,
            SymbolKind::Markup => Self::Markup,
            SymbolKind::Other => Self::Ambiguous,
        }
    }
}
fn jsonc(input: &str) -> Result<String, Error> {
    let mut bytes = input.as_bytes().to_vec();
    let mut at = 0;
    let mut string = false;
    while at < bytes.len() {
        if string {
            if bytes[at] == b'\\' {
                at += 2;
                continue;
            }
            if bytes[at] == b'"' {
                string = false;
            }
        } else if bytes[at] == b'"' {
            string = true;
        } else if bytes[at..].starts_with(b"//") {
            while at < bytes.len() && bytes[at] != b'\n' {
                bytes[at] = b' ';
                at += 1;
            }
            continue;
        } else if bytes[at..].starts_with(b"/*") {
            bytes[at] = b' ';
            bytes[at + 1] = b' ';
            at += 2;
            while at + 1 < bytes.len() && !bytes[at..].starts_with(b"*/") {
                bytes[at] = b' ';
                at += 1;
            }
            if at + 1 < bytes.len() {
                bytes[at] = b' ';
                bytes[at + 1] = b' ';
                at += 2;
            } else {
                return Err(Error("unterminated JSONC block comment".into()));
            }
            continue;
        }
        at += 1;
    }
    string = false;
    at = 0;
    while at < bytes.len() {
        if string {
            if bytes[at] == b'\\' {
                at += 2;
                continue;
            }
            if bytes[at] == b'"' {
                string = false;
            }
        } else if bytes[at] == b'"' {
            string = true;
        } else if bytes[at] == b',' {
            let mut next = at + 1;
            while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                next += 1;
            }
            if bytes.get(next).is_some_and(|b| matches!(b, b']' | b'}')) {
                bytes[at] = b' ';
            }
        }
        at += 1;
    }
    String::from_utf8(bytes).map_err(|error| Error(error.to_string()))
}
