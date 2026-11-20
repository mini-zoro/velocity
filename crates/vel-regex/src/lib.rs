//! Lazy, shareable Oniguruma matching with a replaceable ordered-pattern backend.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    hash::{Hash, Hasher},
    sync::{
        Arc, LazyLock, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

mod frontend;
mod multi;
pub use multi::{MultiDfa, MultiDfaConfig, MultiDfaStats};

/// A recoverable compilation failure. Broken patterns never match.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CompileError(pub String);
/// Aggregate regex diagnostics for this process.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// Lazy compilations attempted.
    pub compilations: u64,
    /// Invalid pattern compilations.
    pub compile_errors: u64,
    /// Searches stopped by Oniguruma resource limits.
    pub search_errors: u64,
}
static COMPILES: AtomicU64 = AtomicU64::new(0);
static COMPILE_ERRORS: AtomicU64 = AtomicU64::new(0);
static SEARCH_ERRORS: AtomicU64 = AtomicU64::new(0);
/// Reads monotonically increasing diagnostic counters.
pub fn metrics() -> Metrics {
    Metrics {
        compilations: COMPILES.load(Ordering::Relaxed),
        compile_errors: COMPILE_ERRORS.load(Ordering::Relaxed),
        search_errors: SEARCH_ERRORS.load(Ordering::Relaxed),
    }
}

/// Capture regions in byte offsets, including capture zero.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Region {
    spans: smallvec::SmallVec<[Option<(usize, usize)>; 4]>,
}
impl Region {
    /// Creates empty capture storage.
    pub fn new() -> Self {
        Self::default()
    }
    /// Creates captures supplied by an alternative backend; slot zero is the whole match.
    pub fn from_spans(spans: Vec<Option<(usize, usize)>>) -> Self {
        Self {
            spans: spans.into(),
        }
    }
    /// Gets a participating capture's span.
    pub fn pos(&self, index: usize) -> Option<(usize, usize)> {
        self.spans.get(index).copied().flatten()
    }
    /// Number of capture slots, including group zero.
    pub fn len(&self) -> usize {
        self.spans.len()
    }
    /// Whether there are no capture slots.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}
/// A compiled single pattern. Implementations must search the whole haystack from `start`.
pub trait CompiledPattern: fmt::Debug + Send + Sync {
    /// Number of capture slots including group zero, when known without a search.
    fn capture_slots(&self) -> Option<usize> {
        None
    }
    /// Returns a span; populate captures only when requested. A limit error is recoverable.
    fn search(
        &self,
        line: &str,
        start: usize,
        end: usize,
        captures: Option<&mut Region>,
    ) -> Result<Option<(usize, usize)>, CompileError>;
    /// Optionally copies the previous search's registers for this identical query.
    fn cached_captures(
        &self,
        _line: &str,
        _start: usize,
        _end: usize,
        _span: (usize, usize),
        _out: &mut Region,
    ) -> bool {
        false
    }
}
/// Compiles individual patterns. The syntax parser depends on this interface, never on Onig.
pub trait RegexBackend: fmt::Debug + Send + Sync {
    /// Compiles a pattern with optional empty-match suppression.
    fn compile(
        &self,
        source: &str,
        allow_empty: bool,
    ) -> Result<Arc<dyn CompiledPattern>, CompileError>;
}
/// Default backend with explicit retry and match-stack limits.
#[derive(Debug, Clone, Copy)]
pub struct OnigBackend {
    /// Maximum backtracking retries per search.
    pub retry_limit: u32,
    /// Maximum matching stack bytes.
    pub stack_limit: u32,
}
impl Default for OnigBackend {
    fn default() -> Self {
        Self {
            retry_limit: 10_000_000,
            stack_limit: 8 * 1024 * 1024,
        }
    }
}
#[derive(Debug)]
struct OnigPattern {
    regex: onig::Regex,
    limits: OnigBackend,
    capture_slots: usize,
}
struct Scratch {
    region: onig::Region,
    owner: usize,
    line: usize,
    len: usize,
    start: usize,
    end: usize,
    span: Option<(usize, usize)>,
}
thread_local! { static SCRATCH:RefCell<Scratch>=RefCell::new(Scratch{region:onig::Region::with_capacity(16),owner:0,line:0,len:0,start:0,end:0,span:None}); }
impl RegexBackend for OnigBackend {
    fn compile(
        &self,
        source: &str,
        allow_empty: bool,
    ) -> Result<Arc<dyn CompiledPattern>, CompileError> {
        let mut options = onig::RegexOptions::REGEX_OPTION_CAPTURE_GROUP;
        if !allow_empty {
            options |= onig::RegexOptions::REGEX_OPTION_FIND_NOT_EMPTY;
        }
        onig::Regex::with_options(source, options, onig::Syntax::default())
            .map(|regex| {
                Arc::new(OnigPattern {
                    capture_slots: regex.captures_len() + 1,
                    regex,
                    limits: *self,
                }) as Arc<dyn CompiledPattern>
            })
            .map_err(|e| CompileError(e.to_string()))
    }
}
impl CompiledPattern for OnigPattern {
    fn capture_slots(&self) -> Option<usize> {
        Some(self.capture_slots)
    }
    fn search(
        &self,
        line: &str,
        start: usize,
        end: usize,
        captures: Option<&mut Region>,
    ) -> Result<Option<(usize, usize)>, CompileError> {
        let mut params = onig::MatchParam::default();
        params.set_retry_limit_in_match(self.limits.retry_limit);
        params.set_match_stack_limit(self.limits.stack_limit);
        SCRATCH.with(|scratch| {
            let mut scratch = scratch.borrow_mut();
            scratch.owner = self as *const Self as usize;
            scratch.line = line.as_ptr() as usize;
            scratch.len = line.len();
            scratch.start = start;
            scratch.end = end;
            scratch.span = None;
            let region = &mut scratch.region;
            let result = self
                .regex
                .search_with_param(
                    line,
                    start,
                    end,
                    onig::SearchOptions::SEARCH_OPTION_NONE,
                    Some(region),
                    params,
                )
                .map_err(|e| CompileError(e.to_string()))?;
            if result.is_none() {
                return Ok(None);
            }
            let span = region.pos(0);
            if let Some(out) = captures {
                out.spans.clear();
                out.spans.extend((0..region.len()).map(|i| region.pos(i)));
            }
            scratch.span = span;
            Ok(span)
        })
    }
    fn cached_captures(
        &self,
        line: &str,
        start: usize,
        end: usize,
        span: (usize, usize),
        out: &mut Region,
    ) -> bool {
        SCRATCH.with(|scratch| {
            let scratch = scratch.borrow();
            if scratch.owner != self as *const Self as usize
                || scratch.line != line.as_ptr() as usize
                || scratch.len != line.len()
                || scratch.start != start
                || scratch.end != end
                || scratch.span != Some(span)
            {
                return false;
            }
            out.spans.clear();
            out.spans
                .extend((0..scratch.region.len()).map(|i| scratch.region.pos(i)));
            true
        })
    }
}
struct Inner {
    onig_semantics: bool,
    position_dependent: bool,
    prefilter: OnceLock<Option<Prefilter>>,
    source: String,
    backend: Arc<dyn RegexBackend>,
    empty: OnceLock<Result<Arc<dyn CompiledPattern>, CompileError>>,
    nonempty: OnceLock<Result<Arc<dyn CompiledPattern>, CompileError>>,
    candidate: OnceLock<Option<frontend::Candidate>>,
}
impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Regex").field(&self.source).finish()
    }
}
/// Compiled lazily once, shared by clones, `Send + Sync`.
#[derive(Clone, Debug)]
pub struct Regex(Arc<Inner>);
static POOL: LazyLock<Mutex<HashMap<String, Weak<Inner>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static DYNAMIC: LazyLock<Mutex<HashMap<String, Regex>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
impl Regex {
    /// Constructs a lazy pattern; duplicate sources share compiled objects.
    pub fn new(source: String) -> Self {
        let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(inner) = pool.get(&source).and_then(Weak::upgrade) {
            return Self(inner);
        }
        let regex = Self::make(source.clone(), Arc::new(OnigBackend::default()), true);
        pool.insert(source, Arc::downgrade(&regex.0));
        regex
    }
    /// Caches a runtime capture substitution. The bounded cache holds 4096 instantiations.
    pub fn new_unfiltered(source: String) -> Self {
        let mut pool = DYNAMIC.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = pool.get(&source) {
            return r.clone();
        }
        if pool.len() >= 4096 {
            pool.clear();
        }
        let regex = Self::new(source.clone());
        pool.insert(source, regex.clone());
        regex
    }
    /// Uses an application-supplied regex backend.
    pub fn with_backend(source: String, backend: Arc<dyn RegexBackend>) -> Self {
        Self::make(source, backend, false)
    }
    fn make(source: String, backend: Arc<dyn RegexBackend>, onig_semantics: bool) -> Self {
        let position_dependent = source.contains(r"\G") || source.contains(r"\K");
        Self(Arc::new(Inner {
            onig_semantics,
            position_dependent,
            prefilter: OnceLock::new(),
            source,
            backend,
            empty: OnceLock::new(),
            nonempty: OnceLock::new(),
            candidate: OnceLock::new(),
        }))
    }
    /// Original Oniguruma pattern source.
    pub fn regex_str(&self) -> &str {
        &self.0.source
    }
    /// Whether next-match caching must be relative to the query's start.
    pub fn position_dependent(&self) -> bool {
        self.0.position_dependent
    }

    fn compiled(&self, allow_empty: bool) -> &Result<Arc<dyn CompiledPattern>, CompileError> {
        let cell = if allow_empty {
            &self.0.empty
        } else {
            &self.0.nonempty
        };
        cell.get_or_init(|| {
            COMPILES.fetch_add(1, Ordering::Relaxed);
            let r = self.0.backend.compile(self.regex_str(), allow_empty);
            if r.is_err() {
                COMPILE_ERRORS.fetch_add(1, Ordering::Relaxed);
            }
            r
        })
    }
    /// Returns the lazy compilation result as a value.
    pub fn compile_error(&self) -> Option<&CompileError> {
        self.compiled(true).as_ref().err()
    }
    /// Explicit validation for callers that want it; loading syntaxes never needs this.
    pub fn try_compile(source: &str) -> Option<Box<dyn std::error::Error + Send + Sync>> {
        OnigBackend::default()
            .compile(source, true)
            .err()
            .map(|e| Box::new(e) as _)
    }
    /// Finds a match from an absolute byte position while preserving lookbehind and `\G`.
    pub fn search(
        &self,
        line: &str,
        start: usize,
        end: usize,
        region: Option<&mut Region>,
        allow_empty: bool,
    ) -> bool {
        self.find(line, start, end, region, allow_empty).is_some()
    }
    fn find(
        &self,
        line: &str,
        start: usize,
        end: usize,
        region: Option<&mut Region>,
        allow_empty: bool,
    ) -> Option<(usize, usize)> {
        if start > end
            || end > line.len()
            || !line.is_char_boundary(start)
            || !line.is_char_boundary(end)
        {
            return None;
        }
        if self
            .0
            .prefilter
            .get_or_init(|| Prefilter::extract(self.regex_str()))
            .as_ref()
            // Keep the full tail for a necessary-literal filter. The native
            // engine applies its own search-range semantics afterwards.
            .is_some_and(|p| !p.maybe(&line.as_bytes()[start..]))
        {
            #[cfg(any(debug_assertions, feature = "verify-prefilters"))]
            if let Ok(compiled) = self.compiled(allow_empty) {
                assert!(
                    !matches!(compiled.search(line, start, end, None), Ok(Some(_))),
                    "literal prefilter disagreement: {}",
                    self.regex_str()
                );
            }
            return None;
        }
        let compiled = self.compiled(allow_empty).as_ref().ok()?;
        match compiled.search(line, start, end, region) {
            Ok(r) => r,
            Err(_) => {
                SEARCH_ERRORS.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
    fn winner_captures(
        &self,
        line: &str,
        start: usize,
        span: (usize, usize),
        allow_empty: bool,
        out: &mut Region,
    ) -> bool {
        if let Ok(compiled) = self.compiled(allow_empty) {
            if self.0.onig_semantics && compiled.capture_slots() == Some(1) {
                // The ordered matcher already proved this span. A rule with
                // no groups has no other registers to recover.
                out.spans.clear();
                out.spans.push(Some(span));
                return true;
            }
            if compiled.cached_captures(line, start, line.len(), span, out) {
                return true;
            }
        }
        self.find(line, start, line.len(), Some(out), allow_empty)
            .is_some()
    }
    /// Whether any substring matches.
    pub fn is_match(&self, text: &str) -> bool {
        self.search(text, 0, text.len(), None, true)
    }
}
impl PartialEq for Regex {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.regex_str() == other.regex_str()
    }
}
impl Eq for Regex {}
impl Hash for Regex {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.regex_str().hash(h);
    }
}
impl Serialize for Regex {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.regex_str())
    }
}
impl<'de> Deserialize<'de> for Regex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::new(String::deserialize(d)?))
    }
}

/// Winning match in an ordered pattern set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Zero-based rule priority.
    pub pattern: usize,
    /// Whole-match starting byte.
    pub start: usize,
    /// Whole-match ending byte.
    pub end: usize,
    /// Captures of the winner only.
    pub captures: Region,
}
type NextMatches = HashMap<(usize, bool), (usize, Option<(usize, usize)>), fnv::FnvBuildHasher>;

/// Per-line next-match cache. Call [`Self::reset_line`] before each new line.
#[derive(Default)]
pub struct SearchCache {
    backend_state: Option<Box<dyn std::any::Any + Send>>,
    entries: NextMatches,
    haystack: Option<(usize, usize)>,
    ascii: bool,
    generation: u64,
}
impl fmt::Debug for SearchCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchCache")
            .field("entries", &self.entries.len())
            .field("has_backend_state", &self.backend_state.is_some())
            .finish()
    }
}
impl SearchCache {
    /// Reuses backend-owned scratch memory. A backend may keep per-context caches inside T.
    /// Scratch survives reset_line; the backend manages any haystack-dependent entries.
    pub fn backend_state<T: std::any::Any + Send>(&mut self, init: impl FnOnce() -> T) -> &mut T {
        if self.backend_state.as_ref().is_none_or(|s| !s.is::<T>()) {
            self.backend_state = Some(Box::new(init()));
        }
        self.backend_state
            .as_mut()
            .and_then(|s| s.downcast_mut::<T>())
            .expect("backend scratch type was checked")
    }
    /// Starts a new line, retaining allocated capacity.
    pub fn reset_line(&mut self) {
        self.entries.clear();
        self.haystack = None;
        self.generation = self.generation.wrapping_add(1);
    }
    /// Invalidates cached results after backtracking or a changed search boundary.
    pub fn clear(&mut self) {
        self.reset_line();
    }
    fn bind(&mut self, line: &str) {
        let key = (line.as_ptr() as usize, line.len());
        if self.haystack != Some(key) {
            self.entries.clear();
            self.haystack = Some(key);
            self.ascii = line.is_ascii();
            self.generation = self.generation.wrapping_add(1);
        }
    }
}
/// Ordered-pattern search interface for a future multi-pattern DFA backend.
pub trait PatternMatcher: fmt::Debug + Send + Sync {
    /// Earliest start wins; equal starts use the lowest rule index. Captures are winner-only.
    fn find_earliest(
        &self,
        line: &str,
        start: usize,
        cache: &mut SearchCache,
        excluded: &[usize],
    ) -> Option<Match>;
}
/// Ordered context patterns, independent of the syntax parser's stack interpreter.
#[derive(Debug, Clone)]
pub struct PatternSet {
    matcher: Arc<dyn PatternMatcher>,
}
impl PatternSet {
    /// Constructs the default Oniguruma pattern set.
    pub fn new(patterns: Vec<Regex>) -> Self {
        Self::from_specs(
            patterns
                .into_iter()
                .map(|regex| PatternSpec {
                    regex,
                    allow_empty: true,
                })
                .collect(),
        )
    }
    /// Builds patterns with per-rule empty-match policies.
    pub fn from_specs(patterns: Vec<PatternSpec>) -> Self {
        Self {
            matcher: Arc::new(OnigSet(patterns)),
        }
    }
    /// Installs a multi-pattern backend without changing the parser API.
    pub fn with_matcher(matcher: Arc<dyn PatternMatcher>) -> Self {
        Self { matcher }
    }
    /// Searches at or after `start`, preserving the entire line for lookbehind.
    pub fn find_earliest(
        &self,
        line: &str,
        start: usize,
        cache: &mut SearchCache,
    ) -> Option<Match> {
        self.matcher.find_earliest(line, start, cache, &[])
    }
    /// Searches after disabling rules invalidated by branch or loop bookkeeping.
    pub fn find_earliest_excluding(
        &self,
        line: &str,
        start: usize,
        cache: &mut SearchCache,
        excluded: &[usize],
    ) -> Option<Match> {
        self.matcher.find_earliest(line, start, cache, excluded)
    }
}
#[derive(Debug)]
struct OnigSet(Vec<PatternSpec>);
impl PatternMatcher for OnigSet {
    fn find_earliest(
        &self,
        line: &str,
        start: usize,
        cache: &mut SearchCache,
        excluded: &[usize],
    ) -> Option<Match> {
        self.find_candidates(line, start, cache, excluded, 0..self.0.len())
    }
}
impl OnigSet {
    fn find_candidates(
        &self,
        line: &str,
        start: usize,
        cache: &mut SearchCache,
        excluded: &[usize],
        candidates: impl Iterator<Item = usize>,
    ) -> Option<Match> {
        let (pattern, span) =
            self.find_candidate_spans(line, start, cache, excluded, candidates)?;
        self.capture_match(line, start, pattern, span)
    }
    fn find_candidate_spans(
        &self,
        line: &str,
        start: usize,
        cache: &mut SearchCache,
        excluded: &[usize],
        candidates: impl Iterator<Item = usize>,
    ) -> Option<(usize, (usize, usize))> {
        cache.bind(line);
        let mut best = None;
        for index in candidates {
            let spec = &self.0[index];
            if excluded.contains(&index) {
                continue;
            }
            let regex = &spec.regex;
            let key = (Arc::as_ptr(&regex.0) as usize, spec.allow_empty);
            let span = match cache.entries.get(&key) {
                Some(&(from, hit))
                    if !regex.position_dependent()
                        && start >= from
                        && hit.is_none_or(|h| h.0 >= start) =>
                {
                    hit
                }
                _ => {
                    let span = regex.find(line, start, line.len(), None, spec.allow_empty);
                    cache.entries.insert(key, (start, span));
                    span
                }
            };
            if let Some(span) = span {
                if best.is_none_or(|(_, b): (usize, (usize, usize))| span.0 < b.0) {
                    best = Some((index, span));
                }
                if span.0 == start {
                    break;
                }
            }
        }
        best
    }
    fn capture_match(
        &self,
        line: &str,
        start: usize,
        pattern: usize,
        (begin, end): (usize, usize),
    ) -> Option<Match> {
        let mut captures = Region::new();
        if !self.0[pattern].regex.winner_captures(
            line,
            start,
            (begin, end),
            self.0[pattern].allow_empty,
            &mut captures,
        ) {
            return None;
        }
        Some(Match {
            pattern,
            start: begin,
            end,
            captures,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn priority_cache_g_and_lookbehind() {
        let set = PatternSet::new(
            [r"(?<=x)a", r"\Ga", r"a"]
                .map(|s| Regex::new(s.into()))
                .into(),
        );
        let mut c = SearchCache::default();
        let m = set.find_earliest("xaa", 1, &mut c).unwrap();
        assert_eq!((m.pattern, m.start, m.end), (0, 1, 2));
        let m = set.find_earliest("xaa", 2, &mut c).unwrap();
        assert_eq!((m.pattern, m.start, m.end), (1, 2, 3));
        c.reset_line();
        assert_eq!(set.find_earliest("a", 0, &mut c).unwrap().pattern, 1);
    }
    #[test]
    fn invalid_and_limit_errors_are_values() {
        assert!(Regex::new("(".into()).compile_error().is_some());
        let r = Regex::with_backend(
            r"(a+)+$".into(),
            Arc::new(OnigBackend {
                retry_limit: 20,
                stack_limit: 1024,
            }),
        );
        let before = metrics().search_errors;
        assert!(!r.is_match("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!"));
        assert!(metrics().search_errors > before);
        let backend = OnigBackend {
            retry_limit: 20,
            stack_limit: 1024,
        };
        let set = backend.build(vec![PatternSpec {
            regex: Regex::new(r"(a+)+$".into()),
            allow_empty: true,
        }]);
        let before = metrics().search_errors;
        assert!(
            set.find_earliest("aaaaaaaaaaa!", 0, &mut SearchCache::default())
                .is_none()
        );
        assert!(
            metrics().search_errors > before,
            "context factory must forward configured limits"
        );
        fn shareable<T: Send + Sync>() {}
        shareable::<Regex>();
    }

    #[test]
    fn winner_preserves_whole_match_and_optional_groups() {
        let line = "xá a";
        for (rule, group) in [("á", None), ("(á)(z)?", Some((1, 3)))] {
            let set = PatternSet::new(vec![Regex::new(rule.into()), Regex::new("a".into())]);
            // Another matching rule overwrites the native capture scratch;
            // the first winner still needs its exact byte-offset registers.
            let hit = set
                .find_earliest(line, 0, &mut SearchCache::default())
                .unwrap();
            assert_eq!((hit.pattern, hit.start, hit.end), (0, 1, 3));
            assert_eq!(hit.captures.pos(0), Some((1, 3)));
            assert_eq!(hit.captures.pos(1), group);
            assert_eq!(hit.captures.pos(2), None);
        }
    }
}

/// One ordered rule and its empty-match policy.
#[derive(Debug, Clone)]
pub struct PatternSpec {
    /// Oniguruma-compatible source, compiled lazily.
    pub regex: Regex,
    /// Stack-changing rules may use zero-width matches.
    pub allow_empty: bool,
}
/// Builds an ordered context matcher. A MultiDfa implementation replaces this factory.
pub trait MatcherBackend: fmt::Debug + Send + Sync {
    /// Compiles a context's ordered patterns, retaining winner-only capture semantics.
    fn build(&self, patterns: Vec<PatternSpec>) -> PatternSet;
    /// Builds a context with an optional diagnostic label; default factories ignore it.
    fn build_named(&self, patterns: Vec<PatternSpec>, _name: &str) -> PatternSet {
        self.build(patterns)
    }
}
impl MatcherBackend for OnigBackend {
    fn build(&self, patterns: Vec<PatternSpec>) -> PatternSet {
        let defaults = Self::default();
        let patterns = if self.retry_limit == defaults.retry_limit
            && self.stack_limit == defaults.stack_limit
        {
            patterns
        } else {
            patterns
                .into_iter()
                .map(|pattern| PatternSpec {
                    regex: Regex::with_backend(
                        pattern.regex.regex_str().to_owned(),
                        Arc::new(*self),
                    ),
                    allow_empty: pattern.allow_empty,
                })
                .collect()
        };
        PatternSet::from_specs(patterns)
    }
}

// Literal extraction follows syntect PR #705 (MIT). Unlike that implementation,
// full case folding and unsupported dialect constructs stay unfiltered.
#[derive(Debug)]
enum Prefilter {
    One(memchr::memmem::Finder<'static>),
    Any(Box<aho_corasick::AhoCorasick>),
}
impl Prefilter {
    fn maybe(&self, bytes: &[u8]) -> bool {
        match self {
            Self::One(f) => f.find(bytes).is_some(),
            Self::Any(a) => a.is_match(bytes),
        }
    }
    fn extract(source: &str) -> Option<Self> {
        if source.contains(r"\u")
            || source.split("(?").skip(1).any(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
                    .any(|c| c == 'i')
            })
        {
            return None;
        }
        // Onig x-mode retains spaces inside classes. Derive only necessary
        // literals from conservative dialect supersets, including positive
        // lookaheads. A required concat member need not be a match prefix: a
        // suffix such as `for` can reject an otherwise expensive identifier scan.
        let mut best = None;
        for hir in frontend::prefilter_hirs(source) {
            Self::required_literals(&hir, &mut best);
        }
        let (_, literals) = best?;
        if let [literal] = literals.as_slice() {
            return Some(Self::One(memchr::memmem::Finder::new(literal).into_owned()));
        }
        Some(Self::Any(Box::new(
            aho_corasick::AhoCorasick::new(&literals).ok()?,
        )))
    }

    fn required_literals(hir: &regex_syntax::hir::Hir, best: &mut Option<(usize, Vec<Vec<u8>>)>) {
        use regex_syntax::hir::{
            HirKind,
            literal::{ExtractKind, Extractor},
        };
        for kind in [ExtractKind::Prefix, ExtractKind::Suffix] {
            let seq = Extractor::new().kind(kind).limit_total(64).extract(hir);
            if let Some(literals) = seq.literals()
                && !literals.is_empty()
                && let Some(min) = seq.min_literal_len()
                && min > 0
                && best.as_ref().is_none_or(|(old, needles)| {
                    min > *old || (min == *old && literals.len() < needles.len())
                })
            {
                *best = Some((
                    min,
                    literals.iter().map(|l| l.as_bytes().to_vec()).collect(),
                ));
            }
        }
        match hir.kind() {
            HirKind::Concat(parts) => {
                for part in parts {
                    Self::required_literals(part, best);
                }
            }
            HirKind::Capture(capture) => Self::required_literals(&capture.sub, best),
            HirKind::Repetition(repetition) if repetition.min > 0 => {
                Self::required_literals(&repetition.sub, best)
            }
            _ => {}
        }
    }
}

/// Escapes runtime capture text, including whitespace in extended-mode patterns.
pub fn escape_literal(text: &str) -> String {
    let escaped = regex_syntax::escape(text);
    if text.chars().any(char::is_whitespace) {
        format!("(?-x:{escaped})")
    } else {
        escaped
    }
}

#[cfg(test)]
mod backend_scratch_tests {
    use super::*;
    #[test]
    fn external_backends_can_supply_captures_and_reuse_scratch() {
        let captures = Region::from_spans(vec![Some((1, 3)), None, Some((2, 3))]);
        assert_eq!(captures.pos(2), Some((2, 3)));
        let mut cache = SearchCache::default();
        cache.backend_state(Vec::<u8>::new).push(42);
        cache.reset_line();
        assert_eq!(cache.backend_state(Vec::<u8>::new).as_slice(), &[42]);
    }
}
