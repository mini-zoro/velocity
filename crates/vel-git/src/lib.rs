#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
mod reftable;
use gix::{
    bstr::{BString, ByteSlice},
    status::{Item, index_worktree},
};
use notify::{RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// Side bar badge precedence, ascending exactly as in the public Git integration docs.
/// Ignored is de-emphasis and ranks below every actual change badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Badge {
    /// Ignored path (usually a collapsed directory).
    Ignored,
    /// Untracked working file.
    Untracked,
    /// Modified contents/type/executable bit.
    Modified,
    /// Tracked file missing on disk.
    Missing,
    /// Index addition relative to HEAD.
    StagedAddition,
    /// Index modification relative to HEAD.
    StagedModification,
    /// Index deletion relative to HEAD.
    StagedDeletion,
    /// Unresolved index stages.
    Unmerged,
}
/// A file may have simultaneous staged and working changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileStatus {
    /// All observed change classifications; no information lost to precedence.
    pub statuses: BTreeSet<Badge>,
}
impl FileStatus {
    /// Most significant badge for display.
    pub fn badge(&self) -> Option<Badge> {
        self.statuses.last().copied()
    }
}
/// Branch identity, including unborn branches and detached HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// Short branch name, and whether it has no commits yet.
    Branch { name: String, unborn: bool },
    /// Full detached object ID (caller may abbreviate for display).
    Detached(String),
}
/// Cheap local graph comparison; no fetch or network operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    /// Commits reachable only from HEAD.
    pub ahead: usize,
    /// Commits reachable only from the locally cached upstream ref.
    pub behind: usize,
}
/// Immutable status publication. Paths are repository-relative, with native byte preservation.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    /// Repository working directory.
    pub root: PathBuf,
    /// Publication generation (advances on successful refreshes).
    pub generation: u64,
    /// Current branch or detached identity.
    pub head: Head,
    /// None when no upstream exists or a bounded local walk would be expensive.
    pub ahead_behind: Option<AheadBehind>,
    /// Only changed/ignored paths; missing entries are clean.
    pub files: BTreeMap<PathBuf, FileStatus>,
    /// Aggregated folder badges, including the empty root path.
    pub folders: BTreeMap<PathBuf, Badge>,
}
impl StatusSnapshot {
    /// Exact file badge, or ignored status inherited from a collapsed ignored directory.
    pub fn badge(&self, path: &Path) -> Option<Badge> {
        let path = if path.is_absolute() {
            path.strip_prefix(&self.root).ok()?
        } else {
            path
        };
        self.files
            .get(path)
            .and_then(FileStatus::badge)
            .or_else(|| {
                path.ancestors().skip(1).find_map(|p| {
                    self.files
                        .get(p)
                        .and_then(FileStatus::badge)
                        .filter(|b| *b == Badge::Ignored)
                })
            })
    }
}
/// Source for an incremental diff reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTarget {
    /// The stage-zero index blob.
    Index,
    /// The blob reachable from HEAD's tree.
    Head,
}
/// Repository, watcher, worker or path failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// gix discovery/status/object error.
    #[error("git: {0}")]
    Git(String),
    /// The repository has no working tree.
    #[error("bare repository has no working tree")]
    Bare,
    /// Paths must stay within the repository worktree.
    #[error("invalid repository-relative path")]
    Path,
    /// The index has conflict stages, not a usable stage-zero diff reference.
    #[error("unmerged index entry")]
    Unmerged,
    /// A requested object is not a blob.
    #[error("reference entry is not a blob")]
    NotBlob,
    /// Watch installation failure.
    #[error(transparent)]
    Watch(#[from] notify::Error),
    /// Worker could not be started.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The background service has stopped.
    #[error("git worker stopped")]
    Stopped,
}
fn git(e: impl ToString) -> Error {
    Error::Git(e.to_string())
}

/// Synchronous read-only repository engine. Keep this on an analysis worker, or use GitService.
#[derive(Debug)]
pub struct Repository {
    repo: gix::Repository,
    reftable: Option<reftable::Store>,
    root: PathBuf,
    snapshot: Option<StatusSnapshot>,
}
impl Repository {
    /// Discovers a containing repository, including linked worktrees and `.git` indirection.
    pub fn discover(folder: impl AsRef<Path>) -> Result<Self, Error> {
        let repo = gix::discover(folder.as_ref()).map_err(git)?;
        let root = repo.workdir().ok_or(Error::Bare)?.canonicalize()?;
        let reftable = reftable::Store::detect(&repo);
        Ok(Self {
            repo,
            reftable,
            root,
            snapshot: None,
        })
    }
    /// Canonical worktree root.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Worktree-specific metadata path (not necessarily `root/.git`).
    pub fn git_dir(&self) -> &Path {
        self.repo.git_dir()
    }
    /// Shared metadata path for linked worktrees.
    pub fn common_dir(&self) -> &Path {
        self.repo.common_dir()
    }
    /// Latest successful snapshot, if any.
    pub fn snapshot(&self) -> Option<&StatusSnapshot> {
        self.snapshot.as_ref()
    }
    /// Performs a complete cold/metadata refresh. Never writes an index or repository file.
    pub fn refresh(&mut self) -> Result<&StatusSnapshot, Error> {
        self.repo.reload().map_err(git)?;
        self.reftable = reftable::Store::detect(&self.repo);
        self.refresh_inner(None)
    }
    /// Refreshes literal changed paths/subtrees and retains all other status entries.
    /// Call full refresh for index/refs/ignore/config changes or lost watcher events.
    pub fn refresh_paths(&mut self, paths: &[PathBuf]) -> Result<&StatusSnapshot, Error> {
        if self.snapshot.is_none() {
            return self.refresh();
        }
        let paths: Result<Vec<_>, _> = paths.iter().map(|p| self.relative(p)).collect();
        let paths = paths?;
        if paths.is_empty() {
            return Ok(self.snapshot.as_ref().expect("snapshot"));
        }
        if paths.iter().any(|p| p.as_os_str().is_empty()) {
            return self.refresh();
        }
        self.refresh_inner(Some(paths))
    }
    fn relative(&self, path: &Path) -> Result<PathBuf, Error> {
        let p = if path.is_absolute() {
            path.strip_prefix(&self.root).map_err(|_| Error::Path)?
        } else {
            path
        };
        if p.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(Error::Path);
        }
        Ok(p.to_path_buf())
    }
    fn refresh_inner(&mut self, paths: Option<Vec<PathBuf>>) -> Result<&StatusSnapshot, Error> {
        let mut files = paths.as_ref().map_or_else(BTreeMap::new, |paths| {
            let mut old = self.snapshot.as_ref().expect("snapshot").files.clone();
            old.retain(|p, _| !paths.iter().any(|path| p.starts_with(path)));
            old
        });
        let mut patterns = Vec::<BString>::new();
        if let Some(paths) = &paths {
            for p in paths {
                let mut pattern = BString::from(":(top,literal)");
                pattern.extend_from_slice(gix::path::into_bstr(p).map_err(git)?.as_ref());
                patterns.push(pattern);
            }
        }
        let table_head = self
            .reftable
            .as_ref()
            .map(reftable::Store::head)
            .transpose()?;
        let head_tree = if let Some((_, id)) = table_head.as_ref() {
            self.tree_id(*id)?
        } else {
            self.repo.head_tree_id_or_empty().map_err(git)?.detach()
        };
        if table_head.is_some() {
            self.reftable_status(head_tree, &patterns, &mut files)?;
        } else {
            let platform = self
                .repo
                .status(gix::progress::Discard)
                .map_err(git)?
                .head_tree(head_tree)
                .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
                .index_worktree_rewrites(None)
                .untracked_files(gix::status::UntrackedFiles::Files)
                .dirwalk_options(|o| {
                    o.emit_ignored(Some(gix::dir::walk::EmissionMode::CollapseDirectory))
                });
            for item in platform.into_iter(patterns).map_err(git)? {
                record_item(&mut files, &item.map_err(git)?)?;
            }
        }
        let mut folders: BTreeMap<PathBuf, Badge> = BTreeMap::new();
        for (path, status) in &files {
            if let Some(badge) = status.badge() {
                for folder in path.ancestors().skip(1) {
                    folders
                        .entry(folder.into())
                        .and_modify(|b| *b = (*b).max(badge))
                        .or_insert(badge);
                }
            }
        }
        let (identity, ahead_behind) = if let Some((head, id)) = table_head {
            let ahead_behind = self.reftable_ahead_behind(&head, id);
            (head, ahead_behind)
        } else {
            let head = self.repo.head().map_err(git)?;
            let head = if head.is_detached() {
                Head::Detached(
                    head.id()
                        .ok_or_else(|| git("detached HEAD without target"))?
                        .to_string(),
                )
            } else {
                Head::Branch {
                    name: head
                        .referent_name()
                        .expect("branch")
                        .shorten()
                        .to_str_lossy()
                        .into_owned(),
                    unborn: head.is_unborn(),
                }
            };
            (head, cheap_ahead_behind(&self.repo))
        };
        let generation = self.snapshot.as_ref().map_or(1, |s| s.generation + 1);
        self.snapshot = Some(StatusSnapshot {
            root: self.root.clone(),
            generation,
            head: identity,
            ahead_behind,
            files,
            folders,
        });
        Ok(self.snapshot.as_ref().expect("just assigned"))
    }

    fn tree_id(&self, id: Option<gix::hash::ObjectId>) -> Result<gix::hash::ObjectId, Error> {
        id.map(|id| {
            self.repo
                .find_commit(id)
                .map_err(git)?
                .tree_id()
                .map(|id| id.detach())
                .map_err(git)
        })
        .unwrap_or_else(|| Ok(gix::hash::ObjectId::empty_tree(self.repo.object_hash())))
    }

    fn reftable_status(
        &self,
        tree: gix::hash::ObjectId,
        patterns: &[BString],
        files: &mut BTreeMap<PathBuf, FileStatus>,
    ) -> Result<(), Error> {
        use gix::{dir::walk::EmissionMode, status::plumbing};
        use plumbing::index_as_worktree::traits::{FastEq, SubmoduleStatus};

        // gix's high-level status probes HEAD for .gitmodules even with a tree
        // override. Use its comparisons directly until gix can resolve reftable.
        #[derive(Clone)]
        struct SkipNestedStatus;
        impl SubmoduleStatus for SkipNestedStatus {
            type Output = gix::submodule::Status;
            fn status(
                &mut self,
                _: &gix::index::Entry,
                _: &gix::bstr::BStr,
            ) -> gix::Result<Option<Self::Output>> {
                Ok(None)
            }
        }
        let index = self.repo.index_or_empty().map_err(git)?;
        let mut pathspec = self
            .repo
            .pathspec(
                true,
                patterns,
                true,
                &index,
                gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            )
            .map_err(git)?;
        let mut record_error = None;
        self.repo
            .tree_index_status(
                &tree,
                &index,
                Some(&mut pathspec),
                gix::status::tree_index::TrackRenames::Disabled,
                |change, _, _| {
                    if let Err(error) = record_item(files, &Item::TreeIndex(change.into_owned())) {
                        record_error = Some(error);
                        return Ok(std::ops::ControlFlow::Break(()));
                    }
                    Ok(std::ops::ControlFlow::Continue(()))
                },
            )
            .map_err(git)?;
        if let Some(error) = record_error {
            return Err(error);
        }
        let mut changes = plumbing::index_as_worktree_with_renames::Recorder {
            records: Vec::new(),
        };
        let options = self
            .repo
            .dirwalk_options()
            .map_err(git)?
            .emit_untracked(EmissionMode::Matching)
            .emit_ignored(Some(EmissionMode::CollapseDirectory));
        self.repo
            .index_worktree_status(
                &index,
                patterns,
                &mut changes,
                FastEq,
                SkipNestedStatus,
                &mut gix::progress::Discard,
                &AtomicBool::new(false),
                index_worktree::Options {
                    dirwalk_options: Some(options),
                    ..Default::default()
                },
            )
            .map_err(git)?;
        for change in changes.records {
            record_item(files, &Item::IndexWorktree(change.into()))?;
        }
        Ok(())
    }

    fn reftable_ahead_behind(
        &self,
        head: &Head,
        id: Option<gix::hash::ObjectId>,
    ) -> Option<AheadBehind> {
        let Head::Branch {
            name,
            unborn: false,
        } = head
        else {
            return None;
        };
        let config = self.repo.config_snapshot();
        let merge = config.string(&format!("branch.{name}.merge"))?;
        let remote = config.string(&format!("branch.{name}.remote"))?;
        let upstream = if remote.as_slice() == b"." {
            merge
        } else {
            let remote = remote.to_str().ok()?;
            let name = merge.as_slice().strip_prefix(b"refs/heads/")?;
            // Only infer the standard mapping when it is configured explicitly.
            let fetch = config.string(&format!("remote.{remote}.fetch"))?;
            let expected = format!("refs/heads/*:refs/remotes/{remote}/*");
            if fetch
                .as_slice()
                .strip_prefix(b"+")
                .unwrap_or(fetch.as_slice())
                != expected.as_bytes()
            {
                return None;
            }
            let mut result = BString::from(format!("refs/remotes/{remote}/"));
            result.extend_from_slice(name);
            result
        };
        let upstream = self.reftable.as_ref()?.resolve(upstream.as_ref()).ok()??;
        compare_heads(&self.repo, id?, upstream)
    }
    /// Returns raw bytes at HEAD/index, or None for an absent path/unborn HEAD.
    /// Binary blobs stay binary; callers choose decoding/large-file policies.
    pub fn blob(&self, path: &Path, target: DiffTarget) -> Result<Option<Vec<u8>>, Error> {
        let path = self.relative(path)?;
        let id = match target {
            DiffTarget::Head => {
                let tree = if let Some(table) = &self.reftable {
                    let Some(id) = table.head()?.1 else {
                        return Ok(None);
                    };
                    self.repo.find_tree(self.tree_id(Some(id))?).map_err(git)?
                } else {
                    if self.repo.head().map_err(git)?.is_unborn() {
                        return Ok(None);
                    }
                    self.repo.head_tree().map_err(git)?
                };
                tree.lookup_entry_by_path(&path)
                    .map_err(git)?
                    .map(|e| e.object_id())
            }
            DiffTarget::Index => {
                let index = self.repo.index_or_empty().map_err(git)?;
                let path = gix::path::into_bstr(&path).map_err(git)?;
                let entry = index.entry_by_path(path.as_ref());
                if entry.is_some_and(|e| e.stage() != gix::index::entry::Stage::Unconflicted) {
                    return Err(Error::Unmerged);
                }
                entry.map(|e| e.id)
            }
        };
        let Some(id) = id else {
            return Ok(None);
        };
        let object = self.repo.find_object(id).map_err(git)?;
        if object.kind != gix::object::Kind::Blob {
            return Err(Error::NotBlob);
        }
        Ok(Some(object.data.clone()))
    }
}
fn record_item(files: &mut BTreeMap<PathBuf, FileStatus>, item: &Item) -> Result<(), Error> {
    use gix::diff::index::ChangeRef;
    use index_worktree::iter::Summary;
    let badge = match item {
        Item::TreeIndex(change) => Some(match change {
            ChangeRef::Addition { .. } => Badge::StagedAddition,
            ChangeRef::Deletion { .. } => Badge::StagedDeletion,
            ChangeRef::Modification { .. } | ChangeRef::Rewrite { .. } => Badge::StagedModification,
        }),
        Item::IndexWorktree(change) => {
            if let index_worktree::Item::DirectoryContents { entry, .. } = change
                && matches!(entry.status, gix::dir::entry::Status::Ignored(_))
            {
                Some(Badge::Ignored)
            } else {
                change.summary().map(|s| match s {
                    Summary::Removed => Badge::Missing,
                    Summary::Added | Summary::IntentToAdd => Badge::Untracked,
                    Summary::Conflict => Badge::Unmerged,
                    Summary::Modified
                    | Summary::TypeChange
                    | Summary::Renamed
                    | Summary::Copied => Badge::Modified,
                })
            }
        }
    };
    if let Some(badge) = badge {
        let path = gix::path::from_bstr(item.location())
            .map_err(git)?
            .into_owned();
        files.entry(path).or_default().statuses.insert(badge);
    }
    Ok(())
}
fn cheap_ahead_behind(repo: &gix::Repository) -> Option<AheadBehind> {
    let head = repo.head_id().ok()?.detach();
    let upstream = repo
        .rev_parse_single("@{upstream}")
        .ok()
        .map(|id| id.detach())
        .or_else(|| {
            // gix does not currently resolve the special local upstream remote ".".
            let head = repo.head().ok()?;
            let branch = head.referent_name()?.shorten();
            let config = repo.config_snapshot();
            let remote = config.string(&format!("branch.{}.remote", branch.to_str_lossy()))?;
            if remote.as_bstr() != "." {
                return None;
            }
            let merge = config.string(&format!("branch.{}.merge", branch.to_str_lossy()))?;
            repo.rev_parse_single(merge.as_bstr())
                .ok()
                .map(|id| id.detach())
        })?;
    compare_heads(repo, head, upstream)
}
fn compare_heads(
    repo: &gix::Repository,
    head: gix::hash::ObjectId,
    upstream: gix::hash::ObjectId,
) -> Option<AheadBehind> {
    if head == upstream {
        return Some(AheadBehind {
            ahead: 0,
            behind: 0,
        });
    }
    let deadline = Instant::now() + Duration::from_millis(2);
    let mut remaining = 1024;
    let mut ancestors = |id| -> Option<BTreeSet<gix::hash::ObjectId>> {
        let mut queue = VecDeque::from([id]);
        let mut found = BTreeSet::new();
        while let Some(id) = queue.pop_front() {
            if !found.insert(id) {
                continue;
            }
            if remaining == 0 || Instant::now() > deadline {
                return None;
            }
            remaining -= 1;
            queue.extend(repo.find_commit(id).ok()?.parent_ids().map(|p| p.detach()));
        }
        Some(found)
    };
    let a = ancestors(head)?;
    let b = ancestors(upstream)?;
    Some(AheadBehind {
        ahead: a.difference(&b).count(),
        behind: b.difference(&a).count(),
    })
}
#[derive(Debug, Default)]
struct Pending {
    full: bool,
    paths: BTreeSet<PathBuf>,
}
/// Filesystem notification backend. Polling is a fallback for sandboxes/filesystems without
/// native event delivery; it scans watched trees, so prefer Native for large repositories.
#[derive(Debug, Clone, Copy)]
pub enum WatchMode {
    /// OS-native notifications.
    Native,
    /// Portable polling at an explicit interval.
    Polling(Duration),
    /// Caller sends all refresh requests explicitly.
    Disabled,
}
/// Nonblocking editor facade. Status work and discovery run exclusively on its worker.
/// Watch callbacks coalesce metadata and worktree changes, without losing paths under load.
#[derive(Debug)]
pub struct GitService {
    pending: Arc<Mutex<Pending>>,
    wake: mpsc::SyncSender<()>,
    published: Arc<RwLock<Option<Arc<StatusSnapshot>>>>,
    errors: mpsc::Receiver<Error>,
    stop: Arc<AtomicBool>,
}
impl GitService {
    /// Starts discovery, initial status and optional watches on the background thread.
    /// Watch failures are reported through `try_error`; manual refreshes still work.
    pub fn new(folder: PathBuf, watch: bool) -> Result<Self, Error> {
        Self::with_watch_mode(
            folder,
            if watch {
                WatchMode::Native
            } else {
                WatchMode::Disabled
            },
        )
    }
    /// Starts with an explicit notification backend (including portable polling fallback).
    pub fn with_watch_mode(folder: PathBuf, mode: WatchMode) -> Result<Self, Error> {
        let pending = Arc::new(Mutex::new(Pending {
            full: true,
            paths: BTreeSet::new(),
        }));
        let (wake, rx) = mpsc::sync_channel(1);
        let (errors_tx, errors) = mpsc::channel();
        let published = Arc::new(RwLock::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let service = Self {
            pending: pending.clone(),
            wake: wake.clone(),
            published: published.clone(),
            errors,
            stop: stop.clone(),
        };
        thread::Builder::new()
            .name("vel-git".into())
            .spawn(move || {
                let mut repo = match Repository::discover(folder) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = errors_tx.send(e);
                        return;
                    }
                };
                let _watcher = if !matches!(mode, WatchMode::Disabled) {
                    let metadata = [
                        canonical_event_path(repo.git_dir()),
                        canonical_event_path(repo.common_dir()),
                    ];
                    let watcher_pending = pending.clone();
                    let watcher_wake = wake;
                    let handler = move |event: notify::Result<notify::Event>| {
                        let mut p = watcher_pending.lock().unwrap_or_else(|e| e.into_inner());
                        match event {
                            Ok(event) => {
                                if matches!(event.kind, notify::EventKind::Access(_)) {
                                    return;
                                }
                                if event.need_rescan() || event.paths.is_empty() {
                                    p.full = true;
                                }
                                for path in event.paths {
                                    let path = canonical_event_path(&path);
                                    if metadata.iter().any(|m| path.starts_with(m))
                                        || path.file_name().is_some_and(|n| {
                                            n == ".gitignore" || n == ".gitmodules" || n == ".git"
                                        })
                                    {
                                        p.full = true;
                                    } else {
                                        p.paths.insert(path);
                                    }
                                }
                            }
                            Err(_) => p.full = true,
                        }
                        if p.paths.len() > 4096 {
                            p.full = true;
                            p.paths.clear();
                        }
                        let _ = watcher_wake.try_send(());
                    };
                    let watcher: notify::Result<Box<dyn Watcher + Send>> = match mode {
                        WatchMode::Native => {
                            notify::recommended_watcher(handler).map(|w| Box::new(w) as _)
                        }
                        WatchMode::Polling(interval) => notify::PollWatcher::new(
                            handler,
                            notify::Config::default()
                                .with_poll_interval(interval)
                                .with_compare_contents(true)
                                .with_follow_symlinks(false),
                        )
                        .map(|w| Box::new(w) as _),
                        WatchMode::Disabled => unreachable!(),
                    };
                    let result = watcher.and_then(|mut watcher| {
                        watcher.watch(repo.root(), RecursiveMode::Recursive)?;
                        if !repo.git_dir().starts_with(repo.root()) {
                            watcher.watch(repo.git_dir(), RecursiveMode::Recursive)?;
                        }
                        if repo.common_dir() != repo.git_dir()
                            && !repo.common_dir().starts_with(repo.root())
                        {
                            watcher.watch(repo.common_dir(), RecursiveMode::Recursive)?;
                        }
                        Ok(watcher)
                    });
                    match result {
                        Ok(w) => Some(w),
                        Err(e) => {
                            let _ = errors_tx.send(Error::Watch(e));
                            None
                        }
                    }
                } else {
                    None
                };
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let request =
                        std::mem::take(&mut *pending.lock().unwrap_or_else(|e| e.into_inner()));
                    let result = if request.full {
                        repo.refresh()
                    } else {
                        repo.refresh_paths(&request.paths.into_iter().collect::<Vec<_>>())
                    };
                    match result {
                        Ok(snapshot) => {
                            *published.write().unwrap_or_else(|e| e.into_inner()) =
                                Some(Arc::new(snapshot.clone()))
                        }
                        Err(e) => {
                            let _ = errors_tx.send(e);
                        }
                    }
                    if rx.recv().is_err() {
                        break;
                    }
                }
            })?;
        Ok(service)
    }
    /// Schedules a full refresh; returns immediately.
    pub fn refresh(&self) -> Result<(), Error> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).full = true;
        self.wake
            .try_send(())
            .or_else(|e| {
                if matches!(e, mpsc::TrySendError::Full(_)) {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .map_err(|_| Error::Stopped)
    }
    /// Schedules literal paths, merging requests with watcher/manual updates.
    pub fn refresh_paths(&self, paths: impl IntoIterator<Item = PathBuf>) -> Result<(), Error> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .paths
            .extend(paths);
        self.wake
            .try_send(())
            .or_else(|e| {
                if matches!(e, mpsc::TrySendError::Full(_)) {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .map_err(|_| Error::Stopped)
    }
    /// Reads the latest immutable publication without waiting for worker-held locks.
    pub fn try_snapshot(&self) -> Option<Arc<StatusSnapshot>> {
        self.published.try_read().ok()?.clone()
    }
    /// Receives a worker/watch failure without blocking.
    pub fn try_error(&self) -> Option<Error> {
        self.errors.try_recv().ok()
    }
}
impl Drop for GitService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.wake.try_send(());
    }
}

fn canonical_event_path(path: &Path) -> PathBuf {
    // Resolve /var -> /private/var aliases without following a tracked leaf symlink.
    if let Some(parent) = path.parent()
        && let Some(name) = path.file_name()
        && let Ok(parent) = parent.canonicalize()
    {
        return parent.join(name);
    }
    // Removed paths no longer canonicalize; resolve their nearest extant ancestor.
    for ancestor in path.ancestors().skip(1) {
        if let Ok(parent) = ancestor.canonicalize()
            && let Ok(tail) = path.strip_prefix(ancestor)
        {
            return parent.join(tail);
        }
    }
    path.to_path_buf()
}
