//! Shared ignore-aware project traversal for the index, side bar and file search.
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::{WalkBuilder, WalkState};
use notify::{RecursiveMode, Watcher};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime},
};

/// Exclusion patterns shared by every project surface.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    files: GlobSet,
    folders: GlobSet,
    binary: GlobSet,
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
        })
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
        if directory {
            return !self.folders.is_match(relative) && !self.folders.is_match(Path::new(name));
        }
        for parent in relative.ancestors().skip(1) {
            if self.folders.is_match(parent)
                || parent
                    .file_name()
                    .is_some_and(|n| self.folders.is_match(Path::new(n)))
            {
                return false;
            }
        }
        !self.files.is_match(relative)
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
    /// Follow directory symlinks (off by default).
    pub follow_links: bool,
}
impl Folder {
    /// Prepares an ignore-aware walker, also respecting .gitignore outside a git repository.
    pub fn walker(&self, start: &Path, depth: Option<usize>, searchable: bool) -> WalkBuilder {
        let mut walk = WalkBuilder::new(start);
        let root = self.path.clone();
        let rules = self.rules.clone();
        walk.hidden(false)
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
    /// Lazy one-level side-bar load; no descendants are enumerated until expanded.
    pub fn children(&self, path: &Path) -> Result<Vec<Entry>, String> {
        let mut entries = Vec::new();
        for result in self.walker(path, Some(1), false).build() {
            let entry = result.map_err(|e| e.to_string())?;
            if entry.depth() == 0 {
                continue;
            }
            entries.push(Entry {
                path: entry.path().into(),
                directory: entry.file_type().is_some_and(|t| t.is_dir()),
            });
        }
        entries.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.path.cmp(&b.path)));
        Ok(entries)
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
    /// Searchable files, absolute and deduplicated.
    pub files: Vec<PathBuf>,
    /// Prepared relative-path candidates; normalization is part of background indexing.
    pub candidates: Arc<Vec<crate::fuzzy::Candidate>>,
    /// Traversal/watch errors, available to the UI.
    pub errors: Vec<String>,
}
fn build(folders: &[Folder], stop: &AtomicBool) -> (Snapshot, Vec<PathBuf>) {
    let files = Mutex::new(Vec::<PathBuf>::new());
    let directories = Mutex::new(Vec::new());
    let errors = Mutex::new(Vec::new());
    for folder in folders {
        folder
            .walker(&folder.path, None, true)
            .build_parallel()
            .run(|| {
                Box::new(|result| {
                    if stop.load(Ordering::Relaxed) {
                        return WalkState::Quit;
                    }
                    match result {
                        Ok(entry) if entry.file_type().is_some_and(|t| t.is_file()) => {
                            files.lock().expect("files").push(entry.path().into())
                        }
                        Ok(entry) if entry.file_type().is_some_and(|t| t.is_dir()) => directories
                            .lock()
                            .expect("directories")
                            .push(entry.path().into()),
                        Err(e) => errors.lock().expect("errors").push(e.to_string()),
                        _ => {}
                    }
                    WalkState::Continue
                })
            });
    }
    let mut files = files.into_inner().expect("files");
    files.sort();
    files.dedup();
    let candidates = Arc::new(
        files
            .iter()
            .map(|path| {
                crate::fuzzy::Candidate::new(
                    folders
                        .iter()
                        .find_map(|folder| path.strip_prefix(&folder.path).ok())
                        .unwrap_or(path)
                        .to_string_lossy(),
                )
            })
            .collect(),
    );
    (
        Snapshot {
            files,
            candidates,
            errors: errors.into_inner().expect("errors"),
        },
        directories.into_inner().expect("directories"),
    )
}
fn stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}
/// Background parallel index with recursive file watching and a directory-mtime fallback.
#[derive(Debug)]
pub struct Index {
    rx: mpsc::Receiver<Snapshot>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}
impl Index {
    /// Starts indexing immediately. Edits/replacements/renames invalidate the file list.
    pub fn new(folders: Vec<Folder>) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let join = thread::spawn(move || {
            let (events, changed) = mpsc::channel();
            let mut watch_errors = Vec::new();
            let mut watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if event
                        .as_ref()
                        .map_or(true, |e| !matches!(e.kind, notify::EventKind::Access(_)))
                    {
                        let _ = events.send(event);
                    }
                })
                .ok();
            if let Some(watcher) = &mut watcher {
                for folder in &folders {
                    if let Err(e) = watcher.watch(&folder.path, RecursiveMode::Recursive) {
                        watch_errors.push(e.to_string());
                    }
                }
            }
            let mut dirty = true;
            let mut stamps = Vec::new();
            let mut ticks = 0;
            while !cancelled.load(Ordering::Acquire) {
                if dirty {
                    let (mut snapshot, directories) = build(&folders, &cancelled);
                    stamps = directories
                        .into_iter()
                        .flat_map(|p| {
                            [
                                p.clone(),
                                p.join(".gitignore"),
                                p.join(".ignore"),
                                p.join(".git/info/exclude"),
                            ]
                        })
                        .map(|p| {
                            let time = stamp(&p);
                            (p, time)
                        })
                        .collect();
                    snapshot.errors.append(&mut watch_errors);
                    if cancelled.load(Ordering::Acquire) || tx.send(snapshot).is_err() {
                        break;
                    }
                    dirty = false;
                }
                if changed.recv_timeout(Duration::from_millis(100)).is_ok() {
                    dirty = true;
                    // Coalesce a burst without blocking publication on an unbounded event stream.
                    for _ in changed.try_iter().take(4096) {}
                }
                ticks += 1;
                if ticks % 20 == 0 {
                    dirty |= stamps.iter().any(|(p, old)| stamp(p) != *old);
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
