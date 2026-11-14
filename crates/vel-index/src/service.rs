use crate::{
    Error, FileAnalysis, Index, SymbolLocation, SymbolMatch, analyze,
    store::{Journal, Stamp},
};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use notify::Watcher;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use vel_syntax::{MetadataSet, SyntaxSet};
use vel_text::Text;
/// Project indexing settings; field names mirror the public editor settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexOptions {
    /// Worker count; zero chooses at most eight available cores.
    pub index_workers: usize,
    /// File/directory glob exclusions, matched against relative paths and basenames.
    pub index_exclude_patterns: Vec<String>,
    /// Respect .gitignore/global git excludes, even outside a git repository.
    pub index_exclude_gitignore: bool,
    /// Skip files whose filename and first line do not identify a syntax.
    pub index_skip_unknown_extensions: bool,
    /// Maximum source bytes per file (also applies to unsaved overlays).
    pub max_file_size: u64,
    /// Coalescing delay for repeated unsaved-buffer edits and filesystem events.
    pub debounce: Duration,
    /// Cache parent; None selects the platform user cache directory.
    pub cache_dir: Option<PathBuf>,
    /// Enable recursive notify monitoring; explicit `reindex_file` remains available if it fails.
    pub watch: bool,
}
impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            index_workers: 0,
            index_exclude_patterns: vec!["*.log".into()],
            index_exclude_gitignore: true,
            index_skip_unknown_extensions: true,
            max_file_size: 2 * 1024 * 1024,
            debounce: Duration::from_millis(15),
            cache_dir: None,
            watch: true,
        }
    }
}
/// Bounded status/log data for an Indexing Status window.
#[derive(Debug, Clone, Default)]
pub struct IndexStatus {
    /// Files found in the latest scan.
    pub total_files: usize,
    /// Successful analyses applied this session, including incremental updates.
    pub indexed_files: usize,
    /// Unchanged files restored from disk without parsing.
    pub cached_files: usize,
    /// Excluded, binary, oversized or unknown files skipped by workers.
    pub skipped_files: usize,
    /// Queued, debouncing and in-flight tasks.
    pub pending_files: usize,
    /// Whether scanning/indexing is active.
    pub active: bool,
    /// Permanent cancellation state; queries remain usable after cancellation.
    pub cancelled: bool,
    /// Most recent 256 status/error messages.
    pub logs: VecDeque<String>,
}
impl IndexStatus {
    /// Approximate latest-scan completion fraction, clamped to 0..=1 for incremental updates.
    pub fn fraction(&self) -> f32 {
        if self.total_files == 0 {
            return if self.active { 0.0 } else { 1.0 };
        }
        1.0 - self.pending_files.min(self.total_files) as f32 / self.total_files as f32
    }
}
#[derive(Debug)]
enum Command {
    Disk(PathBuf),
    Buffer(PathBuf, u64, Text),
    Analyzed(PathBuf, u64, Text, FileAnalysis),
    Close(PathBuf),
    Rescan,
    Flush(mpsc::Sender<std::io::Result<()>>),
    Stop,
}
#[derive(Debug)]
struct Task {
    path: PathBuf,
    generation: u64,
    overlay: Option<Text>,
    prepared: Option<FileAnalysis>,
}
#[derive(Debug)]
struct Completed {
    path: PathBuf,
    generation: u64,
    disk: bool,
    stamp: Option<Stamp>,
    analysis: Result<Option<FileAnalysis>, Error>,
}
/// Project service. Construction returns immediately; cache replay, walking and parsing run on
/// low-priority threads. All writes are serialized, stale tasks and unsaved versions are rejected.
#[derive(Debug)]
pub struct ProjectIndex {
    root: PathBuf,
    source_root: PathBuf,
    shared: Arc<RwLock<Index>>,
    commands: mpsc::Sender<Command>,
    status: Arc<Mutex<IndexStatus>>,
    cancelled: Arc<AtomicBool>,
}
impl ProjectIndex {
    /// Opens one canonical project root and its cache. No project files are written.
    pub fn open(
        root: impl AsRef<Path>,
        syntaxes: Arc<SyntaxSet>,
        metadata: Arc<MetadataSet>,
        options: IndexOptions,
    ) -> Result<Self, Error> {
        let source_root = if root.as_ref().is_absolute() {
            root.as_ref().to_path_buf()
        } else {
            std::env::current_dir()?.join(root.as_ref())
        };
        let root = root.as_ref().canonicalize()?;
        let excludes = excludes(&options)?;
        let shared = Arc::new(RwLock::new(Index::new()));
        let status = Arc::new(Mutex::new(IndexStatus {
            active: true,
            ..IndexStatus::default()
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let mut hash = blake3::Hasher::new();
        hash.update(root.to_string_lossy().as_bytes());
        hash.update(&metadata.fingerprint());
        hash.update(&bincode::serialize(&*syntaxes).map_err(|e| Error::Analysis(e.to_string()))?);
        hash.update(&bincode::serialize(&options).map_err(|e| Error::Analysis(e.to_string()))?);
        let directory = options
            .cache_dir
            .clone()
            .unwrap_or_else(cache_dir)
            .join("velocity/index");
        fs::create_dir_all(&directory)?;
        let directory = directory.canonicalize()?;
        let cache = directory.join(format!("{}.v6", hash.finalize().to_hex()));
        let worker = Coordinator {
            root: root.clone(),
            syntaxes,
            metadata,
            options,
            excludes,
            index: shared.clone(),
            status: status.clone(),
            cancelled: cancelled.clone(),
            commands: rx,
            event_tx: tx.clone(),
            cache,
        };
        thread::Builder::new()
            .name("velocity-index".into())
            .spawn(move || worker.run())?;
        Ok(Self {
            root,
            source_root,
            shared,
            commands: tx,
            status,
            cancelled,
        })
    }
    /// Shared index for asynchronous completion/query workers. Keep read guards short.
    pub fn shared_index(&self) -> Arc<RwLock<Index>> {
        self.shared.clone()
    }
    /// Current bounded status. Contains watcher, parse and persistence errors.
    pub fn status(&self) -> IndexStatus {
        let mut status = self.status.lock().unwrap().clone();
        status.cancelled |= self.cancelled.load(Ordering::Acquire);
        if status.cancelled {
            status.active = false;
        }
        status
    }
    fn path(&self, path: impl AsRef<Path>) -> PathBuf {
        if path.as_ref().is_absolute() {
            path.as_ref().strip_prefix(&self.source_root).map_or_else(
                |_| path.as_ref().to_path_buf(),
                |relative| self.root.join(relative),
            )
        } else {
            self.root.join(path)
        }
    }
    /// Schedules a debounced disk update (or deletion); unsaved overlays take precedence.
    pub fn reindex_file(&self, path: impl AsRef<Path>) {
        let _ = self.commands.send(Command::Disk(self.path(path)));
    }
    /// Coalesces unsaved versions; older/equal revisions never overwrite a newer overlay.
    /// Overlays are never persisted and are independently syntax/size/binary filtered.
    pub fn update_buffer(&self, path: impl AsRef<Path>, revision: u64, text: Text) {
        let _ = self
            .commands
            .send(Command::Buffer(self.path(path), revision, text));
    }
    /// Publishes an analysis prepared from the shared highlighter output, avoiding another
    /// syntax parse. Build it with `analyze_scopes` on the editor's analysis worker. `text` and
    /// analysis must describe this exact revision; normal debounce, staleness and overlay rules apply.
    pub fn update_buffer_analysis(
        &self,
        path: impl AsRef<Path>,
        revision: u64,
        text: Text,
        analysis: FileAnalysis,
    ) {
        let _ = self
            .commands
            .send(Command::Analyzed(self.path(path), revision, text, analysis));
    }
    /// Drops an unsaved overlay and restores the file's current disk contents.
    pub fn close_buffer(&self, path: impl AsRef<Path>) {
        let _ = self.commands.send(Command::Close(self.path(path)));
    }
    /// Rescans after project/ignore file or directory changes.
    pub fn rescan(&self) {
        let _ = self.commands.send(Command::Rescan);
    }
    /// Barrier for all current debounced/in-flight updates, followed by durable journal sync.
    /// Intended for tests/shutdown/background callers, never the editor's typing thread.
    pub fn flush(&self) -> std::io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "index cancelled",
            ));
        }
        let (tx, rx) = mpsc::channel();
        self.commands
            .send(Command::Flush(tx))
            .map_err(|_| std::io::Error::other("index stopped"))?;
        rx.recv()
            .map_err(|_| std::io::Error::other("index stopped"))?
    }
    /// Permanently cancels indexing without blocking on a regex. Already indexed queries remain.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.commands.send(Command::Stop);
    }
    /// Exact definition query.
    pub fn lookup_definition(&self, name: &str) -> Vec<SymbolLocation> {
        self.shared.read().unwrap().lookup_definition(name)
    }
    /// Exact reference query.
    pub fn lookup_references(&self, name: &str) -> Vec<SymbolLocation> {
        self.shared.read().unwrap().lookup_references(name)
    }
    /// Top-K fuzzy project definitions.
    pub fn fuzzy_symbols(&self, query: &str, k: usize) -> Vec<SymbolMatch> {
        self.shared.read().unwrap().fuzzy_symbols(query, k)
    }
}
impl Drop for ProjectIndex {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn cache_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return path.into();
    }
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "macos") {
        base.join("Library/Caches")
    } else {
        base.join(".cache")
    }
}
fn excludes(options: &IndexOptions) -> Result<GlobSet, Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in &options.index_exclude_patterns {
        builder.add(
            GlobBuilder::new(pattern.trim_end_matches('/'))
                .literal_separator(true)
                .build()
                .map_err(|e| Error::Analysis(e.to_string()))?,
        );
    }
    builder.build().map_err(|e| Error::Analysis(e.to_string()))
}
fn excluded(root: &Path, path: &Path, patterns: &GlobSet) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    relative
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
        || patterns.is_match(relative)
        || relative
            .ancestors()
            .take_while(|p| !p.as_os_str().is_empty())
            .any(|p| {
                patterns.is_match(p) || p.file_name().is_some_and(|name| patterns.is_match(name))
            })
}
fn ignored(root: &Path, path: &Path, git: bool) -> bool {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    if git {
        let (global, _) = ignore::gitignore::GitignoreBuilder::new(root).build_global();
        if global.matched_path_or_any_parents(path, false).is_ignore() {
            return true;
        }
        let _ = builder.add(root.join(".git/info/exclude"));
    }
    let mut parents: Vec<_> = path
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|parent| parent.starts_with(root))
        .collect();
    parents.reverse();
    for parent in parents {
        let _ = builder.add(parent.join(".ignore"));
        if git {
            let _ = builder.add(parent.join(".gitignore"));
        }
    }
    builder
        .build()
        .is_ok_and(|rules| rules.matched_path_or_any_parents(path, false).is_ignore())
}
fn priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: changes only the calling worker's QoS; no pointers or shared process state.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}
struct Coordinator {
    root: PathBuf,
    syntaxes: Arc<SyntaxSet>,
    metadata: Arc<MetadataSet>,
    options: IndexOptions,
    excludes: GlobSet,
    index: Arc<RwLock<Index>>,
    status: Arc<Mutex<IndexStatus>>,
    cancelled: Arc<AtomicBool>,
    commands: mpsc::Receiver<Command>,
    event_tx: mpsc::Sender<Command>,
    cache: PathBuf,
}
impl Coordinator {
    fn excluded(&self, path: &Path) -> bool {
        self.cache
            .parent()
            .is_some_and(|cache| path.starts_with(cache))
            || excluded(&self.root, path, &self.excludes)
    }
    fn log(&self, line: impl Into<String>) {
        let mut status = self.status.lock().unwrap();
        status.logs.push_back(line.into());
        if status.logs.len() > 256 {
            status.logs.pop_front();
        }
    }
    fn run(self) {
        priority();
        let loaded = Journal::open(
            &self.cache,
            &mut self.index.write().unwrap(),
            &self.cancelled,
        );
        let (mut journal, mut stamps) = match loaded {
            Ok(value) => value,
            Err(e) => {
                if !self.cancelled.load(Ordering::Acquire) {
                    self.log(format!("cache: {e}"));
                }
                let mut status = self.status.lock().unwrap();
                status.active = false;
                status.cancelled = self.cancelled.load(Ordering::Acquire);
                return;
            }
        };
        let workers = if self.options.index_workers == 0 {
            thread::available_parallelism()
                .map_or(2, usize::from)
                .clamp(1, 8)
        } else {
            self.options.index_workers.clamp(1, 64)
        };
        self.log(format!(
            "{workers} indexing workers; {} cached files",
            stamps.len()
        ));
        let (tasks, task_rx) = mpsc::sync_channel::<Task>(workers * 2);
        let task_rx = Arc::new(Mutex::new(task_rx));
        let (completed, result_rx) = mpsc::channel();
        for n in 0..workers {
            let rx = task_rx.clone();
            let tx = completed.clone();
            let syntaxes = self.syntaxes.clone();
            let metadata = self.metadata.clone();
            let options = self.options.clone();
            let cancelled = self.cancelled.clone();
            if let Err(error) = thread::Builder::new()
                .name(format!("velocity-index-{n}"))
                .spawn(move || {
                    priority();
                    loop {
                        let task = rx.lock().unwrap().recv();
                        let Ok(mut task) = task else {
                            break;
                        };
                        if cancelled.load(Ordering::Acquire) {
                            break;
                        }
                        let disk = task.overlay.is_none();
                        let result = analyze_task(&mut task, &syntaxes, &metadata, &options);
                        let (stamp, analysis) = match result {
                            Ok((stamp, analysis)) => (stamp, Ok(analysis)),
                            Err(error) => (None, Err(error)),
                        };
                        if tx
                            .send(Completed {
                                path: task.path,
                                generation: task.generation,
                                disk,
                                stamp,
                                analysis,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
            {
                self.log(format!("worker: {error}"));
            }
        }
        drop(completed);
        let mut watcher = if self.options.watch {
            let sender = self.event_tx.clone();
            match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                match event {
                    Ok(event) => {
                        if matches!(
                            event.kind,
                            notify::EventKind::Create(_)
                                | notify::EventKind::Remove(_)
                                | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                        ) {
                            let _ = sender.send(Command::Rescan);
                        }
                        for path in event.paths {
                            let path = path.canonicalize().unwrap_or_else(|_| {
                                path.parent()
                                    .and_then(|parent| parent.canonicalize().ok())
                                    .zip(path.file_name())
                                    .map_or_else(
                                        || path.clone(),
                                        |(parent, name)| parent.join(name),
                                    )
                            });
                            let _ = sender.send(Command::Disk(path));
                        }
                    }
                    Err(_) => {
                        let _ = sender.send(Command::Rescan);
                    }
                }
            }) {
                Ok(mut watcher) => {
                    if let Err(e) = watcher.watch(&self.root, notify::RecursiveMode::Recursive) {
                        self.log(format!("watcher unavailable: {e}; use reindex_file/rescan"));
                    }
                    Some(watcher)
                }
                Err(e) => {
                    self.log(format!("watcher unavailable: {e}; use reindex_file/rescan"));
                    None
                }
            }
        } else {
            None
        };
        let mut pending: HashMap<PathBuf, (Instant, u64, Option<Text>)> = HashMap::new();
        let mut desired = HashMap::new();
        let mut overlays: HashMap<PathBuf, (u64, Text)> = HashMap::new();
        let mut prepared: HashMap<PathBuf, FileAnalysis> = HashMap::new();
        let mut generation = 0u64;
        let mut inflight = 0usize;
        let mut running: HashMap<PathBuf, u64> = HashMap::new();
        let mut observed_files: HashMap<PathBuf, Option<Stamp>> = HashMap::new();
        let mut observed_directories: HashMap<PathBuf, Option<Stamp>> = HashMap::new();
        let mut next_poll = Instant::now() + Duration::from_millis(500);
        let mut scan = true;
        let mut flushes = Vec::new();
        while !self.cancelled.load(Ordering::Acquire) {
            while let Ok(command) = self.commands.try_recv() {
                match command {
                    Command::Stop => {
                        self.cancelled.store(true, Ordering::Release);
                        break;
                    }
                    Command::Rescan => scan = true,
                    Command::Flush(reply) => flushes.push(reply),
                    Command::Analyzed(path, revision, text, analysis) => {
                        if self.excluded(&path)
                            || ignored(&self.root, &path, self.options.index_exclude_gitignore)
                            || overlays.get(&path).is_some_and(|(old, _)| *old >= revision)
                        {
                            continue;
                        }
                        overlays.insert(path.clone(), (revision, text.clone()));
                        prepared.insert(path.clone(), analysis);
                        generation += 1;
                        desired.insert(path.clone(), generation);
                        pending.insert(
                            path,
                            (
                                Instant::now() + self.options.debounce,
                                generation,
                                Some(text),
                            ),
                        );
                    }
                    Command::Buffer(path, revision, text) => {
                        if self.excluded(&path)
                            || ignored(&self.root, &path, self.options.index_exclude_gitignore)
                            || overlays.get(&path).is_some_and(|(old, _)| *old >= revision)
                        {
                            continue;
                        }
                        prepared.remove(&path);
                        overlays.insert(path.clone(), (revision, text.clone()));
                        generation += 1;
                        desired.insert(path.clone(), generation);
                        pending.insert(
                            path,
                            (
                                Instant::now() + self.options.debounce,
                                generation,
                                Some(text),
                            ),
                        );
                    }
                    Command::Close(path) => {
                        overlays.remove(&path);
                        prepared.remove(&path);
                        generation += 1;
                        desired.insert(path.clone(), generation);
                        pending.insert(path, (Instant::now(), generation, None));
                    }
                    Command::Disk(path) => {
                        if self
                            .cache
                            .parent()
                            .is_some_and(|cache| path.starts_with(cache))
                        {
                            continue;
                        }
                        if ignored(&self.root, &path, self.options.index_exclude_gitignore)
                            || self.excluded(&path)
                        {
                            generation += 1;
                            desired.insert(path.clone(), generation);
                            pending.remove(&path);
                            prepared.remove(&path);
                            self.index.write().unwrap().remove_file(&path);
                            stamps.remove(&path);
                            if let Err(e) = journal.append(path, None, None) {
                                self.log(format!("persist: {e}"));
                            }
                            continue;
                        }
                        if path.is_dir()
                            || path
                                .file_name()
                                .is_some_and(|name| name == ".gitignore" || name == ".ignore")
                        {
                            scan = true;
                            continue;
                        }
                        if overlays.contains_key(&path) {
                            continue;
                        }
                        generation += 1;
                        desired.insert(path.clone(), generation);
                        pending.insert(
                            path,
                            (Instant::now() + self.options.debounce, generation, None),
                        );
                    }
                }
            }
            if self.options.watch && Instant::now() >= next_poll {
                next_poll = Instant::now() + Duration::from_millis(500);
                if observed_directories
                    .iter()
                    .any(|(path, stamp)| Stamp::read(path).ok() != *stamp)
                {
                    scan = true;
                }
                for (path, stamp) in &observed_files {
                    if overlays.contains_key(path)
                        || pending.contains_key(path)
                        || running.contains_key(path)
                    {
                        continue;
                    }
                    if Stamp::read(path).ok() != *stamp {
                        generation += 1;
                        desired.insert(path.clone(), generation);
                        pending.insert(path.clone(), (Instant::now(), generation, None));
                    }
                }
            }
            if scan {
                self.status.lock().unwrap().active = true;
                observed_files.clear();
                observed_directories.clear();
                if self.options.watch {
                    observed_directories.insert(self.root.clone(), Stamp::read(&self.root).ok());
                }
                let mut walker = ignore::WalkBuilder::new(&self.root);
                walker
                    .hidden(false)
                    .follow_links(false)
                    .git_ignore(self.options.index_exclude_gitignore)
                    .git_global(self.options.index_exclude_gitignore)
                    .git_exclude(self.options.index_exclude_gitignore)
                    .require_git(false);
                let mut seen = HashSet::new();
                let filter_root = self.root.clone();
                let filter_patterns = self.excludes.clone();
                let filter_cache = self.cache.parent().unwrap().to_path_buf();
                walker.filter_entry(move |entry| {
                    !entry.path().starts_with(&filter_cache)
                        && !excluded(&filter_root, entry.path(), &filter_patterns)
                });
                for entry in walker.build() {
                    if self.cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(e) => {
                            self.log(e.to_string());
                            continue;
                        }
                    };
                    if self.options.watch && entry.file_type().is_some_and(|ty| ty.is_dir()) {
                        observed_directories
                            .insert(entry.path().into(), Stamp::read(entry.path()).ok());
                        for name in [".gitignore", ".ignore"] {
                            let path = entry.path().join(name);
                            observed_directories.insert(path.clone(), Stamp::read(&path).ok());
                        }
                    }
                    if !entry.file_type().is_some_and(|ty| ty.is_file())
                        || self.excluded(entry.path())
                    {
                        continue;
                    }
                    let path = entry.into_path();
                    seen.insert(path.clone());
                    if self.options.watch {
                        observed_files.insert(path.clone(), Stamp::read(&path).ok());
                    }
                    if overlays.contains_key(&path) {
                        continue;
                    }
                    if Stamp::read(&path)
                        .ok()
                        .is_some_and(|stamp| stamps.get(&path) == Some(&stamp))
                    {
                        self.status.lock().unwrap().cached_files += 1;
                        continue;
                    }
                    generation += 1;
                    desired.insert(path.clone(), generation);
                    pending.insert(path, (Instant::now(), generation, None));
                }
                for path in stamps
                    .keys()
                    .filter(|path| !seen.contains(*path) && !overlays.contains_key(*path))
                    .cloned()
                    .collect::<Vec<_>>()
                {
                    self.index.write().unwrap().remove_file(&path);
                    stamps.remove(&path);
                    let _ = journal.append(path, None, None);
                }
                self.status.lock().unwrap().total_files = seen.len();
                scan = false;
            }
            for result in result_rx.try_iter() {
                inflight = inflight.saturating_sub(1);
                if running.get(&result.path) == Some(&result.generation) {
                    running.remove(&result.path);
                }
                if desired.get(&result.path) != Some(&result.generation) {
                    continue;
                }
                if result.disk && self.options.watch {
                    observed_files.insert(result.path.clone(), result.stamp);
                }
                match result.analysis {
                    Ok(analysis) => {
                        if result.disk
                            && result.stamp.is_some()
                            && Stamp::read(&result.path).ok() != result.stamp
                        {
                            generation += 1;
                            desired.insert(result.path.clone(), generation);
                            pending.insert(result.path, (Instant::now(), generation, None));
                            continue;
                        }
                        if let Some(analysis) = analysis {
                            if result.disk {
                                if let Err(e) = journal.append(
                                    result.path.clone(),
                                    result.stamp,
                                    Some(analysis.clone()),
                                ) {
                                    self.log(format!("persist: {e}"));
                                }
                                if let Some(stamp) = result.stamp {
                                    stamps.insert(result.path.clone(), stamp);
                                }
                            }
                            self.index
                                .write()
                                .unwrap()
                                .replace_file(result.path, analysis);
                            self.status.lock().unwrap().indexed_files += 1;
                        } else {
                            self.index.write().unwrap().remove_file(&result.path);
                            self.status.lock().unwrap().skipped_files += 1;
                            if result.disk {
                                stamps.remove(&result.path);
                                if let Err(e) = journal.append(result.path, None, None) {
                                    self.log(format!("persist: {e}"));
                                }
                            }
                        }
                    }
                    Err(e) => self.log(format!("{}: {e}", result.path.display())),
                }
            }
            let due: Vec<_> = pending
                .iter()
                .filter(|(_, (at, _, _))| *at <= Instant::now() || !flushes.is_empty())
                .take((workers * 3).saturating_sub(inflight))
                .map(|(path, _)| path.clone())
                .collect();
            for path in due {
                let (_, generation, overlay) = pending.remove(&path).unwrap();
                if self.excluded(&path) {
                    continue;
                }
                let task_path = path.clone();
                let analysis = prepared.remove(&path);
                match tasks.try_send(Task {
                    path,
                    generation,
                    overlay,
                    prepared: analysis,
                }) {
                    Ok(()) => {
                        inflight += 1;
                        running.insert(task_path, generation);
                    }
                    Err(mpsc::TrySendError::Full(task)) => {
                        if let Some(analysis) = task.prepared {
                            prepared.insert(task.path.clone(), analysis);
                        }
                        pending.insert(task.path, (Instant::now(), task.generation, task.overlay));
                        break;
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        self.log("all index workers stopped");
                        self.cancelled.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            let active = !pending.is_empty() || inflight > 0;
            {
                let mut status = self.status.lock().unwrap();
                status.pending_files = pending.len() + inflight;
                status.active = active;
            }
            if !active {
                for reply in flushes.drain(..) {
                    let _ = reply.send(journal.flush());
                }
            }
            thread::park_timeout(Duration::from_millis(1));
        }
        drop(tasks);
        watcher.take();
        if let Err(e) = journal.flush() {
            self.log(format!("persist: {e}"));
        }
        let mut status = self.status.lock().unwrap();
        status.active = false;
        status.cancelled = true;
    }
}
fn analyze_task(
    task: &mut Task,
    syntaxes: &Arc<SyntaxSet>,
    metadata: &MetadataSet,
    options: &IndexOptions,
) -> Result<(Option<Stamp>, Option<FileAnalysis>), Error> {
    let (stamp, source) = if let Some(text) = &task.overlay {
        if text.len_bytes() as u64 > options.max_file_size {
            return Ok((None, None));
        }
        (None, text.to_string())
    } else {
        let stamp = match Stamp::read(&task.path) {
            Ok(stamp) => stamp,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((None, None)),
            Err(e) => return Err(e.into()),
        };
        if stamp.len > options.max_file_size {
            return Ok((Some(stamp), None));
        }
        if fs::symlink_metadata(&task.path)?.file_type().is_symlink() {
            return Ok((Some(stamp), None));
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        fs::File::open(&task.path)?
            .take(options.max_file_size.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > options.max_file_size || bytes.contains(&0) {
            return Ok((Some(stamp), None));
        }
        let source = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => return Ok((Some(stamp), None)),
        };
        (Some(stamp), source)
    };
    if source.contains('\0')
        || options.index_skip_unknown_extensions
            && vel_highlight::detect_syntax(
                syntaxes,
                &task.path.to_string_lossy(),
                source.lines().next().unwrap_or(""),
                None,
            )
            .is_none()
    {
        return Ok((stamp, None));
    }
    let analysis = if let Some(prepared) = task.prepared.take() {
        prepared
    } else {
        analyze(&task.path, &source, syntaxes, metadata)?
    };
    Ok((stamp, Some(analysis)))
}
