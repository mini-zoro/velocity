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
use vel_scope::{Scope, ScopeStack};
use vel_syntax::{MetadataSet, SyntaxSet};
use vel_text::Text;

#[derive(Debug)]
enum HighlightCommand {
    Edit(Text, ChangeId, ChangeSet, usize),
    Scheme(Arc<ColorScheme>, usize),
    Request(ChangeId, Vec<Range<usize>>),
    Measure(mpsc::Sender<Option<vel_highlight::HighlightMemory>>),
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

pub(crate) fn viewport_options(
    settings: &vel_config::settings::Settings,
) -> vel_highlight::ViewportOptions {
    vel_highlight::ViewportOptions {
        max_line_bytes: settings.usize("large_file.max_line_bytes", 16 * 1024),
        max_viewport_lines: settings.usize("large_file.max_viewport_lines", 512),
        max_lookback_lines: settings.usize("large_file.max_lookback_lines", 256),
        checkpoint_interval: settings.usize("large_file.checkpoint_interval", 1024),
        max_checkpoints: settings.usize("large_file.max_checkpoints", 128),
    }
}

impl HighlightFrontend {
    fn viewport(
        text: Text,
        version: ChangeId,
        assets: Arc<Assets>,
        syntax: String,
        scheme: Arc<ColorScheme>,
        settings: &vel_config::settings::Settings,
    ) -> Result<Self, String> {
        let (tx, commands) = mpsc::channel();
        let (results, rx) = mpsc::channel();
        let (_scope_tx, scopes) = mpsc::channel();
        let complete = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ready = complete.clone();
        let latest = Arc::new(AtomicU64::new(version.0));
        let current = latest.clone();
        let options = viewport_options(settings);
        std::thread::Builder::new()
            .name("velocity-viewport-highlight".into())
            .spawn(move || {
                let Some(reference) =
                    vel_highlight::detect_syntax(&assets.syntaxes, "", "", Some(&syntax))
                else {
                    return;
                };
                let mut engine = vel_highlight::ViewportHighlighter::new(
                    text,
                    version,
                    assets.syntaxes.clone(),
                    reference,
                    scheme,
                    options,
                );
                let mut watched: Vec<Range<usize>> = Vec::new();
                let mut refresh = true;
                loop {
                    match commands.recv_timeout(std::time::Duration::from_millis(16)) {
                        Ok(HighlightCommand::Edit(text, version, changes, _)) => {
                            if let Err(e) = engine.apply_change(text, version, &changes) {
                                let _ = results.send(Err(e.to_string()));
                            }
                            refresh = true;
                        }
                        Ok(HighlightCommand::Scheme(scheme, _)) => {
                            engine.set_color_scheme(scheme);
                            refresh = true;
                        }
                        Ok(HighlightCommand::Request(version, lines))
                            if version == engine.version() =>
                        {
                            watched = lines;
                            refresh = true;
                        }
                        Ok(HighlightCommand::Request(..)) => {}
                        Ok(HighlightCommand::Measure(reply)) => {
                            let _ = reply.send(None);
                        }
                        Ok(HighlightCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if engine.version().0 != current.load(Ordering::Acquire) {
                        continue;
                    }
                    let before = engine.progress();
                    if refresh {
                        for lines in &watched {
                            let batch = engine
                                .request_lines(
                                    engine.version(),
                                    lines.clone(),
                                    std::time::Duration::from_millis(2),
                                )
                                .map_err(|e| e.to_string());
                            if results.send(batch).is_err() {
                                return;
                            }
                        }
                    }
                    if !before.complete() {
                        let result = if before.long_line.is_some() {
                            engine.repair_long_line()
                        } else {
                            engine.advance(std::time::Duration::from_millis(2))
                        };
                        if let Err(e) = result {
                            let _ = results.send(Err(e.to_string()));
                        }
                    }
                    let after = engine.progress();
                    ready.store(after.complete(), Ordering::Release);
                    refresh = after.parsed_lines != before.parsed_lines;
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            rx,
            scopes,
            complete,
            latest,
        })
    }
    fn new(
        engine: DocumentHighlighter,
        mut text: Text,
        mut scheme: Arc<ColorScheme>,
        mut tab: usize,
        analysis: mpsc::Sender<Job>,
        indexing: bool,
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
                        Ok(HighlightCommand::Measure(reply)) => {
                            let _ = reply.send(service.memory_usage());
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
                                    if indexing {
                                        let _ = analysis.send(Job {
                                            scopes,
                                            symbols: Arc::new(symbols),
                                            scheme: scheme.clone(),
                                            tab,
                                        });
                                    }
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
use crate::bundled_syntax::INTERNAL_SYNTAXES;
impl Assets {
    pub(crate) fn packed() -> Option<Result<Arc<Self>, String>> {
        const INDEX: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/startup-syntaxes.bin"));
        static ASSETS: OnceLock<Result<Arc<Assets>, String>> = OnceLock::new();
        if INDEX.is_empty() {
            return None; // Source archive built without the syntax submodule.
        }
        Some(
            ASSETS
                .get_or_init(|| {
                    let phase = std::time::Instant::now();
                    let mut syntaxes: SyntaxSet =
                        bincode::deserialize(INDEX).map_err(|e| e.to_string())?;
                    let root = packages_root();
                    let aliases: Vec<_> = crate::shipped::grammars()
                        .map(|path| (path, root.join(&path["Packages/".len()..])))
                        .collect();
                    syntaxes.bind_resource_paths(
                        aliases
                            .iter()
                            .filter_map(|(p, file)| Some((*p, file.to_str()?))),
                    );
                    let syntax_ms = phase.elapsed().as_secs_f64() * 1000.;
                    let phase = std::time::Instant::now();
                    let mut metadata = MetadataSet::new();
                    for (path, bytes) in crate::shipped::grammar_metadata() {
                        metadata
                            .add_plist(bytes)
                            .map_err(|e| format!("{path}: {e}"))?;
                    }
                    Ok(Arc::new(Self {
                        syntaxes: Arc::new(syntaxes),
                        metadata: Arc::new(metadata),
                        startup_ms: [0., syntax_ms, 0., phase.elapsed().as_secs_f64() * 1000.],
                    }))
                })
                .clone(),
        )
    }

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
                // Internal definitions are virtual, so no staging file or cleanup is needed.
                let phase = std::time::Instant::now();
                let syntaxes =
                    SyntaxSet::load_cached_roots(&[root.as_path()], &INTERNAL_SYNTAXES, &cache)
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
    pub fn with_package_overlay(
        base: &Arc<Self>,
        data: Option<&Path>,
        resources: &vel_config::resources::Resources,
        packed: bool,
    ) -> Result<Arc<Self>, String> {
        let root = packages_root();
        // Our own internal grammars (Plain Text, Find Results) are already in every base set.
        // The packaged app also ships Find Results as a resource file; an identical copy must
        // not count as a package override, or every launch rebuilds all grammars (~1.5 s).
        let internal_copy = |r: &&vel_config::resources::Resource| {
            INTERNAL_SYNTAXES
                .iter()
                .any(|(path, text)| r.path == *path && r.bytes.as_ref() == text.as_bytes())
        };
        let effective = [".sublime-syntax", ".tmLanguage", ".tmLanguage.json"]
            .into_iter()
            .flat_map(|suffix| resources.matching(suffix))
            .filter(|r| !internal_copy(r))
            .collect::<Vec<_>>();
        let preferences = resources.matching(".tmPreferences");
        if effective.is_empty()
            && preferences.is_empty()
            && resources
                .get("Packages/Default/Preferences.sublime-settings")
                .is_none()
        {
            return Ok(base.clone());
        }
        let unchanged = if packed {
            effective.len() == crate::shipped::grammars().count()
                && preferences.len() == crate::shipped::grammar_metadata().count()
                && effective.iter().chain(&preferences).all(|r| r.shipped())
        } else {
            effective.len() + INTERNAL_SYNTAXES.len() == base.syntaxes.syntaxes().len()
                && effective.iter().all(|r| r.origin.starts_with(&root))
                && preferences.iter().all(|r| r.origin.starts_with(&root))
        };
        if unchanged {
            return Ok(base.clone());
        }
        let mut warnings = Vec::new();
        let mut grammar = INTERNAL_SYNTAXES.to_vec();
        for resource in effective {
            match resource.text() {
                Ok(text) => grammar.push((resource.path.as_str(), text)),
                Err(warning) => warnings.push(warning),
            }
        }
        // All effective grammars are virtual extras. Physical roots would bypass
        // ignored_packages and reintroduce disabled loose/bundled grammars.
        let load = || SyntaxSet::load_roots(&[], &grammar);
        let mut syntaxes = if let Some(data) = data {
            SyntaxSet::load_cached_roots(&[], &grammar, &data.join("Cache/Syntaxes"))
                .map(|(set, _)| set)
                .or_else(|_| load())
        } else {
            load()
        }
        .map_err(|e| e.to_string())?;
        let mut metadata = MetadataSet::new();
        for resource in preferences {
            // add_plist commits only after validating the entire file, and supports binary plists.
            if let Err(error) = metadata.add_plist(&resource.bytes) {
                warnings.push(format!("{}: {error}", resource.path));
            }
        }
        syntaxes.extend_resource_warnings(warnings);
        Ok(Arc::new(Self {
            syntaxes: Arc::new(syntaxes),
            metadata: Arc::new(metadata),
            startup_ms: base.startup_ms,
        }))
    }
}
pub(crate) fn packages_root() -> PathBuf {
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

#[derive(Clone, Debug)]
pub struct Pair {
    pub open: Range<usize>,
    pub close: Range<usize>,
    pub kind: char,
}
mod pair_index;
use pair_index::PairIndex;

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
    pub non_ascii: bool,
}
#[derive(Debug)]
pub struct Analysis {
    pub scheme: Arc<ColorScheme>,
    pub version: ChangeId,
    pub symbols: Vec<(String, usize)>,
    pub pairs: PairIndex,
    pub folds: Vec<Fold>,
    pub minimap: Arc<[MiniLine]>,
    pub index_symbols: Arc<SymbolList>,
    pub words: Arc<vel_complete::BufferWords>,
    pub guides: Arc<vel_render::guides::GuideIndex>,
}
impl Analysis {
    pub(crate) fn memory_usage(&self) -> [usize; 4] {
        let symbols = self.symbols.capacity() * size_of::<(String, usize)>()
            + self
                .symbols
                .iter()
                .map(|(name, _)| name.capacity())
                .sum::<usize>()
            + self.index_symbols.symbols.capacity() * size_of::<vel_syntax::Symbol>()
            + self
                .index_symbols
                .symbols
                .iter()
                .map(|s| s.name.capacity() + s.index_name.capacity())
                .sum::<usize>();
        let geometry = self.pairs.storage_bytes()
            + self.folds.capacity() * size_of::<Fold>()
            + self.guides.storage_bytes();
        let minimap = size_of_val(self.minimap.as_ref())
            + self
                .minimap
                .iter()
                .map(|l| l.tokens.capacity() * size_of::<MiniToken>())
                .sum::<usize>();
        [
            symbols,
            geometry,
            minimap,
            self.words.estimated_storage_bytes(),
        ]
    }
    /// Finds the innermost enabled pair in logarithmic time plus active nesting depth.
    pub fn pair_at(
        &self,
        byte: usize,
        content: bool,
        enabled: impl Fn(char) -> bool,
    ) -> Option<Pair> {
        if self.pairs.leaf_count() == 0 {
            return None;
        }
        let limit = self.pairs.starts_at_or_before(byte);
        fn visit(
            pairs: &PairIndex,
            node: usize,
            range: Range<usize>,
            limit: usize,
            byte: usize,
            content: bool,
            enabled: &impl Fn(char) -> bool,
        ) -> Option<Pair> {
            if range.start >= limit || pairs.max_end(node) < byte {
                return None;
            }
            if range.len() == 1 {
                let pair = pairs.get(range.start)?;
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
                pairs,
                node * 2 + 1,
                middle..range.end,
                limit,
                byte,
                content,
                enabled,
            )
            .or_else(|| {
                visit(
                    pairs,
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
            &self.pairs,
            1,
            0..self.pairs.leaf_count(),
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
    /// Test probe: the version of the last job the worker finished with, whether it
    /// published an analysis or dropped the job as stale.
    #[cfg(test)]
    handled: Arc<AtomicU64>,
}
#[cfg(test)]
thread_local! {
    /// Simulates a UI thread that is descheduled right after an edit reaches the
    /// highlighter: `Language::change` returns only after the analysis worker has
    /// finished with that version's job.
    pub(crate) static ANALYSIS_RUNS_AHEAD: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
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
        #[cfg(test)]
        let handled = Arc::new(AtomicU64::new(u64::MAX));
        #[cfg(test)]
        let finished = handled.clone();
        std::thread::Builder::new()
            .name("velocity-language".into())
            .spawn(move || {
                while let Ok(mut job) = jobs.recv() {
                    while let Ok(next) = jobs.try_recv() {
                        job = next;
                    }
                    if current.load(Ordering::Acquire) != job.scopes.version.0 {
                        #[cfg(test)]
                        finished.store(job.scopes.version.0, Ordering::Release);
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
                    #[cfg(test)]
                    finished.store(job.scopes.version.0, Ordering::Release);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            rx,
            latest,
            #[cfg(test)]
            handled,
        })
    }
}

#[path = "guide_state.rs"]
mod guide_state;
#[derive(Debug)]
struct ViewportScopes {
    snapshot: Arc<ScopeSnapshot>,
    bytes: Range<usize>,
}
/// One instance per Buffer; split views borrow its results through RefCell.
#[derive(Debug)]
pub struct Language {
    pub viewport_settings: u64,
    pub policy: vel_largefile::FeaturePolicy,
    pub assets: Arc<Assets>,
    pub syntax: String,
    pub name: String,
    pub root: Scope,
    pub closes_tags: bool,
    pub scheme: Arc<ColorScheme>,
    pub version: ChangeId,
    service: HighlightFrontend,
    task: AnalysisTask,
    pub analysis: Option<Arc<Analysis>>,
    pub guides: guide_state::Guides,
    pub styled: BTreeMap<usize, StyledLine>,
    pub errors: Vec<String>,
    pub minimap: Arc<[MiniLine]>,
    pub scopes: Option<Arc<ScopeSnapshot>>,
    viewport_scopes: BTreeMap<usize, ViewportScopes>,
    relief_pending: bool,
    text: Text,
    scheme_revision: u64,
    requested: Option<(ChangeId, u64, Vec<Range<usize>>)>,
    visible_views: BTreeMap<vel_buffer::ViewId, Vec<Range<usize>>>,
}
pub type LanguageCell = std::rc::Rc<RefCell<Option<Language>>>;
impl Language {
    pub(crate) fn measure_highlighter(
        &self,
        timeout: std::time::Duration,
    ) -> Option<vel_highlight::HighlightMemory> {
        let (reply, answer) = mpsc::channel();
        self.service
            .tx
            .send(HighlightCommand::Measure(reply))
            .ok()?;
        answer.recv_timeout(timeout).ok().flatten()
    }
    #[allow(clippy::too_many_arguments)]
    pub fn with_policy(
        assets: Arc<Assets>,
        syntax: &str,
        text: Text,
        version: ChangeId,
        scheme: Arc<ColorScheme>,
        tab: usize,
        policy: vel_largefile::FeaturePolicy,
        settings: &vel_config::settings::Settings,
    ) -> Result<Self, String> {
        let reference = vel_highlight::detect_syntax(&assets.syntaxes, "", "", Some(syntax))
            .ok_or_else(|| format!("unknown syntax {syntax}"))?;
        let path = reference.path.clone();
        let name = reference.name.clone();
        let root = reference.scope;
        let closes_tags = reference.embeds_markup;
        let task = AnalysisTask::new(version)?;
        let service = if policy.highlighting == vel_largefile::HighlightMode::Viewport {
            HighlightFrontend::viewport(
                text.clone(),
                version,
                assets.clone(),
                path.clone(),
                scheme.clone(),
                settings,
            )?
        } else {
            HighlightFrontend::new(
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
                policy.indexing,
            )?
        };
        Ok(Self {
            viewport_settings: settings.cache_identity(),
            policy,
            assets,
            syntax: path,
            name,
            root,
            closes_tags,
            scheme,
            version,
            service,
            task,
            analysis: None,
            guides: Default::default(),
            styled: BTreeMap::new(),
            errors: Vec::new(),
            minimap: Arc::from([]),
            scopes: None,
            viewport_scopes: BTreeMap::new(),
            relief_pending: text.len_bytes() >= vel_platform::memory_pressure::LARGE_WORK_BYTES,
            text,
            scheme_revision: 0,
            requested: None,
            visible_views: BTreeMap::new(),
        })
    }
    pub(crate) fn inherit_guides(
        &mut self,
        previous: &mut Self,
        changes: Option<&[(ChangeId, ChangeSet)]>,
    ) {
        self.guides = std::mem::take(&mut previous.guides);
        if previous.version != self.version
            && let Some(changes) = changes
        {
            self.guides.rebase(&previous.text, &self.text, changes);
        }
    }
    pub fn change(&mut self, text: Text, version: ChangeId, changes: ChangeSet, tab: usize) {
        if !self.relief_pending && vel_platform::memory_pressure::enabled() {
            let mut bytes = 0usize;
            self.relief_pending = changes.changes().iter().any(|change| {
                bytes = bytes.saturating_add(change.old_range.len().max(change.new_len));
                bytes >= vel_platform::memory_pressure::LARGE_WORK_BYTES
            });
        }
        self.guides.change(&self.text, &text, &changes);
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
            // Samples and old row ranges are both ordered. Visit each changed
            // range at most once instead of scanning all edits for each sample.
            let mut pending = changed_rows.as_slice();
            for sample in samples {
                while let Some(rows) = pending.first()
                    && *rows.end() < sample.row
                {
                    pending = &pending[1..];
                }
                let touched = pending
                    .first()
                    .is_some_and(|rows| rows.contains(&sample.row));
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
                    sample.non_ascii = !line.is_ascii();
                    sample.tokens = vec![MiniToken {
                        column: 0,
                        width: crate::editor::cells_with_tab(line.trim_end_matches('\n'), tab),
                        color: rgba(self.scheme.default_style().foreground),
                    }];
                }
            }
        }
        // Publish the version to the analysis worker before the highlighter can parse
        // it. The highlighter hands each version's job to the worker exactly once; a job
        // that arrived before this store was dropped as stale and analysis never came.
        self.task.latest.store(version.0, Ordering::Release);
        self.service.send(HighlightCommand::Edit(
            text.clone(),
            version,
            changes.clone(),
            tab,
        ));
        #[cfg(test)]
        if ANALYSIS_RUNS_AHEAD.get() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while self.task.handled.load(Ordering::Acquire) != version.0
                && std::time::Instant::now() < deadline
            {
                std::thread::park_timeout(std::time::Duration::from_millis(1));
            }
        }
        self.version = version;
        self.text = text.clone();
        if let Some(analysis) = self.analysis.take() {
            retire(analysis);
        }
        self.styled.clear();
        self.requested = None;
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
        if batch
            .scopes
            .lines
            .iter()
            .any(vel_highlight::ScopedLine::exact)
        {
            let text = &batch.scopes.text;
            let start = text.line_to_byte(batch.scopes.range.start);
            let end = if batch.scopes.range.end == text.len_lines() {
                text.len_bytes() + 1 // Include the final line's EOF point.
            } else {
                text.line_to_byte(batch.scopes.range.end)
            };
            self.viewport_scopes.insert(
                batch.lines.start,
                ViewportScopes {
                    snapshot: Arc::new(batch.scopes),
                    bytes: start..end,
                },
            );
            if self.viewport_scopes.len() > 32 {
                self.viewport_scopes.retain(|_, s| {
                    s.snapshot.range.start < batch.lines.end
                        && s.snapshot.range.end > batch.lines.start
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
                    self.guides.publish(Arc::clone(&a.guides));
                    if let Some(old) = self.analysis.replace(a) {
                        retire(old);
                    }
                    changed = true;
                }
                Ok(old) => retire(old),
                Err(e) => self.errors.push(e),
            }
        }
        if self.relief_pending && vel_platform::memory_pressure::enabled() && !self.busy() {
            self.relief_pending = false;
            vel_platform::memory_pressure::request();
        }
        changed
    }
    pub fn ready(&self) -> bool {
        if self.policy.highlighting == vel_largefile::HighlightMode::Viewport {
            return !self.busy();
        }
        self.scopes_ready()
            && (!self.policy.indexing
                || self
                    .analysis
                    .as_ref()
                    .is_some_and(|a| a.version == self.version))
    }
    /// Test timeout diagnostics: which publication `ready` is still waiting for, and
    /// whether the highlight dispatcher and the analysis worker are still running
    /// (each holds a clone of its version counter until its thread exits).
    #[cfg(test)]
    pub(crate) fn readiness(&self) -> String {
        format!(
            "version={:?} policy={:?} scopes={:?} lines={} analysis={:?} \
             highlight_complete={} analysis_handled={} dispatcher_alive={} \
             analysis_alive={} errors={:?}",
            self.version,
            self.policy,
            self.scopes
                .as_ref()
                .map(|s| (s.version, s.range.clone(), s.complete())),
            self.text.len_lines(),
            self.analysis.as_ref().map(|a| a.version),
            self.highlight_complete(),
            self.task.handled.load(Ordering::Acquire),
            Arc::strong_count(&self.service.latest) > 1,
            Arc::strong_count(&self.task.latest) > 1,
            self.errors,
        )
    }
    pub fn scopes_ready(&self) -> bool {
        self.scopes.as_ref().is_some_and(|s| {
            s.version == self.version && s.range == (0..self.text.len_lines()) && s.complete()
        })
    }
    pub fn scopes_ready_at(&self, _text: &Text, byte: usize) -> bool {
        self.scoped_point(byte).is_some()
    }
    pub fn scope_snapshot_at(&self, byte: usize) -> Option<Arc<ScopeSnapshot>> {
        self.scoped_point(byte)
            .map(|(snapshot, _)| snapshot.clone())
    }
    pub(crate) fn scoped_point(&self, byte: usize) -> Option<(&Arc<ScopeSnapshot>, &[Scope])> {
        if let Some(snapshot) = self.scopes.as_ref().filter(|s| s.version == self.version)
            && let Some(scopes) = snapshot.scopes_at(byte)
        {
            return Some((snapshot, scopes));
        }
        self.viewport_scopes
            .values()
            .rev()
            .filter(|s| s.snapshot.version == self.version && s.bytes.contains(&byte))
            .find_map(|s| {
                s.snapshot
                    .scopes_at(byte)
                    .map(|scopes| (&s.snapshot, scopes))
            })
    }
    pub fn busy(&self) -> bool {
        if self.policy.highlighting == vel_largefile::HighlightMode::Viewport {
            return self.requested.as_ref().is_some_and(|(_, _, ranges)| {
                ranges
                    .iter()
                    .any(|r| r.clone().any(|row| !self.styled.contains_key(&row)))
            });
        }
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
        ScopeStack::from_vec(
            self.scoped_point(byte)
                .map_or_else(|| vec![self.root], |(_, scopes)| scopes.to_vec()),
        )
    }
    /// Scope-selector match at a byte. Production keymap contexts use the batched
    /// projection in `editing.rs`; tests keep this direct form.
    #[cfg(test)]
    pub fn selector(&self, text: &Text, selector: &str, byte: usize) -> bool {
        selector
            .parse::<vel_scope::Selector>()
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

fn add_indent_fold(
    text: &Text,
    header: usize,
    last: usize,
    syntax_headers: &[usize],
    folds: &mut Vec<Fold>,
) {
    if last > header {
        let start = text.line_to_byte(header + 1).saturating_sub(1);
        // Syntax and indentation formerly produced identical ranges here. Trimming
        // only the indentation endpoint must not create a synthetic nested level.
        if syntax_headers.binary_search(&start).is_ok() {
            return;
        }
        let end = text.line_to_byte(last) + text.line_len_bytes(last);
        if start < end {
            folds.push(Fold {
                range: start..end,
                level: 1,
                attributes: false,
            });
        }
    }
}

fn analyze(job: &Job, latest: &AtomicU64) -> Result<Option<Analysis>, String> {
    let text = &job.scopes.text;
    let mut blocks: BTreeMap<Scope, Range<usize>> = BTreeMap::new();
    let mut block_folds = Vec::new();
    let mut previous_stack = None;
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut pairs = PairIndex::new(text.len_bytes());
    let mut angles = Vec::new();
    let mut minimap = Vec::new();
    let mut resolver = Highlighter::new(job.scheme.clone());
    // Interned IDs replace per-token scope-name conversion, selector resolution and
    // temporary ScopeStacks. Facts are local to this immutable snapshot and palette.
    let mut facts: Vec<Option<ScopeFacts>> = Vec::new();
    let step = text.len_lines().div_ceil(1024).max(1);
    for (row, record) in job.scopes.range.clone().zip(&job.scopes.lines) {
        if row % 32 == 0 && latest.load(Ordering::Acquire) != job.scopes.version.0 {
            return Ok(None);
        }
        let start = text.line_to_byte(row);
        let line = text.line(row).to_string();
        let mut tokens = Vec::new();
        let mut col = 0;
        for run in record.runs() {
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
            // Track only open blocks, in the same scope order as the old
            // second pass. Append this group after syntax and indentation folds.
            if previous_stack != Some(run.stack) {
                previous_stack = Some(run.stack);
                blocks.retain(|scope, region| {
                    if fact.blocks.contains(scope) {
                        true
                    } else {
                        add_block_fold(text, region.clone(), &mut block_folds);
                        false
                    }
                });
            }
            let absolute = start + range.start..start + range.end;
            for &scope in &fact.blocks {
                blocks
                    .entry(scope)
                    .and_modify(|r| r.end = absolute.end)
                    .or_insert_with(|| absolute.clone());
            }
        }
        if row % step == 0 {
            minimap.push(MiniLine {
                row,
                tokens,
                non_ascii: !line.is_ascii(),
            });
        }
    }
    let source = if facts.iter().flatten().any(|f| f.tag && !f.comment) {
        text.to_string()
    } else {
        String::new()
    };
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
    for pair in pairs.iter() {
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
    // Build 4143: https://www.sublimetext.com/download — exclude the final
    // newline and trailing blank lines from the collapsed block.
    let mut syntax_headers: Vec<_> = folds
        .iter()
        .filter(|f| !f.attributes)
        .map(|f| f.range.start)
        .collect();
    syntax_headers.sort_unstable();
    syntax_headers.dedup();
    let mut indents: Vec<(usize, usize)> = Vec::new();
    let mut previous = None;
    // Most minified/unindented files never need guide storage. Delay allocation
    // until the first indented row; only the preceding blank run can inherit it.
    let mut guide_rows = Vec::new();
    let mut leading_blanks = 0;
    for row in 0..text.len_lines() {
        let line = text.line(row).to_string();
        if line.trim().is_empty() {
            if guide_rows.is_empty() {
                leading_blanks += 1;
            } else {
                guide_rows.push(None);
            }
            continue;
        }
        let indent = vel_syntax::indent::indentation(&line, job.tab);
        if !guide_rows.is_empty() {
            guide_rows.push(Some(indent));
        } else if indent > 0 {
            guide_rows.reserve_exact(text.len_lines());
            guide_rows.resize(row, Some(0));
            for blank in &mut guide_rows[row - leading_blanks..] {
                *blank = None;
            }
            guide_rows.push(Some(indent));
        } else {
            leading_blanks = 0;
        }
        while indents.last().is_some_and(|(level, _)| *level >= indent) {
            let (_, header) = indents.pop().expect("indent");
            if let Some((_, last)) = previous {
                add_indent_fold(text, header, last, &syntax_headers, &mut folds);
            }
        }
        if let Some((old_indent, header)) = previous
            && indent > old_indent
        {
            indents.push((old_indent, header));
        }
        previous = Some((indent, row));
    }
    if let Some((_, last)) = previous {
        for (_, header) in indents {
            add_indent_fold(text, header, last, &syntax_headers, &mut folds);
        }
    }
    for (_, range) in blocks {
        add_block_fold(text, range, &mut block_folds);
    }
    folds.append(&mut block_folds);
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
    pairs.finish();
    if latest.load(Ordering::Acquire) != job.scopes.version.0 {
        return Ok(None);
    }
    Ok(Some(Analysis {
        scheme: job.scheme.clone(),
        version: job.scopes.version,
        symbols,
        pairs,
        folds,
        guides: Arc::new(vel_render::guides::GuideIndex::new(guide_rows)),
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

#[derive(Debug, PartialEq, Eq)]
pub enum Indentation {
    Tabs,
    Spaces(usize),
}

#[derive(Debug)]
pub(crate) struct DetectedTabSize {
    width: usize,
    previous: Option<serde_json::Value>,
}

impl Indentation {
    pub fn apply(self, editor: &mut crate::editor::Editor) {
        use vel_config::settings::Layer;
        let settings = &mut editor.settings;
        let width = settings.layer(Layer::View).get("tab_size");
        let inferred = editor
            .detected_tab_size
            .take()
            .filter(|guess| width.and_then(serde_json::Value::as_u64) == Some(guess.width as u64));
        // https://www.sublimetext.com/docs/indentation.html#indentation-detection
        // Tabs reveal the indentation style, not the display width.
        let spaces = match self {
            Self::Tabs => {
                if let Some(guess) = inferred {
                    if let Some(previous) = guess.previous {
                        settings.set(Layer::View, "tab_size", previous);
                    } else {
                        let mut view = settings.layer(Layer::View).clone();
                        view.remove("tab_size");
                        settings
                            .replace(Layer::View, serde_json::Value::Object(view))
                            .expect("view settings");
                    }
                }
                false
            }
            Self::Spaces(size) => {
                let previous = inferred.map_or_else(|| width.cloned(), |guess| guess.previous);
                settings.set(Layer::View, "tab_size", serde_json::json!(size));
                editor.detected_tab_size = Some(DetectedTabSize {
                    width: size,
                    previous,
                });
                true
            }
        };
        settings.set(
            Layer::View,
            "translate_tabs_to_spaces",
            serde_json::json!(spaces),
        );
    }
}

pub fn detect_indentation(text: &Text) -> Option<Indentation> {
    let mut tabs = 0usize;
    let mut deltas = BTreeMap::<usize, usize>::new();
    let mut previous = 0;
    for line in text.lines().take(1000) {
        let end = line.floor_char_boundary(line.len_bytes().min(8192));
        let line = line.slice(0..end).to_string();
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
        Some(Indentation::Tabs)
    } else {
        deltas
            .into_iter()
            .max_by_key(|(n, count)| (*count, std::cmp::Reverse(*n)))
            .map(|(n, _)| Indentation::Spaces(n))
    }
}

pub fn syntax_path(path: Option<&Path>) -> String {
    path.unwrap_or(Path::new("untitled"))
        .to_string_lossy()
        .into_owned()
}

/// Contiguous occurrences of the caret's active scopes, from inner to outer. Uses only
/// published exact runs; never reparses a document on the input thread.
pub fn command_scope_regions(language: &Language, byte: usize, prefix: &str) -> Vec<Range<usize>> {
    let Some(snapshot) = language.scope_snapshot_at(byte) else {
        return vec![];
    };
    let Some(scopes) = snapshot.scopes_at(byte) else {
        return vec![];
    };
    let row = snapshot.text.byte_to_line(byte);
    let line_index = row - snapshot.range.start;
    let mut result = Vec::new();
    for scope in scopes
        .iter()
        .rev()
        .filter(|s| s.build_string().starts_with(prefix))
    {
        if *scope == language.root {
            result.push(0..snapshot.text.len_bytes());
            continue;
        }
        let contains = |stack| {
            snapshot
                .stacks
                .get(stack)
                .is_some_and(|active| active.contains(scope))
        };
        let mut range = byte..byte;
        // Start at the caret's run and stop at the first different scope in each
        // direction. Unrelated runs before/after this occurrence are never visited.
        'left: for (row, line) in (snapshot.range.start..row + 1)
            .zip(&snapshot.lines[..=line_index])
            .rev()
        {
            if !line.exact() {
                break;
            }
            let start = snapshot.text.line_to_byte(row);
            let local = range.start - start;
            for run in line.runs_before(local).rev() {
                if start + (run.end as usize) < range.start || !contains(run.stack) {
                    break 'left;
                }
                range.start = start + run.start as usize;
            }
        }
        'right: for (row, line) in (row..).zip(&snapshot.lines[line_index..]) {
            if !line.exact() {
                break;
            }
            let start = snapshot.text.line_to_byte(row);
            let local = range.end.saturating_sub(start);
            for run in line.runs_after(local) {
                if start + (run.start as usize) > range.end || !contains(run.stack) {
                    break 'right;
                }
                range.end = start + run.end as usize;
            }
        }
        result.push(range);
    }
    result.sort_by_key(|r| r.len());
    result.dedup();
    result
}

/// Collects only pairs touching/enclosing a point using the existing interval tree.
/// A many-caret command must not clone every unrelated pair once per caret.
fn command_enclosing_pairs(analysis: &Analysis, byte: usize) -> Vec<Pair> {
    fn visit(
        analysis: &Analysis,
        node: usize,
        range: Range<usize>,
        limit: usize,
        byte: usize,
        pairs: &mut Vec<Pair>,
    ) {
        if range.start >= limit || analysis.pairs.max_end(node) < byte {
            return;
        }
        if range.len() == 1 {
            if let Some(pair) = analysis.pairs.get(range.start) {
                pairs.push(pair);
            }
            return;
        }
        let middle = (range.start + range.end) / 2;
        visit(analysis, node * 2, range.start..middle, limit, byte, pairs);
        visit(
            analysis,
            node * 2 + 1,
            middle..range.end,
            limit,
            byte,
            pairs,
        );
    }
    let mut pairs = Vec::new();
    if analysis.pairs.leaf_count() != 0 {
        visit(
            analysis,
            1,
            0..analysis.pairs.leaf_count(),
            analysis.pairs.starts_at_or_before(byte),
            byte,
            &mut pairs,
        );
    }
    pairs
}

/// Reuses analyzed code/tag pairs. Within strings/comments it matches only inside that
/// contiguous scope, so punctuation cannot pair with a delimiter in surrounding code.
pub fn command_pairs(language: Option<&Language>, text: &Text, byte: usize) -> Vec<Pair> {
    let excluded = language.and_then(|l| {
        ["string", "comment"]
            .into_iter()
            .find_map(|prefix| command_scope_regions(l, byte, prefix).into_iter().last())
    });
    if excluded.is_none()
        && let Some(language) = language
    {
        return language
            .analysis
            .as_ref()
            .filter(|a| a.version == language.version)
            .map_or_else(Vec::new, |analysis| command_enclosing_pairs(analysis, byte));
    }
    let range = excluded.unwrap_or(0..text.len_bytes());
    let source = text.slice(range.clone()).to_string();
    let mut stack = Vec::new();
    let mut pairs = Vec::new();
    for (offset, c) in source.char_indices() {
        let at = range.start + offset;
        if matches!(c, '(' | '[' | '{') {
            stack.push((c, at));
        } else if let Some(open) = match c {
            ')' => Some('('),
            ']' => Some('['),
            '}' => Some('{'),
            _ => None,
        } && let Some((_, start)) = stack.pop_if(|(c, _)| *c == open)
        {
            pairs.push(Pair {
                open: start..start + 1,
                close: at..at + 1,
                kind: open,
            });
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
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
            SyntaxSet::load_cached_roots(&[source.path()], &INTERNAL_SYNTAXES[..1], cache.path())
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
    #[test]
    fn indentation_detection_ignores_blank_lines_and_observes_tab_stops() {
        assert_eq!(
            detect_indentation(&Text::from("a\n  b\n    c\n\n  d")),
            Some(Indentation::Spaces(2))
        );
        assert_eq!(
            detect_indentation(&Text::from("a\n\tb\n\t\tc")),
            Some(Indentation::Tabs)
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
    fn analysis_survives_workers_that_finish_an_edit_before_it_returns() {
        // A UI thread descheduled right after sending an edit to the highlighter (common
        // on loaded Windows runners) let the highlighter parse it and hand the analysis
        // worker its job before `change` published the version to that worker. The
        // worker dropped the job as stale, the highlighter publishes each version once,
        // and the language never became ready (no folds, pairs, guides or words).
        let mut editor = crate::editor::Editor::new(Text::from("fn f() {}\n"));
        editor
            .attach_language(
                Assets::bundled().unwrap(),
                "Rust",
                Arc::new(ColorScheme::velocity_dark()),
            )
            .unwrap();
        let wait_ready = |editor: &mut crate::editor::Editor| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                editor.poll_language();
                if let Some(language) = editor.language.borrow().as_ref() {
                    if language.ready() {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "language never ready: {}",
                        language.readiness()
                    );
                }
                std::thread::park_timeout(std::time::Duration::from_millis(1));
            }
        };
        wait_ready(&mut editor);
        ANALYSIS_RUNS_AHEAD.set(true);
        editor
            .run("insert", &vel_config::json!({"characters":"x"}))
            .unwrap();
        ANALYSIS_RUNS_AHEAD.set(false);
        let version = editor.language.borrow().as_ref().unwrap().version;
        assert_eq!(
            editor
                .language
                .borrow()
                .as_ref()
                .unwrap()
                .task
                .handled
                .load(Ordering::Acquire),
            version.0,
            "the analysis worker finished with the edit's job before `change` returned"
        );
        wait_ready(&mut editor);
        let language = editor.language.borrow();
        let analysis = language.as_ref().unwrap().analysis.as_ref().unwrap();
        assert_eq!(analysis.version, version);
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
