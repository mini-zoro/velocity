use crate::{
    Error, FileAnalysis, Index, SymbolLocation, SymbolMatch, analyze,
    store::{Journal, Stamp},
    temporary::TemporaryIndex,
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
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use vel_search::project::{Folder, Patterns};
use vel_syntax::{MetadataSet, SyntaxSet};
use vel_text::Text;
mod polling;
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
    #[cfg(test)]
    idle_counters: Arc<IdleCounters>,
    root: PathBuf,
    source_root: PathBuf,
    shared: Arc<RwLock<Index>>,
    commands: mpsc::Sender<Command>,
    wake: mpsc::SyncSender<()>,
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
        folder: Folder,
        syntaxes: Arc<SyntaxSet>,
        metadata: Arc<MetadataSet>,
        options: IndexOptions,
    ) -> Result<Self, Error> {
        Self::open_folder_with_volume(
            folder,
            syntaxes,
            metadata,
            options,
            polling::local_volume,
            None,
        )
    }

    /// Opens an index with a private, disposable cache, even if a persistent cache is configured.
    pub fn open_folder_temporary(
        folder: Folder,
        syntaxes: Arc<SyntaxSet>,
        metadata: Arc<MetadataSet>,
        options: IndexOptions,
    ) -> Result<Self, Error> {
        let temp = TemporaryIndex::new(&std::env::temp_dir())?;
        Self::open_folder_with_volume(
            folder,
            syntaxes,
            metadata,
            options,
            polling::local_volume,
            Some(temp),
        )
    }

    fn open_folder_with_volume(
        mut folder: Folder,
        syntaxes: Arc<SyntaxSet>,
        metadata: Arc<MetadataSet>,
        options: IndexOptions,
        local_volume: fn(&Path) -> std::io::Result<bool>,
        temp: Option<TemporaryIndex>,
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
        let (wake, signals) = mpsc::sync_channel(1);
        let mut hash = blake3::Hasher::new();
        hash.update(root.to_string_lossy().as_bytes());
        hash.update(
            format!(
                "{:?}:{}:{:?}",
                filter.folder.rules, filter.folder.follow_links, filter.folder.path
            )
            .as_bytes(),
        );
        hash.update(&metadata.fingerprint());
        hash.update(&bincode::serialize(&*syntaxes).map_err(|e| Error::Analysis(e.to_string()))?);
        hash.update(
            &bincode::serialize(&(
                &options.index_exclude_patterns,
                &options.index_include_patterns,
                options.index_exclude_gitignore,
                options.index_skip_unknown_extensions,
                options.max_file_size,
            ))
            .map_err(|e| Error::Analysis(e.to_string()))?,
        );
        // ST4 cache relocation: https://www.sublimetext.com/download
        // Rebuild here rather than replaying an old Application Support cache.
        let directory = if let Some(temp) = &temp {
            temp.path().to_owned()
        } else {
            let dir = options.cache_dir.as_ref().map_or_else(
                || cache_dir().join("Velocity/index"),
                |base| base.join("velocity/index"),
            );
            fs::create_dir_all(&dir)?;
            dir.canonicalize()?
        };
        let cache = directory.join(format!("{}.v6", hash.finalize().to_hex()));
        let worker = Coordinator {
            #[cfg(test)]
            idle_counters: Arc::new(IdleCounters::default()),
            root: root.clone(),
            syntaxes,
            metadata,
            options,
            filter,
            index: shared.clone(),
            status: status.clone(),
            cancelled: cancelled.clone(),
            commands: rx,
            wake: wake.clone(),
            signals,
            cache,
            local_volume,
        };
        #[cfg(test)]
        let idle_counters = worker.idle_counters.clone();
        thread::Builder::new()
            .name("velocity-index".into())
            .spawn(move || {
                let _temp = temp;
                worker.run();
            })?;
        Ok(Self {
            #[cfg(test)]
            idle_counters,
            root,
            source_root,
            shared,
            commands: tx,
            wake,
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
    fn send(&self, command: Command) -> Result<(), ()> {
        self.commands.send(command).map_err(|_| ())?;
        let _ = self.wake.try_send(());
        Ok(())
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
        let _ = self.send(Command::Disk(self.path(path)));
    }
    /// Coalesces unsaved versions; older/equal revisions never overwrite a newer overlay.
    /// Overlays are never persisted and are independently syntax/size/binary filtered.
    pub fn update_buffer(&self, path: impl AsRef<Path>, revision: u64, text: Text) {
        let _ = self.send(Command::Buffer(self.path(path), revision, text));
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
        let _ = self.send(Command::Analyzed(self.path(path), revision, text, analysis));
    }
    /// Drops an unsaved overlay and restores the file's current disk contents.
    pub fn close_buffer(&self, path: impl AsRef<Path>) {
        let _ = self.send(Command::Close(self.path(path)));
    }
    /// Rescans after project/ignore file or directory changes.
    pub fn rescan(&self) {
        let _ = self.send(Command::Rescan);
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
        self.send(Command::Flush(tx))
            .map_err(|_| std::io::Error::other("index stopped"))?;
        rx.recv()
            .map_err(|_| std::io::Error::other("index stopped"))?
    }
    /// Permanently cancels indexing without blocking on a regex. Already indexed queries remain.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.send(Command::Stop);
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
    if !cfg!(target_os = "macos")
        && let Some(path) = std::env::var_os("XDG_CACHE_HOME")
    {
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

// notify can register a watch without delivering events (e.g. in a sandbox).
// A cache fixture is evidence only when it is on the project's own device.
#[cfg(target_os = "macos")]
fn check_event_delivery(root: &Path, cache: &Path, cancelled: &AtomicBool) -> std::io::Result<()> {
    use std::io;
    use std::os::unix::fs::MetadataExt;
    let cache_directory = cache.parent().expect("cache directory");
    if root.metadata()?.dev() != cache_directory.metadata()?.dev() {
        return Err(io::Error::other(
            "waiting for native events from the project volume",
        ));
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = cache_directory.join(format!(
        ".watch-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir)?;
    let result = (|| {
        let (tx, rx) = mpsc::sync_channel(1);
        let probe = dir.join("probe");
        let expected = probe.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok_and(|event| event.paths.contains(&expected)) {
                    let _ = tx.try_send(());
                }
            })
            .map_err(io::Error::other)?;
        watcher
            .watch(&dir, notify::RecursiveMode::NonRecursive)
            .map_err(io::Error::other)?;
        fs::write(probe, b"watch health")?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !cancelled.load(Ordering::Acquire) && Instant::now() < deadline {
            if rx.recv_timeout(Duration::from_millis(50)).is_ok() {
                return Ok(());
            }
        }
        Err(io::Error::other(
            "native watch registered but delivered no events",
        ))
    })();
    let _ = fs::remove_dir_all(dir);
    result
}

type Notices = Arc<Mutex<(bool, HashSet<PathBuf>)>>;

fn is_ignore_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == ".gitignore" || name == ".ignore")
}

fn watch_project(
    root: &Path,
    notices: Notices,
    wake: mpsc::SyncSender<()>,
    failed: Arc<AtomicBool>,
) -> notify::Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_err() {
            failed.store(true, Ordering::Release);
        }
        let mut changes = notices.lock().unwrap();
        if !changes.0 {
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
        }
        let _ = wake.try_send(());
    })?;
    watcher.watch(root, notify::RecursiveMode::Recursive)?;
    Ok(watcher)
}
struct ParseResources {
    syntaxes: Arc<SyntaxSet>,
    metadata: Arc<MetadataSet>,
    options: IndexOptions,
    cancelled: Arc<AtomicBool>,
    completed: mpsc::Sender<Completed>,
    wake: mpsc::SyncSender<()>,
}
struct SharedPool {
    workers: usize,
    tasks: mpsc::SyncSender<(Task, Arc<ParseResources>)>,
}
impl SharedPool {
    fn get() -> &'static Self {
        static POOL: OnceLock<SharedPool> = OnceLock::new();
        POOL.get_or_init(|| {
            let count = thread::available_parallelism()
                .map_or(2, usize::from)
                .clamp(1, 8);
            let (tasks, receiver) = mpsc::sync_channel::<(Task, Arc<ParseResources>)>(count * 4);
            let receiver = Arc::new(Mutex::new(receiver));
            let mut workers = 0;
            for id in 0..count {
                let receiver = receiver.clone();
                if thread::Builder::new()
                    .name(format!("velocity-index-parse-{id}"))
                    .spawn(move || {
                        priority();
                        loop {
                            let entry = receiver.lock().unwrap().recv();
                            let Ok((mut task, resources)) = entry else {
                                break;
                            };
                            if resources.cancelled.load(Ordering::Acquire) {
                                continue;
                            }
                            let disk = task.overlay.is_none();
                            let result = analyze_task(
                                &mut task,
                                &resources.syntaxes,
                                &resources.metadata,
                                &resources.options,
                            );
                            let (stamp, analysis) = result.map_or_else(
                                |error| (None, Err(error)),
                                |(stamp, analysis)| (stamp, Ok(analysis)),
                            );
                            let _ = resources.completed.send(Completed {
                                path: task.path,
                                generation: task.generation,
                                disk,
                                stamp,
                                analysis,
                            });
                            let _ = resources.wake.try_send(());
                        }
                    })
                    .is_ok()
                {
                    workers += 1;
                }
            }
            Self {
                workers: workers.max(1),
                tasks,
            }
        })
    }
}
struct Coordinator {
    #[cfg(test)]
    idle_counters: Arc<IdleCounters>,
    root: PathBuf,
    syntaxes: Arc<SyntaxSet>,
    metadata: Arc<MetadataSet>,
    options: IndexOptions,
    filter: Filter,
    index: Arc<RwLock<Index>>,
    status: Arc<Mutex<IndexStatus>>,
    cancelled: Arc<AtomicBool>,
    commands: mpsc::Receiver<Command>,
    wake: mpsc::SyncSender<()>,
    signals: mpsc::Receiver<()>,
    cache: PathBuf,
    local_volume: fn(&Path) -> std::io::Result<bool>,
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
        let mut local = Index::new();
        let loaded = Journal::open(&self.cache, &mut local, &self.cancelled);
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
        *self.index.write().unwrap() = local;
        let pool = SharedPool::get();
        let workers = if self.options.index_workers == 0 {
            pool.workers
        } else {
            self.options.index_workers.clamp(1, pool.workers)
        };
        self.log(format!(
            "{workers} shared indexing workers; {} cached files",
            stamps.len()
        ));
        let (completed, result_rx) = mpsc::channel();
        let resources = Arc::new(ParseResources {
            syntaxes: self.syntaxes.clone(),
            metadata: self.metadata.clone(),
            options: self.options.clone(),
            cancelled: self.cancelled.clone(),
            completed,
            wake: self.wake.clone(),
        });
        let notices = Arc::new(Mutex::new((false, HashSet::<PathBuf>::new())));
        let watcher_failed = Arc::new(AtomicBool::new(false));
        let mut watcher = None;
        let mut probing = None::<mpsc::Receiver<std::io::Result<()>>>;
        let mut fallback = self.options.watch;
        let mut next_probe = self.options.watch.then(Instant::now);
        let mut pending: HashMap<PathBuf, (Instant, u64, Option<Text>)> = HashMap::new();
        let mut desired = HashMap::new();
        let mut overlays: HashMap<PathBuf, (u64, Text)> = HashMap::new();
        let mut prepared: HashMap<PathBuf, FileAnalysis> = HashMap::new();
        let mut generation = 0u64;
        let mut inflight = 0usize;
        let mut running: HashMap<PathBuf, u64> = HashMap::new();
        let mut observed_files = polling::Stamps::new();
        let mut observed_directories = polling::Stamps::new();
        let mut polling = polling::Polling::new();
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
            #[cfg(test)]
            self.idle_counters.turns.fetch_add(1, Ordering::Relaxed);
            let (overflow, events) = std::mem::take(&mut *notices.lock().unwrap());
            scan |= overflow;
            let now = Instant::now();
            let mut native_confirmed = events.iter().any(|path| {
                path.starts_with(&self.root)
                    && !path.starts_with(self.cache.parent().expect("cache directory"))
            });
            if watcher_failed.swap(false, Ordering::AcqRel) {
                watcher.take();
                probing = None;
                next_probe = Some(now + polling::REPROBE);
                fallback = true;
                self.log("watcher failed; polling filesystem stamps");
            }
            if let Some(probe) = &probing {
                match probe.try_recv() {
                    Ok(result) => {
                        native_confirmed |= result.is_ok();
                        if let Err(error) = result
                            && fallback
                            && !native_confirmed
                        {
                            self.log(format!(
                                "watcher unavailable: {error}; polling filesystem stamps"
                            ));
                        }
                        probing = None;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => probing = None,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            if fallback && watcher.is_some() && native_confirmed {
                // A slow probe can time out while FSEvents is catching up. A
                // real event on this root is also proof, followed by one catch-up scan.
                fallback = false;
                next_probe = None;
                scan = true;
                self.log("native watch confirmed");
            }
            if next_probe.is_some_and(|at| at <= now) {
                next_probe = Some(now + polling::REPROBE);
                match (self.local_volume)(&self.root) {
                    Ok(true) => {
                        watcher = match watch_project(
                            &self.root,
                            notices.clone(),
                            self.wake.clone(),
                            watcher_failed.clone(),
                        ) {
                            Ok(watch) => Some(watch),
                            Err(error) => {
                                self.log(format!(
                                    "watcher unavailable: {error}; polling filesystem stamps"
                                ));
                                None
                            }
                        };
                        if watcher.is_some() && probing.is_none() {
                            #[cfg(target_os = "macos")]
                            {
                                let (answer, result) = mpsc::sync_channel(1);
                                let (root, cache) = (self.root.clone(), self.cache.clone());
                                let (cancel, wake) = (self.cancelled.clone(), self.wake.clone());
                                if thread::Builder::new()
                                    .name("velocity-watch-probe".into())
                                    .spawn(move || {
                                        priority();
                                        let _ = answer
                                            .send(check_event_delivery(&root, &cache, &cancel));
                                        let _ = wake.try_send(());
                                    })
                                    .is_ok()
                                {
                                    probing = Some(result);
                                }
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                fallback = false;
                                next_probe = None;
                                scan = true;
                            }
                        }
                    }
                    Ok(false) => {
                        watcher.take();
                        self.log("project is on a network volume; polling filesystem stamps")
                    }
                    Err(error) => self.log(format!(
                        "cannot identify project volume: {error}; polling filesystem stamps"
                    )),
                }
            }
            let events = events
                .into_iter()
                .filter(|path| path.starts_with(&self.root))
                .map(Command::Disk);
            let mut command_count = 0;
            for command in self
                .commands
                .try_iter()
                .take(8192)
                .inspect(|_| command_count += 1)
                .chain(events)
            {
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
                            && !fallback
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
            let mut shallow = BTreeSet::<PathBuf>::new();
            if fallback {
                scan |= polling.tick(
                    Instant::now(),
                    &inherited,
                    &observed_directories,
                    &observed_files,
                    |path, current| {
                        if is_ignore_file(path) {
                            subtrees.insert(path.parent().unwrap_or(path).to_owned());
                        } else if current.is_none()
                            || self.filter.folder.follow_links
                            || self.options.index_exclude_gitignore
                                && [".gitignore", ".ignore"].into_iter().any(|name| {
                                    let rule = path.join(name);
                                    Stamp::read(&rule).ok()
                                        != observed_directories.get(&rule).copied().flatten()
                                })
                        {
                            subtrees.insert(path.to_owned());
                        } else {
                            shallow.insert(path.to_owned());
                        }
                    },
                    |path, stamp| {
                        if overlays.contains_key(path)
                            || pending.contains_key(path)
                            || running.contains_key(path)
                        {
                            return;
                        }
                        if Stamp::read(path).ok() != stamp {
                            generation += 1;
                            desired.insert(path.to_owned(), generation);
                            pending.insert(path.to_owned(), (Instant::now(), generation, None));
                        }
                    },
                );
            }
            if scan || !subtrees.is_empty() || !shallow.is_empty() {
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
                let under_full_scan = |path: &Path| {
                    roots
                        .binary_search_by(|root| root.as_path().cmp(path))
                        .map_or_else(|pos| pos > 0 && path.starts_with(&roots[pos - 1]), |_| true)
                };
                shallow.retain(|p| !under_full_scan(p));
                let under_scan = |path: &Path| {
                    under_full_scan(path)
                        || path.parent().is_some_and(|parent| shallow.contains(parent))
                };
                observed_files.retain(|p, _| !under_scan(p));
                observed_directories.retain(|p, _| !under_full_scan(p));
                aliases.paths.retain(|p, _| !under_scan(p));
                let mut seen = HashSet::new();
                for (start, shallow_walk) in roots
                    .iter()
                    .map(|root| (root, false))
                    .chain(shallow.iter().map(|root| (root, true)))
                {
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
                        .max_depth(shallow_walk.then_some(1))
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
                            if shallow_walk && entry.depth() == 1 {
                                if !observed_directories.contains_key(entry.path()) {
                                    subtrees.insert(entry.path().to_owned());
                                }
                                continue;
                            }
                            observed_directories
                                .insert(entry.path().into(), Stamp::read(entry.path()).ok());
                            for name in [".gitignore", ".ignore"] {
                                let path = entry.path().join(name);
                                if let Ok(stamp) = Stamp::read(&path) {
                                    observed_directories.insert(path, Some(stamp));
                                }
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
            let mut pool_full = false;
            let due: Vec<_> = pending
                .iter()
                .filter(|(_, (at, _, _))| *at <= Instant::now() || !flushes.is_empty())
                .take(workers.saturating_sub(inflight))
                .map(|(path, _)| path.clone())
                .collect();
            for path in due {
                let (_, generation, overlay) = pending.remove(&path).unwrap();
                let task_path = path.clone();
                let analysis = prepared.remove(&path);
                match pool.tasks.try_send((
                    Task {
                        path,
                        generation,
                        overlay,
                        prepared: analysis,
                    },
                    resources.clone(),
                )) {
                    Ok(()) => {
                        inflight += 1;
                        running.insert(task_path, generation);
                    }
                    Err(mpsc::TrySendError::Full((task, _))) => {
                        if let Some(analysis) = task.prepared {
                            prepared.insert(task.path.clone(), analysis);
                        }
                        pending.insert(task.path, (Instant::now(), task.generation, task.overlay));
                        pool_full = true;
                        break;
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        self.log("all index workers stopped");
                        self.cancelled.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            let active = !pending.is_empty() || inflight > 0 || !subtrees.is_empty();
            {
                let mut status = self.status.lock().unwrap();
                status.total_files = observed_files.len();
                status.pending_files = pending.len() + inflight;
                status.active = active;
            }
            if !active {
                if let Err(e) = journal.compact(&self.cancelled) {
                    self.log(format!("compact: {e}"));
                }
                #[cfg(test)]
                self.idle_counters.stats.store(
                    crate::store::STAT_READS.with(std::cell::Cell::get),
                    Ordering::Relaxed,
                );
                for reply in flushes.drain(..) {
                    let _ = reply.send(journal.flush());
                }
            }
            if command_count == 8192 || !subtrees.is_empty() {
                continue;
            }
            let deadline = if pool_full {
                Some(Instant::now() + Duration::from_millis(5))
            } else if inflight < workers {
                pending.values().map(|(at, _, _)| *at).min()
            } else {
                None
            };
            let deadline = deadline
                .into_iter()
                .chain(fallback.then_some(polling.next))
                .chain(next_probe)
                .min();
            match deadline {
                Some(at) => {
                    let _ = self
                        .signals
                        .recv_timeout(at.saturating_duration_since(Instant::now()));
                }
                None => {
                    let _ = self.signals.recv();
                }
            }
        }
        watcher.take();
        if let Err(e) = journal.flush() {
            self.log(format!("persist: {e}"));
        }
        let mut status = self.status.lock().unwrap();
        status.active = false;
        status.cancelled = true;
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
struct IdleCounters {
    turns: std::sync::atomic::AtomicU64,
    stats: std::sync::atomic::AtomicU64,
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

#[cfg(test)]
mod idle_tests {
    use super::*;

    fn cpu_seconds() -> f64 {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage fills the process usage structure on success.
        let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        assert_eq!(result, 0);
        // SAFETY: getrusage returned success.
        let usage = unsafe { usage.assume_init() };
        let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 * 1e-6;
        seconds(usage.ru_utime) + seconds(usage.ru_stime)
    }

    fn sample(count: usize, interval: Duration, watch: bool) -> (f64, f64, f64) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir(&root).unwrap();
        for folder in 0..count.div_ceil(1000) {
            let dir = root.join(folder.to_string());
            fs::create_dir(&dir).unwrap();
            for file in folder * 1000..count.min((folder + 1) * 1000) {
                fs::write(dir.join(format!("{file}.txt")), "fixture\n").unwrap();
            }
        }
        let mut builder = vel_syntax::SyntaxSetBuilder::new();
        builder.add_plain_text_syntax();
        let options = IndexOptions {
            cache_dir: Some(tmp.path().join("cache")),
            index_skip_unknown_extensions: false,
            watch,
            ..Default::default()
        };
        let index = ProjectIndex::open(
            &root,
            Arc::new(builder.build()),
            Arc::new(MetadataSet::default()),
            options,
        )
        .unwrap();
        index.flush().unwrap();
        assert_eq!(index.status().total_files, count);
        // Settle any coalesced events from watcher setup before the idle sample.
        thread::sleep(Duration::from_millis(2300));
        let snapshot = || {
            (
                index.idle_counters.turns.load(Ordering::Relaxed),
                index.idle_counters.stats.load(Ordering::Relaxed),
            )
        };
        let before = snapshot();
        let cpu_before = cpu_seconds();
        let at = Instant::now();
        thread::sleep(interval);
        let cpu = cpu_seconds() - cpu_before;
        let after = snapshot();
        index.cancel();
        let seconds = at.elapsed().as_secs_f64();
        (
            (after.0 - before.0) as f64 / seconds,
            (after.1 - before.1) as f64 / seconds,
            cpu * 100. / seconds,
        )
    }
    #[test]
    #[ignore = "100k-file idle measurement"]
    fn measure_idle_index_100k() {
        let watch = std::env::var_os("VELOCITY_MEASURE_NATIVE_WATCH").is_some();
        let (turns, stats, cpu) = sample(100_000, Duration::from_secs(10), watch);
        println!("idle {turns:.2} turns/s, {stats:.2} Stamp::read/s, {cpu:.2}% of one core");
        if std::env::var_os("VELOCITY_EXPECT_POLLING").is_some() {
            assert!(stats > 0. && stats <= 5000., "{stats} stats/s");
        }
    }
    #[test]
    fn event_driven_index_idles_without_stat_polling() {
        // No native events in this fixture. The same channel is used for
        // notifications, explicit reindex requests and worker results.
        let (turns, stats, _) = sample(2_000, Duration::from_secs(1), false);
        assert!(turns < 20., "{turns} turns per second");
        assert_eq!(stats, 0.);
    }

    #[test]
    fn network_volume_index_detects_new_files_and_edits_without_parent_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(root.join("nested")).unwrap();
        let edited = root.join("nested/one.txt");
        fs::write(&edited, "originalword\n").unwrap();
        let mut builder = vel_syntax::SyntaxSetBuilder::new();
        builder.add_plain_text_syntax();
        let index = ProjectIndex::open_folder_with_volume(
            Folder {
                path: root.clone(),
                name: String::new(),
                rules: Default::default(),
                follow_links: false,
            },
            Arc::new(builder.build()),
            Arc::new(MetadataSet::default()),
            IndexOptions {
                cache_dir: Some(tmp.path().join("cache")),
                index_skip_unknown_extensions: false,
                ..Default::default()
            },
            |_| Ok(false),
            None,
        )
        .unwrap();
        index.flush().unwrap();
        assert!(
            index
                .status()
                .logs
                .iter()
                .any(|line| line.contains("network volume"))
        );
        let parent = Stamp::read(edited.parent().unwrap()).unwrap();
        fs::write(&edited, "replacementword\n").unwrap();
        assert_eq!(Stamp::read(edited.parent().unwrap()).unwrap(), parent);

        let wait_for = |word: &str| {
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                let shared = index.shared_index();
                if shared.read().unwrap().word_count(word) > 0 {
                    break;
                }
                assert!(Instant::now() < until, "index did not find {word}");
                thread::sleep(Duration::from_millis(20));
            }
        };
        wait_for("replacementword");
        fs::write(root.join("nested/two.txt"), "newword\n").unwrap();
        wait_for("newword");
        // A new subdirectory needs a walk even though known siblings don't.
        fs::create_dir(root.join("added")).unwrap();
        fs::write(root.join("added/three.txt"), "subtreeword\n").unwrap();
        wait_for("subtreeword");
        index.flush().unwrap();
        assert_eq!(index.status().total_files, 3);
    }
    #[test]
    fn worker_count_and_debounce_keep_cache_hits() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("one.txt"), "word").unwrap();
        let mut builder = vel_syntax::SyntaxSetBuilder::new();
        builder.add_plain_text_syntax();
        let syntaxes = Arc::new(builder.build());
        let metadata = Arc::new(MetadataSet::default());
        for (workers, cached) in [(1, 0), (3, 1)] {
            let options = IndexOptions {
                index_workers: workers,
                debounce: Duration::from_millis(workers as u64),
                watch: false,
                cache_dir: Some(tmp.path().join("cache")),
                index_skip_unknown_extensions: false,
                ..Default::default()
            };
            let index =
                ProjectIndex::open(&root, syntaxes.clone(), metadata.clone(), options).unwrap();
            index.flush().unwrap();
            assert_eq!(index.status().cached_files, cached);
        }
    }
}
