//! Sparse, bounded-memory huge-document parsing. Foreground and exact repair are separate.
use super::*;
use std::collections::BTreeMap;

/// Bounds for huge-document requests and retained parsing history.
#[derive(Debug, Clone)]
pub struct ViewportOptions {
    /// Maximum complete line bytes parsed during a viewport request. Default 16 KiB.
    /// Longer lines use plain spans, with all dependent scopes marked approximate.
    pub max_line_bytes: usize,
    /// Maximum lines returned by one request. Default 512; clamped to 1..=4096.
    pub max_viewport_lines: usize,
    /// Maximum prefix lines parsed from an exact checkpoint during a request. Default 256.
    pub max_lookback_lines: usize,
    /// Save stable exact state at this spacing. Default 1024; clamped to at least one.
    pub checkpoint_interval: usize,
    /// Maximum saved checkpoints, including line zero. Default 128; at least two.
    pub max_checkpoints: usize,
}
impl Default for ViewportOptions {
    fn default() -> Self {
        Self {
            max_line_bytes: 16 * 1024,
            max_viewport_lines: 512,
            max_lookback_lines: 256,
            checkpoint_interval: 1024,
            max_checkpoints: 128,
        }
    }
}
/// Exact background progress. A long line requires an explicitly scheduled repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportProgress {
    /// Number of contiguous lines parsed from an exact state.
    pub parsed_lines: usize,
    /// Document line count, including an empty final line.
    pub total_lines: usize,
    /// Next line exceeds the foreground cap; repair it on an analysis worker.
    pub long_line: Option<usize>,
}
impl ViewportProgress {
    /// Whether the exact pass reached EOF.
    pub fn complete(self) -> bool {
        self.parsed_lines == self.total_lines
    }
}
#[derive(Debug, Clone)]
struct Checkpoint {
    state: ParseState,
    scopes: ScopeStack,
}
#[derive(Debug, Clone)]
struct Cached {
    record: LineRecord,
    exact: bool,
}
#[derive(Debug)]
struct Cursor {
    state: ParseState,
    scopes: ScopeStack,
    next: usize,
    trusted: bool,
    anchor: ScopeStack,
    history: BTreeMap<usize, Cached>,
}
impl Cursor {
    fn new(mut checkpoint: Checkpoint, next: usize, trusted: bool) -> Self {
        assert!(checkpoint.state.resume_at(next), "stable sparse checkpoint");
        Self {
            state: checkpoint.state,
            anchor: checkpoint.scopes.clone(),
            scopes: checkpoint.scopes,
            next,
            trusted,
            history: BTreeMap::new(),
        }
    }
    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            state: self.state.clone(),
            scopes: self.scopes.clone(),
        }
    }
}
/// Huge-document engine with O(checkpoint cap + viewport + 128-line replay history)
/// records, independent of document line count. Use this instead of the dense
/// DocumentHighlighter/HighlightService when LargeDocumentPolicy selects viewport mode.
///
/// `request_lines` never parses the distant prefix. An initial best-effort state
/// yields approximate spans; `advance` plus worker-only `repair_long_line` establish
/// exact checkpoints and replace visible approximations. Call request again after
/// background progress/rewinds; output remains versioned like ordinary highlighting.
#[derive(Debug)]
pub struct ViewportHighlighter {
    text: Text,
    version: ChangeId,
    scheme_revision: u64,
    initial: Checkpoint,
    resolver: Highlighter,
    interner: ScopeInterner,
    visual: SpanCache,
    options: ViewportOptions,
    checkpoints: BTreeMap<usize, Checkpoint>,
    background: Cursor,
    foreground: Option<(Range<usize>, Cursor)>,
    watched: Range<usize>,
    visible: BTreeMap<usize, Cached>,
    stats: HighlightStats,
}
impl ViewportHighlighter {
    /// Creates a sparse engine without allocating an array proportional to line count.
    /// `syntax` must belong to `set`. Parser transition memoization is also bounded.
    pub fn new(
        text: Text,
        version: ChangeId,
        set: Arc<SyntaxSet>,
        syntax: &SyntaxReference,
        scheme: Arc<ColorScheme>,
        mut options: ViewportOptions,
    ) -> Self {
        options.max_viewport_lines = options.max_viewport_lines.clamp(1, 4096);
        options.checkpoint_interval = options.checkpoint_interval.max(1);
        options.max_checkpoints = options.max_checkpoints.max(2);
        let initial = Checkpoint {
            state: ParseState::new(set, syntax).with_line_cache_capacity(1024),
            scopes: ScopeStack::new(),
        };
        Self {
            text,
            version,
            scheme_revision: 0,
            resolver: Highlighter::new(scheme),
            interner: ScopeInterner::new(Arc::new(MetadataSet::new())),
            visual: SpanCache::default(),
            options,
            checkpoints: BTreeMap::from([(0, initial.clone())]),
            background: Cursor::new(initial.clone(), 0, true),
            initial,
            foreground: None,
            watched: 0..0,
            visible: BTreeMap::new(),
            stats: HighlightStats::default(),
        }
    }
    /// Current immutable document snapshot.
    pub fn text(&self) -> &Text {
        &self.text
    }
    /// Current buffer identity.
    pub fn version(&self) -> ChangeId {
        self.version
    }
    /// Work/rewind counts for diagnostics, including best-effort viewport parsing.
    pub fn stats(&self) -> HighlightStats {
        self.stats
    }
    /// Retained line records, useful for checking memory bounds (not text bytes).
    pub fn retained_lines(&self) -> usize {
        self.visible.len()
            + self.background.history.len()
            + self.foreground.as_ref().map_or(0, |(_, c)| c.history.len())
    }
    /// Number of sparse stable checkpoints; never exceeds max_checkpoints.
    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }
    /// Foreground-safe request. Clips to the document and max_viewport_lines;
    /// checks the cooperative deadline between lines, before copying any line.
    /// Scope queries remain unavailable for approximate results.
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
        let start = lines.start.min(end);
        let lines = start..end.min(start.saturating_add(self.options.max_viewport_lines));
        if self.watched != lines {
            self.visible.clear();
            self.watched = lines.clone();
            self.foreground = None;
        }
        self.publish_background();
        if !lines
            .clone()
            .all(|n| self.visible.get(&n).is_some_and(|c| c.exact))
        {
            let mut cursor = if let Some((old, cursor)) =
                self.foreground.take().filter(|(old, _)| *old == lines)
            {
                let _ = old;
                cursor
            } else {
                let (at, checkpoint) = self
                    .checkpoints
                    .range(..=start)
                    .next_back()
                    .expect("line-zero checkpoint");
                if start - at <= self.options.max_lookback_lines {
                    Cursor::new(checkpoint.clone(), *at, true)
                } else if self.background.next <= start
                    && start - self.background.next <= self.options.max_lookback_lines
                    && !self.background.state.is_speculative()
                {
                    Cursor::new(self.background.checkpoint(), self.background.next, true)
                } else {
                    Cursor::new(self.initial.clone(), start, start == 0)
                }
            };
            let deadline = Instant::now().checked_add(budget);
            while cursor.next < lines.end && !deadline.is_some_and(|d| Instant::now() >= d) {
                self.parse_one(&mut cursor, false)?;
            }
            for (&n, record) in cursor.history.range(lines.clone()) {
                if !self.visible.get(&n).is_some_and(|c| c.exact) {
                    self.visible.insert(n, record.clone());
                }
            }
            self.foreground = Some((lines.clone(), cursor));
        }
        Ok(self.batch(lines))
    }
    /// Advances the exact prefix, stopping before oversized lines. Check long_line
    /// and schedule repair_long_line on a worker; then continue advancing.
    /// Like ordinary highlighting, one bounded-size regex call is not preemptible.
    pub fn advance(&mut self, budget: Duration) -> Result<ViewportProgress, Error> {
        let deadline = Instant::now().checked_add(budget);
        let mut cursor = std::mem::replace(
            &mut self.background,
            Cursor::new(self.initial.clone(), 0, true),
        );
        let result = (|| {
            while cursor.next < self.text.len_lines()
                && !deadline.is_some_and(|d| Instant::now() >= d)
            {
                if self.line_is_long(cursor.next) {
                    break;
                }
                self.parse_one(&mut cursor, true)?;
                self.save_checkpoint(&cursor);
            }
            Ok(())
        })();
        self.background = cursor;
        result?;
        self.publish_background();
        Ok(self.progress())
    }
    /// Parses exactly one oversized line from the exact prefix, preserving real
    /// multiline/branch semantics. WORKER ONLY: this intentionally has no deadline
    /// and may allocate a contiguous line or spend a long time in a regex call.
    /// A capped parse cannot in general establish the state after arbitrary text.
    /// No skipped text is ever declared exact. Call advance after repair to continue.
    pub fn repair_long_line(&mut self) -> Result<ViewportProgress, Error> {
        if self.background.next < self.text.len_lines() && self.line_is_long(self.background.next) {
            let mut cursor = std::mem::replace(
                &mut self.background,
                Cursor::new(self.initial.clone(), 0, true),
            );
            let result = self.parse_one(&mut cursor, true);
            if result.is_ok() {
                self.save_checkpoint(&cursor);
            }
            self.background = cursor;
            result?;
            self.publish_background();
        }
        Ok(self.progress())
    }
    /// Restores exact visible output from the nearest retained exact checkpoint,
    /// even when that checkpoint is beyond the foreground lookback limit. WORKER
    /// ONLY: may parse a distant prefix and oversized lines, without a deadline.
    /// This is the eventual-correction path after checkpoint eviction or a skipped
    /// long line. Later branch corrections are republished by the exact pass.
    pub fn repair_lines(
        &mut self,
        version: ChangeId,
        lines: Range<usize>,
    ) -> Result<HighlightBatch, Error> {
        if version != self.version {
            return Err(Error::Snapshot("requested version is stale"));
        }
        let end = lines.end.min(self.text.len_lines());
        let start = lines.start.min(end);
        let lines = start..end.min(start.saturating_add(self.options.max_viewport_lines));
        let (&at, checkpoint) = self
            .checkpoints
            .range(..=start)
            .next_back()
            .expect("line-zero checkpoint");
        let mut cursor = Cursor::new(checkpoint.clone(), at, true);
        self.watched = lines.clone();
        self.visible.clear();
        // Read ahead through the documented branch rewind horizon so the returned
        // window includes any later fail that can revise its last line.
        let through = lines.end.saturating_add(128).min(self.text.len_lines());
        while cursor.next < through {
            self.parse_one(&mut cursor, true)?;
            self.save_checkpoint(&cursor);
            for (&n, record) in cursor.history.range(lines.clone()) {
                self.visible.insert(n, record.clone());
            }
        }
        self.foreground = None;
        Ok(self.batch(lines))
    }
    /// Current exact prefix and whether it is waiting for explicit long-line repair.
    pub fn progress(&self) -> ViewportProgress {
        ViewportProgress {
            parsed_lines: self.background.next,
            total_lines: self.text.len_lines(),
            long_line: (self.background.next < self.text.len_lines()
                && self.line_is_long(self.background.next))
            .then_some(self.background.next),
        }
    }
    fn line_is_long(&self, row: usize) -> bool {
        complete_line_range(&self.text, row).len() > self.options.max_line_bytes
    }
    fn save_checkpoint(&mut self, cursor: &Cursor) {
        if cursor.trusted
            && !cursor.state.is_speculative()
            && cursor.next.is_multiple_of(self.options.checkpoint_interval)
        {
            self.checkpoints.insert(cursor.next, cursor.checkpoint());
            while self.checkpoints.len() > self.options.max_checkpoints {
                let oldest = *self.checkpoints.keys().nth(1).expect("nonzero checkpoint");
                self.checkpoints.remove(&oldest);
            }
        }
    }
    fn publish_background(&mut self) {
        for (&n, record) in self.background.history.range(self.watched.clone()) {
            self.visible.insert(n, record.clone());
        }
    }
    fn parse_one(&mut self, cursor: &mut Cursor, repair: bool) -> Result<(), Error> {
        let row = cursor.next;
        let range = complete_line_range(&self.text, row);
        if !repair && range.len() > self.options.max_line_bytes {
            // Leave the incoming parser untouched. Approximation is contagious until
            // the exact background pass supplies the real state after this line.
            cursor.trusted = false;
            if !cursor.state.resume_at(row + 1) {
                cursor.state = self.initial.state.clone();
                cursor.state.resume_at(row + 1);
                cursor.scopes = ScopeStack::new();
            }
            cursor.history.insert(
                row,
                Cached {
                    exact: false,
                    record: LineRecord {
                        ops: Arc::from([]),
                        scopes: Some(Arc::new(cursor.scopes.clone())),
                        checkpoint: None,
                        spans: default_spans(range.len(), self.resolver.scheme().default_style()),
                        runs: Arc::from([]),
                        end_stack: ScopeStackId(0),
                        has_symbols: false,
                    },
                },
            );
        } else {
            let line = self.text.slice(range).to_string();
            let output = if line.len() > self.options.max_line_bytes {
                cursor.state.parse_line_uncached(&line)
            } else {
                cursor.state.parse_line(&line)
            }
            .map_err(|e| Error::Parse(e.to_string()))?;
            self.stats.parsed_lines += 1;
            if let Some(rewind) = output.rewind {
                let from = rewind.from_line;
                self.stats.rewinds += 1;
                self.stats.rewind_from =
                    Some(self.stats.rewind_from.map_or(from, |old| old.min(from)));
                let first = cursor.history.first_key_value().map_or(row, |(&n, _)| n);
                if from < first {
                    return Err(Error::Parse(
                        "branch exceeded retained replay history".into(),
                    ));
                }
                for (n, ops) in output.corrections {
                    if let Some(cached) = cursor.history.get_mut(&n) {
                        cached.record.ops = ops.into();
                    }
                }
                let mut scopes = if from == first {
                    cursor.anchor.clone()
                } else {
                    cursor.history[&(from - 1)]
                        .record
                        .scopes
                        .as_deref()
                        .expect("sparse rewind anchor")
                        .clone()
                };
                for n in from..row {
                    let cached = cursor
                        .history
                        .get_mut(&n)
                        .ok_or_else(|| Error::Parse("missing sparse branch history".into()))?;
                    let line = self
                        .text
                        .slice(complete_line_range(&self.text, n))
                        .to_string();
                    let rendered = style_line(
                        &line,
                        &cached.record.ops,
                        &mut scopes,
                        &mut self.resolver,
                        &mut self.interner,
                        &mut self.visual,
                    )?;
                    cached.record.spans = rendered.spans;
                    cached.record.runs = rendered.runs;
                    cached.record.end_stack = rendered.end_stack;
                    cached.record.scopes = Some(Arc::new(scopes.clone()));
                }
                cursor.scopes = scopes;
                self.checkpoints.retain(|&n, _| n <= from);
            }
            let rendered = style_line(
                &line,
                &output.ops,
                &mut cursor.scopes,
                &mut self.resolver,
                &mut self.interner,
                &mut self.visual,
            )?;
            cursor.history.insert(
                row,
                Cached {
                    exact: cursor.trusted,
                    record: LineRecord {
                        ops: output.ops.into(),
                        scopes: Some(Arc::new(cursor.scopes.clone())),
                        checkpoint: None,
                        spans: rendered.spans,
                        runs: rendered.runs,
                        end_stack: rendered.end_stack,
                        has_symbols: false,
                    },
                },
            );
        }
        cursor.next += 1;
        // Parser replay is limited to 128 preceding lines. Keep an extra anchor,
        // independently of viewport width, plus a bounded foreground lookback.
        let limit = self
            .options
            .max_viewport_lines
            .saturating_add(self.options.max_lookback_lines)
            .max(256);
        while cursor.history.len() > limit {
            cursor.anchor = cursor
                .history
                .pop_first()
                .expect("history nonempty")
                .1
                .record
                .scopes
                .as_deref()
                .expect("sparse history anchor")
                .clone();
        }
        Ok(())
    }
    fn batch(&self, lines: Range<usize>) -> HighlightBatch {
        let highlighted: Vec<_> = lines
            .clone()
            .map(|line| {
                let cached = self.visible.get(&line);
                StyledLine {
                    line,
                    exact: cached.is_some_and(|c| c.exact),
                    spans: cached.map_or_else(
                        || {
                            default_spans(
                                complete_line_range(&self.text, line).len(),
                                self.resolver.scheme().default_style(),
                            )
                        },
                        |c| c.record.spans.clone(),
                    ),
                }
            })
            .collect();
        let scopes = ScopeSnapshot {
            version: self.version,
            text: self.text.clone(),
            range: lines.clone(),
            stacks: self.interner.stacks.clone(),
            lines: lines
                .clone()
                .map(|line| {
                    let c = self.visible.get(&line).filter(|c| c.exact);
                    ScopedLine {
                        line,
                        exact: c.is_some(),
                        runs: c.map_or_else(|| Arc::from([]), |c| c.record.runs.clone()),
                        end_stack: c.map_or(ScopeStackId(0), |c| c.record.end_stack),
                    }
                })
                .collect(),
        };
        HighlightBatch {
            version: self.version,
            scheme_revision: self.scheme_revision,
            complete: highlighted.iter().all(|l| l.exact),
            lines,
            highlighted,
            scopes,
        }
    }
    /// Installs a transaction and discards sparse checkpoints after the first changed
    /// line. Work is bounded by retained metadata, with no scan of the document tail.
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
        for c in changes.changes() {
            if c.old_range.end > self.text.len_bytes()
                || !self.text.is_char_boundary(c.old_range.start)
                || !self.text.is_char_boundary(c.old_range.end)
            {
                return Err(Error::Snapshot("invalid old-text range"));
            }
            expected = expected
                .checked_sub(c.old_range.len())
                .and_then(|n| n.checked_add(c.new_len))
                .ok_or(Error::Snapshot("invalid replacement lengths"))?;
        }
        if expected != after.len_bytes() {
            return Err(Error::Snapshot("new text length does not match change set"));
        }
        let first = if let Some(c) = changes.changes().first() {
            self.text.byte_to_line(c.old_range.start)
        } else {
            if after != self.text {
                return Err(Error::Snapshot("empty change set changed text"));
            }
            self.text.len_lines()
        };
        self.checkpoints.retain(|&n, _| n <= first);
        let (&at, checkpoint) = self
            .checkpoints
            .last_key_value()
            .expect("line-zero checkpoint");
        self.background = Cursor::new(checkpoint.clone(), at, true);
        self.text = after;
        self.version = version;
        self.visible.clear();
        self.foreground = None;
        self.stats = HighlightStats::default();
        Ok(())
    }
    /// Recolors through bounded reparsing; old batches carry an older scheme_revision.
    pub fn set_color_scheme(&mut self, scheme: Arc<ColorScheme>) {
        self.resolver = Highlighter::new(scheme);
        self.interner = ScopeInterner::new(Arc::new(MetadataSet::new()));
        self.scheme_revision += 1;
        self.checkpoints = BTreeMap::from([(0, self.initial.clone())]);
        self.background = Cursor::new(self.initial.clone(), 0, true);
        self.visible.clear();
        self.foreground = None;
    }
}
