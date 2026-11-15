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
use vel_highlight::{
    DocumentHighlighter, HighlightBatch, HighlightService, ScopeProvider, ScopeSnapshot,
    StyledLine, SymbolList,
};
use vel_scope::{Scope, ScopeStack, Selector};
use vel_syntax::{MetadataSet, SyntaxSet};
use vel_text::Text;

#[derive(Debug)]
enum HighlightCommand {
    Edit(Text, ChangeId, ChangeSet, usize),
    Scheme(Arc<ColorScheme>, usize),
    Request(ChangeId, Vec<Range<usize>>),
    Stop,
}
/// The service's cooperative 2ms request can include an indivisible regex call.
/// A dispatcher keeps that call, as well as all parsing, off the UI thread.
#[derive(Debug)]
struct HighlightFrontend {
    tx: mpsc::Sender<HighlightCommand>,
    rx: mpsc::Receiver<Result<HighlightBatch, String>>,
    scopes: mpsc::Receiver<Arc<ScopeSnapshot>>,
    complete: Arc<std::sync::atomic::AtomicBool>,
    latest: Arc<AtomicU64>,
}
impl HighlightFrontend {
    fn new(
        engine: DocumentHighlighter,
        mut text: Text,
        mut scheme: Arc<ColorScheme>,
        mut tab: usize,
        analysis: mpsc::Sender<Job>,
    ) -> Result<Self, String> {
        let version = engine.version().0;
        let mut service = HighlightService::new(engine).map_err(|e| e.to_string())?;
        let (tx, commands) = mpsc::channel();
        let (results, rx) = mpsc::channel();
        let (scope_tx, scopes) = mpsc::channel();
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
                let mut published = false;
                let mut refresh = std::time::Instant::now();
                loop {
                    let idle = validated && pending.is_empty();
                    match commands.recv_timeout(std::time::Duration::from_millis(if idle {
                        50
                    } else {
                        1
                    })) {
                        Ok(HighlightCommand::Edit(after, version, changes, size)) => {
                            text = after;
                            tab = size;
                            published = false;
                            viewport = None;
                            pending.clear();
                            validated = false;
                            if let Err(e) = service.apply_change(text.clone(), version, changes) {
                                let _ = results.send(Err(e.to_string()));
                            }
                        }
                        Ok(HighlightCommand::Scheme(palette, size)) => {
                            scheme = palette;
                            tab = size;
                            published = false;
                            viewport = None;
                            pending.clear();
                            validated = false;
                            if let Err(e) = service.set_color_scheme(scheme.clone()) {
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
                        if full
                            && !published
                            && let Some(scopes) =
                                service.scope_snapshot(p.version, 0..text.len_lines())
                            && scopes.complete()
                            && let Some(symbols) = service.symbols(p.version)
                        {
                            match symbols {
                                Ok(symbols) if symbols.complete => {
                                    let scopes = Arc::new(scopes);
                                    let _ = scope_tx.send(scopes.clone());
                                    let _ = analysis.send(Job {
                                        scopes,
                                        symbols: Arc::new(symbols),
                                        scheme: scheme.clone(),
                                        tab,
                                    });
                                    published = true;
                                }
                                Err(e) => {
                                    let _ = results.send(Err(e.to_string()));
                                    published = true;
                                }
                                _ => {}
                            }
                        }

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
            scopes,
            latest,
        })
    }
    fn send(&self, command: HighlightCommand) {
        match &command {
            HighlightCommand::Edit(_, version, _, _) => {
                self.latest.store(version.0, Ordering::Release);
                self.complete.store(false, Ordering::Release);
            }
            HighlightCommand::Scheme(_, _) => {
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
    /// Corpus staging, index loading, cleanup dispatch and concurrent metadata setup, in ms.
    pub startup_ms: [f64; 4],
}
impl Assets {
    pub fn bundled() -> Result<Arc<Self>, String> {
        static ASSETS: OnceLock<Result<Arc<Assets>, String>> = OnceLock::new();
        ASSETS
            .get_or_init(|| {
                let root = packages_root();
                let metadata_root = root.clone();
                let metadata = std::thread::Builder::new()
                    .name("velocity-metadata".into())
                    .spawn(move || {
                        let phase = std::time::Instant::now();
                        let result = MetadataSet::load_directories(&[metadata_root])
                            .map_err(|e| e.to_string());
                        (result, phase.elapsed().as_secs_f64() * 1000.)
                    })
                    .map_err(|e| e.to_string())?;
                let cache = std::env::temp_dir().join("velocity-syntax-cache");
                // The public corpus has no Plain Text syntax; ours is an in-memory definition
                // under a virtual package path, so no staging tree is built or cleaned up.
                let phase = std::time::Instant::now();
                let syntaxes = SyntaxSet::load_cached_roots(
                    &[root.as_path()],
                    &[(PLAIN_TEXT_PATH, PLAIN_TEXT_SYNTAX)],
                    &cache,
                )
                .map_err(|e| e.to_string());
                let load_ms = phase.elapsed().as_secs_f64() * 1000.;
                let (metadata, metadata_ms) =
                    metadata.join().map_err(|_| "metadata loader panicked")?;
                Ok(Arc::new(Self {
                    syntaxes: Arc::new(syntaxes?.0),
                    metadata: Arc::new(metadata?),
                    startup_ms: [0., load_ms, 0., metadata_ms],
                }))
            })
            .clone()
    }
    /// Loads user YAML/TextMate definitions after the shipped root. Effective archive/loose
    /// resources are virtual extras, retaining resource precedence without extracting archives.
    pub fn with_user_packages(
        base: &Arc<Self>,
        data: Option<&Path>,
        resources: &vel_config::resources::Resources,
    ) -> Result<Arc<Self>, String> {
        let Some(data) = data else {
            return Ok(base.clone());
        };
        let bundled = packages_root();
        let packages = data.join("Packages");
        let _ = std::fs::create_dir_all(packages.join("User"));
        let mut roots = vec![bundled.as_path()];
        if packages.is_dir() {
            roots.push(&packages);
        }
        let mut grammar = vec![(PLAIN_TEXT_PATH, PLAIN_TEXT_SYNTAX)];
        let effective = [".sublime-syntax", ".tmLanguage", ".tmLanguage.json"]
            .into_iter()
            .flat_map(|suffix| resources.matching(suffix))
            .collect::<Vec<_>>();
        if effective.iter().any(|r| {
            r.origin
                .extension()
                .is_some_and(|ext| ext == "sublime-package")
        }) {
            // An archive has no physical root. Its effective overlay includes loose winners,
            // preserving loose-over-archive and User-last lookup ties without extraction.
            for resource in effective {
                if let Ok(text) = resource.text() {
                    grammar.push((resource.path.as_str(), text));
                }
            }
            grammar[1..].sort_by_key(|(path, _)| (path.starts_with("Packages/User/"), *path));
        }
        let cache = data.join("Cache/Syntaxes");
        let syntaxes = SyntaxSet::load_cached_roots(&roots, &grammar, &cache)
            .map(|(set, _)| set)
            .or_else(|_| SyntaxSet::load_roots(&roots, &grammar))
            .map_err(|e| e.to_string())?;
        let resources = resources.matching(".tmPreferences");
        let preferences = if resources.iter().any(|r| {
            r.origin
                .extension()
                .is_some_and(|ext| ext == "sublime-package")
        }) {
            resources
                .iter()
                .map(|r| Ok((r.path.as_str(), r.text()?)))
                .collect::<Result<Vec<_>, String>>()?
        } else {
            Vec::new()
        };
        let metadata = MetadataSet::load_roots(&roots, &preferences).map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            syntaxes: Arc::new(syntaxes),
            metadata: Arc::new(metadata),
            startup_ms: base.startup_ms,
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
/// Virtual package path of our original Plain Text definition (the public corpus has none).
const PLAIN_TEXT_PATH: &str = "Packages/Default/Plain Text.sublime-syntax";
const PLAIN_TEXT_SYNTAX: &str =
    "name: Plain Text\nfile_extensions: [txt]\nscope: text.plain\ncontexts: { main: [] }\n";

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
    pub index_symbols: Arc<SymbolList>,
    pub words: Arc<vel_complete::BufferWords>,
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
#[derive(Debug)]
struct AnalysisTask {
    tx: mpsc::Sender<Job>,
    rx: mpsc::Receiver<Result<Arc<Analysis>, String>>,
    latest: Arc<AtomicU64>,
}
#[derive(Debug)]
struct Job {
    scopes: Arc<ScopeSnapshot>,
    symbols: Arc<SymbolList>,
    scheme: Arc<ColorScheme>,
    tab: usize,
}
impl Drop for AnalysisTask {
    fn drop(&mut self) {
        self.latest.store(u64::MAX, Ordering::Release);
        while let Ok(Ok(old)) = self.rx.try_recv() {
            retire(old);
        }
    }
}
impl AnalysisTask {
    fn new(version: ChangeId) -> Result<Self, String> {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (results, rx) = mpsc::channel();
        let latest = Arc::new(AtomicU64::new(version.0));
        let current = latest.clone();
        std::thread::Builder::new()
            .name("velocity-language".into())
            .spawn(move || {
                while let Ok(mut job) = jobs.recv() {
                    while let Ok(next) = jobs.try_recv() {
                        job = next;
                    }
                    if current.load(Ordering::Acquire) != job.scopes.version.0 {
                        continue;
                    }
                    match analyze(&job, &current) {
                        Ok(Some(value)) => {
                            if results.send(Ok(Arc::new(value))).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let _ = results.send(Err(error));
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, rx, latest })
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
    pub scopes: Option<Arc<ScopeSnapshot>>,
    viewport_scopes: BTreeMap<usize, Arc<ScopeSnapshot>>,
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
        let task = AnalysisTask::new(version)?;
        let service = HighlightFrontend::new(
            DocumentHighlighter::new(
                text.clone(),
                version,
                assets.syntaxes.clone(),
                reference,
                scheme.clone(),
            )
            .with_metadata(assets.metadata.clone()),
            text.clone(),
            scheme.clone(),
            tab,
            task.tx.clone(),
        )?;
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
            viewport_scopes: BTreeMap::new(),
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
            tab,
        ));
        self.version = version;
        self.text = text.clone();
        if let Some(analysis) = self.analysis.take() {
            retire(analysis);
        }
        self.styled.clear();
        self.requested = None;
        self.task.latest.store(version.0, Ordering::Release);
        self.viewport_scopes.clear();
        if let Some(viewport) = viewport {
            self.request_visible(viewport);
        }
    }
    pub fn recolor(&mut self, scheme: Arc<ColorScheme>, _text: Text, tab: usize) {
        if Arc::ptr_eq(&scheme, &self.scheme) {
            return;
        }
        let viewport = self.requested.as_ref().map(|(_, _, r)| r.clone());
        self.service
            .send(HighlightCommand::Scheme(scheme.clone(), tab));
        self.scheme = scheme.clone();
        self.scheme_revision += 1;
        self.styled.clear();
        self.requested = None;
        self.minimap = Arc::from([]);
        if let Some(analysis) = self.analysis.take() {
            retire(analysis);
        }
        self.viewport_scopes.clear();
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
        if batch.scopes.lines.iter().any(|line| line.exact) {
            self.viewport_scopes
                .insert(batch.lines.start, Arc::new(batch.scopes));
            if self.viewport_scopes.len() > 32 {
                self.viewport_scopes.retain(|_, s| {
                    s.range.start < batch.lines.end && s.range.end > batch.lines.start
                });
            }
        }
        for mut line in batch.highlighted {
            let body = self.text.line(line.line).len_bytes();
            if line.spans.iter().any(|s| s.range.end > body) {
                line.spans = line
                    .spans
                    .iter()
                    .filter_map(|s| {
                        let mut s = s.clone();
                        s.range.end = s.range.end.min(body);
                        (s.range.start < s.range.end).then_some(s)
                    })
                    .collect();
            }
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
        while let Ok(scopes) = self.service.scopes.try_recv() {
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
        self.scopes.as_ref().is_some_and(|s| {
            s.version == self.version && s.range == (0..self.text.len_lines()) && s.complete()
        })
    }
    pub fn scopes_ready_at(&self, _text: &Text, byte: usize) -> bool {
        self.scope_snapshot_at(byte).is_some()
    }
    pub fn scope_snapshot_at(&self, byte: usize) -> Option<Arc<ScopeSnapshot>> {
        self.scopes
            .as_ref()
            .filter(|s| s.version == self.version && s.scopes_at(byte).is_some())
            .or_else(|| {
                self.viewport_scopes
                    .values()
                    .rev()
                    .find(|s| s.version == self.version && s.scopes_at(byte).is_some())
            })
            .cloned()
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
    pub fn scopes(&self, _text: &Text, byte: usize) -> ScopeStack {
        let mut stack = ScopeStack::new();
        if let Some(snapshot) = self.scope_snapshot_at(byte) {
            for scope in snapshot.scopes_at(byte).unwrap_or(&[]) {
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
/// Releases analysis allocations away from native event and frame callbacks.
pub(crate) fn retire<T: Send + 'static>(value: T) {
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
#[derive(Debug)]
struct ScopeFacts {
    excluded: bool,
    comment: bool,
    tag: bool,
    blocks: BTreeSet<Scope>,
    style: vel_color_scheme::Style,
}
impl ScopeFacts {
    fn new(scopes: &[Scope], resolver: &mut Highlighter) -> Self {
        let mut stack = ScopeStack::new();
        let mut excluded = false;
        let mut comment = false;
        let mut tag = false;
        let mut blocks = BTreeSet::new();
        for scope in scopes {
            stack.push(*scope);
            let name = scope.build_string();
            comment |= name.starts_with("comment");
            excluded |= name.starts_with("comment") || name.starts_with("string");
            tag |= name.starts_with("meta.tag") || name.starts_with("punctuation.definition.tag");
            if name.starts_with("meta.block") || name.starts_with("meta.function.body") {
                blocks.insert(*scope);
            }
        }
        Self {
            excluded,
            comment,
            tag,
            blocks,
            style: resolver.style_for(&stack),
        }
    }
}

fn analyze(job: &Job, latest: &AtomicU64) -> Result<Option<Analysis>, String> {
    let text = &job.scopes.text;
    let mut spans = Vec::new();
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut pairs = Vec::new();
    let mut angles = Vec::new();
    let mut minimap = Vec::new();
    let mut resolver = Highlighter::new(job.scheme.clone());
    // Interned IDs replace per-token scope-name conversion, selector resolution and
    // temporary ScopeStacks. Facts are local to this immutable snapshot and palette.
    let mut facts: Vec<Option<ScopeFacts>> = Vec::new();
    let step = text.len_lines().div_ceil(1024).max(1);
    for record in &job.scopes.lines {
        let row = record.line;
        if row % 32 == 0 && latest.load(Ordering::Acquire) != job.scopes.version.0 {
            return Ok(None);
        }
        let start = text.line_to_byte(row);
        let line = text.line(row).to_string();
        let mut tokens = Vec::new();
        let mut col = 0;
        for run in record.runs.iter() {
            let range = run.start as usize..(run.end as usize).min(line.len());
            if range.is_empty() {
                continue;
            }
            let part = &line[range.clone()];
            let id = run.stack.0 as usize;
            if facts.len() <= id {
                facts.resize_with(id + 1, || None);
            }
            let fact = facts[id].get_or_insert_with(|| {
                ScopeFacts::new(
                    job.scopes.stacks.get(run.stack).unwrap_or(&[]),
                    &mut resolver,
                )
            });
            if row % step == 0 {
                let style = fact.style;
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
            if !fact.excluded {
                for (at, c) in part.char_indices() {
                    let byte = start + range.start + at;
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
            spans.push((start + range.start..start + range.end, run.stack));
        }
        if row % step == 0 {
            minimap.push(MiniLine { row, tokens });
        }
    }
    let source = text.to_string();
    let symbols = job
        .symbols
        .symbols
        .iter()
        .filter(|s| s.local && !s.name.trim().is_empty())
        .map(|s| (s.name.clone(), s.range.start))
        .collect();
    let mut folds = Vec::new();
    // Tags use scope-selected markup, with quote-aware scanning of attributes.
    let mut tags: Vec<(String, Range<usize>)> = Vec::new();
    let mut at = 0;
    while let Some(relative) = source[at..].find('<') {
        let begin = at + relative;
        at = begin + 1;
        let row = text.byte_to_line(begin);
        let fact = job
            .scopes
            .scope_at(row, begin - text.line_to_byte(row))
            .and_then(|id| facts.get(id.0 as usize))
            .and_then(Option::as_ref);
        if !fact.is_some_and(|fact| !fact.comment && fact.tag) {
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
        let active = &facts[span.1.0 as usize]
            .as_ref()
            .expect("visited scope stack")
            .blocks;
        let ended: Vec<_> = blocks
            .keys()
            .filter(|s| !active.contains(s))
            .copied()
            .collect();
        for scope in ended {
            let range = blocks.remove(&scope).expect("block");
            add_block_fold(text, range, &mut folds);
        }
        for &scope in active {
            blocks
                .entry(scope)
                .and_modify(|r| r.end = span.0.end)
                .or_insert(span.0.clone());
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
    if latest.load(Ordering::Acquire) != job.scopes.version.0 {
        return Ok(None);
    }
    Ok(Some(Analysis {
        scheme: job.scheme.clone(),
        version: job.scopes.version,
        symbols,
        pairs,
        pair_tree,
        folds,
        minimap: minimap.into(),
        index_symbols: job.symbols.clone(),
        words: Arc::new(vel_complete::BufferWords::new(text)),
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
    use vel_syntax::{SyntaxDefinition, SyntaxSetBuilder};
    #[test]
    fn first_launch_without_user_packages_dir_succeeds_and_creates_it() {
        // Regression: a fresh install has no `<data>/Packages`; loading must not fail.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Velocity");
        assert!(!data.exists());
        let base = Arc::new(fixture());
        Assets::with_user_packages(
            &base,
            Some(&data),
            &vel_config::resources::Resources::default(),
        )
        .expect("first launch must succeed");
        assert!(data.join("Packages/User").is_dir());
    }
    #[test]
    fn plain_text_is_virtual_source_is_untouched_and_cache_observes_source_changes() {
        // No staging tree: the corpus is loaded in place, our Plain Text definition is an
        // in-memory virtual resource, and the public source files are never written.
        let source = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let fixture = source.path().join("Fixture.sublime-syntax");
        std::fs::write(
            &fixture,
            "name: Before\nscope: source.fixture\ncontexts: { main: [] }\n",
        )
        .unwrap();
        let load = || {
            SyntaxSet::load_cached_roots(
                &[source.path()],
                &[(PLAIN_TEXT_PATH, PLAIN_TEXT_SYNTAX)],
                cache.path(),
            )
            .unwrap()
        };
        let (loaded, _) = load();
        assert!(loaded.find_syntax_by_name("Before").is_some());
        assert!(loaded.find_syntax_by_name("Plain Text").is_some());
        assert!(
            !source.path().join("Default").exists(),
            "source tree was written to"
        );
        std::fs::write(
            &fixture,
            "name: After\nscope: source.fixture\ncontexts: { main: [] }\n",
        )
        .unwrap();
        let (loaded, status) = load();
        assert_eq!(status, vel_syntax::CacheStatus::Yaml);
        assert!(loaded.find_syntax_by_name("After").is_some());
        assert_eq!(
            std::fs::read_dir(source.path()).unwrap().count(),
            1,
            "only the fixture may exist in the source root"
        );
    }
    fn fixture() -> Assets {
        let mut builder = SyntaxSetBuilder::new();
        builder.add_with_path(SyntaxDefinition::load_from_str("name: Branch\nscope: source.fixture\nversion: 2\ncontexts:\n  main:\n    - match: '(?=a)'\n      branch_point: reading\n      branch: [first, second]\n  first:\n    - meta_scope: first.fixture\n    - match: z\n      fail: reading\n  second:\n    - meta_scope: second.fixture\n    - match: z\n      pop: true\n", true, None).unwrap(), "Fixture.sublime-syntax");
        Assets {
            syntaxes: Arc::new(builder.build()),
            metadata: Arc::new(MetadataSet::new()),
            startup_ms: [0.; 4],
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
    fn shared_scopes_include_lf_without_slicing_past_body_and_drop_stale_versions() {
        let mut editor = crate::editor::Editor::new(Text::from("// 你好\nfn f() {}\n"));
        editor
            .attach_language(
                Assets::bundled().unwrap(),
                "Rust",
                Arc::new(ColorScheme::velocity_dark()),
            )
            .unwrap();
        editor
            .language
            .borrow_mut()
            .as_mut()
            .unwrap()
            .request_visible(std::iter::once(0..3).collect());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !editor.language.borrow().as_ref().unwrap().ready() {
            editor.poll_language();
            assert!(std::time::Instant::now() < deadline);
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
        {
            let language = editor.language.borrow();
            let language = language.as_ref().unwrap();
            assert!(language.selector(&editor.text, "comment", 3));
            for line in language.styled.values() {
                assert!(
                    line.spans
                        .iter()
                        .all(|s| s.range.end <= editor.text.line(line.line).len_bytes())
                );
            }
        }
        editor
            .run("insert", &vel_config::json!({"characters":"x"}))
            .unwrap();
        assert!(
            !editor
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .scopes_ready_at(&editor.text, 0)
        );
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
