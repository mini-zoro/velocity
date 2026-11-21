//! Versioned incremental highlighting. The UI uses nonblocking requests and discards stale batches.
#![forbid(unsafe_code)]
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use vel_buffer::{Bias, ChangeId, ChangeSet};
use vel_color_scheme::{ColorScheme, Highlighter, Style};
use vel_scope::{ScopeStack, ScopeStackOp};
use vel_syntax::{
    MetadataSet, ParseState, SharedLineOps, SharedScopeOperations, SyntaxReference, SyntaxSet,
};
mod scopes;
mod viewport;
use scopes::ScopeInterner;
pub use scopes::{
    ScopeLines, ScopeProvider, ScopeRun, ScopeSnapshot, ScopeStackId, ScopeStacks, ScopedLine,
    SymbolList,
};
use vel_text::Text;
pub use viewport::{ViewportHighlighter, ViewportOptions, ViewportProgress};

/// Maximum synchronous catch-up time; a single regex search is cooperative, not preemptible.
pub const VISIBLE_BUDGET: Duration = Duration::from_millis(2);
/// A half-open UTF-8 byte span relative to its line (including any trailing LF).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleSpan {
    /// Line-local byte range.
    pub range: Range<usize>,
    /// Resolved colors and decorations.
    pub style: Style,
}
/// One requested line, exact or explicitly approximate.
#[derive(Debug, Clone)]
pub struct StyledLine {
    /// Zero-based document line.
    pub line: usize,
    /// Shared spans, safe to hand to the renderer without copying their contents.
    pub spans: Arc<[StyleSpan]>,
    /// False if default styling was used while the parser catches up.
    pub exact: bool,
}
/// Results are always tagged with their snapshot version; apply only matching versions.
#[derive(Debug, Clone)]
pub struct HighlightBatch {
    /// Buffer change ID used by these spans.
    pub version: ChangeId,
    /// Scheme generation; discard batches older than the latest scheme change.
    pub scheme_revision: u64,
    /// Clipped requested line interval.
    pub lines: Range<usize>,
    /// One entry per line in the clipped interval.
    pub highlighted: Vec<StyledLine>,
    /// Scope output from the very same parse, with matching version and line interval.
    pub scopes: ScopeSnapshot,
    /// True only when every returned line is exact.
    pub complete: bool,
}
/// Diagnostics for convergence, branch repair, and budget accounting.
#[derive(Debug, Clone, Copy, Default)]
pub struct HighlightStats {
    /// Lines actually interpreted since construction or the most recent edit.
    pub parsed_lines: usize,
    /// Number of parses that converged with a cached suffix.
    pub convergences: usize,
    /// Earliest corrected line from branch replay, if any.
    pub rewind_from: Option<usize>,
    /// Number of branch replay events, including repeated repairs of the same line.
    pub rewinds: usize,
}
/// Nonblocking observation of background completion for the current snapshot.
#[derive(Debug, Clone, Copy)]
pub struct HighlightProgress {
    /// Snapshot identity.
    pub version: ChangeId,
    /// Palette generation.
    pub scheme_revision: u64,
    /// Contiguous validated lines.
    pub parsed_lines: usize,
    /// Total lines, including an empty trailing line.
    pub total_lines: usize,
}
/// Errors leave a failed snapshot unavailable; callers can keep rendering approximate text.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A parser or scope-stack operation failed.
    #[error("highlight parse: {0}")]
    Parse(String),
    /// Snapshot version or change-set length did not match the supplied transaction.
    #[error("highlight snapshot mismatch: {0}")]
    Snapshot(&'static str),
    /// The asynchronous worker is no longer available.
    #[error("highlight worker disconnected")]
    Disconnected,
}
#[derive(Debug, Clone)]
struct LineRecord {
    ops: Arc<[(usize, ScopeStackOp)]>,
    // If there is no saved clear history, end_stack is the whole checkpoint.
    scopes: Option<Arc<ScopeStack>>,
    checkpoint: Option<ParseState>,
    spans: Arc<[StyleSpan]>,
    runs: Arc<[ScopeRun]>,
    end_stack: ScopeStackId,
    has_symbols: bool,
}
/// Instrumented component wall times; available only with the `profile` feature.
/// Regex time is part of `parse`; use an instrumented matcher backend to split it.
#[cfg(feature = "profile")]
#[derive(Debug, Clone, Copy, Default)]
pub struct HighlightProfile {
    /// Rope lookup and copying into the reusable line buffer.
    pub input: Duration,
    /// Parser, including regex searches, context transitions and transition memoization.
    pub parse: Duration,
    /// Scope operations, selectors, styling, scope runs and branch correction rendering.
    pub render: Duration,
    /// Operation/stack sharing, dense checkpoints, convergence and record publication.
    pub checkpoint: Duration,
    /// Complete rendered-line reuse hits, excluding uncached shapes and token hashes.
    pub render_hits: u64,
    /// Renderer calls including branch corrections and palette restyling.
    pub render_queries: u64,
    /// Number of retained render shapes (bounded independently of source text).
    pub render_entries: usize,
    /// Number of new style spans built on render cache misses.
    pub built_spans: usize,
    /// Distinct style arrays retained by the visual shape cache.
    pub visual_shapes: usize,
    /// Lines that reused style arrays despite a complete render cache miss.
    pub visual_hits: usize,
    /// Distinct active scope stacks stored in the interner.
    pub interned_stacks: usize,
    /// New owned end-of-line scope stacks, including branch repairs.
    pub checkpoint_scope_copies: usize,
    /// Line records retaining an additional owned scope stack.
    pub retained_scope_snapshots: usize,
}
/// Stateful engine, also usable without threads in tests or editor-owned scheduling.
/// Dense checkpoints (N=1) include every visible line and permit single-line convergence.
#[derive(Debug)]
pub struct DocumentHighlighter {
    text: Text,
    set: Arc<SyntaxSet>,
    syntax_path: String,
    version: ChangeId,
    scheme_revision: u64,
    resolver: Highlighter,
    initial: ParseState,
    state: ParseState,
    scopes: ScopeCursor,
    records: Vec<Option<LineRecord>>,
    frontier: usize,
    reusable_to: usize,
    must_parse_to: usize,
    stats: HighlightStats,
    line_scratch: String,
    parse_scratch: SharedLineOps,
    scope_cache: HashSet<Arc<ScopeStack>>,
    scope_interner: ScopeInterner,
    render_cache: RenderCache,
    #[cfg(feature = "profile")]
    profile: HighlightProfile,
}
/// Detects syntax by explicit name/path/scope/token, then whole filename, compound extension,
/// then first-line expression. Hidden extensions participate; no filesystem reads are needed.
pub fn detect_syntax<'a>(
    set: &'a SyntaxSet,
    path: &str,
    first_line: &str,
    user_override: Option<&str>,
) -> Option<&'a SyntaxReference> {
    if let Some(value) = user_override {
        // https://www.sublimetext.com/download: set_file_type accepts scope: syntax names.
        if let Some(scope) = value.strip_prefix("scope:") {
            return set.find_syntax_by_scope(scope.parse().ok()?);
        }
        return set
            .find_syntax_by_path(value)
            .or_else(|| set.find_syntax_by_name(value))
            .or_else(|| {
                set.syntaxes()
                    .iter()
                    .find(|s| s.scope.build_string() == value)
            })
            .or_else(|| set.find_syntax_by_token(value));
    }
    let filename = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if let Some(s) = set.find_syntax_by_extension(filename) {
        return Some(s);
    }
    for (at, c) in filename.char_indices() {
        if c == '.'
            && let Some(s) = set.find_syntax_by_extension(&filename[at + 1..])
        {
            return Some(s);
        }
    }
    set.find_syntax_by_first_line(first_line.trim_start_matches('\u{feff}'))
}
impl DocumentHighlighter {
    /// Creates an unparsed snapshot. Syntax must belong to `set`.
    pub fn new(
        text: Text,
        version: ChangeId,
        set: Arc<SyntaxSet>,
        syntax: &SyntaxReference,
        scheme: Arc<ColorScheme>,
    ) -> Self {
        let initial = ParseState::new(set.clone(), syntax);
        let records = vec![None; text.len_lines()];
        Self {
            text,
            set,
            syntax_path: syntax.path.clone(),
            version,
            scheme_revision: 0,
            resolver: Highlighter::new(scheme),
            state: initial.clone(),
            initial,
            scopes: ScopeCursor::default(),
            records,
            frontier: 0,
            reusable_to: 0,
            must_parse_to: 0,
            stats: HighlightStats::default(),
            line_scratch: String::new(),
            parse_scratch: SharedLineOps::default(),
            scope_cache: HashSet::new(),
            scope_interner: ScopeInterner::new(Arc::new(MetadataSet::new())),
            render_cache: RenderCache::default(),
            #[cfg(feature = "profile")]
            profile: HighlightProfile::default(),
        }
    }
    /// Selects package/User preferences before parsing. Existing checkpoints are invalidated.
    pub fn with_metadata(mut self, metadata: Arc<MetadataSet>) -> Self {
        self.scope_interner = ScopeInterner::new(metadata);
        self.render_cache = RenderCache::default();
        self.records.fill(None);
        self.frontier = 0;
        self.reusable_to = 0;
        self.state = self.initial.clone();
        self.scopes = ScopeCursor::default();
        self
    }
    /// Scopes for validated lines only, without any parser work.
    pub fn scope_snapshot(&self, lines: Range<usize>) -> ScopeSnapshot {
        let end = lines.end.min(self.text.len_lines());
        let start = lines.start.min(end);
        ScopeSnapshot {
            version: self.version,
            text: self.text.clone(),
            range: start..end,
            stacks: self.scope_interner.stacks.clone(),
            lines: (start..end)
                .map(|line| {
                    let record = self.records[line].as_ref().filter(|_| line < self.frontier);
                    ScopedLine {
                        line,
                        runs: record.map_or_else(|| Arc::from([]), |r| r.runs.clone()),
                        end_stack: record.map_or(ScopeStackId(0), |r| r.end_stack),
                        exact: record.is_some(),
                    }
                })
                .collect(),
        }
    }
    /// Query one validated line; no synchronous catch-up.
    pub fn scope_at(&self, line: usize, byte: usize) -> Option<ScopeStackId> {
        self.scope_snapshot(line..line.saturating_add(1))
            .scope_at(line, byte)
    }
    /// Maximal equal-stack region within validated scope output, using absolute byte coordinates.
    pub fn extract_scope(&self, point: usize) -> Option<Range<usize>> {
        self.scope_snapshot(0..self.frontier).extract_scope(point)
    }
    /// Selector specificity at an absolute byte point; no parser work.
    pub fn match_selector(
        &self,
        point: usize,
        selector: &vel_scope::Selector,
    ) -> Option<vel_scope::MatchScore> {
        let row = (point <= self.text.len_bytes()).then(|| self.text.byte_to_line(point))?;
        self.scope_snapshot(row..row + 1)
            .match_selector(point, selector)
    }
    /// Metadata-selected document/index symbols, maintained with the line records.
    pub fn symbols(&self) -> Result<SymbolList, Error> {
        Ok(SymbolList {
            version: self.version,
            complete: self.frontier == self.records.len(),
            symbols: self.scope_interner.symbols(
                &self.text,
                self.records[..self.frontier]
                    .iter()
                    .enumerate()
                    .filter_map(|(row, record)| {
                        record
                            .as_ref()
                            .filter(|r| r.has_symbols)
                            .map(|r| (row, r.runs.as_ref()))
                    }),
            )?,
        })
    }
    /// Current snapshot identity.
    pub fn version(&self) -> ChangeId {
        self.version
    }
    /// Shared text snapshot; cloning is O(1).
    pub fn text(&self) -> &Text {
        &self.text
    }
    /// Immutable syntax set used by this document.
    pub fn syntax_set(&self) -> &Arc<SyntaxSet> {
        &self.set
    }
    /// Assigned syntax resource path.
    pub fn syntax_path(&self) -> &str {
        &self.syntax_path
    }
    /// Number of contiguous, validated lines.
    pub fn parsed_through(&self) -> usize {
        self.frontier
    }
    /// Most recent edit/catch-up statistics.
    pub fn stats(&self) -> HighlightStats {
        self.stats
    }
    /// Component timings since construction. Timer overhead is included; do not
    /// enable this feature when measuring production throughput.
    #[cfg(feature = "profile")]
    pub fn profile(&self) -> HighlightProfile {
        HighlightProfile {
            render_hits: self.render_cache.hits,
            render_queries: self.render_cache.queries,
            render_entries: self.render_cache.entries.len(),
            built_spans: self.render_cache.visual.built_spans,
            visual_shapes: self.render_cache.visual.shapes.len(),
            visual_hits: self.render_cache.visual.hits,
            interned_stacks: self.scope_interner.stacks.0.len(),
            retained_scope_snapshots: self
                .records
                .iter()
                .flatten()
                .filter(|r| r.scopes.is_some())
                .count(),
            ..self.profile
        }
    }
    /// Swaps the color scheme and recolors parsed operations without running regexes again.
    pub fn set_color_scheme(&mut self, scheme: Arc<ColorScheme>) -> Result<(), Error> {
        self.scheme_revision += 1;
        self.resolver = Highlighter::new(scheme);
        self.scope_interner.invalidate_styles();
        self.render_cache = RenderCache::default();
        // Unvalidated suffix spans still use the previous palette. They must be reparsed
        // or recolored before reuse, even if their parsing checkpoints converge.
        self.reusable_to = self.frontier;
        let mut scopes = ScopeCursor::default();
        for row in 0..self.frontier {
            let Some(record) = &self.records[row] else {
                break;
            };
            let ops = record.ops.clone();
            let line = self
                .text
                .slice(complete_line_range(&self.text, row))
                .to_string();
            let rendered = self.render_cache.render(
                &line,
                &ops,
                &mut scopes,
                &mut self.resolver,
                &mut self.scope_interner,
            )?;
            let record = self.records[row].as_mut().unwrap();
            record.spans = rendered.spans;
            record.runs = rendered.runs;
            record.end_stack = rendered.end_stack;
            record.has_symbols = self.scope_interner.has_symbols(&record.runs);
            record.scopes = scopes
                .stack
                .has_pending_clears()
                .then(|| Arc::new(scopes.stack.clone()));
        }
        Ok(())
    }
    /// Applies an already committed vel-buffer transaction using old-text change coordinates.
    /// `after` must be the new snapshot and `version` must increase. Never flatten the document.
    pub fn apply_change(
        &mut self,
        after: Text,
        version: ChangeId,
        changes: &ChangeSet,
    ) -> Result<(), Error> {
        if version <= self.version {
            return Err(Error::Snapshot("version must increase"));
        }
        let mut expected = self.text.len_bytes();
        for change in changes.changes() {
            if change.old_range.end > self.text.len_bytes()
                || !self.text.is_char_boundary(change.old_range.start)
                || !self.text.is_char_boundary(change.old_range.end)
            {
                return Err(Error::Snapshot("invalid old-text range"));
            }
            expected = expected
                .checked_sub(change.old_range.len())
                .and_then(|n| n.checked_add(change.new_len))
                .ok_or(Error::Snapshot("invalid replacement lengths"))?;
        }
        if expected != after.len_bytes() {
            return Err(Error::Snapshot("new text length does not match change set"));
        }
        let Some(first_change) = changes.changes().first() else {
            if after != self.text {
                return Err(Error::Snapshot("empty change set changed text"));
            }
            self.text = after;
            self.version = version;
            return Ok(());
        };
        let last = changes.changes().last().unwrap();
        let first = self.text.byte_to_line(first_change.old_range.start);
        let old_end = (self.text.byte_to_line(last.old_range.end) + 1).min(self.records.len());
        let suffix = self.records.len() - old_end;
        let new_end = after
            .len_lines()
            .checked_sub(suffix)
            .ok_or(Error::Snapshot("line count mismatch"))?;
        if new_end < first {
            return Err(Error::Snapshot("line mapping mismatch"));
        }
        let mapped_last = changes.transform_point(last.old_range.end, Bias::Right);
        if mapped_last > after.len_bytes() {
            return Err(Error::Snapshot("change mapping outside new text"));
        }
        let old_frontier = self.frontier;
        let old_reusable = self.reusable_to;
        let old_need = if self.must_parse_to > self.frontier {
            self.must_parse_to
        } else {
            0
        };
        let comparison = self.records[old_end - 1].clone();
        self.records
            .splice(first..old_end, std::iter::repeat_n(None, new_end - first));
        let reusable = old_frontier.max(old_reusable);
        self.reusable_to = if reusable >= old_end {
            reusable - old_end + new_end
        } else {
            first.min(reusable)
        };
        if new_end > first {
            self.records[new_end - 1] = comparison;
        }
        let mapped_need = if old_need >= old_end {
            old_need - old_end + new_end
        } else if old_need > first {
            new_end
        } else {
            old_need
        };
        self.must_parse_to = new_end.max(mapped_need);
        let mut resume = first.min(self.frontier);
        while resume > 0
            && self.records[resume - 1]
                .as_ref()
                .and_then(|r| r.checkpoint.as_ref())
                .is_none()
        {
            resume -= 1;
        }
        if resume == 0 {
            self.state = self.initial.clone();
            self.scopes = ScopeCursor::default();
        } else {
            let record = self.records[resume - 1].as_ref().unwrap();
            self.state = record.checkpoint.clone().unwrap();
            self.scopes = ScopeCursor::from_record(record, &self.scope_interner.stacks);
            if !self.state.resume_at(resume) {
                return Err(Error::Snapshot("cannot relocate speculative prefix"));
            }
        }
        self.frontier = resume;
        self.text = after;
        self.version = version;
        self.stats = HighlightStats::default();
        Ok(())
    }
    /// Parses towards a requested viewport until the cooperative deadline. Missing lines use
    /// base styling, never stale offsets. Call again or let HighlightService fill them later.
    pub fn request_lines(
        &mut self,
        version: ChangeId,
        lines: Range<usize>,
        budget: Duration,
    ) -> Result<HighlightBatch, Error> {
        if version != self.version {
            return Err(Error::Snapshot("requested version is stale"));
        }
        let end = lines.end.min(self.text.len_lines());
        self.advance_until(end, Instant::now().checked_add(budget))?;
        Ok(self.batch(lines))
    }
    /// Performs bounded background work and returns whether the whole file is exact.
    pub fn advance(&mut self, budget: Duration) -> Result<bool, Error> {
        self.advance_until(self.text.len_lines(), Instant::now().checked_add(budget))?;
        Ok(self.frontier == self.text.len_lines())
    }
    /// Parses all remaining lines, intended for background jobs and benchmark harnesses.
    pub fn finish(&mut self) -> Result<(), Error> {
        self.advance_until(self.text.len_lines(), None)
    }
    fn advance_until(&mut self, end: usize, deadline: Option<Instant>) -> Result<(), Error> {
        while self.frontier < end {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            self.parse_next()?;
        }
        Ok(())
    }
    fn parse_next(&mut self) -> Result<(), Error> {
        #[cfg(feature = "profile")]
        let phase = Instant::now();
        let row = self.frontier;
        self.line_scratch.clear();
        for chunk in self
            .text
            .slice(complete_line_range(&self.text, row))
            .chunks()
        {
            self.line_scratch.push_str(chunk);
        }
        #[cfg(feature = "profile")]
        {
            self.profile.input += phase.elapsed();
        }
        #[cfg(feature = "profile")]
        let phase = Instant::now();
        self.state
            .parse_line_shared(&self.line_scratch, &mut self.parse_scratch)
            .map_err(|e| Error::Parse(e.to_string()))?;
        #[cfg(feature = "profile")]
        {
            self.profile.parse += phase.elapsed();
        }
        #[cfg(feature = "profile")]
        let phase = Instant::now();
        let output = &mut self.parse_scratch;
        self.stats.parsed_lines += 1;
        let mut rewind = output.rewind().map(|r| r.from_line);
        for (n, ops) in output.drain_corrections() {
            if n >= row {
                continue;
            }
            if let Some(record) = self.records.get_mut(n).and_then(Option::as_mut) {
                record.ops = ops.into();
                record.checkpoint = None;
                rewind = Some(rewind.map_or(n, |old| old.min(n)));
            }
        }
        if let Some(from) = rewind {
            self.stats.rewinds += 1;
            self.stats.rewind_from = Some(self.stats.rewind_from.map_or(from, |old| old.min(from)));
            let mut scopes = if from == 0 {
                ScopeCursor::default()
            } else {
                let anchor = self.records[from - 1]
                    .as_ref()
                    .ok_or_else(|| Error::Parse("missing rewind anchor".into()))?;
                ScopeCursor::from_record(anchor, &self.scope_interner.stacks)
            };
            for n in from..row {
                let record = self.records[n]
                    .as_ref()
                    .ok_or_else(|| Error::Parse("missing branch history".into()))?;
                let ops = record.ops.clone();
                let line = self
                    .text
                    .slice(complete_line_range(&self.text, n))
                    .to_string();
                let rendered = self.render_cache.render(
                    &line,
                    &ops,
                    &mut scopes,
                    &mut self.resolver,
                    &mut self.scope_interner,
                )?;
                let record = self.records[n].as_mut().unwrap();
                record.spans = rendered.spans;
                record.runs = rendered.runs;
                record.end_stack = rendered.end_stack;
                record.has_symbols = self.scope_interner.has_symbols(&record.runs);
                record.scopes = scopes
                    .stack
                    .has_pending_clears()
                    .then(|| Arc::new(scopes.stack.clone()));
                #[cfg(feature = "profile")]
                {
                    self.profile.checkpoint_scope_copies += usize::from(record.scopes.is_some());
                }
                record.checkpoint = None;
            }
            self.scopes = scopes;
        }
        let rendered = self.render_cache.render(
            &self.line_scratch,
            &output.ops,
            &mut self.scopes,
            &mut self.resolver,
            &mut self.scope_interner,
        )?;
        #[cfg(feature = "profile")]
        {
            self.profile.render += phase.elapsed();
        }
        #[cfg(feature = "profile")]
        let phase = Instant::now();
        let converged = row + 1 >= self.must_parse_to
            && !self.state.is_speculative()
            && self.records[row].as_ref().is_some_and(|old| {
                self.scopes.matches(old)
                    && old
                        .checkpoint
                        .as_ref()
                        .is_some_and(|state| !state.is_speculative() && *state == self.state)
            });
        let scopes = if !self.scopes.stack.has_pending_clears() {
            None
        } else if let Some(scopes) = self.scope_cache.get(&self.scopes.stack) {
            Some(scopes.clone())
        } else {
            #[cfg(feature = "profile")]
            {
                self.profile.checkpoint_scope_copies += 1;
            }
            let scopes = Arc::new(self.scopes.stack.clone());
            if self.scope_cache.len() < 4096 {
                self.scope_cache.insert(scopes.clone());
            }
            Some(scopes)
        };
        self.records[row] = Some(LineRecord {
            ops: Arc::clone(&output.ops),
            scopes,
            checkpoint: Some(self.state.clone()),
            spans: rendered.spans,
            has_symbols: self.scope_interner.has_symbols(&rendered.runs),
            runs: rendered.runs,
            end_stack: rendered.end_stack,
        });
        self.frontier += 1;
        if converged && self.reusable_to > self.frontier {
            let mut to = self.reusable_to.min(self.records.len());
            while to > self.frontier {
                if let Some(record) = &self.records[to - 1]
                    && let Some(state) = &record.checkpoint
                    && !state.is_speculative()
                {
                    let mut state = state.clone();
                    if state.resume_at(to) {
                        self.state = state;
                        self.scopes = ScopeCursor::from_record(record, &self.scope_interner.stacks);
                        self.frontier = to;
                        self.stats.convergences += 1;
                        break;
                    }
                }
                to -= 1;
            }
        }
        #[cfg(feature = "profile")]
        {
            self.profile.checkpoint += phase.elapsed();
        }
        Ok(())
    }
    fn batch(&self, lines: Range<usize>) -> HighlightBatch {
        let end = lines.end.min(self.text.len_lines());
        let start = lines.start.min(end);
        let mut highlighted = Vec::with_capacity(end - start);
        for row in start..end {
            let exact = row < self.frontier;
            let spans = if exact {
                self.records[row].as_ref().unwrap().spans.clone()
            } else {
                default_spans(
                    complete_line_range(&self.text, row).len(),
                    self.resolver.scheme().default_style(),
                )
            };
            highlighted.push(StyledLine {
                line: row,
                spans,
                exact,
            });
        }
        HighlightBatch {
            version: self.version,
            scheme_revision: self.scheme_revision,
            lines: start..end,
            complete: highlighted.iter().all(|l| l.exact),
            highlighted,
            scopes: self.scope_snapshot(start..end),
        }
    }
}
fn complete_line_range(text: &Text, line: usize) -> Range<usize> {
    let start = text.line_to_byte(line);
    let end = if line + 1 < text.len_lines() {
        text.line_to_byte(line + 1)
    } else {
        text.len_bytes()
    };
    start..end
}
fn default_spans(len: usize, style: Style) -> Arc<[StyleSpan]> {
    if len == 0 {
        Arc::from([])
    } else {
        Arc::from([StyleSpan {
            range: 0..len,
            style,
        }])
    }
}
#[derive(Debug, Clone)]
struct RenderedLine {
    spans: Arc<[StyleSpan]>,
    runs: Arc<[ScopeRun]>,
    end_stack: ScopeStackId,
    text_dependent: bool,
}

#[derive(Debug, Default)]
struct ScopeCursor {
    active: ScopeStackId,
    stack: ScopeStack,
}
impl ScopeCursor {
    fn from_record(record: &LineRecord, stacks: &ScopeStacks) -> Self {
        Self {
            active: record.end_stack,
            stack: record.scopes.as_deref().cloned().unwrap_or_else(|| {
                ScopeStack::from_vec(stacks.get(record.end_stack).unwrap().to_vec())
            }),
        }
    }
    fn matches(&self, record: &LineRecord) -> bool {
        self.active == record.end_stack
            && record.scopes.as_deref().map_or_else(
                || !self.stack.has_pending_clears(),
                |old| *old == self.stack,
            )
    }
}

#[derive(Debug)]
struct CachedRender {
    // Keep the allocation alive as long as its address is a cache key.
    _ops: SharedScopeOperations,
    output: RenderedLine,
}
#[derive(Debug, PartialEq, Eq, Hash)]
struct RenderKey {
    active: ScopeStackId,
    ops_address: usize,
    len: usize,
}
#[derive(Debug, Default)]
struct RenderCache {
    entries: HashMap<RenderKey, CachedRender, fnv::FnvBuildHasher>,
    bytes: usize,
    visual: SpanCache,
    #[cfg(feature = "profile")]
    hits: u64,
    #[cfg(feature = "profile")]
    queries: u64,
}
impl RenderCache {
    fn render(
        &mut self,
        line: &str,
        ops: &SharedScopeOperations,
        cursor: &mut ScopeCursor,
        resolver: &mut Highlighter,
        interner: &mut ScopeInterner,
    ) -> Result<RenderedLine, Error> {
        #[cfg(feature = "profile")]
        {
            self.queries += 1;
        }
        let key = RenderKey {
            active: cursor.active,
            ops_address: ops.as_ptr() as usize,
            len: line.len(),
        };
        if let Some(cached) = self.entries.get(&key)
            && (line.is_ascii() || ops.iter().all(|(at, _)| line.is_char_boundary(*at)))
        {
            #[cfg(feature = "profile")]
            {
                self.hits += 1;
            }
            let end = cached.output.end_stack;
            if cursor.active != end {
                cursor.stack.scopes.clear();
                cursor
                    .stack
                    .scopes
                    .extend_from_slice(interner.stacks.get(end).unwrap());
                cursor.active = end;
            }
            return Ok(cached.output.clone());
        }
        // Bound both the number of keys and retained arrays. Uncached shapes
        // continue through the ordinary renderer. Clear/restore operations
        // touch history that is deliberately not part of an active stack ID.
        let output = style_line_at(
            line,
            ops,
            &mut cursor.stack,
            cursor.active,
            resolver,
            interner,
            &mut self.visual,
        )?;
        cursor.active = output.end_stack;
        let bytes = std::mem::size_of_val(ops.as_ref())
            + std::mem::size_of_val(output.spans.as_ref())
            + std::mem::size_of_val(output.runs.as_ref());
        if !output.text_dependent
            && self.entries.len() < 8192
            && self.bytes.saturating_add(bytes) <= 4 * 1024 * 1024
            && ops
                .iter()
                .all(|(_, op)| !matches!(op, ScopeStackOp::Clear(_) | ScopeStackOp::Restore))
        {
            self.bytes += bytes;
            self.entries.insert(
                key,
                CachedRender {
                    _ops: Arc::clone(ops),
                    output: output.clone(),
                },
            );
        }
        Ok(output)
    }
}

// A style shape is independent of exact scope IDs and saved clear history.
// The contiguous byte intervals need only an end offset and palette-local ID.
type VisualShape = [(u32, u32)];

#[derive(Debug, Default)]
struct SpanCache {
    scratch: Vec<(u32, u32)>,
    shapes: HashMap<Box<VisualShape>, Arc<[StyleSpan]>, fnv::FnvBuildHasher>,
    bytes: usize,
    #[cfg(feature = "profile")]
    built_spans: usize,
    #[cfg(feature = "profile")]
    hits: usize,
}
impl SpanCache {
    fn push(&mut self, end: usize, style: u32) {
        if let Some(last) = self.scratch.last_mut()
            && last.1 == style
        {
            last.0 = end as u32;
        } else {
            self.scratch.push((end as u32, style));
        }
    }

    fn finish(&mut self, interner: &ScopeInterner, text_dependent: bool) -> Arc<[StyleSpan]> {
        let shape = self.scratch.as_slice();
        if !text_dependent && let Some(spans) = self.shapes.get(shape) {
            #[cfg(feature = "profile")]
            {
                self.hits += 1;
            }
            return Arc::clone(spans);
        }
        let mut start = 0;
        let spans: Arc<[StyleSpan]> = Arc::from_iter(shape.iter().map(|&(end, style)| StyleSpan {
            range: std::mem::replace(&mut start, end as usize)..end as usize,
            style: interner.style(style),
        }));
        #[cfg(feature = "profile")]
        {
            self.built_spans += shape.len();
        }
        let bytes = std::mem::size_of_val(shape) + std::mem::size_of_val(spans.as_ref());
        if !text_dependent
            && self.shapes.len() < 8192
            && self.bytes.saturating_add(bytes) <= 4 * 1024 * 1024
        {
            self.shapes.insert(shape.into(), Arc::clone(&spans));
            self.bytes += bytes;
        }
        spans
    }
}

fn style_line(
    line: &str,
    ops: &[(usize, ScopeStackOp)],
    stack: &mut ScopeStack,
    resolver: &mut Highlighter,
    interner: &mut ScopeInterner,
    visual: &mut SpanCache,
) -> Result<RenderedLine, Error> {
    let active = interner.intern(stack);
    style_line_at(line, ops, stack, active, resolver, interner, visual)
}
fn style_line_at(
    line: &str,
    ops: &[(usize, ScopeStackOp)],
    stack: &mut ScopeStack,
    mut active: ScopeStackId,
    resolver: &mut Highlighter,
    interner: &mut ScopeInterner,
    visual: &mut SpanCache,
) -> Result<RenderedLine, Error> {
    if line.len() > u32::MAX as usize {
        return Err(Error::Parse(
            "line exceeds compact scope range (4 GiB)".into(),
        ));
    }
    let mut runs: Vec<ScopeRun> = Vec::with_capacity(ops.len() / 2 + 1);
    visual.scratch.clear();
    let mut at = 0;
    let mut text_dependent = false;
    let mut emit = |range: Range<usize>,
                    id: ScopeStackId,
                    resolver: &mut Highlighter,
                    interner: &mut ScopeInterner,
                    visual: &mut SpanCache| {
        if range.is_empty() {
            return;
        }
        if let Some(last) = runs.last_mut()
            && last.stack == id
            && last.end as usize == range.start
        {
            last.end = range.end as u32;
        } else {
            runs.push(ScopeRun {
                start: range.start as u32,
                end: range.end as u32,
                stack: id,
            });
        }
        let style_id = interner.style_id(id, resolver);
        let style = interner.style(style_id);
        if style.hashed_foreground.is_some() {
            text_dependent = true;
            let text = &line[range.clone()];
            let mut start = range.start;
            let mut word = None;
            for (i, ch) in text.char_indices() {
                let is_word = ch.is_alphanumeric() || ch == '_';
                if word.is_some_and(|w| w != is_word) {
                    let end = range.start + i;
                    let token = resolver.scheme().style_for_token(style, &line[start..end]);
                    visual.push(end, interner.intern_style(token));
                    start = end;
                }
                word = Some(is_word);
            }
            let token = resolver
                .scheme()
                .style_for_token(style, &line[start..range.end]);
            visual.push(range.end, interner.intern_style(token));
        } else {
            visual.push(range.end, style_id);
        }
    };
    for group in ops.chunk_by(|a, b| a.0 == b.0) {
        let byte = group[0].0;
        if byte < at || byte > line.len() || !line.is_char_boundary(byte) {
            return Err(Error::Parse("invalid UTF-8 scope operation".into()));
        }
        emit(at..byte, active, resolver, interner, visual);
        active = interner
            .apply_group(active, stack, group)
            .map_err(|e| Error::Parse(e.to_string()))?;
        at = byte;
    }
    emit(at..line.len(), active, resolver, interner, visual);
    Ok(RenderedLine {
        spans: visual.finish(interner, text_dependent),
        runs: runs.into(),
        end_stack: active,
        text_dependent,
    })
}
#[derive(Debug)]
enum Command {
    Update(Text, ChangeId, ChangeSet),
    Request(ChangeId, Range<usize>),
    Scheme(Arc<ColorScheme>),
    Stop,
}
/// Nonblocking editor-facing worker. Commands are ordered, stale results are dropped, and idle
/// background work runs in short slices with a yield between slices. Drop never joins the worker.
#[derive(Debug)]
pub struct HighlightService {
    shared: Arc<Mutex<DocumentHighlighter>>,
    commands: mpsc::Sender<Command>,
    results: mpsc::Receiver<Result<HighlightBatch, Error>>,
    current: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    text: Text,
    version: ChangeId,
    scheme_revision: u64,
    default_style: Style,
}
impl HighlightService {
    /// Starts a named background thread. Construct one service per buffer, share it across views.
    pub fn new(engine: DocumentHighlighter) -> Result<Self, std::io::Error> {
        let text = engine.text.clone();
        let version = engine.version;
        let scheme_revision = engine.scheme_revision;
        let default_style = engine.resolver.scheme().default_style();
        let shared = Arc::new(Mutex::new(engine));
        let (tx, rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let current = Arc::new(AtomicU64::new(version.0));
        let stop = Arc::new(AtomicBool::new(false));
        let state = shared.clone();
        let latest = current.clone();
        let stopping = stop.clone();
        thread::Builder::new()
            .name("velocity-highlight".into())
            .spawn(move || worker(state, rx, result_tx, latest, stopping))?;
        Ok(Self {
            shared,
            commands: tx,
            results: result_rx,
            current,
            stop,
            text,
            version,
            scheme_revision,
            default_style,
        })
    }
    /// Enqueues a committed transaction and updates the frontend snapshot in O(1), excluding
    /// the ChangeSet descriptor. Versions must increase; full validation happens in the worker.
    pub fn apply_change(
        &mut self,
        after: Text,
        version: ChangeId,
        changes: ChangeSet,
    ) -> Result<(), Error> {
        if version <= self.version {
            return Err(Error::Snapshot("version must increase"));
        }
        self.commands
            .send(Command::Update(after.clone(), version, changes))
            .map_err(|_| Error::Disconnected)?;
        self.text = after;
        self.version = version;
        self.current.store(version.0, Ordering::Release);
        Ok(())
    }
    /// Tries synchronous visible-first work within 2 ms, using try_lock so typing never waits
    /// for a worker. Incomplete views are queued; the returned fallback uses current-text offsets.
    pub fn request_lines(
        &self,
        version: ChangeId,
        lines: Range<usize>,
    ) -> Result<HighlightBatch, Error> {
        if version != self.version {
            return Err(Error::Snapshot("requested version is stale"));
        }
        self.commands
            .send(Command::Request(version, lines.clone()))
            .map_err(|_| Error::Disconnected)?;
        if let Ok(mut state) = self.shared.try_lock()
            && state.version == version
            && state.scheme_revision == self.scheme_revision
        {
            return state.request_lines(version, lines, VISIBLE_BUDGET);
        }
        let end = lines.end.min(self.text.len_lines());
        let start = lines.start.min(end);
        Ok(HighlightBatch {
            version,
            scheme_revision: self.scheme_revision,
            lines: start..end,
            highlighted: (start..end)
                .map(|row| StyledLine {
                    line: row,
                    spans: default_spans(
                        complete_line_range(&self.text, row).len(),
                        self.default_style,
                    ),
                    exact: false,
                })
                .collect(),
            scopes: ScopeSnapshot {
                version,
                text: self.text.clone(),
                range: start..end,
                stacks: ScopeStacks::default(),
                lines: (start..end)
                    .map(|line| ScopedLine {
                        line,
                        runs: Arc::from([]),
                        end_stack: ScopeStackId(0),
                        exact: false,
                    })
                    .collect(),
            },
            complete: start == end,
        })
    }
    /// Nonblocking scopes without parser work; None while the worker owns the state or is stale.
    pub fn scope_snapshot(&self, version: ChangeId, lines: Range<usize>) -> Option<ScopeSnapshot> {
        if version != self.version {
            return None;
        }
        let state = self.shared.try_lock().ok()?;
        (state.version == version).then(|| state.scope_snapshot(lines))
    }
    /// Nonblocking single-point query. Resolve the returned ID through a scope snapshot.
    pub fn scope_at(&self, version: ChangeId, line: usize, byte: usize) -> Option<ScopeStackId> {
        self.scope_snapshot(version, line..line.saturating_add(1))?
            .scope_at(line, byte)
    }
    /// Nonblocking scope-region query over currently validated lines.
    pub fn extract_scope(&self, version: ChangeId, point: usize) -> Option<Range<usize>> {
        let state = self.shared.try_lock().ok()?;
        (version == self.version && state.version == version)
            .then(|| state.extract_scope(point))
            .flatten()
    }
    /// Nonblocking selector query, including specificity score.
    pub fn match_selector(
        &self,
        version: ChangeId,
        point: usize,
        selector: &vel_scope::Selector,
    ) -> Option<vel_scope::MatchScore> {
        let state = self.shared.try_lock().ok()?;
        (version == self.version && state.version == version)
            .then(|| state.match_selector(point, selector))
            .flatten()
    }
    /// Nonblocking symbol-list snapshot. Use on an editor dispatcher for large files.
    pub fn symbols(&self, version: ChangeId) -> Option<Result<SymbolList, Error>> {
        if version != self.version {
            return None;
        }
        let state = self.shared.try_lock().ok()?;
        (state.version == version).then(|| state.symbols())
    }
    /// Polls completed viewport work, discarding all results for previous buffer versions.
    pub fn try_recv(&self) -> Option<Result<HighlightBatch, Error>> {
        loop {
            match self.results.try_recv() {
                Ok(Ok(batch))
                    if batch.version == self.version
                        && batch.scheme_revision == self.scheme_revision =>
                {
                    return Some(Ok(batch));
                }
                Ok(Ok(_)) => continue,
                Ok(Err(error)) => return Some(Err(error)),
                Err(_) => return None,
            }
        }
    }
    /// Returns None while the worker owns the lock or has not applied the latest transaction.
    pub fn progress(&self) -> Option<HighlightProgress> {
        let state = self.shared.try_lock().ok()?;
        (state.version == self.version && state.scheme_revision == self.scheme_revision).then(
            || HighlightProgress {
                version: state.version,
                scheme_revision: state.scheme_revision,
                parsed_lines: state.frontier,
                total_lines: state.text.len_lines(),
            },
        )
    }
    /// Returns component timers when the worker is idle enough to acquire its lock.
    #[cfg(feature = "profile")]
    pub fn profile(&self) -> Option<HighlightProfile> {
        Some(self.shared.try_lock().ok()?.profile())
    }
    /// Changes scheme on the worker, keeping parsing checkpoints.
    pub fn set_color_scheme(&mut self, scheme: Arc<ColorScheme>) -> Result<(), Error> {
        self.default_style = scheme.default_style();
        self.scheme_revision += 1;
        self.commands
            .send(Command::Scheme(scheme))
            .map_err(|_| Error::Disconnected)
    }
}
impl Drop for HighlightService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.commands.send(Command::Stop);
    }
}
fn worker(
    shared: Arc<Mutex<DocumentHighlighter>>,
    rx: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Result<HighlightBatch, Error>>,
    latest: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) {
    let mut viewport = None;
    let mut watched: Option<(ChangeId, Range<usize>)> = None;
    let mut complete = false;
    while !stop.load(Ordering::Acquire) {
        let command = if complete {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        } else {
            rx.try_recv().ok()
        };
        if let Some(command) = command {
            let mut state = shared.lock().unwrap();
            match command {
                Command::Stop => return,
                Command::Update(text, version, changes) => {
                    match state.apply_change(text, version, &changes) {
                        Ok(()) => {
                            complete = false;
                            viewport = None;
                            watched = None;
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            complete = true;
                        }
                    }
                }
                Command::Request(version, lines) => {
                    if version == state.version {
                        watched = Some((version, lines.clone()));
                        viewport = Some((version, lines));
                        complete = false;
                    }
                }
                Command::Scheme(scheme) => {
                    if let Err(e) = state.set_color_scheme(scheme) {
                        let _ = tx.send(Err(e));
                    }
                    complete = false;
                    viewport = watched.clone();
                }
            }
            continue;
        }
        let mut state = shared.lock().unwrap();
        if state.version.0 != latest.load(Ordering::Acquire) {
            drop(state);
            thread::yield_now();
            continue;
        }
        let rewinds_before = state.stats.rewinds;
        let result = if let Some((version, lines)) = &viewport {
            state
                .request_lines(*version, lines.clone(), VISIBLE_BUDGET)
                .map(|batch| {
                    let done = batch.complete;
                    if done {
                        let _ = tx.send(Ok(batch));
                    }
                    done
                })
        } else {
            state.advance(Duration::from_millis(1))
        };
        // A completed viewport may change later when idle parsing resolves a branch.
        // Republish it even when the UI has not issued another request.
        if viewport.is_none()
            && state.stats.rewinds != rewinds_before
            && let Some((version, lines)) = &watched
            && *version == state.version
            && state.stats.rewind_from.is_some_and(|from| from < lines.end)
        {
            let _ = tx.send(Ok(state.batch(lines.clone())));
        }
        match result {
            Ok(done) => {
                if viewport.is_some() {
                    if done {
                        viewport = None;
                    }
                } else {
                    complete = done;
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e));
                complete = true;
                viewport = None;
            }
        }
        drop(state);
        thread::yield_now();
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use vel_scope::{ClearAmount, Scope};

    #[test]
    fn equal_visual_shapes_share_spans_but_keep_exact_scope_paths() {
        let mut cache = RenderCache::default();
        let mut resolver = Highlighter::new(Arc::new(ColorScheme::velocity_dark()));
        let mut interner = ScopeInterner::new(Arc::new(MetadataSet::new()));
        let keyword = Scope::new("keyword.control.fixture").unwrap();
        let ops = Arc::from([(1, ScopeStackOp::Push(keyword))]);
        let mut render = |prefix: &str, text: &str| {
            let stack = ScopeStack::from_vec(vec![Scope::new(prefix).unwrap()]);
            let mut cursor = ScopeCursor {
                active: interner.intern(&stack),
                stack,
            };
            cache
                .render(text, &ops, &mut cursor, &mut resolver, &mut interner)
                .unwrap()
        };
        let a = render("source.a", "abc");
        let b = render("source.b", "xyz");
        let c = render("source.b", "xyz!");
        assert!(Arc::ptr_eq(&a.spans, &b.spans));
        assert!(!Arc::ptr_eq(&b.spans, &c.spans));
        assert_ne!(a.runs, b.runs);
        assert_ne!(a.end_stack, b.end_stack);
        assert_ne!(
            interner.stacks.get(a.end_stack),
            interner.stacks.get(b.end_stack)
        );
    }

    #[test]
    fn cursor_checkpoint_shares_the_end_path_only_without_saved_clears() {
        let mut interner = ScopeInterner::new(Arc::new(MetadataSet::new()));
        let visible = Scope::new("source.visible").unwrap();
        let hidden = Scope::new("source.hidden").unwrap();
        let cursor = |interner: &mut ScopeInterner, has_history: bool| {
            let mut stack = ScopeStack::new();
            if has_history {
                stack.push(hidden);
                stack.apply(&ScopeStackOp::Clear(ClearAmount::All)).unwrap();
            }
            stack.push(visible);
            ScopeCursor {
                active: interner.intern(&stack),
                stack,
            }
        };
        let bare = cursor(&mut interner, false);
        let saved = cursor(&mut interner, true);
        let mut record = LineRecord {
            ops: Arc::from([]),
            scopes: None,
            checkpoint: None,
            spans: Arc::from([]),
            runs: Arc::from([]),
            end_stack: bare.active,
            has_symbols: false,
        };
        assert!(bare.matches(&record));
        assert!(!saved.matches(&record));
        let restored = ScopeCursor::from_record(&record, &interner.stacks);
        assert_eq!(restored.stack, bare.stack);
        record.scopes = Some(Arc::new(saved.stack));
        assert!(!bare.matches(&record));
        let mut restored = ScopeCursor::from_record(&record, &interner.stacks);
        restored.stack.apply(&ScopeStackOp::Restore).unwrap();
        assert_eq!(restored.stack.as_slice(), &[visible, hidden]);
    }

    #[test]
    fn shared_rendering_preserves_saved_scopes_and_utf8_validation() {
        let mut cache = RenderCache::default();
        let mut resolver = Highlighter::new(Arc::new(ColorScheme::velocity_dark()));
        let mut interner = ScopeInterner::new(Arc::new(MetadataSet::new()));
        let visible = Scope::new("source.visible").unwrap();
        for saved in ["source.first", "source.second", "source.first"] {
            let mut stack = ScopeStack::from_vec(vec![Scope::new(saved).unwrap()]);
            stack.apply(&ScopeStackOp::Clear(ClearAmount::All)).unwrap();
            stack.push(visible);
            let mut cursor = ScopeCursor {
                active: interner.intern(&stack),
                stack,
            };
            let mut expected_cursor = ScopeCursor {
                active: cursor.active,
                stack: cursor.stack.clone(),
            };
            let ops = Arc::from([(1, ScopeStackOp::Restore)]);
            let expected = style_line(
                "ab",
                &ops,
                &mut expected_cursor.stack,
                &mut resolver,
                &mut interner,
                &mut SpanCache::default(),
            )
            .unwrap();
            let actual = cache
                .render("ab", &ops, &mut cursor, &mut resolver, &mut interner)
                .unwrap();
            assert_eq!(cursor.stack, expected_cursor.stack);
            assert_eq!(actual.spans, expected.spans);
            assert_eq!(actual.runs, expected.runs);
            assert_eq!(actual.end_stack, expected.end_stack);
        }
        assert!(cache.entries.is_empty());
        let ops = Arc::from([(1, ScopeStackOp::Noop)]);
        cache
            .render(
                "ab",
                &ops,
                &mut ScopeCursor::default(),
                &mut resolver,
                &mut interner,
            )
            .unwrap();
        assert!(
            cache
                .render(
                    "é",
                    &ops,
                    &mut ScopeCursor::default(),
                    &mut resolver,
                    &mut interner
                )
                .is_err()
        );
    }

    #[test]
    fn ordinary_render_hits_preserve_different_nested_clear_histories() {
        let mut cache = RenderCache::default();
        let mut resolver = Highlighter::new(Arc::new(ColorScheme::velocity_dark()));
        let mut interner = ScopeInterner::new(Arc::new(MetadataSet::new()));
        let visible = Scope::new("source.visible").unwrap();
        let inner = Scope::new("meta.inner").unwrap();
        let end = Scope::new("meta.end").unwrap();
        let ops = Arc::from([(0, ScopeStackOp::Pop(1)), (1, ScopeStackOp::Push(end))]);
        let mut first = None;
        for saved in ["source.first", "source.second"] {
            let saved = Scope::new(saved).unwrap();
            let mut stack = ScopeStack::from_vec(vec![saved]);
            stack.apply(&ScopeStackOp::Clear(ClearAmount::All)).unwrap();
            stack.push(inner);
            stack.apply(&ScopeStackOp::Clear(ClearAmount::All)).unwrap();
            stack.push(visible);
            let mut cursor = ScopeCursor {
                active: interner.intern(&stack),
                stack,
            };
            let rendered = cache
                .render("ab", &ops, &mut cursor, &mut resolver, &mut interner)
                .unwrap();
            if let Some(first) = &first {
                assert!(Arc::ptr_eq(first, &rendered.runs));
            } else {
                first = Some(rendered.runs);
            }
            cursor.stack.apply(&ScopeStackOp::Restore).unwrap();
            cursor.stack.apply(&ScopeStackOp::Restore).unwrap();
            assert_eq!(cursor.stack.as_slice(), &[end, inner, saved]);
        }
    }

    #[test]
    fn hashed_colors_are_text_dependent_and_never_shared_by_shape() {
        let scheme = Arc::new(
            ColorScheme::parse(
                r##"{"rules":[{"scope":"source.fixture","foreground":["#f00","#00f"]}]}"##,
            )
            .unwrap(),
        );
        let mut resolver = Highlighter::new(scheme);
        let mut interner = ScopeInterner::new(Arc::new(MetadataSet::new()));
        let mut cache = RenderCache::default();
        let initial = ScopeStack::from_vec(vec![Scope::new("source.fixture").unwrap()]);
        let id = interner.intern(&initial);
        let cursor = || ScopeCursor {
            active: id,
            stack: initial.clone(),
        };
        let ops = Arc::from([]);
        for line in ["foo", "bar", "éx", "foo"] {
            let actual = cache
                .render(line, &ops, &mut cursor(), &mut resolver, &mut interner)
                .unwrap();
            let expected = style_line(
                line,
                &[],
                &mut cursor().stack,
                &mut resolver,
                &mut interner,
                &mut SpanCache::default(),
            )
            .unwrap();
            assert_eq!(actual.spans, expected.spans);
            assert!(actual.text_dependent);
        }
        assert!(cache.entries.is_empty());
    }
}
