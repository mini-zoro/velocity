//! Per-window project services. All cache I/O, publication and queries run off the UI thread.
pub(crate) use crate::editor::navigation::Jump;
use crate::workspace::Workspace;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use vel_buffer::{BufferId, ChangeId};
use vel_complete::CompletionEngine;
use vel_index::{IndexOptions, ProjectIndex, SymbolLocation};
#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub index: Arc<ProjectIndex>,
    pub engine: Arc<CompletionEngine>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    Symbols(String),
    Definition(String),
    Reference(String),
    Hover(String),
}
#[derive(Debug)]
pub struct ResultSet {
    pub query: Query,
    pub source: (u64, BufferId, ChangeId, usize),
    pub locations: Vec<SymbolLocation>,
    pub references: Vec<SymbolLocation>,
}
#[derive(Debug)]
enum Work {
    Query(
        u64,
        Query,
        (u64, BufferId, ChangeId, usize),
        Vec<Arc<ProjectIndex>>,
        Vec<OpenSymbols>,
    ),
}
#[derive(Debug)]
struct OpenSymbols {
    path: PathBuf,
    scopes: Arc<vel_highlight::ScopeSnapshot>,
    symbols: Arc<vel_highlight::SymbolList>,
}
impl OpenSymbols {
    fn lookup(&self, name: &str, reference: bool) -> impl Iterator<Item = SymbolLocation> {
        use vel_highlight::ScopeProvider;
        self.symbols
            .symbols
            .iter()
            .filter(move |symbol| {
                symbol.index_name == name
                    && if reference {
                        symbol.reference
                    } else {
                        symbol.indexed
                    }
            })
            .map(|symbol| {
                let point = self.scopes.text.byte_to_point(symbol.range.start);
                SymbolLocation {
                    file: self.path.clone(),
                    name: symbol.index_name.clone(),
                    row: point.row,
                    column: point.column,
                    range: symbol.range.clone(),
                    kind: vel_index::SymbolKind::from_scopes(
                        self.scopes
                            .scopes_at(symbol.range.start)
                            .unwrap_or_default(),
                    ),
                }
            })
    }
}
#[derive(Debug)]
enum Publication {
    Publish(
        Arc<ProjectIndex>,
        PathBuf,
        u64,
        Arc<vel_highlight::ScopeSnapshot>,
        Arc<vel_highlight::SymbolList>,
    ),
    Words(
        Vec<Arc<CompletionEngine>>,
        u64,
        Option<Arc<vel_complete::BufferWords>>,
    ),
    Close(Arc<ProjectIndex>, PathBuf),
    Flush(Vec<Arc<ProjectIndex>>, mpsc::Sender<Result<(), String>>),
}
#[derive(Debug, Default)]
pub struct StatusSource {
    pub indices: Vec<Arc<ProjectIndex>>,
    pub loading: bool,
    pub enabled: bool,
    pub(crate) labels: crate::ui_text::IndexLabels,
}
impl StatusSource {
    fn text(&self) -> String {
        if self.loading && self.indices.is_empty() {
            return format!("{}\n\n{}\n", self.labels.heading, self.labels.preparing);
        }
        let mut text = status_text_localized(&self.indices, &self.labels);
        if self.loading {
            text.push('\n');
            text.push_str(&self.labels.preparing);
            text.push('\n');
        }
        text
    }
}
#[derive(Debug)]
pub struct Intelligence {
    pub projects: Vec<Project>,
    pub status_source: Arc<Mutex<StatusSource>>,
    key: String,
    loading: Option<mpsc::Receiver<(Vec<Project>, Vec<String>)>>,
    tx: mpsc::Sender<Work>,
    publisher: mpsc::Sender<Publication>,
    publication_errors: mpsc::Receiver<String>,
    rx: mpsc::Receiver<(u64, ResultSet)>,
    generation: Arc<AtomicU64>,
    published: BTreeMap<PathBuf, (BufferId, ChangeId)>,
    words: BTreeMap<u64, ChangeId>,
    revision: u64,
    pub pending: bool,
    last_status: Instant,
    pub percentage: Option<u32>,
}
impl Default for Intelligence {
    fn default() -> Self {
        let (tx, jobs) = mpsc::channel();
        let (results, rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let latest = generation.clone();
        std::thread::Builder::new()
            .name("velocity-project-navigation".into())
            .spawn(move || {
                while let Ok(Work::Query(id, query, source, projects, open)) = jobs.recv() {
                    if latest.load(Ordering::Acquire) != id {
                        continue;
                    }
                    let mut locations = Vec::new();
                    let mut references = Vec::new();
                    // SymbolSource.ANY, plus the current-file priority restored in build 4186:
                    // https://www.sublimetext.com/docs/api_reference.html
                    // https://www.sublimetext.com/download
                    for tab in &open {
                        if latest.load(Ordering::Relaxed) != id {
                            break;
                        }
                        match &query {
                            Query::Definition(name) => locations.extend(tab.lookup(name, false)),
                            Query::Reference(name) => locations.extend(tab.lookup(name, true)),
                            Query::Hover(name) => {
                                locations.extend(tab.lookup(name, false));
                                references.extend(tab.lookup(name, true));
                            }
                            Query::Symbols(_) => {}
                        }
                    }
                    let open_paths: BTreeSet<_> = open.iter().map(|tab| &tab.path).collect();
                    for index in projects {
                        if latest.load(Ordering::Acquire) != id {
                            break;
                        }
                        match &query {
                            Query::Symbols(term) => locations.extend(
                                index
                                    .shared_index()
                                    .read()
                                    .unwrap()
                                    .fuzzy_symbols_cancel(term, 100, || {
                                        latest.load(Ordering::Relaxed) != id
                                    })
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|m| m.location),
                            ),
                            Query::Reference(name) => locations.extend(
                                index
                                    .lookup_references(name)
                                    .into_iter()
                                    .filter(|l| !open_paths.contains(&l.file)),
                            ),
                            Query::Definition(name) => locations.extend(
                                index
                                    .lookup_definition(name)
                                    .into_iter()
                                    .filter(|l| !open_paths.contains(&l.file)),
                            ),
                            Query::Hover(name) => {
                                locations.extend(
                                    index
                                        .lookup_definition(name)
                                        .into_iter()
                                        .filter(|l| !open_paths.contains(&l.file)),
                                );
                                references.extend(
                                    index
                                        .lookup_references(name)
                                        .into_iter()
                                        .filter(|l| !open_paths.contains(&l.file)),
                                );
                            }
                        }
                    }
                    if let Query::Symbols(term) = &query {
                        let mut scorer = vel_search::fuzzy::LabelMatcher::new(term);
                        locations.sort_by_cached_key(|l| {
                            std::cmp::Reverse(scorer.score(&l.name, None).unwrap_or(i64::MIN))
                        });
                        locations.truncate(100);
                    }
                    let mut seen = BTreeSet::new();
                    locations.retain(|l| seen.insert((l.file.clone(), l.range.start, l.range.end)));
                    seen.clear();
                    references
                        .retain(|l| seen.insert((l.file.clone(), l.range.start, l.range.end)));
                    if latest.load(Ordering::Acquire) == id {
                        let _ = results.send((
                            id,
                            ResultSet {
                                query,
                                source,
                                locations,
                                references,
                            },
                        ));
                    }
                }
            })
            .expect("project navigation worker");
        let (publisher, publications) = mpsc::channel();
        let (errors, publication_errors) = mpsc::channel();
        std::thread::Builder::new()
            .name("velocity-project-publish".into())
            .spawn(move || {
                while let Ok(work) = publications.recv() {
                    match work {
                        Publication::Publish(index, path, version, scopes, symbols) => {
                            if index.status().cancelled {
                                continue;
                            }
                            match vel_index::analyze_scopes(&scopes, &symbols) {
                                Ok(analysis) => index.update_buffer_analysis(
                                    path,
                                    version,
                                    scopes.text.clone(),
                                    analysis,
                                ),
                                Err(error) => {
                                    let _ = errors.send(format!("index overlay: {error}"));
                                }
                            }
                        }
                        Publication::Words(engines, id, words) => {
                            for engine in engines {
                                if let Some(words) = &words {
                                    engine.set_open_buffer(id, words.clone());
                                } else {
                                    engine.remove_open_buffer(id);
                                }
                            }
                        }
                        Publication::Close(index, path) => index.close_buffer(path),
                        Publication::Flush(indices, done) => {
                            let result = indices
                                .iter()
                                .try_for_each(|index| index.flush().map_err(|e| e.to_string()));
                            let _ = done.send(result);
                        }
                    }
                }
            })
            .expect("project publication worker");
        Self {
            projects: Vec::new(),
            status_source: Default::default(),
            key: String::new(),
            loading: None,
            tx,
            publisher,
            publication_errors,
            rx,
            generation,
            published: BTreeMap::new(),
            words: BTreeMap::new(),
            revision: 0,
            pending: false,
            last_status: Instant::now(),
            percentage: None,
        }
    }
}
impl Intelligence {
    pub(crate) fn rescan(&self) {
        for project in &self.projects {
            project.index.rescan();
        }
    }
    pub fn poll(&mut self, workspace: &mut Workspace) -> bool {
        let settings = &workspace.settings;
        // View-local changes do not restart indexing. Only index settings, roots and resource identities do.
        let key = format!(
            "{:?}:{}:{}:{}:{}:{:?}:{:?}:{}:{}:{:?}:{:?}",
            workspace.folders,
            settings.bool("index_files", true),
            settings.usize("index_workers", 0),
            settings.bool("index_exclude_gitignore", true),
            settings.bool("index_skip_unknown_extensions", true),
            settings.get("index_exclude_patterns"),
            settings.get("index_include_patterns"),
            workspace.resource_revision,
            settings.usize("index_max_file_size", 2 * 1024 * 1024),
            Arc::as_ptr(&workspace.catalog),
            Arc::as_ptr(&workspace.assets)
        );
        let mut changed = false;
        for error in self.publication_errors.try_iter() {
            workspace.messages.push(error);
            changed = true;
        }
        if self.key != key {
            if !self.key.is_empty() {
                self.generation.fetch_add(1, Ordering::AcqRel);
                self.pending = false;
            }
            self.key = key;
            for project in &self.projects {
                project.index.cancel();
            }
            self.projects.clear();
            self.published.clear();
            self.words.clear();
            self.loading = None;
            for tab in &mut workspace.tabs {
                tab.editor
                    .completion
                    .set_engine(workspace.catalog.engine.clone());
            }
            if settings.bool("index_files", true) && !workspace.folders.is_empty() {
                let folders = workspace.folders.clone();
                let project_folders = workspace.project_model.folders.clone();
                let project_base = workspace
                    .project_path()
                    .and_then(Path::parent)
                    .map(Path::to_owned);
                let assets = workspace.assets.clone();
                let catalog = workspace.catalog.clone();
                let options = IndexOptions {
                    cache_dir: if cfg!(test) {
                        workspace.data_path().map(|p| p.join("Cache/Index"))
                    } else {
                        std::env::var_os("VELOCITY_CACHE_DIR").map(PathBuf::from)
                    },
                    ..index_options(settings)
                };
                let (tx, rx) = mpsc::channel();
                self.loading = Some(rx);
                let _ = std::thread::Builder::new()
                    .name("velocity-project-start".into())
                    .spawn(move || {
                        let mut projects = Vec::new();
                        let mut messages = Vec::new();
                        // https://www.sublimetext.com/docs/projects.html#folders-key
                        // Per-folder index arrays extend, not replace, global arrays.
                        let local: std::collections::HashMap<_, _> = project_folders
                            .into_iter()
                            .filter_map(|f| {
                                let path = project_base
                                    .as_ref()
                                    .map_or_else(|| f.path.clone(), |base| base.join(&f.path));
                                Some((path.canonicalize().ok()?, f.extra))
                            })
                            .collect();
                        for folder in folders {
                            let root = folder.path.clone();
                            let mut options = options.clone();
                            if let Some(local) = local.get(&root) {
                                for (key, patterns) in [
                                    (
                                        "index_include_patterns",
                                        &mut options.index_include_patterns,
                                    ),
                                    (
                                        "index_exclude_patterns",
                                        &mut options.index_exclude_patterns,
                                    ),
                                ] {
                                    patterns.extend(
                                        local
                                            .get(key)
                                            .and_then(vel_config::Value::as_array)
                                            .into_iter()
                                            .flatten()
                                            .filter_map(vel_config::Value::as_str)
                                            .map(str::to_owned),
                                    );
                                }
                            }
                            match open_index(folder, &assets, options, &mut messages) {
                                Ok(index) => {
                                    let index = Arc::new(index);
                                    let engine = catalog
                                        .engine_for(Some(index.shared_index()), &mut messages);
                                    projects.push(Project {
                                        root,
                                        index,
                                        engine,
                                    });
                                }
                                Err(e) => messages.push(e),
                            }
                        }
                        let _ = tx.send((projects, messages));
                    });
            }
            changed = true;
        }
        if let Some((projects, messages)) = self.loading.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            self.loading = None;
            self.projects = projects;
            // Ready analysis can arrive before the new services. Publish those
            // snapshots to the new consumers even if the buffer hasn't changed.
            self.published.clear();
            self.words.clear();
            workspace.messages.extend(messages);
            changed = true;
        }
        let mut live = BTreeSet::new();
        let engines: Vec<_> = std::iter::once(workspace.catalog.engine.clone())
            .chain(self.projects.iter().map(|p| p.engine.clone()))
            .collect();
        for tab in &mut workspace.tabs {
            if tab.lazy.is_some()
                || !tab.editor.features.indexing
                || tab.editor.binary.is_some()
                || tab.loading.is_some()
            {
                continue;
            }
            let engine = self
                .projects
                .iter()
                .find(|p| tab.path.as_ref().is_some_and(|f| f.starts_with(&p.root)))
                .or_else(|| self.projects.first())
                .map_or(&workspace.catalog.engine, |p| &p.engine)
                .clone();
            tab.editor.completion.set_engine(engine);
            let language = tab.editor.language.borrow();
            let Some(language) = language.as_ref() else {
                continue;
            };
            let Some(analysis) = language
                .analysis
                .as_ref()
                .filter(|a| a.version == language.version)
            else {
                continue;
            };
            let Some(scopes) = language
                .scopes
                .as_ref()
                .filter(|s| s.version == analysis.version)
            else {
                continue;
            };
            if !tab.editor.shared_shadow && analysis.version != tab.editor.buffer.change_id() {
                continue;
            }
            // IME previews and clone placeholders are view data. Publish the exact committed
            // rope that produced these scopes, symbols and words.
            let snapshot = scopes.clone();
            let id = tab.shared_view.0;
            if self.words.get(&id) != Some(&analysis.version) {
                self.words.insert(id, analysis.version);
                let _ = self.publisher.send(Publication::Words(
                    engines.clone(),
                    id,
                    Some(analysis.words.clone()),
                ));
            }
            if let Some(path) = &tab.path {
                live.insert(path.clone());
                let identity = (tab.document, analysis.version);
                if self.published.get(path) != Some(&identity) {
                    self.published.insert(path.clone(), identity);
                    self.revision += 1;
                    for project in &self.projects {
                        if path.starts_with(&project.root) {
                            let _ = self.publisher.send(Publication::Publish(
                                project.index.clone(),
                                path.clone(),
                                self.revision,
                                snapshot.clone(),
                                analysis.index_symbols.clone(),
                            ));
                        }
                    }
                }
            }
        }
        // A pending edit retains its overlay until replacement analysis is available.
        live.extend(workspace.tabs.iter().filter_map(|t| t.path.clone()));
        for path in self
            .published
            .keys()
            .filter(|p| !live.contains(*p))
            .cloned()
            .collect::<Vec<_>>()
        {
            self.published.remove(&path);
            for project in &self.projects {
                let _ = self
                    .publisher
                    .send(Publication::Close(project.index.clone(), path.clone()));
            }
        }
        for id in self
            .words
            .keys()
            .filter(|id| !workspace.tabs.iter().any(|t| t.shared_view.0 == **id))
            .copied()
            .collect::<Vec<_>>()
        {
            self.words.remove(&id);
            let _ = self
                .publisher
                .send(Publication::Words(engines.clone(), id, None));
        }
        if self.last_status.elapsed() >= Duration::from_millis(100) || changed {
            let before = self.percentage;
            self.percentage = if self.loading.is_some() {
                Some(0)
            } else {
                let states: Vec<_> = self.projects.iter().map(|p| p.index.status()).collect();
                states.iter().any(|s| s.active).then(|| {
                    (states.iter().map(|s| s.fraction()).sum::<f32>() / states.len().max(1) as f32
                        * 100.) as u32
                })
            };
            changed |= before != self.percentage;
            self.last_status = Instant::now();
        }
        if changed {
            let mut status = self.status_source.lock().unwrap();
            status.indices = self.projects.iter().map(|p| p.index.clone()).collect();
            status.loading = self.loading.is_some();
            status.enabled = workspace.settings.bool("index_files", true);
        }
        changed
    }
    /// Benchmark/setup barrier; never called by an interactive event handler.
    pub fn flush_publications(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.publisher
            .send(Publication::Flush(
                self.projects.iter().map(|p| p.index.clone()).collect(),
                tx,
            ))
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(60))
            .map_err(|e| e.to_string())?
    }
    #[cfg(test)]
    pub(crate) fn defer_project_attachment(&mut self) -> impl FnOnce() + use<> {
        let projects = std::mem::take(&mut self.projects);
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        move || tx.send((projects, Vec::new())).unwrap()
    }
    pub fn busy(&self) -> bool {
        self.loading.is_some() || self.pending || self.percentage.is_some()
    }
    pub fn symbol_source(&self) -> Vec<(PathBuf, usize, usize, usize)> {
        self.projects
            .iter()
            .map(|project| {
                let status = project.index.status();
                (
                    project.root.clone(),
                    status.indexed_files,
                    status.skipped_files,
                    status.cached_files,
                )
            })
            .collect()
    }
    pub fn query(&mut self, query: Query, workspace: &Workspace) {
        self.query_at(query, workspace, workspace.editor().caret);
    }
    pub fn cancel_query(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.pending = false;
    }
    pub fn query_at(&mut self, query: Query, workspace: &Workspace, point: usize) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let e = workspace.editor();
        let open = if matches!(query, Query::Symbols(_)) {
            Vec::new()
        } else {
            let tabs = &workspace.tabs;
            std::iter::once(&tabs[workspace.active])
                .chain(
                    tabs.iter()
                        .enumerate()
                        .filter(|(index, _)| *index != workspace.active)
                        .map(|(_, tab)| tab),
                )
                .filter_map(|tab| {
                    if tab.editor.shared_shadow {
                        return None;
                    }
                    let lang = tab.editor.language.borrow();
                    let language = lang.as_ref()?;
                    let version = tab.editor.buffer.change_id();
                    let analysis = language
                        .analysis
                        .as_ref()
                        .filter(|analysis| analysis.version == version)?;
                    let scopes = language
                        .scopes
                        .as_ref()
                        .filter(|scopes| scopes.version == version)?;
                    let symbols = &analysis.index_symbols;
                    // A partial analysis must not suppress a valid on-disk result.
                    if !symbols.complete {
                        return None;
                    }
                    Some(OpenSymbols {
                        path: tab
                            .path
                            .clone()
                            .unwrap_or_else(|| PathBuf::from(format!("untitled:{}", tab.sheet))),
                        scopes: scopes.clone(),
                        symbols: symbols.clone(),
                    })
                })
                .collect()
        };
        let _ = self.tx.send(Work::Query(
            generation,
            query,
            (
                workspace.tabs[workspace.active].sheet,
                e.buffer.id(),
                e.buffer.change_id(),
                point,
            ),
            self.projects.iter().map(|p| p.index.clone()).collect(),
            open,
        ));
        self.pending = true;
    }
    pub fn result(&mut self) -> Option<ResultSet> {
        let result = self
            .rx
            .try_iter()
            .filter(|(id, _)| *id == self.generation.load(Ordering::Acquire))
            .last()
            .map(|(_, r)| r);
        if result.is_some() {
            self.pending = false;
        }
        result
    }
}
pub(crate) fn index_options(settings: &vel_config::settings::Settings) -> IndexOptions {
    IndexOptions {
        index_workers: settings.usize("index_workers", 0),
        index_exclude_patterns: patterns(settings, "index_exclude_patterns"),
        index_include_patterns: patterns(settings, "index_include_patterns"),
        index_exclude_gitignore: settings.bool("index_exclude_gitignore", true),
        index_skip_unknown_extensions: settings.bool("index_skip_unknown_extensions", true),
        max_file_size: settings.usize("index_max_file_size", 2 * 1024 * 1024) as u64,
        ..IndexOptions::default()
    }
}

fn patterns(settings: &vel_config::settings::Settings, key: &str) -> Vec<String> {
    settings
        .get(key)
        .and_then(vel_config::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(vel_config::Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn open_index(
    folder: vel_search::project::Folder,
    assets: &crate::language::Assets,
    options: IndexOptions,
    messages: &mut Vec<String>,
) -> Result<ProjectIndex, String> {
    if cfg!(test) && options.cache_dir.is_none() {
        return ProjectIndex::open_folder_temporary(
            folder,
            assets.syntaxes.clone(),
            assets.metadata.clone(),
            options,
        )
        .map_err(|e| e.to_string());
    }
    match ProjectIndex::open_folder(
        folder.clone(),
        assets.syntaxes.clone(),
        assets.metadata.clone(),
        options.clone(),
    ) {
        Ok(index) => Ok(index),
        Err(error) => {
            messages.push(format!(
                "Index cache unavailable ({error}); using temporary cache"
            ));
            ProjectIndex::open_folder_temporary(
                folder,
                assets.syntaxes.clone(),
                assets.metadata.clone(),
                options,
            )
            .map_err(|e| e.to_string())
        }
    }
}
pub fn word(editor: &crate::editor::Editor, point: usize) -> String {
    let text = &editor.text;
    let point = text.floor_char_boundary(point.min(text.len_bytes()));
    let row = text.byte_to_line(point);
    if text.line_len_bytes(row) > 8192 {
        return String::new();
    }
    let start = text.line_to_byte(row);
    let line = text.line(row).to_string();
    let col = point - start;
    let begin = line[..col]
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
        .map_or(0, |(at, c)| at + c.len_utf8());
    let end = line[col..]
        .char_indices()
        .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
        .map_or(line.len(), |(at, _)| col + at);
    line[begin..end].into()
}
pub fn capture(workspace: &Workspace) -> Jump {
    let t = &workspace.tabs[workspace.active];
    Jump {
        sheet: t.sheet,
        buffer: t.editor.buffer.id(),
        version: t.editor.buffer.change_id(),
        selection: t.editor.buffer.view(t.editor.id).sel().clone(),
        caret_count: t.editor.buffer.view(t.editor.id).sel().len(),
        scroll: t.scroll,
        path: t.path.clone().map(Arc::new),
        position: crate::editor::navigation::position(&t.editor.text, t.editor.caret),
    }
}
pub(crate) fn restore(workspace: &mut Workspace, jump: &Jump) -> bool {
    let index = workspace
        .tabs
        .iter()
        .position(|t| t.sheet == jump.sheet)
        .or_else(|| {
            jump.path.as_deref().and_then(|path| {
                workspace
                    .tabs
                    .iter()
                    .position(|t| t.path.as_ref() == Some(path))
            })
        });
    let index = if let Some(index) = index {
        index
    } else {
        let Some(path) = jump.path.as_deref() else {
            return false;
        };
        if !path.is_file() {
            workspace.jump_history.borrow_mut().notice =
                Some(format!("{} no longer exists", path.display()));
            return false;
        }
        if workspace
            .open(crate::cli::FileLocation {
                path: path.clone(),
                line: jump.position.row + 1,
                column: jump.position.column + 1,
            })
            .is_err()
        {
            return false;
        }
        workspace.active
    };
    if workspace.prepare_sheet(index).is_err() {
        return false;
    }
    if workspace.tabs[index].loading.is_some() {
        workspace.select(index);
        workspace.jump_history.borrow_mut().pending_restore = Some(workspace.tabs[index].sheet);
        return true;
    }
    let editor = &workspace.tabs[index].editor;
    let mut selection = if editor.buffer.id() == jump.buffer {
        if let Some(changes) = editor.buffer.retained_changes_since(jump.version) {
            let changes: Vec<_> = changes.collect();
            jump.selection.map(|mut region| {
                for (_, map) in &changes {
                    region = vel_buffer::Region::new(
                        map.transform_point(region.a, vel_buffer::Bias::Right),
                        map.transform_point(region.b, vel_buffer::Bias::Right),
                    );
                }
                region
            })
        } else {
            vel_buffer::Selection::from_regions([vel_buffer::Region::caret(
                crate::editor::navigation::position_byte(editor.buffer.text(), jump.position),
            )])
        }
    } else {
        // Closed/reloaded documents retain a path and a clipped line/column, never stale bytes.
        vel_buffer::Selection::from_regions([vel_buffer::Region::caret(
            crate::editor::navigation::position_byte(editor.buffer.text(), jump.position),
        )])
    };
    let text = editor.buffer.text();
    selection = selection.map(|r| {
        vel_buffer::Region::new(
            text.floor_char_boundary(r.a.min(text.len_bytes())),
            text.floor_char_boundary(r.b.min(text.len_bytes())),
        )
    });
    workspace.select(index);
    workspace.editor_mut().set_selection(selection);
    workspace.tabs[index].scroll = jump.scroll;
    true
}

pub fn jump(
    workspace: &mut Workspace,
    location: &SymbolLocation,
    preview: bool,
) -> Result<(), String> {
    if let Some(id) = location
        .file
        .to_str()
        .and_then(|p| p.strip_prefix("untitled:"))
        .and_then(|id| id.parse().ok())
    {
        let index = workspace
            .sheet_index(id)
            .ok_or("definition tab was closed")?;
        workspace.select(index);
        select_symbol(workspace, location);
        return Ok(());
    }
    let target = crate::cli::FileLocation {
        path: location.file.clone(),
        line: location.row + 1,
        column: 1,
    };
    if preview {
        workspace.preview(target)?;
    } else {
        workspace.open(target)?;
    }
    select_symbol(workspace, location);
    Ok(())
}
fn select_symbol(workspace: &mut Workspace, location: &SymbolLocation) {
    let text = &workspace.editor().text;
    let row = location.row.min(text.len_lines() - 1);
    let at = text.floor_char_boundary(
        text.line_to_byte(row) + location.column.min(text.line(row).len_bytes()),
    );
    workspace.editor_mut().set_caret(at, false);
}
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OpenOptions {
    pub side_by_side: bool,
    pub clear_to_right: bool,
    pub origin: u64,
}
pub(crate) fn jump_with_options(
    workspace: &mut Workspace,
    location: &SymbolLocation,
    preview: bool,
    options: OpenOptions,
) -> Result<(), String> {
    let _history = crate::editor::navigation::Guard::suppress(&workspace.jump_history);
    let source = workspace.sheet_index(options.origin);
    let group = source.and_then(|i| workspace.workbench.group_of(workspace.tabs[i].sheet));
    if !location.file.is_absolute() {
        return jump(workspace, location, preview);
    }
    let Some(group) = group.filter(|_| options.side_by_side) else {
        return jump(workspace, location, preview);
    };
    let source = source.expect("source group");
    let source_sheet = workspace.tabs[source].sheet;
    workspace.prepare_sheet(source)?;
    let source_tab = &workspace.tabs[source];
    let source_jump = Jump {
        sheet: source_sheet,
        buffer: source_tab.editor.buffer.id(),
        version: source_tab.editor.buffer.change_id(),
        selection: source_tab
            .editor
            .buffer
            .view(source_tab.editor.id)
            .sel()
            .clone(),
        caret_count: source_tab
            .editor
            .buffer
            .view(source_tab.editor.id)
            .sel()
            .len(),
        scroll: source_tab.scroll,
        path: source_tab.path.clone().map(Arc::new),
        position: crate::editor::navigation::position(
            &source_tab.editor.text,
            source_tab.editor.caret,
        ),
    };
    let previous_groups = workspace.workbench.groups.clone();
    let mut selected = workspace.workbench.groups[group].selected.clone();
    if options.clear_to_right {
        // CLEAR_TO_RIGHT unselects sheets after the source, not groups and not files:
        // https://www.sublimetext.com/docs/api_reference.html#sublime.OpenFlags.CLEAR_TO_RIGHT
        let sheets = &workspace.workbench.groups[group].sheets;
        let source_at = sheets
            .iter()
            .position(|&sheet| sheet == source_sheet)
            .unwrap_or(0);
        selected.retain(|sheet| sheets[..=source_at].contains(sheet));
    }
    selected.insert(source_sheet);
    workspace.tabs[source].transient = false;
    workspace.workbench.active = group;
    let existing = workspace.tabs.iter().position(|tab| {
        tab.sheet != source_sheet
            && tab.path.as_deref() == Some(location.file.as_path())
            && workspace.workbench.group_of(tab.sheet) == Some(group)
    });
    if let Some(index) = existing {
        // Reuse a side preview of this document, including a clone of the source.
        // Repeated arrow navigation must not accumulate hidden transient clones.
        workspace.select(index);
        if !preview {
            workspace.tabs[index].transient = false;
        }
        select_symbol(workspace, location);
    } else {
        jump(workspace, location, preview)?;
    }
    let target = workspace.tabs[workspace.active].sheet;
    if target == source_sheet || workspace.workbench.group_of(target) != Some(group) {
        // Existing views elsewhere stay in their group. The existing clone primitive
        // shares the buffer without moving or replacing the user's original view.
        workspace.clone_file()?;
        if workspace.workbench.active != group {
            workspace.workbench.move_selected(group)?;
        }
        workspace.tabs[workspace.active].transient = preview;
        let clone_sheet = workspace.tabs[workspace.active].sheet;
        if target == source_sheet {
            restore(workspace, &source_jump);
            if let Some(index) = workspace.sheet_index(clone_sheet) {
                workspace.select(index);
            }
        }
        for (i, previous) in previous_groups
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != group)
        {
            let current = &mut workspace.workbench.groups[i];
            current.selected.clone_from(&previous.selected);
            current.focused = previous.focused;
            current.anchor = previous.anchor;
        }
    }
    let target = workspace.tabs[workspace.active].sheet;
    let g = &mut workspace.workbench.groups[group];
    g.sheets.retain(|&sheet| sheet != target);
    let after = g
        .sheets
        .iter()
        .position(|&sheet| sheet == source_sheet)
        .map_or(g.sheets.len(), |i| i + 1);
    g.sheets.insert(after, target);
    g.selected = selected
        .into_iter()
        .filter(|s| g.sheets.contains(s))
        .collect();
    g.selected.insert(target);
    g.focused = Some(target);
    g.anchor = Some(target);
    workspace.workbench.active = group;
    workspace.observe_navigation();
    Ok(())
}
#[cfg(test)]
pub fn status_text(indices: &[Arc<ProjectIndex>]) -> String {
    status_text_localized(indices, &crate::ui_text::IndexLabels::default())
}

pub fn status_text_localized(
    indices: &[Arc<ProjectIndex>],
    labels: &crate::ui_text::IndexLabels,
) -> String {
    let mut output = format!("{}\n\n", labels.heading);
    if indices.is_empty() {
        output.push_str(&labels.inactive);
        output.push('\n');
    }
    for index in indices {
        let status = index.status();
        output.push_str(&vel_i18n::format_named(
            &labels.progress,
            &[
                ("percent", ((status.fraction() * 100.) as u32).to_string()),
                ("files", status.indexed_files.to_string()),
                ("cached", status.cached_files.to_string()),
                ("pending", status.pending_files.to_string()),
                ("skipped", status.skipped_files.to_string()),
            ],
        ));
        output.push('\n');
        for line in &status.logs {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct StatusWindow {
    source: Arc<Mutex<StatusSource>>,
    owner: vel_platform::WindowHandle,
    renderer: Option<vel_render::Renderer>,
    fonts: Option<vel_render::font::Fonts>,
    scroll: f64,
}
#[cfg(target_os = "macos")]
impl StatusWindow {
    pub fn new(source: Arc<Mutex<StatusSource>>, owner: vel_platform::WindowHandle) -> Self {
        Self {
            source,
            owner,
            renderer: None,
            fonts: None,
            scroll: 0.,
        }
    }
    fn toggle(&self, w: &vel_platform::WindowHandle) {
        self.owner.dispatch_named_command("toggle_indexing", "{}");
        w.request_frame();
    }
}
#[cfg(target_os = "macos")]
impl vel_platform::AppHandler for StatusWindow {
    fn on_start(&mut self, w: &vel_platform::WindowHandle) -> Result<(), String> {
        self.fonts = Some(vel_render::font::Fonts::load_named("Menlo")?);
        self.renderer = Some(vel_render::Renderer::new(
            w.clone(),
            false,
            &std::env::temp_dir().join("velocity-index-status-renderer"),
            |_| {},
        )?);
        Ok(())
    }
    fn on_scroll(&mut self, w: &vel_platform::WindowHandle, event: vel_platform::ScrollEvent) {
        let lines = self.source.lock().unwrap().text().lines().count() as f64;
        let maximum = (lines - (w.content_size().height - 32.) / 20.).max(0.);
        self.scroll = (self.scroll - event.delta_y / 20.).clamp(0., maximum);
        w.request_frame();
    }
    fn on_key(&mut self, w: &vel_platform::WindowHandle, event: vel_platform::KeyEvent) -> bool {
        if event.state == vel_platform::KeyState::Down
            && event.characters == " "
            && !event.repeat
            && !event.modifiers.command()
            && !event.modifiers.option()
            && !event.modifiers.control()
        {
            self.toggle(w);
            return true;
        }
        false
    }
    fn on_mouse(&mut self, w: &vel_platform::WindowHandle, event: vel_platform::MouseEvent) {
        if event.kind == vel_platform::MouseKind::Down
            && event.button == 0
            && (8. ..32.).contains(&event.position.y)
            && (8. ..300.).contains(&event.position.x)
        {
            self.toggle(w);
        }
    }
    fn on_resize(&mut self, _: &vel_platform::WindowHandle, size: vel_platform::Size, scale: f64) {
        if let Some(renderer) = &mut self.renderer {
            renderer.resize((size.width * scale) as u32, (size.height * scale) as u32);
        }
    }
    fn on_frame(&mut self, ctx: &mut vel_platform::FrameContext<'_>) -> Result<(), String> {
        let size = ctx.window.content_size();
        let clip = vel_platform::Rect {
            x: 0.,
            y: 0.,
            width: size.width,
            height: size.height,
        };
        let mut layer = vel_render::scene::Layer::default();
        let (status_text, toggle) = {
            let source = self.source.lock().unwrap();
            ctx.window.set_title(&source.labels.title);
            let check = if source.enabled { "x" } else { " " };
            (source.text(), format!("[{check}] {}", source.labels.enable))
        };
        vel_ui::label(&mut layer, &toggle, 12., 24., clip, [0.85, 0.9, 0.96, 1.]);
        for (i, line) in status_text
            .lines()
            .skip(self.scroll.floor() as usize)
            .take(((size.height - 32.) / 20.) as usize)
            .enumerate()
        {
            vel_ui::label(
                &mut layer,
                line,
                12.,
                54. + i as f64 * 20.,
                clip,
                [0.85, 0.9, 0.96, 1.],
            );
        }
        let scene = vel_render::scene::Scene {
            background: [0.09, 0.12, 0.16, 1.],
            layers: vec![layer],
        };
        if ctx.reason == vel_platform::FrameReason::StartupPrepare {
            self.renderer
                .as_mut()
                .ok_or("status renderer")?
                .pre_render(
                    &scene,
                    self.fonts.as_mut().ok_or("status fonts")?,
                    ctx.window.scale_factor() as f32,
                )?;
            return Ok(());
        }
        if self
            .renderer
            .as_ref()
            .is_some_and(|r| r.has_prepared_frame())
        {
            self.renderer.as_mut().unwrap().present_prepared()?;
            self.renderer.as_mut().unwrap().release_prepared();
        }
        self.renderer.as_mut().ok_or("status renderer")?.render(
            &scene,
            self.fonts.as_mut().ok_or("status fonts")?,
            ctx.window.scale_factor() as f32,
            None,
        )?;
        ctx.window
            .request_frame_at(Instant::now() + Duration::from_millis(250));
        Ok(())
    }
}
