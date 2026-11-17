//! Shared ignore-aware project traversal for the index, side bar and file search.
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::{WalkBuilder, WalkState};
use notify::{RecursiveMode, Watcher};
use std::{
    collections::BTreeSet,
    ops::Bound,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

/// Exclusion patterns shared by every project surface.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    files: GlobSet,
    folders: GlobSet,
    binary: GlobSet,
    include_files: GlobSet,
    include_folders: GlobSet,
}
fn globs(patterns: &[String]) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(false)
                .backslash_escape(false)
                .build()
                .map_err(|e| e.to_string())?,
        );
    }
    builder.build().map_err(|e| e.to_string())
}
impl Rules {
    /// Compiles global + per-folder exclusion arrays. Malformed patterns are errors.
    pub fn new(files: &[String], folders: &[String], binary: &[String]) -> Result<Self, String> {
        Ok(Self {
            files: globs(files)?,
            folders: globs(folders)?,
            binary: globs(binary)?,
            ..Self::default()
        })
    }
    /// Adds folder-local include patterns; exclusions still take precedence.
    pub fn with_includes(mut self, files: &[String], folders: &[String]) -> Result<Self, String> {
        self.include_files = globs(files)?;
        self.include_folders = globs(folders)?;
        Ok(self)
    }
    /// Tests relative names and every ancestor folder; binary exclusions apply to searchable files.
    pub fn allows(&self, root: &Path, path: &Path, directory: bool, searchable: bool) -> bool {
        let relative = path.strip_prefix(root).unwrap_or(path);
        if relative.as_os_str().is_empty() && directory {
            return true;
        }
        let relative = if relative.as_os_str().is_empty() {
            Path::new(path.file_name().unwrap_or_default())
        } else {
            relative
        };
        let name = relative.file_name().unwrap_or_default();
        // Replace in Files owns these recovery artifacts. Never search or replace them,
        // even when a project explicitly includes all files and excludes nothing.
        if !directory && name.to_string_lossy().contains(".velocity-backup-") {
            return false;
        }
        let admitted = |patterns: &GlobSet, path: &Path| {
            patterns.is_empty()
                || patterns.is_match(path)
                || path
                    .file_name()
                    .is_some_and(|n| patterns.is_match(Path::new(n)))
        };
        if directory {
            return admitted(&self.include_folders, relative)
                && !self.folders.is_match(relative)
                && !self.folders.is_match(Path::new(name));
        }
        // https://www.sublimetext.com/docs/projects.html and file_patterns.html:
        // unmatched folders are excluded; slash-free patterns match names at any depth.
        // Prune an unmatched ancestor too, just as the walker and Folder::admits do.
        for parent in relative.ancestors().skip(1) {
            if (!parent.as_os_str().is_empty() && !admitted(&self.include_folders, parent))
                || self.folders.is_match(parent)
                || parent
                    .file_name()
                    .is_some_and(|n| self.folders.is_match(Path::new(n)))
            {
                return false;
            }
        }
        admitted(&self.include_files, relative)
            && !self.files.is_match(relative)
            && !self.files.is_match(Path::new(name))
            && (!searchable
                || (!self.binary.is_match(relative) && !self.binary.is_match(Path::new(name))))
    }
}
/// Project folder, with settings resolved by the application.
#[derive(Clone, Debug)]
pub struct Folder {
    /// Absolute root.
    pub path: PathBuf,
    /// Side-bar caption.
    pub name: String,
    /// Global plus folder-specific exclusions.
    pub rules: Rules,
    /// Follow directory symlinks (true unless the project explicitly disables it).
    pub follow_links: bool,
}
impl Folder {
    /// Prepares an ignore-aware walker, also respecting .gitignore outside a git repository.
    pub fn walker(&self, start: &Path, depth: Option<usize>, searchable: bool) -> WalkBuilder {
        let mut walk = WalkBuilder::new(start);
        let root = self.path.clone();
        let rules = self.rules.clone();
        // https://www.sublimetext.com/docs/indexing.html: index_exclude_gitignore
        // controls symbol indexing, not Goto Anything / Find in Files. Preserve the
        // side bar's ignore policy; searchable file lists use explicit project rules.
        walk.git_ignore(!searchable)
            .git_global(!searchable)
            .git_exclude(!searchable)
            .ignore(!searchable)
            .parents(!searchable)
            .hidden(false)
            .require_git(false)
            .follow_links(self.follow_links)
            .max_depth(depth)
            .threads(thread::available_parallelism().map_or(1, |n| n.get().min(4)))
            .filter_entry(move |entry| {
                entry.file_name() != ".git"
                    && rules.allows(
                        &root,
                        entry.path(),
                        entry.file_type().is_some_and(|t| t.is_dir()),
                        searchable,
                    )
            });
        walk
    }
    /// Whether the searchable walk would descend to `path`: it lies below the root, outside `.git`,
    /// and every folder between the root and it passes the folder rules. The entry's own rule is
    /// checked by the caller once its type is known.
    fn admits(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.path) else {
            return false;
        };
        if relative.components().any(|c| c.as_os_str() == ".git") {
            return false;
        }
        let mut current = self.path.clone();
        let mut parts = relative.components().peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                break;
            }
            current.push(part);
            if !self.rules.allows(&self.path, &current, true, true) {
                return false;
            }
        }
        true
    }
    /// Lazy one-level side-bar load; no descendants are enumerated until expanded.
    pub fn children(&self, path: &Path) -> Result<Vec<Entry>, String> {
        let mut entries = Vec::new();
        for result in self.walker(path, Some(1), false).build() {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) if child_entry_error(&error, path) => continue,
                Err(error) => return Err(error.to_string()),
            };
            if entry.depth() == 0 {
                continue;
            }
            let directory = entry.file_type().is_some_and(|t| t.is_dir());
            if directory && self.follow_links {
                let Ok(canonical) = std::fs::canonicalize(entry.path()) else {
                    // A link may disappear between directory enumeration and expansion.
                    continue;
                };
                let cyclic = path
                    .ancestors()
                    .take_while(|p| p.starts_with(&self.path))
                    .any(|parent| std::fs::canonicalize(parent).ok().as_ref() == Some(&canonical));
                if cyclic {
                    continue;
                }
            }
            entries.push(Entry {
                path: entry.path().into(),
                directory,
            });
        }
        entries.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.path.cmp(&b.path)));
        Ok(entries)
    }
}
// Ignore wraps walkdir failures in path/depth context. A bad child must not hide
// its siblings, while failure to enumerate the requested directory remains visible.
fn child_entry_error(error: &ignore::Error, root: &Path) -> bool {
    fn recoverable(error: &ignore::Error) -> bool {
        match error {
            ignore::Error::Io(_) | ignore::Error::Loop { .. } => true,
            ignore::Error::WithPath { err, .. }
            | ignore::Error::WithDepth { err, .. }
            | ignore::Error::WithLineNumber { err, .. } => recoverable(err),
            ignore::Error::Partial(errors) => !errors.is_empty() && errors.iter().all(recoverable),
            _ => false,
        }
    }
    match error {
        ignore::Error::WithPath { path, err } => path != root && recoverable(err),
        ignore::Error::WithDepth { depth, err } => *depth > 0 && recoverable(err),
        ignore::Error::WithLineNumber { err, .. } => child_entry_error(err, root),
        ignore::Error::Loop { .. } => true,
        _ => false,
    }
}
/// A lazy tree row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Absolute path.
    pub path: PathBuf,
    /// Whether expanding is meaningful.
    pub directory: bool,
}
/// Complete, sorted project snapshot. Partial traversals are never published.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// Searchable files, absolute, sorted and deduplicated.
    pub files: Arc<Vec<PathBuf>>,
    /// Prepared relative-path candidates; normalization is part of background indexing.
    pub candidates: Arc<Vec<crate::fuzzy::Candidate>>,
    /// Traversal/watch errors, available to the UI.
    pub errors: Vec<String>,
}
/// One traversal's results.
#[derive(Default)]
struct Walked {
    files: Vec<PathBuf>,
    directories: Vec<PathBuf>,
    /// Directories reached through a symbolic link; file events below them are not reported.
    linked: Vec<PathBuf>,
    errors: Vec<String>,
}
fn walk(folder: &Folder, start: &Path, stop: &AtomicBool) -> Walked {
    let out = Mutex::new(Walked::default());
    folder.walker(start, None, true).build_parallel().run(|| {
        Box::new(|result| {
            if stop.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            let mut out = out.lock().expect("walk");
            match result {
                Ok(entry) if entry.file_type().is_some_and(|t| t.is_file()) => {
                    out.files.push(entry.into_path())
                }
                Ok(entry) if entry.file_type().is_some_and(|t| t.is_dir()) => {
                    if entry.path_is_symlink() && entry.path() != folder.path {
                        out.linked.push(entry.path().into());
                    }
                    out.directories.push(entry.into_path());
                }
                Err(e) => out.errors.push(e.to_string()),
                _ => {}
            }
            WalkState::Continue
        })
    });
    out.into_inner().expect("walk")
}
fn label(folders: &[Folder], path: &Path) -> crate::fuzzy::Candidate {
    crate::fuzzy::Candidate::new(
        folders
            .iter()
            .find_map(|folder| path.strip_prefix(&folder.path).ok())
            .unwrap_or(path)
            .to_string_lossy(),
    )
}
/// Removes `root` and everything below it; descendants sort directly after their ancestor.
fn remove_below(set: &mut BTreeSet<PathBuf>, root: &Path) {
    let gone: Vec<_> = set
        .range::<Path, _>((Bound::Included(root), Bound::Unbounded))
        .take_while(|path| path.starts_with(root))
        .cloned()
        .collect();
    for path in &gone {
        set.remove(path);
    }
}
/// The searchable file set, kept current from file events instead of re-walking the project.
/// The sorted lists are shared with published snapshots and copied only when they change.
struct Tree {
    folders: Vec<Folder>,
    /// Canonical spelling of each root: macOS reports events below `/private/var` for `/var`.
    canonical: Vec<Option<PathBuf>>,
    files: Arc<Vec<PathBuf>>,
    candidates: Arc<Vec<crate::fuzzy::Candidate>>,
    directories: BTreeSet<PathBuf>,
    linked: Vec<PathBuf>,
    errors: Vec<String>,
}
impl Tree {
    fn scan(folders: &[Folder], stop: &AtomicBool) -> Self {
        let mut tree = Self {
            canonical: folders
                .iter()
                .map(|f| std::fs::canonicalize(&f.path).ok())
                .collect(),
            folders: folders.to_vec(),
            files: Arc::default(),
            candidates: Arc::default(),
            directories: BTreeSet::new(),
            linked: Vec::new(),
            errors: Vec::new(),
        };
        let mut files = Vec::new();
        for folder in folders {
            files.append(&mut tree.absorb(walk(folder, &folder.path, stop)));
        }
        files.sort();
        files.dedup();
        tree.candidates = Arc::new(files.iter().map(|p| label(folders, p)).collect());
        tree.files = Arc::new(files);
        tree
    }
    /// Records directories and errors, returning the files.
    fn absorb(&mut self, walked: Walked) -> Vec<PathBuf> {
        self.directories.extend(walked.directories);
        self.linked.extend(walked.linked);
        self.errors.extend(walked.errors);
        walked.files
    }
    fn snapshot(&mut self) -> Snapshot {
        Snapshot {
            files: self.files.clone(),
            candidates: self.candidates.clone(),
            errors: std::mem::take(&mut self.errors),
        }
    }
    /// Maps an event path to the project's spelling of it.
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        for (folder, canonical) in self.folders.iter().zip(&self.canonical) {
            if path.starts_with(&folder.path) {
                return Some(path.into());
            }
            if let Some(rest) = canonical.as_ref().and_then(|c| path.strip_prefix(c).ok()) {
                return Some(folder.path.join(rest));
            }
        }
        None
    }
    /// Re-reads the changed paths from disk. `None` asks for a full rescan.
    fn apply(&mut self, paths: Vec<PathBuf>, stop: &AtomicBool) -> Option<bool> {
        let mut paths = paths
            .iter()
            .map(|p| self.resolve(p))
            .collect::<Option<Vec<_>>>()?;
        paths.sort();
        paths.dedup();
        let mut changed = false;
        let mut last: Option<&Path> = None;
        for path in &paths {
            if last.is_some_and(|l| path.starts_with(l)) {
                continue;
            }
            last = Some(path);
            changed |= self.refresh(path, stop);
        }
        Some(changed)
    }
    /// Replaces `path` and its descendants, a contiguous run of the sorted list, with what is on
    /// disk now.
    fn refresh(&mut self, path: &Path, stop: &AtomicBool) -> bool {
        remove_below(&mut self.directories, path);
        self.linked.retain(|l| !l.starts_with(path));
        let mut fresh = Vec::new();
        for index in 0..self.folders.len() {
            let folder = &self.folders[index];
            if !folder.admits(path) {
                continue;
            }
            let metadata = if folder.follow_links {
                std::fs::metadata(path)
            } else {
                std::fs::symlink_metadata(path)
            };
            let walked = match metadata {
                Ok(m) if m.is_file() && folder.rules.allows(&folder.path, path, false, true) => {
                    Walked {
                        files: vec![path.into()],
                        ..Walked::default()
                    }
                }
                Ok(m) if m.is_dir() && folder.rules.allows(&folder.path, path, true, true) => {
                    let mut walked = walk(folder, path, stop);
                    if path != folder.path
                        && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_symlink())
                    {
                        walked.linked.push(path.into());
                    }
                    walked
                }
                _ => continue,
            };
            fresh.append(&mut self.absorb(walked));
        }
        fresh.sort();
        fresh.dedup();
        let start = self.files.partition_point(|p| p.as_path() < path);
        let end = start
            + self.files[start..]
                .iter()
                .take_while(|p| p.starts_with(path))
                .count();
        if self.files[start..end] == fresh[..] {
            return false;
        }
        let candidates: Vec<_> = fresh.iter().map(|p| label(&self.folders, p)).collect();
        Arc::make_mut(&mut self.candidates).splice(start..end, candidates);
        Arc::make_mut(&mut self.files).splice(start..end, fresh);
        true
    }
    /// Directories whose changes file events cannot report.
    fn polled(&self, everything: bool) -> Vec<(PathBuf, Option<SystemTime>)> {
        self.directories
            .iter()
            .filter(|d| everything || self.linked.iter().any(|l| d.starts_with(l)))
            .map(|p| (p.clone(), stamp(p)))
            .collect()
    }
}
#[cfg(test)]
fn build(folders: &[Folder], stop: &AtomicBool) -> (Snapshot, Vec<PathBuf>) {
    let mut tree = Tree::scan(folders, stop);
    let directories = tree.directories.iter().cloned().collect();
    (tree.snapshot(), directories)
}
fn stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}
/// What a file event means for the searchable file list.
enum Change {
    Rescan,
    Paths(Vec<PathBuf>),
}
fn change(event: notify::Result<notify::Event>) -> Option<Change> {
    use notify::{EventKind, event::ModifyKind};
    let Ok(event) = event else {
        return Some(Change::Rescan);
    };
    if event.need_rescan() {
        return Some(Change::Rescan);
    }
    match event.kind {
        // Content and metadata changes leave the list of names alone.
        EventKind::Access(_)
        | EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Metadata(_)) => None,
        _ if event.paths.is_empty() => Some(Change::Rescan),
        _ => Some(Change::Paths(event.paths)),
    }
}
/// More changed paths than this in one burst re-walk the project instead.
const INCREMENTAL_LIMIT: usize = 20_000;
/// Background parallel index, updated from file events, with a directory-mtime fallback for
/// symlinked folders and for volumes without file events.
#[derive(Debug)]
pub struct Index {
    rx: mpsc::Receiver<Snapshot>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}
impl Index {
    /// Starts indexing immediately. Edits/replacements/renames invalidate the file list.
    pub fn new(folders: Vec<Folder>) -> Self {
        Self::start(folders, true)
    }
    // A constructor seam lets headless tests exercise unavailable-watcher polling
    // without depending on delivery from the host's native filesystem event service.
    fn start(folders: Vec<Folder>, watch: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let join = thread::spawn(move || {
            let (events, changed) = mpsc::channel();
            let mut watch_errors = Vec::new();
            let mut watcher = if watch {
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if let Some(change) = change(event) {
                        let _ = events.send(change);
                    }
                })
                .map_err(|e| watch_errors.push(e.to_string()))
                .ok()
            } else {
                None
            };
            if let Some(watcher) = &mut watcher {
                for folder in &folders {
                    if let Err(e) = watcher.watch(&folder.path, RecursiveMode::Recursive) {
                        watch_errors.push(e.to_string());
                    }
                }
            }
            // Without reliable events every directory is polled, as before.
            let poll_everything = watcher.is_none() || !watch_errors.is_empty();
            let mut tree = Tree::scan(&folders, &cancelled);
            tree.errors.append(&mut watch_errors);
            let mut stamps = tree.polled(poll_everything);
            if cancelled.load(Ordering::Acquire) || tx.send(tree.snapshot()).is_err() {
                return;
            }
            let mut ticks = 0u32;
            while !cancelled.load(Ordering::Acquire) {
                let mut rescan = false;
                let mut paths = Vec::new();
                let mut take = |change: Change| match change {
                    Change::Rescan => rescan = true,
                    Change::Paths(mut p) => paths.append(&mut p),
                };
                match changed.recv_timeout(Duration::from_millis(100)) {
                    Ok(first) => {
                        take(first);
                        // Coalesce a burst: wait for a short quiet period, at most 250 ms.
                        let start = Instant::now();
                        while start.elapsed() < Duration::from_millis(250) {
                            match changed.recv_timeout(Duration::from_millis(50)) {
                                Ok(next) => take(next),
                                Err(_) => break,
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        thread::sleep(Duration::from_millis(100))
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                ticks = ticks.wrapping_add(1);
                if ticks.is_multiple_of(20) && stamps.iter().any(|(p, old)| stamp(p) != *old) {
                    rescan = true;
                }
                let publish = if rescan || paths.len() > INCREMENTAL_LIMIT {
                    tree = Tree::scan(&folders, &cancelled);
                    true
                } else if paths.is_empty() {
                    false
                } else {
                    match tree.apply(paths, &cancelled) {
                        Some(changed) => changed,
                        None => {
                            tree = Tree::scan(&folders, &cancelled);
                            true
                        }
                    }
                };
                if publish {
                    stamps = tree.polled(poll_everything);
                    if cancelled.load(Ordering::Acquire) || tx.send(tree.snapshot()).is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            rx,
            stop,
            join: Some(join),
        }
    }
    /// Returns the newest complete snapshot without waiting.
    pub fn poll(&self) -> Option<Snapshot> {
        self.rx.try_iter().last()
    }
}
impl Drop for Index {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidebar_and_index_share_exclusions_and_lazy_loading() {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["src", "vendor", "nested"] {
            std::fs::create_dir(dir.path().join(sub)).unwrap();
        }
        for file in [
            "src/main.rs",
            "vendor/bad.rs",
            "nested/deep.rs",
            "hidden.rs",
            "cache.tmp",
            "image.png",
        ] {
            std::fs::write(dir.path().join(file), "text").unwrap();
        }
        std::fs::write(dir.path().join(".gitignore"), "hidden.rs\n").unwrap();
        let folder = Folder {
            path: dir.path().into(),
            name: "root".into(),
            follow_links: false,
            rules: Rules::new(&["*.tmp".into()], &["vendor".into()], &["*.png".into()]).unwrap(),
        };
        let children = folder.children(dir.path()).unwrap();
        assert!(children.iter().any(|e| e.path.ends_with("src")));
        assert!(!children.iter().any(|e| e.path.ends_with("deep.rs")
            || e.path.ends_with("vendor")
            || e.path.ends_with("hidden.rs")
            || e.path.ends_with("cache.tmp")));
        let (snapshot, _) = build(&[folder], &AtomicBool::new(false));
        assert!(snapshot.files.iter().any(|p| p.ends_with("main.rs")));
        assert!(
            !snapshot
                .files
                .iter()
                .any(|p| p.ends_with("image.png") || p.ends_with("bad.rs"))
        );
    }
}

#[cfg(test)]
mod round15_tests {
    use super::*;
    #[test]
    fn searchable_walk_ignores_gitignore_but_applies_includes_and_excludes() {
        let dir = tempfile::tempdir().unwrap();
        for path in ["src", "skip", "src/nested"] {
            std::fs::create_dir_all(dir.path().join(path)).unwrap();
        }
        for path in [
            "src/ignored.rs",
            "src/a.txt",
            "src/blocked.rs",
            "skip/a.rs",
            "src/nested/b.rs",
        ] {
            std::fs::write(dir.path().join(path), "text").unwrap();
        }
        std::fs::write(dir.path().join(".gitignore"), "ignored.rs\n").unwrap();
        let folder = Folder {
            path: dir.path().into(),
            name: "root".into(),
            follow_links: true,
            rules: Rules::new(&["blocked.rs".into()], &[], &[])
                .unwrap()
                .with_includes(&["*.rs".into()], &["src".into(), "nested".into()])
                .unwrap(),
        };
        let sidebar = folder.children(&dir.path().join("src")).unwrap();
        assert!(!sidebar.iter().any(|e| e.path.ends_with("ignored.rs")));
        let (snapshot, _) = build(&[folder], &AtomicBool::new(false));
        assert!(snapshot.files.iter().any(|p| p.ends_with("ignored.rs")));
        assert!(snapshot.files.iter().any(|p| p.ends_with("nested/b.rs")));
        assert!(!snapshot.files.iter().any(|p| p.ends_with("blocked.rs")
            || p.ends_with("a.txt")
            || p.ends_with("skip/a.rs")));
    }
    #[cfg(unix)]
    #[test]
    fn symlink_directories_expand_and_search_without_cycles_and_can_be_disabled() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("linked.rs"), "linked").unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        symlink(&root, outside.join("cycle")).unwrap();
        let mut folder = Folder {
            path: root.clone(),
            name: "root".into(),
            follow_links: true,
            rules: Rules::default(),
        };
        assert!(
            folder
                .children(&root)
                .unwrap()
                .iter()
                .any(|e| e.directory && e.path.ends_with("linked"))
        );
        let children = folder.children(&root.join("linked")).unwrap();
        assert!(children.iter().any(|e| e.path.ends_with("linked.rs")));
        assert!(!children.iter().any(|e| e.path.ends_with("cycle")));
        let (snapshot, _) = build(&[folder.clone()], &AtomicBool::new(false));
        assert_eq!(snapshot.files.len(), 1);
        folder.follow_links = false;
        let (snapshot, _) = build(&[folder], &AtomicBool::new(false));
        assert!(snapshot.files.is_empty());
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::*;
    use notify::{
        Event, EventKind,
        event::{CreateKind, DataChange, Flag, ModifyKind},
    };
    fn folder(root: &Path) -> Folder {
        Folder {
            path: root.into(),
            name: "root".into(),
            follow_links: true,
            rules: Rules::new(&["*.tmp".into()], &["skip".into()], &["*.png".into()]).unwrap(),
        }
    }
    #[test]
    fn content_events_are_dropped_and_unknown_events_rescan() {
        let path = PathBuf::from("/p/a.rs");
        let data = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(path.clone());
        assert!(change(Ok(data)).is_none());
        let create = Event::new(EventKind::Create(CreateKind::File)).add_path(path.clone());
        assert!(matches!(change(Ok(create)), Some(Change::Paths(p)) if p == [path.clone()]));
        assert!(matches!(
            change(Ok(Event::new(EventKind::Create(CreateKind::File)))),
            Some(Change::Rescan)
        ));
        let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert!(matches!(change(Ok(rescan)), Some(Change::Rescan)));
        assert!(matches!(
            change(Err(notify::Error::generic("lost"))),
            Some(Change::Rescan)
        ));
    }
    #[test]
    fn incremental_updates_match_a_full_scan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        for sub in ["src/old", "skip", ".git"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        for file in ["src/a.rs", "src/old/b.rs", "src/gone.rs", "skip/c.rs"] {
            std::fs::write(root.join(file), "x").unwrap();
        }
        let folders = [folder(&root)];
        let stop = AtomicBool::new(false);
        let mut tree = Tree::scan(&folders, &stop);
        let published = tree.snapshot();
        std::fs::create_dir_all(root.join("src/new/deeper")).unwrap();
        std::fs::write(root.join("src/new/deeper/d.rs"), "x").unwrap();
        std::fs::rename(root.join("src/old"), root.join("src/moved")).unwrap();
        std::fs::remove_file(root.join("src/gone.rs")).unwrap();
        for ignored in ["src/e.tmp", "src/f.png", "skip/g.rs", ".git/HEAD"] {
            std::fs::write(root.join(ignored), "x").unwrap();
        }
        // Events arrive in the canonical spelling (`/private/var/...` on macOS) and out of order.
        let canonical = std::fs::canonicalize(&root).unwrap();
        let touched = [
            "src/new/deeper/d.rs",
            "src/new",
            "src/old",
            "src/moved",
            "src/gone.rs",
            "src/e.tmp",
            "src/f.png",
            "skip/g.rs",
            ".git/HEAD",
        ];
        let paths = touched.iter().map(|p| canonical.join(p)).collect();
        assert_eq!(tree.apply(paths, &stop), Some(true));
        let full = Tree::scan(&folders, &stop);
        assert_eq!(tree.files, full.files);
        let labels: Vec<_> = tree.candidates.iter().map(|c| c.label.clone()).collect();
        let expected: Vec<_> = full.candidates.iter().map(|c| c.label.clone()).collect();
        assert_eq!(labels, expected);
        assert!(tree.files.iter().any(|p| p.ends_with("src/moved/b.rs")));
        assert!(!tree.files.iter().any(|p| p.ends_with("gone.rs")));
        // The snapshot published before the change still holds the old list.
        assert!(published.files.iter().any(|p| p.ends_with("gone.rs")));
        // Touching an unchanged file, or a path outside the project, publishes nothing new.
        assert_eq!(tree.apply(vec![root.join("src/a.rs")], &stop), Some(false));
        assert_eq!(tree.apply(vec![dir.path().join("elsewhere")], &stop), None);
    }
    #[test]
    fn index_follows_creations_and_removals_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "x").unwrap();
        // Native event delivery needs a native check (FSEvents can register successfully
        // but deliver no events inside the sandbox). Exercise the polling fallback here;
        // incremental_updates_match_a_full_scan covers the event-driven subtree updates.
        let index = Index::start(vec![folder(&root)], false);
        let wait = |want: &dyn Fn(&Snapshot) -> bool| {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(20) {
                if let Some(snapshot) = index.poll()
                    && want(&snapshot)
                {
                    return true;
                }
                thread::sleep(Duration::from_millis(20));
            }
            false
        };
        assert!(wait(&|s| s.files.len() == 1));
        std::fs::create_dir(root.join("src/new")).unwrap();
        std::fs::write(root.join("src/new/b.rs"), "x").unwrap();
        assert!(wait(&|s| s
            .files
            .iter()
            .any(|p| p.ends_with("src/new/b.rs"))));
        std::fs::remove_file(root.join("src/a.rs")).unwrap();
        assert!(wait(
            &|s| s.files.len() == 1 && s.candidates[0].label.ends_with("b.rs")
        ));
    }
    /// The event-driven path with the native watcher. FSEvents registers but delivers nothing
    /// inside the agents' sandbox, so this runs in review: `cargo test -p vel-search --release
    /// -- --ignored native_events`.
    #[test]
    #[ignore = "needs native FSEvents delivery"]
    fn native_events_update_the_index_without_polling() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "x").unwrap();
        let index = Index::new(vec![folder(&root)]);
        let wait = |want: &dyn Fn(&Snapshot) -> bool| {
            // Shorter than the 2 s stamp poll, which is off here anyway (no symlinks).
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(10) {
                if let Some(snapshot) = index.poll()
                    && want(&snapshot)
                {
                    return start.elapsed();
                }
                thread::sleep(Duration::from_millis(10));
            }
            panic!("no snapshot");
        };
        wait(&|s| s.files.len() == 1);
        std::fs::create_dir(root.join("src/new")).unwrap();
        std::fs::write(root.join("src/new/b.rs"), "x").unwrap();
        assert!(
            wait(&|s| s.files.iter().any(|p| p.ends_with("src/new/b.rs"))) < Duration::from_secs(2)
        );
        std::fs::rename(root.join("src/new"), root.join("src/moved")).unwrap();
        assert!(
            wait(&|s| s.files.iter().any(|p| p.ends_with("src/moved/b.rs"))
                && !s.files.iter().any(|p| p.ends_with("src/new/b.rs")))
                < Duration::from_secs(2)
        );
        std::fs::remove_file(root.join("src/a.rs")).unwrap();
        assert!(wait(&|s| s.files.len() == 1) < Duration::from_secs(2));
    }
    #[cfg(unix)]
    #[test]
    fn files_below_symlinked_folders_are_found_by_polling() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        let index = Index::new(vec![folder(&root)]);
        let start = Instant::now();
        let mut seen = None;
        while start.elapsed() < Duration::from_secs(20) {
            if let Some(snapshot) = index.poll() {
                if seen.is_none() {
                    // Only after the first walk: file events do not cross the link.
                    std::fs::write(outside.join("late.rs"), "x").unwrap();
                }
                seen = Some(snapshot.files.clone());
                if seen
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|p| p.ends_with("linked/late.rs"))
                {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("{seen:?}");
    }
}

#[cfg(test)]
mod review15_tests {
    use super::*;
    fn folder(path: &Path) -> Folder {
        Folder {
            path: path.into(),
            name: "root".into(),
            rules: Rules::default(),
            follow_links: true,
        }
    }
    #[cfg(unix)]
    #[test]
    fn broken_unreadable_and_looping_children_do_not_hide_healthy_siblings() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("healthy.txt"), "visible").unwrap();
        symlink("absent", root.join("dangling")).unwrap();
        symlink("self", root.join("self")).unwrap();
        symlink(root, root.join("ancestor-loop")).unwrap();
        let private = root.join("private.txt");
        std::fs::write(&private, "secret").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o0)).unwrap();
        let folder = folder(root);
        let children = folder.children(root).unwrap();
        assert!(children.iter().any(|e| e.path.ends_with("healthy.txt")));
        assert!(!children.iter().any(|e| e.path.ends_with("dangling")
            || e.path.ends_with("self")
            || e.path.ends_with("ancestor-loop")));
        assert!(folder.children(&root.join("absent")).is_err());
        std::fs::set_permissions(private, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]
    fn replacement_backups_are_excluded_from_sidebar_full_incremental_and_file_search() {
        use crate::{
            files::{self, FileChange},
            text::{Options, Search},
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        std::fs::write(&path, "cat\n").unwrap();
        let search = Search::new("cat", Options::default()).unwrap();
        let backup = FileChange::prepare(path.clone(), &search, "cat changed")
            .unwrap()
            .commit()
            .unwrap();
        let mut folder = folder(dir.path());
        folder.rules = Rules::new(&[], &[], &[])
            .unwrap()
            .with_includes(&["*".into()], &[])
            .unwrap();
        assert_eq!(
            folder
                .children(dir.path())
                .unwrap()
                .iter()
                .map(|e| e.path.clone())
                .collect::<Vec<_>>(),
            vec![path.clone()]
        );
        let mut tree = Tree::scan(&[folder.clone()], &AtomicBool::new(false));
        assert_eq!(*tree.files, vec![path.clone()]);
        assert_eq!(
            tree.apply(vec![backup.clone()], &AtomicBool::new(false)),
            Some(false)
        );
        let task = files::Task::start(files::Request {
            search,
            scope: files::Where::parse("<project>", dir.path()).unwrap(),
            folders: vec![folder],
            open: vec![],
            context: 0,
        })
        .unwrap();
        loop {
            match task.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                files::Event::Line(line) => {
                    assert_eq!(line.path, path);
                }
                files::Event::Error(error) => panic!("{error}"),
                files::Event::Done { matches, files } => {
                    assert_eq!((matches, files), (1, 1));
                    break;
                }
            }
        }
        for file in tree.files.iter() {
            FileChange::prepare(
                file.clone(),
                &Search::new("cat", Options::default()).unwrap(),
                "dog",
            )
            .unwrap()
            .commit()
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "cat\n");
    }
    #[test]
    fn folder_name_includes_prune_ancestors_consistently_in_full_and_incremental_walks() {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["included/included", "excluded/included"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        for file in [
            "included/a.txt",
            "included/included/b.txt",
            "excluded/included/c.txt",
        ] {
            std::fs::write(dir.path().join(file), "text").unwrap();
        }
        let mut folder = folder(dir.path());
        folder.rules = Rules::default()
            .with_includes(&[], &["included".into()])
            .unwrap();
        let mut tree = Tree::scan(&[folder.clone()], &AtomicBool::new(false));
        assert_eq!(tree.files.len(), 2);
        assert!(folder.admits(&dir.path().join("included/included/b.txt")));
        assert!(!folder.admits(&dir.path().join("excluded/included/c.txt")));
        assert!(
            tree.files
                .iter()
                .all(|p| !p.starts_with(dir.path().join("excluded")))
        );
        std::fs::write(dir.path().join("excluded/included/new.txt"), "no").unwrap();
        tree.apply(
            vec![dir.path().join("excluded/included")],
            &AtomicBool::new(false),
        )
        .unwrap();
        let full = Tree::scan(&[folder], &AtomicBool::new(false));
        assert_eq!(tree.files, full.files);
        assert_eq!(tree.files.len(), 2);
    }
}

#[cfg(test)]
mod lane_b_round18_tests {
    use super::*;
    fn folder(root: &Path, binary: &[&str]) -> Folder {
        Folder {
            path: root.into(),
            name: "fixture".into(),
            rules: Rules::new(
                &[],
                &[],
                &binary.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
            )
            .unwrap(),
            follow_links: true,
        }
    }
    const DOCUMENTED: &[&str] = &[
        "*.jpg", "*.jpeg", "*.png", "*.gif", "*.ttf", "*.tga", "*.dds", "*.ico", "*.eot", "*.pdf",
        "*.swf", "*.jar", "*.zip",
    ];
    #[test]
    fn round18_binary_patterns_cover_file_lists_but_leave_sidebar_entries() {
        let dir = tempfile::tempdir().unwrap();
        for extension in DOCUMENTED {
            std::fs::write(
                dir.path().join(format!("file.{}", &extension[2..])),
                "searchable bytes",
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("text.rs"), "source").unwrap();
        for (patterns, expected) in [(DOCUMENTED, 1), (&[][..], DOCUMENTED.len() + 1)] {
            let f = folder(dir.path(), patterns);
            let tree = Tree::scan(std::slice::from_ref(&f), &AtomicBool::new(false));
            assert_eq!(tree.files.len(), expected);
            assert_eq!(f.children(dir.path()).unwrap().len(), DOCUMENTED.len() + 1);
        }
    }
    #[test]
    fn round18_file_events_are_local_and_ignore_rules_do_not_hide_target_from_search() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::write(dir.path().join("target/a.rs"), "first").unwrap();
        let f = folder(dir.path(), DOCUMENTED);
        let stop = AtomicBool::new(false);
        let mut tree = Tree::scan(std::slice::from_ref(&f), &stop);
        assert!(tree.files.iter().any(|p| p.ends_with("target/a.rs")));
        let unreported = dir.path().join("unreported.rs");
        std::fs::write(&unreported, "second").unwrap();
        assert_eq!(
            tree.apply(vec![dir.path().join("target/a.rs")], &stop),
            Some(false)
        );
        assert!(
            !tree.files.contains(&unreported),
            "one file event must not trigger a full rewalk"
        );
        tree.apply(vec![unreported.clone()], &stop).unwrap();
        assert!(tree.files.contains(&unreported));
        std::fs::remove_file(dir.path().join("target/a.rs")).unwrap();
        tree.apply(vec![dir.path().join("target/a.rs")], &stop)
            .unwrap();
        assert_eq!(tree.files, Tree::scan(&[f], &stop).files);
    }
    #[test]
    #[ignore = "paired cold project walk on 60,000 files; run explicitly"]
    fn round18_project_walk_timing() {
        let dir = tempfile::tempdir().unwrap();
        for directory in 0..300 {
            let path = dir.path().join(format!("target/d{directory}"));
            std::fs::create_dir_all(&path).unwrap();
            for file in 0..200 {
                std::fs::write(
                    path.join(format!("ragged_{file}_{}.rs", directory % 17)),
                    "fn item() {}\n",
                )
                .unwrap();
            }
        }
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        let base = folder(
            dir.path(),
            &[
                "*.png", "*.jpg", "*.gif", "*.pdf", "*.zip", "*.exe", "*.dylib",
            ],
        );
        let candidate = folder(dir.path(), DOCUMENTED);
        let stop = AtomicBool::new(false);
        let mut samples = [vec![], vec![]];
        for pair in 0..9 {
            for index in if pair % 2 == 0 { [0, 1] } else { [1, 0] } {
                let f = if index == 0 { &base } else { &candidate };
                let at = Instant::now();
                let mut tree = Tree::scan(std::slice::from_ref(f), &stop);
                let elapsed = at.elapsed().as_secs_f64() * 1000.;
                assert_eq!(tree.files.len(), 60_001);
                samples[index].push(elapsed);
                let new = dir.path().join(format!("new_{pair}_{index}.rs"));
                std::fs::write(&new, "new").unwrap();
                let update = Instant::now();
                tree.apply(vec![new.clone()], &stop).unwrap();
                eprintln!(
                    "round18 walk pair={pair} variant={index} walk_ms={elapsed:.3} one_file_ms={:.3}",
                    update.elapsed().as_secs_f64() * 1000.
                );
                std::fs::remove_file(new).unwrap();
            }
        }
        for sample in &mut samples {
            sample.sort_by(f64::total_cmp);
        }
        eprintln!(
            "round18 project walk cold-index OS-cache-warm files=60001 baseline_median_ms={:.3} candidate_median_ms={:.3} delta_pct={:.3}",
            samples[0][4],
            samples[1][4],
            (samples[1][4] / samples[0][4] - 1.) * 100.
        );
    }
}
