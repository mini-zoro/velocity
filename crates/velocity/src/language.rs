//! Versioned language services. Parsing and document analysis stay on workers.
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};
use vel_buffer::{ChangeId, ChangeSet};
use vel_color_scheme::{ColorScheme, Highlighter};
use vel_highlight::{DocumentHighlighter, HighlightBatch, HighlightService, StyledLine};
use vel_scope::{Scope, ScopeStack, ScopeStackOp, Selector};
use vel_syntax::{MetadataSet, ParseState, ScopedSpan, SyntaxSet};
use vel_text::Text;

#[derive(Debug)]
enum HighlightCommand {
    Edit(Text, ChangeId, ChangeSet),
    Scheme(Arc<ColorScheme>),
    Request(ChangeId, Vec<Range<usize>>),
    Stop,
}
/// The service's cooperative 2ms request can include an indivisible regex call.
/// A dispatcher keeps that call, as well as all parsing, off the UI thread.
#[derive(Debug)]
struct HighlightFrontend {
    tx: mpsc::Sender<HighlightCommand>,
    rx: mpsc::Receiver<Result<HighlightBatch, String>>,
    complete: Arc<std::sync::atomic::AtomicBool>,
    latest: Arc<AtomicU64>,
}
impl HighlightFrontend {
    fn new(engine: DocumentHighlighter) -> Result<Self, String> {
        let version = engine.version().0;
        let mut service = HighlightService::new(engine).map_err(|e| e.to_string())?;
        let (tx, commands) = mpsc::channel();
        let (results, rx) = mpsc::channel();
        let complete = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ready = complete.clone();
        let latest = Arc::new(AtomicU64::new(version));
        let current = latest.clone();
        std::thread::Builder::new()
            .name("velocity-highlight-dispatch".into())
            .spawn(move || {
                let mut viewport: Option<(ChangeId, Vec<Range<usize>>)> = None;
                let mut pending = std::collections::VecDeque::new();
                let mut validated = false;
                let mut refresh = std::time::Instant::now();
                loop {
                    let idle = validated && pending.is_empty();
                    match commands.recv_timeout(std::time::Duration::from_millis(if idle {
                        50
                    } else {
                        1
                    })) {
                        Ok(HighlightCommand::Edit(text, version, changes)) => {
                            viewport = None;
                            pending.clear();
                            validated = false;
                            if let Err(e) = service.apply_change(text, version, changes) {
                                let _ = results.send(Err(e.to_string()));
                            }
                        }
                        Ok(HighlightCommand::Scheme(scheme)) => {
                            viewport = None;
                            pending.clear();
                            validated = false;
                            if let Err(e) = service.set_color_scheme(scheme) {
                                let _ = results.send(Err(e.to_string()));
                            }
                        }
                        Ok(HighlightCommand::Request(version, lines)) => {
                            pending = lines.iter().cloned().collect();
                            viewport = Some((version, lines));
                            refresh = std::time::Instant::now();
                        }
                        Ok(HighlightCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    while let Some(batch) = service.try_recv() {
                        if let Ok(batch) = &batch
                            && batch.complete
                        {
                            pending.retain(|r| *r != batch.lines);
                        }
                        if results.send(batch.map_err(|e| e.to_string())).is_err() {
                            return;
                        }
                    }
                    if let Some(p) = service.progress() {
                        let full = p.version.0 == current.load(Ordering::Acquire)
                            && p.parsed_lines == p.total_lines;
                        ready.store(full, Ordering::Release);
                        // A service watches one interval. Refresh every visible folded interval
                        // after convergence, and during parsing to pick up branch corrections.
                        if let Some((_, ranges)) = &viewport
                            && ((full && !validated)
                                || (!full
                                    && ranges.len() > 1
                                    && pending.is_empty()
                                    && refresh.elapsed() >= std::time::Duration::from_millis(16)))
                        {
                            pending = ranges.iter().cloned().collect();
                            refresh = std::time::Instant::now();
                        }
                        if full {
                            validated = true;
                        }
                    }
                    if let Some((version, _)) = &viewport
                        && version.0 == current.load(Ordering::Acquire)
                    {
                        for _ in 0..4 {
                            let Some(lines) = pending.pop_front() else {
                                break;
                            };
                            let result = service.request_lines(*version, lines.clone());
                            if result.as_ref().is_ok_and(|b| !b.complete) {
                                pending.push_back(lines);
                            }
                            if results.send(result.map_err(|e| e.to_string())).is_err() {
                                return;
                            }
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            rx,
            complete,
            latest,
        })
    }
    fn send(&self, command: HighlightCommand) {
        match &command {
            HighlightCommand::Edit(_, version, _) => {
                self.latest.store(version.0, Ordering::Release);
                self.complete.store(false, Ordering::Release);
            }
            HighlightCommand::Scheme(_) => {
                self.complete.store(false, Ordering::Release);
            }
            _ => {}
        }
        let _ = self.tx.send(command);
    }
}
impl Drop for HighlightFrontend {
    fn drop(&mut self) {
        let _ = self.tx.send(HighlightCommand::Stop);
    }
}

#[derive(Debug)]
pub struct Assets {
    pub syntaxes: Arc<SyntaxSet>,
    pub metadata: Arc<MetadataSet>,
}
impl Assets {
    pub fn bundled() -> Result<Arc<Self>, String> {
        static ASSETS: OnceLock<Result<Arc<Assets>, String>> = OnceLock::new();
        ASSETS.get_or_init(|| {
            let root = packages_root();
            let cache = std::env::temp_dir().join("velocity-syntax-cache");
            // The public corpus has no Plain Text YAML. Compose a caller-owned source
            // tree before load_cached so its lazy index includes our original fallback.
            let stage = cache.join(format!("sources-{}", std::process::id()));
            stage_syntaxes(&root, &stage)?;
            let default = stage.join("Default"); std::fs::create_dir_all(&default).map_err(|e| e.to_string())?;
            std::fs::write(default.join("Plain Text.sublime-syntax"), "name: Plain Text\nfile_extensions: [txt]\nscope: text.plain\ncontexts: { main: [] }\n").map_err(|e| e.to_string())?;
            let syntaxes = SyntaxSet::load_cached(&stage, &cache).map_err(|e| e.to_string());
            let _ = std::fs::remove_dir_all(&stage);
            Ok(Arc::new(Self {
                syntaxes: Arc::new(syntaxes?.0),
                metadata: Arc::new(MetadataSet::load_directories(&[root]).map_err(|e| e.to_string())?),
            }))
        }).clone()
    }
    pub fn with_user_metadata(base: &Arc<Self>, data: Option<&Path>) -> Result<Arc<Self>, String> {
        let Some(data) = data else {
            return Ok(base.clone());
        };
        let root = packages_root();
        let user_packages = data.join("Packages");
        // First launch: create the user package tree (as Sublime Text does). Failing to create
        // it must never prevent startup, and missing directories are simply not scanned.
        let _ = std::fs::create_dir_all(user_packages.join("User"));
        fn has_metadata(dir: &Path) -> bool {
            std::fs::read_dir(dir)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry.file_type().is_ok_and(|ty| {
                        if ty.is_dir() {
                            has_metadata(&entry.path())
                        } else {
                            ty.is_file()
                                && entry
                                    .path()
                                    .extension()
                                    .is_some_and(|e| e == "tmPreferences")
                        }
                    })
                })
        }
        if !has_metadata(&user_packages) {
            return Ok(base.clone());
        }
        let roots: Vec<PathBuf> = [root, user_packages]
            .into_iter()
            .filter(|p| p.is_dir())
            .collect();
        Ok(Arc::new(Self {
            syntaxes: base.syntaxes.clone(),
            metadata: Arc::new(MetadataSet::load_directories(&roots).map_err(|e| e.to_string())?),
        }))
    }
}
fn packages_root() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(contents) = exe.parent().and_then(Path::parent)
    {
        let resources = contents.join("Resources/Syntaxes");
        if resources.is_dir() {
            return resources;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/sublime-packages")
}
fn stage_syntaxes(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            stage_syntaxes(&path, &target.join(entry.file_name()))?;
        } else if path.extension().is_some_and(|e| e == "sublime-syntax") {
            std::fs::copy(&path, target.join(entry.file_name())).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Pair {
    pub open: Range<usize>,
    pub close: Range<usize>,
    pub kind: char,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    pub range: Range<usize>,
    pub level: usize,
    pub attributes: bool,
}
#[derive(Clone, Debug)]
pub struct MiniToken {
    pub column: usize,
    pub width: usize,
    pub color: vel_render::scene::Color,
}
#[derive(Clone, Debug)]
pub struct MiniLine {
    pub row: usize,
    pub tokens: Vec<MiniToken>,
}
#[derive(Debug)]
pub struct Analysis {
    pub scheme: Arc<ColorScheme>,
    pub version: ChangeId,
    pub symbols: Vec<(String, usize)>,
    pub pairs: Vec<Pair>,
    pair_tree: Vec<usize>,
    pub folds: Vec<Fold>,
    pub minimap: Arc<[MiniLine]>,
}
impl Analysis {
    /// Finds the innermost enabled pair in logarithmic time plus active nesting depth.
    pub fn pair_at(
        &self,
        byte: usize,
        content: bool,
        enabled: impl Fn(char) -> bool,
    ) -> Option<&Pair> {
        if self.pair_tree.is_empty() {
            return None;
        }
        let limit = self.pairs.partition_point(|p| p.open.start <= byte);
        fn visit<'a>(
            analysis: &'a Analysis,
            node: usize,
            range: Range<usize>,
            limit: usize,
            byte: usize,
            content: bool,
            enabled: &impl Fn(char) -> bool,
        ) -> Option<&'a Pair> {
            if range.start >= limit || analysis.pair_tree[node] < byte {
                return None;
            }
            if range.len() == 1 {
                let pair = analysis.pairs.get(range.start)?;
                return (enabled(pair.kind)
                    && (pair.open.contains(&byte)
                        || pair.close.contains(&byte)
                        || pair.open.end == byte
                        || pair.close.end == byte
                        || (content && pair.open.end <= byte && byte <= pair.close.start)))
                    .then_some(pair);
            }
            let middle = (range.start + range.end) / 2;
            visit(
                analysis,
                node * 2 + 1,
                middle..range.end,
                limit,
                byte,
                content,
                enabled,
            )
            .or_else(|| {
                visit(
                    analysis,
                    node * 2,
                    range.start..middle,
                    limit,
                    byte,
                    content,
                    enabled,
                )
            })
        }
        visit(
            self,
            1,
            0..self.pair_tree.len() / 2,
            limit,
            byte,
            content,
            &enabled,
        )
    }
}
#[derive(Clone, Debug)]
pub struct LocalScope {
    pub range: Range<usize>,
    pub scopes: Arc<[Scope]>,
}
#[derive(Debug)]
struct AnalysisTask {
    tx: mpsc::Sender<Job>,
    rx: mpsc::Receiver<Result<Arc<Analysis>, String>>,
    latest: Arc<AtomicU64>,
    scopes: mpsc::Receiver<Arc<ScopeSnapshot>>,
}
#[derive(Debug)]
struct ScopeSnapshot {
    version: ChangeId,
    lines: Vec<Arc<[LocalScope]>>,
}
#[derive(Debug)]
struct Job {
    text: Text,
    version: ChangeId,
    changes: Option<ChangeSet>,
    scheme: Arc<ColorScheme>,
    tab: usize,
}
impl Drop for AnalysisTask {
    fn drop(&mut self) {
        self.latest.store(u64::MAX, Ordering::Release);
        while let Ok(Ok(old)) = self.rx.try_recv() {
            retire(old);
        }
        while let Ok(old) = self.scopes.try_recv() {
            retire(old);
        }
    }
}
#[derive(Clone, Debug)]
struct Record {
    checkpoint: Option<ParseState>,
    end: ScopeStack,
    ops: Arc<[(usize, ScopeStackOp)]>,
    spans: Arc<[LocalScope]>,
}
#[derive(Debug)]
struct ScopeCache {
    corrected_from: Option<usize>,
    text: Text,
    records: Vec<Option<Record>>,
    frontier: usize,
    must_parse: usize,
    reusable: usize,
    initial: ParseState,
    parser: ParseState,
    stack: ScopeStack,
}
impl ScopeCache {
    fn new(text: Text, assets: &Assets, syntax: &str) -> Self {
        let reference = assets
            .syntaxes
            .find_syntax_by_path(syntax)
            .unwrap_or_else(|| assets.syntaxes.find_syntax_plain_text());
        let initial = ParseState::new(assets.syntaxes.clone(), reference);
        Self {
            corrected_from: None,
            records: vec![None; text.len_lines()],
            text,
            frontier: 0,
            must_parse: 0,
            reusable: 0,
            parser: initial.clone(),
            initial,
            stack: ScopeStack::new(),
        }
    }
    fn update(&mut self, after: Text, changes: &ChangeSet) {
        let Some(first) = changes.changes().first() else {
            self.text = after;
            return;
        };
        let last = changes.changes().last().expect("first change");
        let start = self.text.byte_to_line(first.old_range.start);
        let end = self.text.byte_to_line(last.old_range.end) + 1;
        let new_end = after.len_lines() - (self.records.len() - end);
        let compare = self.records[end - 1].clone();
        let need = self.must_parse;
        let reusable = self.frontier.max(self.reusable);
        self.records
            .splice(start..end, std::iter::repeat_n(None, new_end - start));
        if new_end > start {
            self.records[new_end - 1] = compare;
        }
        self.reusable = if reusable >= end {
            reusable - end + new_end
        } else {
            reusable.min(start)
        };
        let mapped_need = if need >= end {
            need - end + new_end
        } else if need > start {
            new_end
        } else {
            need
        };
        self.must_parse = new_end.max(mapped_need);
        let mut resume = self.frontier.min(start);
        while resume > 0
            && self.records[resume - 1]
                .as_ref()
                .and_then(|r| r.checkpoint.as_ref())
                .is_none()
        {
            resume -= 1;
        }
        while resume > 0 {
            let record = self.records[resume - 1].as_ref().expect("checkpoint");
            let mut parser = record.checkpoint.clone().expect("checkpoint");
            if parser.resume_at(resume) {
                self.parser = parser;
                self.stack = record.end.clone();
                break;
            }
            resume -= 1;
        }
        if resume == 0 {
            self.parser = self.initial.clone();
            self.stack = ScopeStack::new();
        }
        self.frontier = resume;
        self.text = after;
    }
    fn line_scopes(
        line: &str,
        ops: &[(usize, ScopeStackOp)],
        stack: &mut ScopeStack,
    ) -> Result<Arc<[LocalScope]>, String> {
        let mut spans = Vec::new();
        let mut at = 0;
        for (byte, op) in ops {
            if *byte > at {
                spans.push(LocalScope {
                    range: at..*byte,
                    scopes: stack.as_slice().into(),
                });
            }
            stack.apply(op).map_err(|e| e.to_string())?;
            at = *byte;
        }
        if at < line.len() {
            spans.push(LocalScope {
                range: at..line.len(),
                scopes: stack.as_slice().into(),
            });
        }
        if spans.is_empty() {
            spans.push(LocalScope {
                range: 0..line.len(),
                scopes: stack.as_slice().into(),
            });
        }
        Ok(spans.into())
    }
    fn next(&mut self) -> Result<(), String> {
        self.corrected_from = None;
        let row = self.frontier;
        let line = self.text.line(row).to_string();
        let output = self.parser.parse_line(&line).map_err(|e| e.to_string())?;
        let mut rewind = output.rewind.map(|r| r.from_line);
        for (n, ops) in output.corrections {
            if n < row
                && let Some(record) = self.records[n].as_mut()
            {
                record.ops = ops.into();
                record.checkpoint = None;
                rewind = Some(rewind.map_or(n, |old| old.min(n)));
            }
        }
        if let Some(from) = rewind {
            self.corrected_from = Some(from);
            let mut stack = if from == 0 {
                ScopeStack::new()
            } else {
                self.records[from - 1]
                    .as_ref()
                    .ok_or("missing rewind anchor")?
                    .end
                    .clone()
            };
            for n in from..row {
                let record = self.records[n].as_mut().ok_or("missing rewind line")?;
                record.spans =
                    Self::line_scopes(&self.text.line(n).to_string(), &record.ops, &mut stack)?;
                record.end = stack.clone();
                record.checkpoint = None;
            }
            self.stack = stack;
        }
        let spans = Self::line_scopes(&line, &output.ops, &mut self.stack)?;
        let converged = row + 1 >= self.must_parse
            && !self.parser.is_speculative()
            && self.records[row].as_ref().is_some_and(|r| {
                r.end == self.stack
                    && r.checkpoint
                        .as_ref()
                        .is_some_and(|p| !p.is_speculative() && *p == self.parser)
            });
        self.records[row] = Some(Record {
            checkpoint: Some(self.parser.clone()),
            end: self.stack.clone(),
            ops: output.ops.into(),
            spans,
        });
        self.frontier += 1;
        if converged && self.reusable > self.frontier {
            let end = self.reusable.min(self.records.len());
            if let Some(record) = &self.records[end - 1]
                && let Some(mut parser) = record.checkpoint.clone()
                && !parser.is_speculative()
                && parser.resume_at(end)
            {
                self.parser = parser;
                self.stack = record.end.clone();
                self.frontier = end;
            }
        }
        Ok(())
    }
}
impl AnalysisTask {
    fn new(assets: Arc<Assets>, syntax: String) -> Result<Self, String> {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (results, rx) = mpsc::channel();
        let (scope_results, scopes) = mpsc::channel();
        let latest = Arc::new(AtomicU64::new(0));
        let current = latest.clone();
        std::thread::Builder::new()
            .name("velocity-language".into())
            .spawn(move || {
                let mut cache: Option<ScopeCache> = None;
                while let Ok(mut job) = jobs.recv() {
                    loop {
                        if let Some(old) = &mut cache {
                            if let Some(changes) = &job.changes {
                                old.update(job.text.clone(), changes);
                            } else {
                                cache = None;
                            }
                        }
                        let state = cache.get_or_insert_with(|| {
                            ScopeCache::new(job.text.clone(), &assets, &syntax)
                        });
                        let result = (|| {
                            let mut published = 0;
                            let mut next_publish = 64;
                            while state.frontier < state.text.len_lines() {
                                if current.load(Ordering::Acquire) != job.version.0 {
                                    return Ok(None);
                                }
                                state.next()?;
                                let advance = state.frontier >= next_publish;
                                let correction =
                                    state.corrected_from.is_some_and(|from| from < published);
                                if advance || correction {
                                    let count = if advance { state.frontier } else { published };
                                    let lines = state.records[..count]
                                        .iter()
                                        .map(|r| {
                                            r.as_ref().expect("parsed scope line").spans.clone()
                                        })
                                        .collect();
                                    let _ = scope_results.send(Arc::new(ScopeSnapshot {
                                        version: job.version,
                                        lines,
                                    }));
                                    if advance {
                                        published = count;
                                        next_publish = count.saturating_mul(2);
                                    }
                                }
                            }
                            if current.load(Ordering::Acquire) != job.version.0 {
                                return Ok(None);
                            }
                            if published != state.frontier {
                                let lines = state
                                    .records
                                    .iter()
                                    .map(|r| r.as_ref().expect("parsed scope line").spans.clone())
                                    .collect();
                                let _ = scope_results.send(Arc::new(ScopeSnapshot {
                                    version: job.version,
                                    lines,
                                }));
                            }
                            analyze(state, &assets.metadata, &job, &current)
                                .map(|a| a.map(Arc::new))
                        })();
                        match result {
                            Ok(Some(a)) => {
                                let _ = results.send(Ok(a));
                            }
                            Ok(None) => {}
                            Err(e) => {
                                let _ = results.send(Err(e));
                                cache = None;
                            }
                        }
                        match jobs.try_recv() {
                            Ok(next) => job = next,
                            Err(_) => break,
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            rx,
            latest,
            scopes,
        })
    }
    fn submit(&self, job: Job) {
        self.latest.store(job.version.0, Ordering::Release);
        let _ = self.tx.send(job);
    }
}

/// One instance per Buffer; split views borrow its results through RefCell.
#[derive(Debug)]
pub struct Language {
    pub assets: Arc<Assets>,
    pub syntax: String,
    pub name: String,
    pub root: Scope,
    pub scheme: Arc<ColorScheme>,
    pub version: ChangeId,
    service: HighlightFrontend,
    task: AnalysisTask,
    pub analysis: Option<Arc<Analysis>>,
    pub styled: BTreeMap<usize, StyledLine>,
    pub errors: Vec<String>,
    pub minimap: Arc<[MiniLine]>,
    scopes: Option<Arc<ScopeSnapshot>>,
    text: Text,
    scheme_revision: u64,
    requested: Option<(ChangeId, u64, Vec<Range<usize>>)>,
    visible_views: BTreeMap<vel_buffer::ViewId, Vec<Range<usize>>>,
}
pub type LanguageCell = std::rc::Rc<RefCell<Option<Language>>>;
impl Language {
    pub fn new(
        assets: Arc<Assets>,
        syntax: &str,
        text: Text,
        version: ChangeId,
        scheme: Arc<ColorScheme>,
        tab: usize,
    ) -> Result<Self, String> {
        let reference = vel_highlight::detect_syntax(&assets.syntaxes, "", "", Some(syntax))
            .ok_or_else(|| format!("unknown syntax {syntax}"))?;
        let path = reference.path.clone();
        let name = reference.name.clone();
        let root = reference.scope;
        let service = HighlightFrontend::new(DocumentHighlighter::new(
            text.clone(),
            version,
            assets.syntaxes.clone(),
            reference,
            scheme.clone(),
        ))?;
        let task = AnalysisTask::new(assets.clone(), path.clone())?;
        task.submit(Job {
            text: text.clone(),
            version,
            changes: None,
            scheme: scheme.clone(),
            tab,
        });
        Ok(Self {
            assets,
            syntax: path,
            name,
            root,
            scheme,
            version,
            service,
            task,
            analysis: None,
            styled: BTreeMap::new(),
            errors: Vec::new(),
            minimap: Arc::from([]),
            scopes: None,
            text,
            scheme_revision: 0,
            requested: None,
            visible_views: BTreeMap::new(),
        })
    }
    pub fn change(&mut self, text: Text, version: ChangeId, changes: ChangeSet, tab: usize) {
        let viewport = self.requested.as_ref().map(|(_, _, r)| r.clone());
        if !self.minimap.is_empty() {
            let changed_rows: Vec<_> = changes
                .changes()
                .iter()
                .map(|c| {
                    self.text.byte_to_line(c.old_range.start)
                        ..=self.text.byte_to_line(c.old_range.end)
                })
                .collect();
            let same_lines = self.text.len_lines() == text.len_lines()
                && changes.changes().iter().all(|c| {
                    let at = changes.transform_point(c.old_range.start, vel_buffer::Bias::Left);
                    self.text.byte_to_line(c.old_range.start)
                        == self.text.byte_to_line(c.old_range.end)
                        && text.byte_to_line(at) == text.byte_to_line(at + c.new_len)
                });
            let samples = Arc::make_mut(&mut self.minimap);
            for sample in samples {
                let touched = changed_rows.iter().any(|rows| rows.contains(&sample.row));
                if !same_lines {
                    let start = self
                        .text
                        .line_to_byte(sample.row.min(self.text.len_lines() - 1));
                    sample.row = text.byte_to_line(
                        changes
                            .transform_point(start, vel_buffer::Bias::Left)
                            .min(text.len_bytes()),
                    );
                }
                if touched {
                    let line = text.line(sample.row).to_string();
                    sample.tokens = vec![MiniToken {
                        column: 0,
                        width: crate::editor::cells_with_tab(line.trim_end_matches('\n'), tab),
                        color: rgba(self.scheme.default_style().foreground),
                    }];
                }
            }
        }
        self.service.send(HighlightCommand::Edit(
            text.clone(),
            version,
            changes.clone(),
        ));
        self.version = version;
        self.text = text.clone();
        if let Some(analysis) = self.analysis.take() {
            retire(analysis);
        }
        self.styled.clear();
        self.requested = None;
        self.task.submit(Job {
            text,
            version,
            changes: Some(changes),
            scheme: self.scheme.clone(),
            tab,
        });
        if let Some(viewport) = viewport {
            self.request_visible(viewport);
        }
    }
    pub fn recolor(&mut self, scheme: Arc<ColorScheme>, text: Text, tab: usize) {
        if Arc::ptr_eq(&scheme, &self.scheme) {
            return;
        }
        let viewport = self.requested.as_ref().map(|(_, _, r)| r.clone());
        self.service.send(HighlightCommand::Scheme(scheme.clone()));
        self.scheme = scheme.clone();
        self.scheme_revision += 1;
        self.styled.clear();
        self.requested = None;
        self.minimap = Arc::from([]);
        if let Some(analysis) = self.analysis.take() {
            retire(analysis);
        }
        self.task.submit(Job {
            text,
            version: self.version,
            changes: Some(ChangeSet::new(vec![])),
            scheme,
            tab,
        });
        if let Some(viewport) = viewport {
            self.request_visible(viewport);
        }
    }
    fn accept(&mut self, batch: vel_highlight::HighlightBatch) -> bool {
        if batch.version != self.version || batch.scheme_revision != self.scheme_revision {
            return false;
        }
        let changed = batch.highlighted.iter().any(|line| {
            !self
                .styled
                .get(&line.line)
                .is_some_and(|old| old.spans == line.spans && old.exact == line.exact)
        });
        for line in batch.highlighted {
            if !line.exact && self.styled.get(&line.line).is_some_and(|l| l.exact) {
                continue;
            }
            self.styled.insert(line.line, line);
        }
        // Bounded viewport cache. Analysis owns the small whole-document overview.
        if self.styled.len() > 4096 {
            self.styled.retain(|row, _| batch.lines.contains(row));
        }
        changed
    }
    pub fn retain_visible_views(&mut self, views: &BTreeSet<vel_buffer::ViewId>) {
        let before = self.visible_views.len();
        self.visible_views.retain(|id, _| views.contains(id));
        if self.visible_views.len() != before {
            self.requested = None;
        }
    }
    pub fn request_view(&mut self, id: vel_buffer::ViewId, lines: Vec<Range<usize>>) {
        if self.visible_views.get(&id) == Some(&lines)
            && self.requested.as_ref().is_some_and(|(version, scheme, _)| {
                *version == self.version && *scheme == self.scheme_revision
            })
        {
            return;
        }
        self.visible_views.insert(id, lines);
        let mut ranges: Vec<_> = self.visible_views.values().flatten().cloned().collect();
        ranges.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in ranges {
            if let Some(last) = merged.last_mut()
                && range.start <= last.end
            {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        self.request_visible(merged);
    }
    pub fn request_visible(&mut self, lines: Vec<Range<usize>>) {
        let identity = (self.version, self.scheme_revision, lines.clone());
        if self.requested.as_ref() == Some(&identity) {
            return;
        }
        self.requested = Some(identity);
        self.service
            .send(HighlightCommand::Request(self.version, lines));
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(scopes) = self.task.scopes.try_recv() {
            if scopes.version == self.version {
                if let Some(old) = self.scopes.replace(scopes) {
                    retire(old);
                }
                changed = true;
            } else {
                retire(scopes);
            }
        }
        while let Ok(result) = self.service.rx.try_recv() {
            match result {
                Ok(batch) => changed |= self.accept(batch),
                Err(e) => self.errors.push(e),
            }
        }
        while let Ok(result) = self.task.rx.try_recv() {
            match result {
                Ok(a) if a.version == self.version && Arc::ptr_eq(&a.scheme, &self.scheme) => {
                    self.minimap = a.minimap.clone();
                    if let Some(old) = self.analysis.replace(a) {
                        retire(old);
                    }
                    changed = true;
                }
                Ok(old) => retire(old),
                Err(e) => self.errors.push(e),
            }
        }
        changed
    }
    pub fn ready(&self) -> bool {
        self.scopes_ready()
            && self
                .analysis
                .as_ref()
                .is_some_and(|a| a.version == self.version)
    }
    pub fn scopes_ready(&self) -> bool {
        self.scopes
            .as_ref()
            .is_some_and(|s| s.version == self.version && s.lines.len() == self.text.len_lines())
    }
    pub fn scopes_ready_at(&self, text: &Text, byte: usize) -> bool {
        self.scopes
            .as_ref()
            .is_some_and(|s| s.version == self.version && text.byte_to_line(byte) < s.lines.len())
    }
    pub fn busy(&self) -> bool {
        !self.ready()
            || !self.highlight_complete()
            || self.requested.as_ref().is_some_and(|(_, _, ranges)| {
                ranges.iter().any(|r| {
                    r.clone()
                        .any(|row| !self.styled.get(&row).is_some_and(|line| line.exact))
                })
            })
    }
    pub fn highlight_complete(&self) -> bool {
        self.service.complete.load(Ordering::Acquire)
    }
    pub fn scopes(&self, text: &Text, byte: usize) -> ScopeStack {
        let row = text.byte_to_line(byte);
        let col = byte - text.line_to_byte(row);
        let mut stack = ScopeStack::new();
        let scopes = self
            .scopes
            .as_ref()
            .filter(|a| a.version == self.version)
            .and_then(|a| a.lines.get(row))
            .and_then(|line| {
                line.iter()
                    .find(|s| s.range.contains(&col))
                    .or_else(|| line.last())
            });
        if let Some(span) = scopes {
            for scope in span.scopes.iter() {
                stack.push(*scope);
            }
        } else {
            stack.push(self.root);
        }
        stack
    }
    pub fn selector(&self, text: &Text, selector: &str, byte: usize) -> bool {
        selector
            .parse::<Selector>()
            .ok()
            .and_then(|s| s.does_match(self.scopes(text, byte).as_slice()))
            .is_some()
    }
}
impl Drop for Language {
    fn drop(&mut self) {
        if let Some(old) = self.scopes.take() {
            retire(old);
        }
        if let Some(old) = self.analysis.take() {
            retire(old);
        }
    }
}
fn retire<T: Send + 'static>(value: T) {
    static RETIRE: OnceLock<mpsc::Sender<Box<dyn Send>>> = OnceLock::new();
    let tx = RETIRE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Box<dyn Send>>();
        std::thread::Builder::new()
            .name("velocity-analysis-retire".into())
            .spawn(move || {
                for old in rx {
                    drop(old);
                }
            })
            .expect("analysis retirement worker");
        tx
    });
    let _ = tx.send(Box::new(value));
}

pub fn rgba(color: vel_color_scheme::Color) -> vel_render::scene::Color {
    color.rgba8().map(|c| f32::from(c) / 255.)
}
fn excluded(scopes: &[Scope]) -> bool {
    scopes.iter().any(|s| {
        let n = s.build_string();
        n.starts_with("string") || n.starts_with("comment")
    })
}

fn analyze(
    cache: &ScopeCache,
    metadata: &MetadataSet,
    job: &Job,
    latest: &AtomicU64,
) -> Result<Option<Analysis>, String> {
    let text = &cache.text;
    let lines: Vec<_> = cache
        .records
        .iter()
        .map(|r| r.as_ref().expect("parsed line").spans.clone())
        .collect();
    let mut spans = Vec::new();
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut pairs = Vec::new();
    let mut angles = Vec::new();
    let mut minimap = Vec::new();
    let mut resolver = Highlighter::new(job.scheme.clone());
    let step = text.len_lines().div_ceil(1024).max(1);
    for (row, line_spans) in lines.iter().enumerate() {
        if row % 32 == 0 && latest.load(Ordering::Acquire) != job.version.0 {
            return Ok(None);
        }
        let start = text.line_to_byte(row);
        let line = text.line(row).to_string();
        let mut tokens = Vec::new();
        let mut col = 0;
        for span in line_spans.iter() {
            let part = &line[span.range.clone()];
            let mut scopes = ScopeStack::new();
            for scope in span.scopes.iter() {
                scopes.push(*scope);
            }
            if row % step == 0 {
                let style = resolver.style_for(&scopes);
                let width =
                    crate::editor::cells_with_tab_at(part.trim_end_matches('\n'), job.tab, col);
                if !part.trim().is_empty() {
                    tokens.push(MiniToken {
                        column: col,
                        width,
                        color: rgba(job.scheme.style_for_token(style, part).foreground),
                    });
                }
                col += width;
            }
            if !excluded(&span.scopes) {
                for (at, c) in part.char_indices() {
                    let byte = start + span.range.start + at;
                    match c {
                        '<' => angles.push(byte),
                        '>' => {
                            if let Some(at) = angles.pop() {
                                pairs.push(Pair {
                                    open: at..at + 1,
                                    close: byte..byte + 1,
                                    kind: '<',
                                });
                            }
                        }
                        '(' | '[' | '{' => stack.push((c, byte)),
                        ')' | ']' | '}' => {
                            let open = match c {
                                ')' => '(',
                                ']' => '[',
                                _ => '{',
                            };
                            if let Some((_, at)) = stack.pop_if(|s| s.0 == open) {
                                pairs.push(Pair {
                                    open: at..at + 1,
                                    close: byte..byte + 1,
                                    kind: open,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            spans.push(ScopedSpan {
                range: start + span.range.start..start + span.range.end,
                scopes: span.scopes.to_vec(),
            });
        }
        if row % step == 0 {
            minimap.push(MiniLine { row, tokens });
        }
    }
    let source = text.to_string();
    let symbols = metadata
        .symbols(&source, &spans)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|s| s.local && !s.name.trim().is_empty())
        .map(|s| (s.name, s.range.start))
        .collect();
    let mut folds = Vec::new();
    // Tags use scope-selected markup, with quote-aware scanning of attributes.
    let mut tags: Vec<(String, Range<usize>)> = Vec::new();
    let mut at = 0;
    while let Some(relative) = source[at..].find('<') {
        let begin = at + relative;
        at = begin + 1;
        let selected = spans.get(spans.partition_point(|s| s.range.end <= begin));
        if !selected.is_some_and(|s| {
            !s.scopes
                .iter()
                .any(|s| s.build_string().starts_with("comment"))
                && s.scopes.iter().any(|s| {
                    let n = s.build_string();
                    n.starts_with("meta.tag") || n.starts_with("punctuation.definition.tag")
                })
        }) {
            continue;
        }
        let closing = source.as_bytes().get(at) == Some(&b'/');
        if closing {
            at += 1;
        }
        let name_start = at;
        while source
            .as_bytes()
            .get(at)
            .is_some_and(|c| c.is_ascii_alphanumeric() || b"-_:".contains(c))
        {
            at += 1;
        }
        if at == name_start {
            continue;
        }
        let name = source[name_start..at].to_ascii_lowercase();
        let name_end = at;
        let mut quote = None;
        for (offset, c) in source[at..].char_indices() {
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
            } else if c == '\'' || c == '"' {
                quote = Some(c);
            } else if c == '>' {
                at += offset + 1;
                break;
            }
        }
        if !source[..at].ends_with('>') {
            break;
        }
        let range = begin..at;
        if closing {
            if tags.last().is_some_and(|(n, _)| *n == name) {
                let (_, open) = tags.pop().expect("tag");
                pairs.push(Pair {
                    open,
                    close: range,
                    kind: 't',
                });
            }
        } else if !source[..at].ends_with("/>")
            && ![
                "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta",
                "param", "source", "track", "wbr",
            ]
            .contains(&name.as_str())
        {
            tags.push((name, range));
        }
        if !closing && !source[name_end..at - 1].trim().is_empty() {
            folds.push(Fold {
                range: name_end..at - 1,
                level: 1,
                attributes: true,
            });
        }
    }
    for pair in &pairs {
        if pair.kind == '<' {
            continue;
        }
        let first = text.byte_to_line(pair.open.start);
        let last = text.byte_to_line(pair.close.start);
        if last > first + 1 {
            folds.push(Fold {
                range: text.line_to_byte(first + 1).saturating_sub(1)..text.line_to_byte(last),
                level: 1,
                attributes: false,
            });
        }
    }
    // Indentation regions supplement brace/tag languages (notably Python).
    let mut indents: Vec<(usize, usize)> = Vec::new();
    let mut previous = None;
    for row in 0..text.len_lines() {
        let line = text.line(row).to_string();
        if line.trim().is_empty() {
            continue;
        }
        let indent = vel_syntax::indent::indentation(&line, job.tab);
        while indents.last().is_some_and(|(level, _)| *level >= indent) {
            let (_, header) = indents.pop().expect("indent");
            if row > header + 1 {
                folds.push(Fold {
                    range: text.line_to_byte(header + 1) - 1..text.line_to_byte(row),
                    level: 1,
                    attributes: false,
                });
            }
        }
        if let Some((old_indent, header)) = previous
            && indent > old_indent
        {
            indents.push((old_indent, header));
        }
        previous = Some((indent, row));
    }
    for (_, header) in indents {
        if text.len_lines() > header + 1 {
            folds.push(Fold {
                range: text.line_to_byte(header + 1) - 1..text.len_bytes(),
                level: 1,
                attributes: false,
            });
        }
    }
    // Contiguous block meta scopes contribute regions even without literal braces.
    let mut blocks: BTreeMap<Scope, Range<usize>> = BTreeMap::new();
    for span in &spans {
        let active: BTreeSet<_> = span
            .scopes
            .iter()
            .copied()
            .filter(|s| {
                let n = s.build_string();
                n.starts_with("meta.block") || n.starts_with("meta.function.body")
            })
            .collect();
        let ended: Vec<_> = blocks
            .keys()
            .filter(|s| !active.contains(s))
            .copied()
            .collect();
        for scope in ended {
            let range = blocks.remove(&scope).expect("block");
            add_block_fold(text, range, &mut folds);
        }
        for scope in active {
            blocks
                .entry(scope)
                .and_modify(|r| r.end = span.range.end)
                .or_insert(span.range.clone());
        }
    }
    for (_, range) in blocks {
        add_block_fold(text, range, &mut folds);
    }
    folds.sort_by_key(|f| (f.range.start, std::cmp::Reverse(f.range.end), f.attributes));
    folds.dedup_by(|a, b| a.range == b.range);
    let mut ends = Vec::new();
    for fold in &mut folds {
        while ends.last().is_some_and(|end| *end <= fold.range.start) {
            ends.pop();
        }
        fold.level = ends.len() + 1;
        ends.push(fold.range.end);
    }
    pairs.sort_by_key(|p| p.open.start);
    let mut pair_tree = vec![0; pairs.len().max(1).next_power_of_two() * 2];
    let base = pair_tree.len() / 2;
    for (i, pair) in pairs.iter().enumerate() {
        pair_tree[base + i] = pair.close.end;
    }
    for i in (1..base).rev() {
        pair_tree[i] = pair_tree[i * 2].max(pair_tree[i * 2 + 1]);
    }
    if latest.load(Ordering::Acquire) != job.version.0 {
        return Ok(None);
    }
    Ok(Some(Analysis {
        scheme: job.scheme.clone(),
        version: job.version,
        symbols,
        pairs,
        pair_tree,
        folds,
        minimap: minimap.into(),
    }))
}
fn add_block_fold(text: &Text, range: Range<usize>, folds: &mut Vec<Fold>) {
    let first = text.byte_to_line(range.start);
    let last = text.byte_to_line(range.end);
    if last > first + 1 {
        folds.push(Fold {
            range: text.line_to_byte(first + 1) - 1..text.line_to_byte(last),
            level: 1,
            attributes: false,
        });
    }
}

pub fn detect_indentation(text: &Text) -> Option<(usize, bool)> {
    let mut tabs = 0usize;
    let mut deltas = BTreeMap::<usize, usize>::new();
    let mut previous = 0;
    for line in text.lines().take(1000) {
        let line = line.to_string();
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tabs += 1;
        }
        let n = line.bytes().take_while(|b| *b == b' ').count();
        let delta = n.abs_diff(previous);
        if (1..=8).contains(&delta) {
            *deltas.entry(delta).or_default() += 1;
        }
        previous = n;
    }
    if tabs > deltas.values().sum::<usize>() {
        Some((4, false))
    } else {
        deltas
            .into_iter()
            .max_by_key(|(n, count)| (*count, std::cmp::Reverse(*n)))
            .map(|(n, _)| (n, true))
    }
}

pub fn syntax_path(path: Option<&Path>) -> String {
    path.unwrap_or(Path::new("untitled"))
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_buffer::Change;
    use vel_syntax::{SyntaxDefinition, SyntaxSetBuilder};
    #[test]
    fn first_launch_without_user_packages_dir_succeeds_and_creates_it() {
        // Regression: a fresh install has no `<data>/Packages`; loading must not fail.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Velocity");
        assert!(!data.exists());
        let base = Arc::new(fixture());
        Assets::with_user_metadata(&base, Some(&data)).expect("first launch must succeed");
        assert!(data.join("Packages/User").is_dir());
    }
    fn fixture() -> Assets {
        let mut builder = SyntaxSetBuilder::new();
        builder.add_with_path(SyntaxDefinition::load_from_str("name: Branch\nscope: source.fixture\nversion: 2\ncontexts:\n  main:\n    - match: '(?=a)'\n      branch_point: reading\n      branch: [first, second]\n  first:\n    - meta_scope: first.fixture\n    - match: z\n      fail: reading\n  second:\n    - meta_scope: second.fixture\n    - match: z\n      pop: true\n", true, None).unwrap(), "Fixture.sublime-syntax");
        Assets {
            syntaxes: Arc::new(builder.build()),
            metadata: Arc::new(MetadataSet::new()),
        }
    }
    fn finish(cache: &mut ScopeCache) {
        while cache.frontier < cache.text.len_lines() {
            cache.next().unwrap();
        }
    }
    fn stacks(cache: &ScopeCache) -> Vec<(Range<usize>, Vec<Scope>)> {
        cache
            .records
            .iter()
            .enumerate()
            .flat_map(|(row, r)| {
                let start = cache.text.line_to_byte(row);
                r.as_ref().unwrap().spans.iter().map(move |s| {
                    (
                        start + s.range.start..start + s.range.end,
                        s.scopes.to_vec(),
                    )
                })
            })
            .collect()
    }
    #[test]
    fn scope_cache_pending_changes_and_branch_rewinds_match_fresh_parses() {
        let assets = fixture();
        let mut text = Text::from("a\nline\nz\na\nline\nz\n");
        let mut cache = ScopeCache::new(text.clone(), &assets, "Fixture.sublime-syntax");
        finish(&mut cache);
        for i in 0..40 {
            let byte = if i % 2 == 0 { 0 } else { text.line_to_byte(2) };
            let replacement = if i % 3 == 0 { "你\n" } else { "x" };
            text.replace(byte..byte, replacement);
            cache.update(
                text.clone(),
                &ChangeSet::new(vec![Change {
                    old_range: byte..byte,
                    new_len: replacement.len(),
                }]),
            );
            if i % 4 == 0 {
                cache.next().unwrap();
            }
            if i % 2 == 0 {
                continue;
            }
            finish(&mut cache);
            let mut fresh = ScopeCache::new(text.clone(), &assets, "Fixture.sublime-syntax");
            finish(&mut fresh);
            assert_eq!(stacks(&cache), stacks(&fresh));
        }
    }
    #[test]
    fn indentation_detection_ignores_blank_lines_and_observes_tab_stops() {
        assert_eq!(
            detect_indentation(&Text::from("a\n  b\n    c\n\n  d")),
            Some((2, true))
        );
        assert_eq!(
            detect_indentation(&Text::from("a\n\tb\n\t\tc")),
            Some((4, false))
        );
        assert_eq!(detect_indentation(&Text::new()), None);
    }
    #[test]
    fn language_workers_publish_valid_prefix_scopes_before_the_full_document() {
        let assets = Assets::bundled().unwrap();
        let task = AnalysisTask::new(assets, "Rust.sublime-syntax".into()).unwrap();
        let text = Text::from("// comment\n".repeat(10_000).as_str());
        let count = text.len_lines();
        task.submit(Job {
            text,
            version: ChangeId(0),
            changes: None,
            scheme: Arc::new(ColorScheme::velocity_dark()),
            tab: 4,
        });
        let prefix = task
            .scopes
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert!(!prefix.lines.is_empty() && prefix.lines.len() < count);
        assert!(prefix.lines[0].iter().any(|span| {
            span.scopes
                .iter()
                .any(|s| s.build_string().starts_with("comment"))
        }));
    }

    #[test]
    fn folded_viewports_request_visible_lines_instead_of_the_hidden_document() {
        let source = format!("fn f() {{\n{}}}\n", "    work();\n".repeat(5000));
        let mut editor = crate::editor::Editor::new(Text::from(source.as_str()));
        editor
            .attach_language(
                Assets::bundled().unwrap(),
                "Rust",
                Arc::new(ColorScheme::velocity_dark()),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !editor.language.borrow().as_ref().unwrap().ready() {
            editor.poll_language();
            assert!(std::time::Instant::now() < deadline);
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
        editor
            .run("fold_by_level", &vel_config::json!({"level":1}))
            .unwrap();
        let mut view = crate::view::View::new(
            vel_platform::Size {
                width: 800.,
                height: 600.,
            },
            8.,
            12.,
        );
        view.scene(&editor);
        let styled_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !view.exact(&editor) {
            view.scene(&editor);
            assert!(std::time::Instant::now() < styled_deadline);
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
        let language = editor.language.borrow();
        let ranges = &language.as_ref().unwrap().requested.as_ref().unwrap().2;
        assert_eq!(ranges.iter().map(|r| r.len()).sum::<usize>(), 3);
        assert!(ranges.iter().all(|r| r.len() <= 2));
    }
}
