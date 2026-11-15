//! Per-window project services. All cache I/O, publication and queries run off the UI thread.
use crate::workspace::Workspace;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use vel_buffer::{BufferId, ChangeId, Selection};
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
    pub source: (BufferId, ChangeId, usize),
    pub locations: Vec<SymbolLocation>,
}
#[derive(Debug)]
enum Work {
    Query(
        u64,
        Query,
        (BufferId, ChangeId, usize),
        Vec<Arc<ProjectIndex>>,
    ),
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
}
impl StatusSource {
    fn text(&self) -> String {
        if self.loading && self.indices.is_empty() {
            return "Indexing Status\n\nPreparing project index…\n".into();
        }
        let mut text = status_text(&self.indices);
        if self.loading {
            text.push_str("\nPreparing project index…\n");
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
    pub back: VecDeque<Jump>,
    pub forward: VecDeque<Jump>,
    pub pending: bool,
    last_status: Instant,
    pub percentage: Option<u32>,
}
#[derive(Clone, Debug)]
pub struct Jump {
    pub sheet: u64,
    pub path: Option<PathBuf>,
    pub selection: Selection,
    pub scroll: (f64, f64),
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
                while let Ok(Work::Query(id, query, source, projects)) = jobs.recv() {
                    if latest.load(Ordering::Acquire) != id {
                        continue;
                    }
                    let mut locations = Vec::new();
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
                            Query::Reference(name) => {
                                locations.extend(index.lookup_references(name))
                            }
                            Query::Definition(name) | Query::Hover(name) => {
                                locations.extend(index.lookup_definition(name))
                            }
                        }
                    }
                    if let Query::Symbols(term) = &query {
                        locations.sort_by_key(|l| {
                            std::cmp::Reverse(
                                vel_index::fuzzy_score(&l.name, term).unwrap_or(i64::MIN),
                            )
                        });
                        locations.truncate(100);
                    }
                    let mut seen = BTreeSet::new();
                    locations.retain(|l| seen.insert((l.file.clone(), l.range.start, l.range.end)));
                    if latest.load(Ordering::Acquire) == id {
                        let _ = results.send((
                            id,
                            ResultSet {
                                query,
                                source,
                                locations,
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
            back: VecDeque::new(),
            forward: VecDeque::new(),
            pending: false,
            last_status: Instant::now(),
            percentage: None,
        }
    }
}
impl Intelligence {
    pub fn poll(&mut self, workspace: &mut Workspace) -> bool {
        let settings = &workspace.settings;
        // View-local changes do not restart indexing. Only index settings, roots and resource identities do.
        let key = format!(
            "{:?}:{}:{}:{}:{}:{}:{}:{:?}:{:?}",
            workspace.folders,
            settings.bool("index_files", true),
            settings.usize("index_workers", 0),
            settings.bool("index_exclude_gitignore", true),
            settings.bool("index_skip_unknown_extensions", true),
            settings
                .get("index_exclude_patterns")
                .unwrap_or(&vel_config::Value::Null),
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
            self.key = key;
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.pending = false;
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
                let roots: Vec<_> = workspace.folders.iter().map(|f| f.path.clone()).collect();
                let assets = workspace.assets.clone();
                let catalog = workspace.catalog.clone();
                let options = IndexOptions {
                    index_workers: settings.usize("index_workers", 0),
                    index_exclude_patterns: patterns(settings, "index_exclude_patterns"),
                    index_exclude_gitignore: settings.bool("index_exclude_gitignore", true),
                    index_skip_unknown_extensions: settings
                        .bool("index_skip_unknown_extensions", true),
                    max_file_size: settings.usize("index_max_file_size", 2 * 1024 * 1024) as u64,
                    cache_dir: Some(
                        workspace
                            .data_path()
                            .map(|p| p.join("Cache/Index"))
                            .unwrap_or_else(|| {
                                std::env::temp_dir()
                                    .join(format!("velocity-index-{}", std::process::id()))
                            }),
                    ),
                    ..IndexOptions::default()
                };
                let (tx, rx) = mpsc::channel();
                self.loading = Some(rx);
                let _ = std::thread::Builder::new()
                    .name("velocity-project-start".into())
                    .spawn(move || {
                        let mut projects = Vec::new();
                        let mut messages = Vec::new();
                        for root in roots {
                            match open_index(&root, &assets, options.clone(), &mut messages) {
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
            workspace.messages.extend(messages);
            changed = true;
        }
        let mut live = BTreeSet::new();
        let engines: Vec<_> = std::iter::once(workspace.catalog.engine.clone())
            .chain(self.projects.iter().map(|p| p.engine.clone()))
            .collect();
        for tab in &mut workspace.tabs {
            if tab.lazy.is_some() {
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
    pub fn busy(&self) -> bool {
        self.loading.is_some() || self.pending || self.percentage.is_some()
    }
    pub fn query(&mut self, query: Query, workspace: &Workspace) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let e = workspace.editor();
        let _ = self.tx.send(Work::Query(
            generation,
            query,
            (e.buffer.id(), e.buffer.change_id(), e.caret),
            self.projects.iter().map(|p| p.index.clone()).collect(),
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
    pub fn remember(&mut self, jump: Jump) {
        if self
            .back
            .back()
            .is_none_or(|old| old.sheet != jump.sheet || old.selection != jump.selection)
        {
            self.back.push_back(jump);
            while self.back.len() > 128 {
                self.back.pop_front();
            }
        }
        self.forward.clear();
    }
    pub fn history(&mut self, workspace: &mut Workspace, forward: bool) -> Result<(), String> {
        let current = capture(workspace);
        let target = if forward {
            self.forward.pop_back()
        } else {
            self.back.pop_back()
        };
        if let Some(target) = target {
            restore(workspace, &target)?;
            let stack = if forward {
                &mut self.back
            } else {
                &mut self.forward
            };
            stack.push_back(current);
            while stack.len() > 128 {
                stack.pop_front();
            }
        }
        Ok(())
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
    root: &Path,
    assets: &crate::language::Assets,
    mut options: IndexOptions,
    messages: &mut Vec<String>,
) -> Result<ProjectIndex, String> {
    match ProjectIndex::open(
        root,
        assets.syntaxes.clone(),
        assets.metadata.clone(),
        options.clone(),
    ) {
        Ok(index) => Ok(index),
        Err(error) => {
            messages.push(format!(
                "Index cache unavailable ({error}); using temporary cache"
            ));
            static NEXT: AtomicU64 = AtomicU64::new(0);
            options.cache_dir = Some(std::env::temp_dir().join(format!(
                "velocity-index-fallback-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )));
            ProjectIndex::open(
                root,
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
        path: t.path.clone(),
        selection: t.editor.buffer.view(t.editor.id).sel().clone(),
        scroll: t.scroll,
    }
}
pub fn restore(workspace: &mut Workspace, jump: &Jump) -> Result<(), String> {
    if let Some(index) = workspace.tabs.iter().position(|t| t.sheet == jump.sheet) {
        workspace.select(index);
    } else if let Some(path) = &jump.path {
        workspace.open(crate::cli::FileLocation {
            path: path.clone(),
            line: 1,
            column: 1,
        })?;
    } else {
        return Ok(());
    }
    let text = &workspace.editor().text;
    let selection = vel_buffer::Selection::from_regions(jump.selection.iter().map(|r| {
        vel_buffer::Region::new(
            text.floor_char_boundary(r.a.min(text.len_bytes())),
            text.floor_char_boundary(r.b.min(text.len_bytes())),
        )
    }));
    workspace.editor_mut().set_selection(selection);
    workspace.tabs[workspace.active].scroll = jump.scroll;
    Ok(())
}
pub fn jump(
    workspace: &mut Workspace,
    location: &SymbolLocation,
    preview: bool,
) -> Result<(), String> {
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
    let text = &workspace.editor().text;
    let row = location.row.min(text.len_lines() - 1);
    let at = text.floor_char_boundary(
        text.line_to_byte(row) + location.column.min(text.line(row).len_bytes()),
    );
    workspace.editor_mut().set_caret(at, false);
    Ok(())
}
pub fn status_text(indices: &[Arc<ProjectIndex>]) -> String {
    let mut output = String::from("Indexing Status\n\n");
    if indices.is_empty() {
        output.push_str("No project index. Open a folder and enable index_files.\n");
    }
    for index in indices {
        let status = index.status();
        output.push_str(&format!(
            "{}%  {} files, {} cached, {} pending, {} skipped\n",
            (status.fraction() * 100.) as u32,
            status.indexed_files,
            status.cached_files,
            status.pending_files,
            status.skipped_files
        ));
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
    renderer: Option<vel_render::Renderer>,
    fonts: Option<vel_render::font::Fonts>,
    scroll: f64,
}
#[cfg(target_os = "macos")]
impl StatusWindow {
    pub fn new(source: Arc<Mutex<StatusSource>>) -> Self {
        Self {
            source,
            renderer: None,
            fonts: None,
            scroll: 0.,
        }
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
        let maximum = (lines - w.content_size().height / 20.).max(0.);
        self.scroll = (self.scroll - event.delta_y / 20.).clamp(0., maximum);
        w.request_frame();
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
        let status_text = self.source.lock().unwrap().text();
        for (i, line) in status_text
            .lines()
            .skip(self.scroll.floor() as usize)
            .take((size.height / 20.) as usize)
            .enumerate()
        {
            vel_ui::label(
                &mut layer,
                line,
                12.,
                22. + i as f64 * 20.,
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
