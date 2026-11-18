use crate::{
    Error, FileAnalysis, Index, SymbolLocation, SymbolMatch, analyze,
    store::{Journal, Stamp},
};
use hashlink::LinkedHashMap;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::Watcher;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
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
use vel_search::project::{Folder, Patterns};
use vel_syntax::{MetadataSet, SyntaxSet};
use vel_text::Text;
/// Project indexing settings; field names mirror the public editor settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexOptions {
    /// Worker count; zero chooses at most eight available cores.
    pub index_workers: usize,
    /// File patterns excluded from indexing.
    pub index_exclude_patterns: Vec<String>,
    /// File patterns admitted before exclusions; empty admits every project file.
    #[serde(default)]
    pub index_include_patterns: Vec<String>,
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
            index_include_patterns: Vec::new(),
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
    /// Regular files admitted across the project, including files skipped by analysis.
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
        Self::open_folder(
            Folder {
                path: root.as_ref().into(),
                name: String::new(),
                rules: Default::default(),
                follow_links: false,
            },
            syntaxes,
            metadata,
            options,
        )
    }

    /// Opens the symbol index using the same project filters as file navigation.
    pub fn open_folder(
        mut folder: Folder,
        syntaxes: Arc<SyntaxSet>,
        metadata: Arc<MetadataSet>,
        options: IndexOptions,
    ) -> Result<Self, Error> {
        let source_root = std::path::absolute(&folder.path)?;
        let root = source_root.canonicalize()?;
        // Keep paths under the original project spelling for absolute patterns.
        folder.path.clone_from(&source_root);
        let filter = Filter {
            folder,
            includes: Patterns::new(&options.index_include_patterns),
            excludes: Patterns::new(&options.index_exclude_patterns),
        };
        let shared = Arc::new(RwLock::new(Index::new()));
        let status = Arc::new(Mutex::new(IndexStatus {
            active: true,
            ..IndexStatus::default()
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let mut hash = blake3::Hasher::new();
        hash.update(root.to_string_lossy().as_bytes());
        hash.update(format!("{:?}", filter.folder).as_bytes());
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
            filter,
            index: shared.clone(),
            status: status.clone(),
            cancelled: cancelled.clone(),
            commands: rx,
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
#[derive(Clone, Debug)]
struct Filter {
    folder: Folder,
    includes: Patterns,
    excludes: Patterns,
}
impl Filter {
    fn admits(&self, path: &Path, directory: bool) -> bool {
        let root = &self.folder.path;
        self.folder.rules.allows(root, path, directory, true)
            && (path == root || !self.excludes.excludes_index_path(root, path, directory))
            && (directory || self.includes.is_empty() || self.includes.matches(root, path, false))
    }

    fn admits_entry(&self, path: &Path, directory: bool) -> bool {
        let root = &self.folder.path;
        path == root
            || (self.folder.rules.allows_entry(root, path, directory, true)
            // Bare exclusions also prune directory names (the old default used
            // entries like "vendor" and "target"), not just file names.
            && !self.excludes.matches(root, path, directory)
            && (directory || self.includes.is_empty() || self.includes.matches(root, path, false)))
    }
}
fn read_small(path: &Path, limit: u64) -> Option<String> {
    use std::io::Read;
    let file = vel_fileio::regular::read(path).ok()?;
    if file.metadata().ok()?.len() > limit {
        return None;
    }
    let mut source = String::new();
    file.take(limit + 1).read_to_string(&mut source).ok()?;
    (source.len() as u64 <= limit).then_some(source)
}

fn local_ignore(root: &Path, path: &Path) -> Gitignore {
    let Some(source) = read_small(path, 4 * 1024 * 1024) else {
        return Gitignore::empty();
    };
    let mut builder = GitignoreBuilder::new(root);
    for line in source.trim_start_matches('\u{feff}').lines() {
        let _ = builder.add_line(None, line);
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

fn git_directory(root: &Path) -> Option<PathBuf> {
    let marker = root.join(".git");
    if marker.is_dir() {
        return Some(marker);
    }
    let source = read_small(&marker, 4096)?;
    let git = root.join(source.trim().strip_prefix("gitdir: ")?);
    if let Some(common) = read_small(&git.join("commondir"), 4096) {
        Some(git.join(common.trim()))
    } else {
        Some(git)
    }
}

struct IgnoreLayer {
    rules: [Gitignore; 2],
    parent: Option<Arc<Self>>,
}

struct IgnoreFiles {
    project: PathBuf,
    boundary: PathBuf,
    git: bool,
    dirs: LinkedHashMap<PathBuf, Option<Arc<IgnoreLayer>>>,
    global: Gitignore,
    repository: Gitignore,
    exclude_file: Option<PathBuf>,
}
impl IgnoreFiles {
    fn new(root: &Path, git: bool) -> Self {
        // Like parents(true), load rules between the repository and a project
        // opened on one of its subdirectories. Keep no-repository projects local.
        let repo = git
            .then(|| {
                root.ancestors()
                    .find_map(|p| git_directory(p).map(|dir| (p, dir)))
            })
            .flatten();
        let boundary = repo.as_ref().map_or(root, |(p, _)| *p);
        let exclude_file = repo.map(|(_, dir)| dir.join("info/exclude"));
        Self {
            project: root.into(),
            boundary: boundary.into(),
            git,
            dirs: LinkedHashMap::new(),
            global: if git {
                ignore::gitignore::gitconfig_excludes_path()
                    .map_or_else(Gitignore::empty, |path| local_ignore(boundary, &path))
            } else {
                Gitignore::empty()
            },
            repository: exclude_file
                .as_ref()
                .map_or_else(Gitignore::empty, |path| local_ignore(boundary, path)),
            exclude_file,
        }
    }
    fn refresh(&mut self, dir: &Path) {
        if dir == self.project {
            self.dirs.clear();
            if let Some(path) = &self.exclude_file {
                self.repository = local_ignore(&self.boundary, path);
            }
        } else {
            self.dirs.retain(|key, _| !key.starts_with(dir));
        }
    }
    fn layer(&mut self, dir: &Path) -> Option<Arc<IgnoreLayer>> {
        if let Some(layer) = self.dirs.to_back(dir) {
            return layer.clone();
        }
        let parent = if dir == self.boundary {
            None
        } else {
            dir.parent().and_then(|p| self.layer(p))
        };
        let rules = [
            local_ignore(dir, &dir.join(".ignore")),
            if self.git {
                local_ignore(dir, &dir.join(".gitignore"))
            } else {
                Gitignore::empty()
            },
        ];
        let layer = if rules.iter().all(Gitignore::is_empty) {
            parent
        } else {
            Some(Arc::new(IgnoreLayer { rules, parent }))
        };
        if self.dirs.len() == 4096 {
            self.dirs.pop_front();
        }
        self.dirs.insert(dir.to_owned(), layer.clone());
        layer
    }
    fn matches(&mut self, path: &Path, directory: bool) -> bool {
        let layers = path
            .parent()
            .filter(|p| p.starts_with(&self.boundary))
            .and_then(|p| self.layer(p));
        let mut next = layers.as_deref();
        while let Some(layer) = next {
            for rule in &layer.rules {
                let result = rule.matched(path, directory);
                if !result.is_none() {
                    return result.is_ignore();
                }
            }
            next = layer.parent.as_deref();
        }
        let result = self.repository.matched(path, directory);
        if result.is_none() {
            self.global.matched(path, directory).is_ignore()
        } else {
            result.is_ignore()
        }
    }
    fn pruned(&mut self, path: &Path, directory: bool) -> bool {
        let mut directory = directory;
        for p in path.ancestors() {
            if !p.starts_with(&self.project) || p == self.project {
                break;
            }
            if self.matches(p, directory) {
                return true;
            }
            directory = true;
        }
        false
    }

    fn inherited_stamps(&self) -> Vec<(PathBuf, Option<Stamp>)> {
        let mut paths = Vec::new();
        if self.git {
            for dir in self
                .project
                .ancestors()
                .skip(1)
                .take_while(|p| p.starts_with(&self.boundary))
            {
                paths.extend([dir.join(".ignore"), dir.join(".gitignore")]);
            }
            paths.extend(self.exclude_file.iter().cloned());
        }
        paths
            .into_iter()
            .map(|p| {
                let stamp = Stamp::read(&p).ok();
                (p, stamp)
            })
            .collect()
    }
}

/// Only paths reached through a link need a canonical lookup. Ordinary files
/// already have canonical names because the walk starts at the canonical root.
#[derive(Default)]
struct Aliases {
    paths: BTreeMap<PathBuf, PathBuf>,
    preferred: HashMap<PathBuf, PathBuf>,
}
impl Aliases {
    fn has_descendants(&self, path: &Path) -> bool {
        self.paths
            .range::<Path, _>((std::ops::Bound::Excluded(path), std::ops::Bound::Unbounded))
            .next()
            .is_some_and(|(alias, _)| alias.starts_with(path))
    }

    fn primary<'a>(&'a self, path: &'a Path) -> &'a Path {
        let target = self.paths.get(path).map_or(path, PathBuf::as_path);
        self.preferred.get(target).map_or(path, PathBuf::as_path)
    }

    fn reconcile(
        &mut self,
        seen: &mut HashSet<PathBuf>,
        mut usable: impl FnMut(&Path) -> bool,
    ) -> HashSet<PathBuf> {
        let mut choices = HashMap::<&Path, &Path>::new();
        for (path, target) in &self.paths {
            choices
                .entry(target)
                .and_modify(|choice| *choice = (*choice).min(path))
                .or_insert(path);
        }
        let preferred: HashMap<_, _> = choices
            .into_iter()
            .map(|(target, alias)| {
                (
                    target.to_owned(),
                    if seen.contains(target) || usable(target) {
                        target
                    } else {
                        alias
                    }
                    .to_owned(),
                )
            })
            .collect();
        let mut retired = HashSet::new();
        for (target, path) in &preferred {
            if let Some(old) = self.preferred.get(target).filter(|&old| old != path) {
                retired.insert(old.clone());
                seen.insert(path.clone());
            }
        }
        seen.retain(|path| {
            self.paths
                .get(path)
                .is_none_or(|target| preferred.get(target) == Some(path))
        });
        self.preferred = preferred;
        retired
    }
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
    filter: Filter,
    index: Arc<RwLock<Index>>,
    status: Arc<Mutex<IndexStatus>>,
    cancelled: Arc<AtomicBool>,
    commands: mpsc::Receiver<Command>,
    cache: PathBuf,
}
impl Coordinator {
    fn excluded(&self, path: &Path) -> bool {
        self.cache
            .parent()
            .is_some_and(|cache| path.starts_with(cache))
            || path.strip_prefix(&self.root).map_or(true, |relative| {
                relative
                    .components()
                    .any(|p| p == std::path::Component::ParentDir)
                    || if self.filter.folder.path == self.root {
                        !self.filter.admits(path, false)
                    } else {
                        !self
                            .filter
                            .admits(&self.filter.folder.path.join(relative), false)
                    }
            })
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
        let notices = Arc::new(Mutex::new((false, HashSet::<PathBuf>::new())));
        let mut watcher = if self.options.watch {
            let notices = notices.clone();
            match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let mut changes = notices.lock().unwrap();
                if changes.0 {
                    return;
                }
                if let Ok(event) = event
                    && !event.need_rescan()
                    && event.paths.len() <= 4096usize.saturating_sub(changes.1.len())
                {
                    if !event.kind.is_access() {
                        changes.1.extend(event.paths);
                    }
                } else {
                    changes.0 = true;
                    changes.1.clear();
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
        let mut subtrees = BTreeSet::new();
        let ignores = Arc::new(Mutex::new(IgnoreFiles::new(
            &self.root,
            self.options.index_exclude_gitignore,
        )));
        let mut inherited = ignores.lock().unwrap().inherited_stamps();
        let mut aliases = Aliases::default();
        let mut flushes = Vec::new();
        while !self.cancelled.load(Ordering::Acquire) {
            let (overflow, events) = std::mem::take(&mut *notices.lock().unwrap());
            scan |= overflow;
            let events = events
                .into_iter()
                .filter(|path| path.starts_with(&self.root))
                .map(Command::Disk);
            for command in self.commands.try_iter().take(8192).chain(events) {
                if self.cancelled.load(Ordering::Relaxed) {
                    break;
                }
                match command {
                    Command::Stop => {
                        self.cancelled.store(true, Ordering::Release);
                        break;
                    }
                    Command::Rescan => scan = true,
                    Command::Flush(reply) => flushes.push(reply),
                    Command::Analyzed(path, revision, text, analysis) => {
                        let path = aliases.primary(&path).to_owned();
                        if self.excluded(&path)
                            || ignores.lock().unwrap().pruned(&path, false)
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
                        let path = aliases.primary(&path).to_owned();
                        if self.excluded(&path)
                            || ignores.lock().unwrap().pruned(&path, false)
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
                        let path = aliases.primary(&path).to_owned();
                        overlays.remove(&path);
                        prepared.remove(&path);
                        generation += 1;
                        desired.insert(path.clone(), generation);
                        pending.insert(path, (Instant::now(), generation, None));
                    }
                    Command::Disk(path) => {
                        if inherited.iter().any(|(p, _)| p == &path) {
                            scan = true;
                            continue;
                        }
                        if !path.starts_with(&self.root) {
                            continue;
                        }
                        if self
                            .cache
                            .parent()
                            .is_some_and(|cache| path.starts_with(cache))
                        {
                            continue;
                        }
                        if path
                            .file_name()
                            .is_some_and(|name| name == ".gitignore" || name == ".ignore")
                            && let Some(parent) = path.parent()
                        {
                            subtrees.insert(parent.to_owned());
                            continue;
                        }
                        if path.is_dir()
                            || observed_directories.contains_key(&path)
                            || aliases.has_descendants(&path)
                        {
                            subtrees.insert(path);
                            continue;
                        }
                        let path = if let Some(primary) = aliases.paths.get(&path) {
                            aliases.primary(primary).to_owned()
                        } else if self.filter.folder.follow_links
                            && path.canonicalize().is_ok_and(|p| p != path)
                        {
                            if let Some(parent) = path.parent() {
                                subtrees.insert(parent.to_owned());
                            }
                            continue;
                        } else {
                            path
                        };
                        let path = aliases.primary(&path).to_owned();
                        if self.options.watch
                            && let Some(parent) = path.parent()
                            && let Some(stamp) = observed_directories.get_mut(parent)
                        {
                            *stamp = Stamp::read(parent).ok();
                        }
                        if self.excluded(&path) || ignores.lock().unwrap().pruned(&path, false) {
                            generation += 1;
                            desired.insert(path.clone(), generation);
                            pending.remove(&path);
                            prepared.remove(&path);
                            observed_files.remove(&path);
                            self.index.write().unwrap().remove_file(&path);
                            stamps.remove(&path);
                            if let Err(e) = journal.append(path, None, None) {
                                self.log(format!("persist: {e}"));
                            }
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
                scan |= inherited
                    .iter()
                    .any(|(path, stamp)| Stamp::read(path).ok() != *stamp);
                for (path, stamp) in &observed_directories {
                    if Stamp::read(path).ok() != *stamp {
                        let directory = if path
                            .file_name()
                            .is_some_and(|name| name == ".gitignore" || name == ".ignore")
                        {
                            path.parent().unwrap_or(path)
                        } else {
                            path.as_path()
                        };
                        subtrees.insert(directory.to_owned());
                    }
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
            if scan || !subtrees.is_empty() {
                self.status.lock().unwrap().active = true;
                let mut roots = Vec::<PathBuf>::new();
                if scan {
                    roots.push(self.root.clone());
                    subtrees.clear();
                    inherited = ignores.lock().unwrap().inherited_stamps();
                } else {
                    for path in std::mem::take(&mut subtrees) {
                        if roots.last().is_none_or(|root| !path.starts_with(root)) {
                            roots.push(path);
                        }
                    }
                }
                let under_scan = |path: &Path| {
                    roots
                        .binary_search_by(|root| root.as_path().cmp(path))
                        .map_or_else(|pos| pos > 0 && path.starts_with(&roots[pos - 1]), |_| true)
                };
                observed_files.retain(|p, _| !under_scan(p));
                observed_directories.retain(|p, _| !under_scan(p));
                aliases.paths.retain(|p, _| !under_scan(p));
                let mut seen = HashSet::new();
                for start in &roots {
                    if self.cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    ignores.lock().unwrap().refresh(start);
                    let filter_allows = if self.filter.folder.path == self.root {
                        self.filter.admits(start, true)
                    } else {
                        start.strip_prefix(&self.root).is_ok_and(|rel| {
                            self.filter.admits(&self.filter.folder.path.join(rel), true)
                        })
                    };
                    if !filter_allows
                        || !start.is_dir()
                        || ignores.lock().unwrap().pruned(start, true)
                    {
                        continue;
                    }
                    if self.options.watch {
                        observed_directories.insert(start.clone(), Stamp::read(start).ok());
                    }
                    let mut walker = ignore::WalkBuilder::new(start);
                    walker
                        .hidden(false)
                        .follow_links(self.filter.folder.follow_links)
                        .git_ignore(false)
                        .git_global(false)
                        .git_exclude(false)
                        .ignore(false)
                        .parents(false)
                        .require_git(false);
                    let filter_root = self.root.clone();
                    let filter = self.filter.clone();
                    let filter_cache = self.cache.parent().unwrap().to_path_buf();
                    let ignores = ignores.clone();
                    walker.filter_entry(move |entry| {
                        let path = entry.path();
                        let directory = entry.file_type().is_some_and(|kind| kind.is_dir());
                        !path.starts_with(&filter_cache)
                            && !ignores.lock().unwrap().matches(path, directory)
                            && if filter_root == filter.folder.path {
                                filter.admits_entry(path, directory)
                            } else {
                                path.strip_prefix(&filter_root).is_ok_and(|relative| {
                                    filter
                                        .admits_entry(&filter.folder.path.join(relative), directory)
                                })
                            }
                    });
                    let mut linked = (self.filter.folder.follow_links
                        && start.canonicalize().is_ok_and(|p| p != *start))
                    .then_some(0);
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
                        if linked.is_some_and(|depth| depth >= entry.depth()) && entry.depth() != 0
                        {
                            linked = None;
                        }
                        if entry.path_is_symlink() && linked.is_none() {
                            linked = Some(entry.depth());
                        }
                        if self.options.watch && entry.file_type().is_some_and(|ty| ty.is_dir()) {
                            observed_directories
                                .insert(entry.path().into(), Stamp::read(entry.path()).ok());
                            for name in [".gitignore", ".ignore"] {
                                let path = entry.path().join(name);
                                observed_directories.insert(path.clone(), Stamp::read(&path).ok());
                            }
                        }
                        if !entry.file_type().is_some_and(|ty| ty.is_file()) {
                            continue;
                        }
                        let path = entry.into_path();
                        if linked.is_some()
                            && let Ok(target) = path.canonicalize()
                            && target != path
                        {
                            aliases.paths.insert(path.clone(), target);
                        }
                        seen.insert(path);
                    }
                }
                let retired = aliases.reconcile(&mut seen, |target| {
                    !self.excluded(target)
                        && target.is_file()
                        && !ignores.lock().unwrap().pruned(target, false)
                });
                for path in &retired {
                    observed_files.remove(path);
                }
                for path in &seen {
                    let stamp = (self.options.watch || stamps.contains_key(path))
                        .then(|| Stamp::read(path).ok())
                        .flatten();
                    // Keep membership even without watching. A subtree scan only
                    // replaces the entries it visited, not the project's total.
                    observed_files.insert(path.clone(), stamp);
                    if overlays.contains_key(path) {
                        continue;
                    }
                    if stamp.is_some_and(|stamp| stamps.get(path) == Some(&stamp)) {
                        self.status.lock().unwrap().cached_files += 1;
                        continue;
                    }
                    generation += 1;
                    desired.insert(path.clone(), generation);
                    pending.insert(path.clone(), (Instant::now(), generation, None));
                }
                for path in stamps
                    .keys()
                    .chain(desired.keys())
                    .filter(|path| {
                        (under_scan(path) || retired.contains(*path))
                            && !seen.contains(*path)
                            && !overlays.contains_key(*path)
                    })
                    .cloned()
                    .collect::<HashSet<_>>()
                {
                    desired.remove(&path);
                    pending.remove(&path);
                    observed_files.remove(&path);
                    self.index.write().unwrap().remove_file(&path);
                    stamps.remove(&path);
                    let _ = journal.append(path, None, None);
                }
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
                if result.disk && result.analysis.is_ok() {
                    if result.stamp.is_some() {
                        if let Some(stamp) = observed_files.get_mut(&result.path) {
                            *stamp = result.stamp;
                        } else {
                            observed_files.insert(result.path.clone(), result.stamp);
                        }
                    } else {
                        observed_files.remove(&result.path);
                    }
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
                status.total_files = observed_files.len();
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
        use std::io::Read;
        let mut bytes = Vec::new();
        vel_fileio::regular::read(&task.path)
            .map_err(Error::Analysis)?
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
