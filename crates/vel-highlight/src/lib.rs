//! Versioned incremental highlighting. The UI uses nonblocking requests and discards stale batches.
#![forbid(unsafe_code)]
use std::{
    collections::HashSet,
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
use vel_syntax::{ParseState, SyntaxReference, SyntaxSet};
use vel_text::Text;

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
    scopes: Arc<ScopeStack>,
    checkpoint: Option<ParseState>,
    spans: Arc<[StyleSpan]>,
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
    scopes: ScopeStack,
    records: Vec<Option<LineRecord>>,
    frontier: usize,
    reusable_to: usize,
    must_parse_to: usize,
    stats: HighlightStats,
    line_scratch: String,
    parse_scratch: vel_syntax::LineOps,
    operation_cache: HashSet<Arc<[(usize, ScopeStackOp)]>>,
    scope_cache: HashSet<Arc<ScopeStack>>,
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
            scopes: ScopeStack::new(),
            records,
            frontier: 0,
            reusable_to: 0,
            must_parse_to: 0,
            stats: HighlightStats::default(),
            line_scratch: String::new(),
            parse_scratch: vel_syntax::LineOps::default(),
            operation_cache: HashSet::new(),
            scope_cache: HashSet::new(),
        }
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
    /// Swaps the color scheme and recolors parsed operations without running regexes again.
    pub fn set_color_scheme(&mut self, scheme: Arc<ColorScheme>) -> Result<(), Error> {
        self.scheme_revision += 1;
        self.resolver = Highlighter::new(scheme);
        // Unvalidated suffix spans still use the previous palette. They must be reparsed
        // or recolored before reuse, even if their parsing checkpoints converge.
        self.reusable_to = self.frontier;
        let mut scopes = ScopeStack::new();
        for row in 0..self.frontier {
            let Some(record) = &self.records[row] else {
                break;
            };
            let ops = record.ops.clone();
            let line = self.text.line(row).to_string();
            let spans = style_line(&line, &ops, &mut scopes, &mut self.resolver)?;
            let record = self.records[row].as_mut().unwrap();
            record.spans = spans;
            record.scopes = Arc::new(scopes.clone());
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
            self.scopes = ScopeStack::new();
        } else {
            let record = self.records[resume - 1].as_ref().unwrap();
            self.state = record.checkpoint.clone().unwrap();
            self.scopes = (*record.scopes).clone();
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
        let row = self.frontier;
        self.line_scratch.clear();
        for chunk in self.text.line(row).chunks() {
            self.line_scratch.push_str(chunk);
        }
        self.state
            .parse_line_into(&self.line_scratch, &mut self.parse_scratch)
            .map_err(|e| Error::Parse(e.to_string()))?;
        let output = &mut self.parse_scratch;
        self.stats.parsed_lines += 1;
        let mut rewind = output.rewind.map(|r| r.from_line);
        for (n, ops) in output.corrections.drain(..) {
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
                ScopeStack::new()
            } else {
                self.records[from - 1]
                    .as_ref()
                    .ok_or_else(|| Error::Parse("missing rewind anchor".into()))?
                    .scopes
                    .as_ref()
                    .clone()
            };
            for n in from..row {
                let record = self.records[n]
                    .as_ref()
                    .ok_or_else(|| Error::Parse("missing branch history".into()))?;
                let ops = record.ops.clone();
                let line = self.text.line(n).to_string();
                let spans = style_line(&line, &ops, &mut scopes, &mut self.resolver)?;
                let record = self.records[n].as_mut().unwrap();
                record.spans = spans;
                record.scopes = Arc::new(scopes.clone());
                record.checkpoint = None;
            }
            self.scopes = scopes;
        }
        let spans = style_line(
            &self.line_scratch,
            &output.ops,
            &mut self.scopes,
            &mut self.resolver,
        )?;
        let converged = row + 1 >= self.must_parse_to
            && !self.state.is_speculative()
            && self.records[row].as_ref().is_some_and(|old| {
                *old.scopes == self.scopes
                    && old
                        .checkpoint
                        .as_ref()
                        .is_some_and(|state| !state.is_speculative() && *state == self.state)
            });
        let operations = if let Some(operations) = self.operation_cache.get(output.ops.as_slice()) {
            operations.clone()
        } else {
            let operations: Arc<[_]> = Arc::from(output.ops.as_slice());
            if self.operation_cache.len() < 4096 {
                self.operation_cache.insert(operations.clone());
            }
            operations
        };
        let scopes = if let Some(scopes) = self.scope_cache.get(&self.scopes) {
            scopes.clone()
        } else {
            let scopes = Arc::new(self.scopes.clone());
            if self.scope_cache.len() < 4096 {
                self.scope_cache.insert(scopes.clone());
            }
            scopes
        };
        self.records[row] = Some(LineRecord {
            ops: operations,
            scopes,
            checkpoint: Some(self.state.clone()),
            spans,
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
                        self.scopes = (*record.scopes).clone();
                        self.frontier = to;
                        self.stats.convergences += 1;
                        break;
                    }
                }
                to -= 1;
            }
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
                    self.text.line(row).len_bytes(),
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
        }
    }
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
fn style_line(
    line: &str,
    ops: &[(usize, ScopeStackOp)],
    stack: &mut ScopeStack,
    resolver: &mut Highlighter,
) -> Result<Arc<[StyleSpan]>, Error> {
    let mut spans: Vec<StyleSpan> = Vec::with_capacity(ops.len() / 2 + 1);
    let mut at = 0;
    let emit = |range: Range<usize>,
                stack: &ScopeStack,
                resolver: &mut Highlighter,
                spans: &mut Vec<StyleSpan>| {
        if range.is_empty() {
            return;
        }
        let style = resolver.style_for(stack);
        let mut append = |range: Range<usize>, style: Style| {
            if let Some(last) = spans.last_mut()
                && last.range.end == range.start
                && last.style == style
            {
                last.range.end = range.end;
            } else {
                spans.push(StyleSpan { range, style });
            }
        };
        if style.hashed_foreground.is_some() {
            let text = &line[range.clone()];
            let mut start = range.start;
            let mut word = None;
            for (i, ch) in text.char_indices() {
                let is_word = ch.is_alphanumeric() || ch == '_';
                if word.is_some_and(|w| w != is_word) {
                    let end = range.start + i;
                    append(
                        start..end,
                        resolver.scheme().style_for_token(style, &line[start..end]),
                    );
                    start = end;
                }
                word = Some(is_word);
            }
            append(
                start..range.end,
                resolver
                    .scheme()
                    .style_for_token(style, &line[start..range.end]),
            );
        } else {
            append(range, style);
        }
    };
    for &(byte, op) in ops {
        if byte < at || byte > line.len() || !line.is_char_boundary(byte) {
            return Err(Error::Parse("invalid UTF-8 scope operation".into()));
        }
        emit(at..byte, stack, resolver, &mut spans);
        stack.apply(&op).map_err(|e| Error::Parse(e.to_string()))?;
        at = byte;
    }
    emit(at..line.len(), stack, resolver, &mut spans);
    Ok(spans.into())
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
                    spans: default_spans(self.text.line(row).len_bytes(), self.default_style),
                    exact: false,
                })
                .collect(),
            complete: start == end,
        })
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
