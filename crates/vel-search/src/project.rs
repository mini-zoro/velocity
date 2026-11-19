//! Shared ignore-aware project traversal for the index, side bar and file search.
use ignore::{WalkBuilder, WalkState};
use notify::{RecursiveMode, Watcher};
use std::{
    cell::OnceCell,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ops::Bound,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

/// Exclusion patterns shared by every project surface.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    files: Patterns,
    folders: Patterns,
    binary: Patterns,
    include_files: Patterns,
    include_folders: Patterns,
}

/// The editor's file patterns are not shell globs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patterns(Vec<Pattern>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Name,
    Suffix,
    Absolute,
    Project,
}

#[derive(Clone, Debug)]
struct Pattern {
    body: String,
    scope: Scope,
    directory: bool,
    wildcard: bool,
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        (self.scope, &self.body, self.directory) == (other.scope, &other.body, other.directory)
    }
}
impl Eq for Pattern {}

struct MatchPath<'a> {
    root: &'a Path,
    path: &'a Path,
    name: &'a [u8],
    relative: OnceCell<Option<&'a [u8]>>,
    directory: bool,
}

impl<'a> MatchPath<'a> {
    fn new(root: &'a Path, path: &'a Path, directory: bool) -> Self {
        Self {
            root,
            path,
            name: path.file_name().unwrap_or_default().as_encoded_bytes(),
            relative: OnceCell::new(),
            directory,
        }
    }

    fn relative(&self) -> Option<&'a [u8]> {
        *self.relative.get_or_init(|| {
            self.path
                .strip_prefix(self.root)
                .ok()
                .map(|path| path.as_os_str().as_encoded_bytes())
        })
    }
}

fn note_pattern(pattern: &str) {
    static NOTED: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    if (pattern.contains(['{', '[']) || pattern.contains("**"))
        && NOTED
            .get_or_init(Mutex::default)
            .lock()
            .expect("pattern notes")
            .insert(pattern.to_owned())
    {
        eprintln!(
            "File pattern {pattern:?}: brackets and braces are literal; consecutive stars act as one '*'."
        );
    }
}

impl Patterns {
    /// Compiles literal patterns and the two wildcard operators; no pattern is an error.
    pub fn new(patterns: &[String]) -> Self {
        use Scope::*;
        Self(
            patterns
                .iter()
                .map(|p| {
                    note_pattern(p);
                    let (scope, body) = if let Some(rest) = p.strip_prefix("//") {
                        (Project, rest)
                    } else if p.starts_with('/') {
                        (Absolute, p.as_str())
                    } else if p.contains('/') {
                        (Suffix, p.as_str())
                    } else {
                        (Name, p.as_str())
                    };
                    let directory = p.ends_with('/');
                    let body = if body == "/" {
                        body
                    } else {
                        body.strip_suffix('/').unwrap_or(body)
                    };
                    let mut collapsed = String::with_capacity(body.len());
                    for c in body.chars() {
                        if c != '*' || !collapsed.ends_with('*') {
                            collapsed.push(c);
                        }
                    }
                    let wildcard = body.contains(['*', '?']);
                    Pattern {
                        body: collapsed,
                        scope,
                        directory,
                        wildcard,
                    }
                })
                .collect(),
        )
    }

    /// True when there is no configured constraint.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Matches a project path, using the lexical spelling reached by the walker.
    pub fn matches(&self, root: &Path, path: &Path, directory: bool) -> bool {
        !self.is_empty() && self.matches_path(&MatchPath::new(root, path, directory))
    }

    fn matches_path(&self, path: &MatchPath<'_>) -> bool {
        self.0.iter().any(|p| p.matches(path))
    }

    // Where filters also treat directory basenames as subtree filters, but the
    // chosen search root itself is not a basename candidate.
    pub(crate) fn matches_within(
        &self,
        project: &Path,
        root: &Path,
        path: &Path,
        directory: bool,
    ) -> bool {
        path != root
            && (self.matches(project, path, directory) || self.matches_parent_name(root, path))
    }

    /// Evaluates index exclusions on a standalone path whose ancestors were not walked.
    pub fn excludes_index_path(&self, root: &Path, path: &Path, directory: bool) -> bool {
        !self.is_empty()
            && (self.matches(root, path, directory) || self.excludes_parents(root, path, directory))
    }

    fn excludes_parents(&self, root: &Path, path: &Path, directory: bool) -> bool {
        let Some(parent) = path.parent().filter(|p| *p != root && p.starts_with(root)) else {
            return false;
        };
        // Path rules already match descendants. When checking a file, test its
        // parent once to include directory-only rules. Bare names need each component.
        (!directory && {
            let parent = MatchPath::new(root, parent, true);
            self.0
                .iter()
                .any(|p| p.scope != Scope::Name && p.matches(&parent))
        }) || self.matches_parent_name(root, path)
    }

    fn matches_parent_name(&self, root: &Path, path: &Path) -> bool {
        self.0.iter().any(|p| p.scope == Scope::Name)
            && path
                .parent()
                .and_then(|parent| parent.strip_prefix(root).ok())
                .is_some_and(|relative| {
                    relative.iter().any(|name| {
                        self.0.iter().any(|p| {
                            p.scope == Scope::Name && p.matches_name(name.as_encoded_bytes())
                        })
                    })
                })
    }
}

impl Pattern {
    // https://www.sublimetext.com/docs/file_patterns.html#path-rules (4066+):
    // suffixes start at a component boundary and match descendants, not name prefixes.
    fn matches(&self, path: &MatchPath<'_>) -> bool {
        if self.scope == Scope::Name {
            return !path.name.is_empty() && self.matches_name(path.name);
        }
        let mut bytes = if self.scope == Scope::Project {
            let Some(relative) = path.relative() else {
                return false;
            };
            relative
        } else {
            path.path.as_os_str().as_encoded_bytes()
        };
        // A directory-only rule cannot match the file component, but can match
        // any of the containing directories. No ancestor strings are constructed.
        if self.directory && !path.directory {
            bytes = match bytes.iter().rposition(|&b| b == b'/') {
                Some(0) if bytes.starts_with(b"/") => &bytes[..1],
                Some(at) => &bytes[..at],
                None => return false,
            };
        }
        // The empty project-relative path is the root, not a child entry.
        if self.scope == Scope::Project && bytes.is_empty() {
            return false;
        }
        if self.wildcard {
            return path_glob(self.body.as_bytes(), bytes, self.scope == Scope::Suffix);
        }
        if self.scope == Scope::Suffix {
            let body = self.body.as_bytes();
            // An unanchored literal starts at a component and may end at any
            // following boundary, including a directory with descendants.
            let mut tail = bytes;
            loop {
                if tail.strip_prefix(body).is_some_and(at_component_end) {
                    return true;
                }
                let Some(slash) = tail.iter().position(|&b| b == b'/') else {
                    return false;
                };
                tail = &tail[slash + 1..];
            }
        }
        let Some(rest) = bytes.strip_prefix(self.body.as_bytes()) else {
            return false;
        };
        rest.is_empty()
            || rest.starts_with(b"/")
            || self.scope == Scope::Absolute && self.body == "/"
            || self.scope == Scope::Project && self.body.is_empty()
    }

    fn matches_name(&self, name: &[u8]) -> bool {
        let body = self.body.as_bytes();
        if !self.wildcard {
            return body == name;
        }
        // Most settings are *.extension. Avoid walking the name in that case.
        if let Some(suffix) = body.strip_prefix(b"*")
            && !suffix.contains(&b'*')
            && !suffix.contains(&b'?')
        {
            return name.ends_with(suffix);
        }
        let next = |at: usize| {
            let mut end = at + 1;
            while end < name.len() && name[end] & 0xc0 == 0x80 {
                end += 1;
            }
            end
        };
        let (mut p, mut n) = (0, 0);
        let mut retry = None;
        while p < body.len() || n < name.len() {
            match body.get(p) {
                Some(b'*') => {
                    p += 1;
                    retry = Some((p, n));
                    continue;
                }
                Some(b'?') if name.get(n).is_some_and(|b| *b != b'/') => {
                    n = next(n);
                    p += 1;
                    continue;
                }
                Some(b) if *b != b'?' && name.get(n) == Some(b) => {
                    n += 1;
                    p += 1;
                    continue;
                }
                _ => {}
            }
            let Some((pattern, consumed)) = retry else {
                return false;
            };
            if consumed == name.len() {
                return false;
            }
            n = next(consumed);
            p = pattern;
            retry = Some((p, n));
        }
        true
    }
}

fn at_component_end(rest: &[u8]) -> bool {
    rest.is_empty() || rest[0] == b'/'
}

fn path_glob(body: &[u8], path: &[u8], suffix: bool) -> bool {
    let (mut p, mut n, mut start) = (0, 0, 0);
    let mut retry = None;
    loop {
        match body.get(p) {
            None if at_component_end(&path[n..]) => return true,
            Some(b'*') => {
                p += 1;
                retry = Some((p, n));
                continue;
            }
            Some(b'?') => {
                if let Some(width) = scalar_width(&path[n..]) {
                    p += 1;
                    n += width;
                    continue;
                }
            }
            Some(byte) if path.get(n) == Some(byte) => {
                p += 1;
                n += 1;
                continue;
            }
            _ => {}
        }
        // A path star can consume any byte. Once we reach the first real
        // star, its earliest possible start covers all later suffix starts.
        if let Some((pattern, consumed)) = retry {
            if consumed == path.len() {
                return false;
            }
            p = pattern;
            n = consumed + 1;
            retry = Some((p, n));
        } else if suffix && let Some(slash) = path[start..].iter().position(|&b| b == b'/') {
            start += slash + 1;
            n = start;
            p = 0;
        } else {
            return false;
        }
    }
}

fn scalar_width(bytes: &[u8]) -> Option<usize> {
    let width = match *bytes.first()? {
        b'/' => return None,
        0..=0x7f => return Some(1),
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    std::str::from_utf8(bytes.get(..width)?).ok().map(|_| width)
}

impl Rules {
    /// Compiles global and per-folder exclusion arrays.
    pub fn new(files: &[String], folders: &[String], binary: &[String]) -> Result<Self, String> {
        Ok(Self {
            files: Patterns::new(files),
            folders: Patterns::new(folders),
            binary: Patterns::new(binary),
            ..Self::default()
        })
    }
    /// Adds folder-local include patterns; exclusions still take precedence.
    pub fn with_includes(mut self, files: &[String], folders: &[String]) -> Result<Self, String> {
        self.include_files = Patterns::new(files);
        self.include_folders = Patterns::new(folders);
        Ok(self)
    }
    /// Tests relative names and every ancestor folder; binary exclusions apply to searchable files.
    pub fn allows(&self, root: &Path, path: &Path, directory: bool, searchable: bool) -> bool {
        self.allows_entry(root, path, directory, searchable)
            && (self.folders.is_empty() || !self.folders.excludes_parents(root, path, directory))
            && (self.include_folders.is_empty()
                || path
                    .ancestors()
                    .skip(1)
                    .take_while(|p| *p != root && p.starts_with(root))
                    .all(|parent| self.include_folders.matches(root, parent, true)))
    }

    /// Checks one entry in a traversal that has already pruned its parents.
    pub fn allows_entry(
        &self,
        root: &Path,
        path: &Path,
        directory: bool,
        searchable: bool,
    ) -> bool {
        let matches = MatchPath::new(root, path, directory);
        let admitted = |includes: &Patterns, excludes: &Patterns| {
            (includes.is_empty() || includes.matches_path(&matches))
                && !excludes.matches_path(&matches)
        };
        if directory {
            return path == root || admitted(&self.include_folders, &self.folders);
        }
        // Replace in Files, document saves and project session writes own these
        // recovery artifacts. Never expose them even with empty exclusion rules.
        !matches
            .name
            .windows(b".velocity-backup-".len())
            .any(|part| part == b".velocity-backup-")
            && !matches.name.starts_with(b".velocity-save-")
            && !matches.name.starts_with(b".velocity-session-")
            && admitted(&self.include_files, &self.files)
            && (!searchable || !self.binary.matches_path(&matches))
    }
}
/// Project folder, with settings resolved by the application.
#[derive(Clone, Debug, PartialEq, Eq)]
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
        let admits_start = self.admits(start);
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
                admits_start
                    && rules.allows_entry(
                        &root,
                        entry.path(),
                        entry.file_type().is_some_and(|t| t.is_dir()),
                        searchable,
                    )
            });
        walk
    }
    /// Whether the searchable walk would descend to `path`: it lies below the root,
    /// and every folder between the root and it passes the folder rules. The entry's own rule is
    /// checked by the caller once its type is known.
    fn admits(&self, path: &Path) -> bool {
        path == self.path
            || (path.starts_with(&self.path)
                && path
                    .parent()
                    .is_none_or(|parent| self.rules.allows(&self.path, parent, true, true)))
    }
    /// Lazy one-level load with ignore files.
    pub fn children(&self, path: &Path) -> Result<Vec<Entry>, String> {
        self.read_children(path, true, &AtomicBool::new(false))
    }
    /// Side-bar listing; ignored entries are visible, with badges supplied separately.
    pub fn sidebar_children(&self, path: &Path, stop: &AtomicBool) -> Result<Vec<Entry>, String> {
        self.read_children(path, false, stop)
    }

    /// Collects a capped recursive expansion using the walker's bounded thread pool.
    /// External symlink directories remain visible but are not expanded by this action.
    pub fn sidebar_subtree(
        &self,
        path: &Path,
        roots: &[PathBuf],
        limit: usize,
        stop: &AtomicBool,
    ) -> SidebarSubtree {
        let roots: Vec<_> = roots.iter().filter_map(|p| p.canonicalize().ok()).collect();
        let out = Mutex::new(SidebarSubtree::default());
        let mut walker = self.walker(path, None, false);
        walker
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .ignore(false)
            .parents(false);
        if path
            .canonicalize()
            .is_ok_and(|target| roots.iter().all(|p| !target.starts_with(p)))
        {
            return SidebarSubtree::default();
        }
        walker.build_parallel().run(|| {
            Box::new(|entry| {
                if stop.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                let Some(kind) = entry.file_type() else {
                    return WalkState::Continue;
                };
                let directory = kind.is_dir();
                if !directory && !kind.is_file() {
                    return WalkState::Continue;
                }
                let external = directory
                    && entry.path_is_symlink()
                    && entry
                        .path()
                        .canonicalize()
                        .is_ok_and(|target| roots.iter().all(|p| !target.starts_with(p)));
                let mut out = out.lock().expect("sidebar subtree");
                if entry.depth() > 0
                    && let Some(parent) = entry.path().parent()
                    && let Some(parent_entries) = out.listings.get_mut(parent)
                {
                    parent_entries.push(Entry {
                        path: entry.path().to_owned(),
                        directory,
                    });
                }
                if !directory {
                    return WalkState::Continue;
                }
                if external || out.listings.len() >= limit {
                    out.limited |= !external;
                    return WalkState::Skip;
                }
                out.listings.entry(entry.into_path()).or_default();
                WalkState::Continue
            })
        });
        let mut out = out.into_inner().expect("sidebar subtree");
        for entries in out.listings.values_mut() {
            sort_sidebar_entries(entries);
        }
        out
    }
    fn read_children(
        &self,
        path: &Path,
        ignored: bool,
        stop: &AtomicBool,
    ) -> Result<Vec<Entry>, String> {
        let mut entries = Vec::new();
        let ancestors: Vec<_> = if self.follow_links {
            path.ancestors()
                .take_while(|p| p.starts_with(&self.path))
                .filter_map(|p| std::fs::canonicalize(p).ok())
                .collect()
        } else {
            Vec::new()
        };
        let mut walker = self.walker(path, Some(1), false);
        if !ignored {
            walker
                .git_ignore(false)
                .git_global(false)
                .git_exclude(false)
                .ignore(false)
                .parents(false);
        }
        for result in walker.build() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let entry = match result {
                Ok(entry) => entry,
                Err(error) if child_entry_error(&error, path) => continue,
                Err(error) => return Err(error.to_string()),
            };
            if entry.depth() == 0 {
                continue;
            }
            let Some(kind) = entry.file_type() else {
                continue;
            };
            let directory = kind.is_dir();
            if !(directory || kind.is_file()) {
                continue;
            }
            if directory && entry.path_is_symlink() {
                let Ok(canonical) = std::fs::canonicalize(entry.path()) else {
                    // A link may disappear between directory enumeration and expansion.
                    continue;
                };
                if ancestors.contains(&canonical) {
                    continue;
                }
            }
            entries.push(Entry {
                path: entry.path().into(),
                directory,
            });
        }
        if !stop.load(Ordering::Relaxed) {
            sort_sidebar_entries(&mut entries);
        }
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

/// Listings for the directories expanded by a single recursive side-bar action.
#[derive(Debug, Default)]
pub struct SidebarSubtree {
    /// Expanded directories and their immediate children.
    pub listings: HashMap<PathBuf, Vec<Entry>>,
    /// Whether the directory budget prevented further expansion.
    pub limited: bool,
}

/// The side bar groups directories first and folds case (original spelling breaks ties).
/// The public docs do not specify a numeric collation; keep lexical number ordering.
pub fn sort_sidebar_entries(entries: &mut [Entry]) {
    entries
        .sort_by_cached_key(|e| sidebar_key(e.path.file_name().unwrap_or_default(), e.directory));
}
pub(crate) type SidebarKey = (bool, String, std::ffi::OsString);
pub(crate) fn sidebar_key(name: &std::ffi::OsStr, directory: bool) -> SidebarKey {
    (
        !directory,
        name.to_string_lossy().to_lowercase(),
        name.to_owned(),
    )
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
                Err(e) if out.errors.len() < 64 => out.errors.push(e.to_string()),
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
            if stop.load(Ordering::Relaxed) {
                return tree;
            }
            files.append(&mut tree.absorb(walk(folder, &folder.path, stop)));
        }
        if stop.load(Ordering::Relaxed) {
            return tree;
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
        self.errors.extend(
            walked
                .errors
                .into_iter()
                .take(64usize.saturating_sub(self.errors.len())),
        );
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

/// Coalesced listing invalidations independent of the searchable file set.
#[derive(Default, Debug)]
pub struct DirectoryChanges {
    /// A watcher overflow invalidated every expanded listing.
    pub all: bool,
    /// Exact expanded directories that may have changed.
    pub paths: BTreeSet<PathBuf>,
}
impl DirectoryChanges {
    fn mark(&mut self, path: PathBuf) {
        if !self.all {
            self.paths.insert(path);
            if self.paths.len() > INCREMENTAL_LIMIT {
                self.invalidate();
            }
        }
    }
    fn invalidate(&mut self) {
        self.paths.clear();
        self.all = true;
    }
}

type Expanded = Arc<Mutex<Arc<HashSet<PathBuf>>>>;
/// Background parallel index, updated from file events, with a directory-mtime fallback for
/// symlinked folders and for volumes without file events.
#[derive(Debug)]
pub struct Index {
    latest: Arc<Mutex<Option<Snapshot>>>,
    directory_changes: Arc<Mutex<DirectoryChanges>>,
    expanded: Expanded,
    stop: Arc<AtomicBool>,
}
impl Index {
    /// Starts indexing immediately. Edits/replacements/renames invalidate the file list.
    pub fn new(folders: Vec<Folder>) -> Self {
        Self::start(folders, true)
    }
    // A constructor seam lets headless tests exercise unavailable-watcher polling
    // without depending on delivery from the host's native filesystem event service.
    fn start(folders: Vec<Folder>, watch: bool) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let published = latest.clone();
        let directory_changes = Arc::new(Mutex::new(DirectoryChanges::default()));
        let listings = directory_changes.clone();
        let expanded: Expanded = Arc::default();
        let directories = expanded.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        thread::spawn(move || {
            let (events, changed) = mpsc::sync_channel(256);
            let overflow = Arc::new(AtomicBool::new(false));
            let mut watch_errors = Vec::new();
            let mut watcher = if watch {
                let overflow = overflow.clone();
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if let Some(change) = change(event)
                        && events.try_send(change).is_err()
                    {
                        overflow.store(true, Ordering::Release);
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
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            *published.lock().expect("snapshot") = Some(tree.snapshot());
            let mut ticks = 0u32;
            let mut directory_stamps = BTreeMap::new();
            while !cancelled.load(Ordering::Acquire) {
                let mut rescan = overflow.swap(false, Ordering::AcqRel);
                let mut paths = Vec::new();
                let mut take = |change: Change| match change {
                    Change::Rescan => rescan = true,
                    Change::Paths(mut p)
                        if p.len() <= INCREMENTAL_LIMIT.saturating_sub(paths.len()) =>
                    {
                        paths.append(&mut p)
                    }
                    Change::Paths(_) => rescan = true,
                };
                match changed.recv_timeout(Duration::from_millis(100)) {
                    Ok(first) => {
                        take(first);
                        // Coalesce a burst: wait for a short quiet period, at most 250 ms.
                        let start = Instant::now();
                        while !cancelled.load(Ordering::Relaxed)
                            && start.elapsed() < Duration::from_millis(250)
                        {
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
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                paths = paths.iter().filter_map(|path| tree.resolve(path)).collect();
                let expanded = directories.lock().expect("expanded directories").clone();
                directory_stamps.retain(|path, _| expanded.contains(path));
                let mut changed_dirs = DirectoryChanges::default();
                if rescan {
                    changed_dirs.invalidate();
                }
                for path in &paths {
                    if expanded.contains(path) {
                        changed_dirs.mark(path.clone());
                    }
                    if let Some(parent) = path.parent().filter(|p| expanded.contains(*p)) {
                        changed_dirs.mark(parent.to_owned());
                    }
                }
                // FSEvents may not cross links. This also covers a sandbox or volume
                // that registers a watcher but delivers no events.
                for path in expanded.iter() {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    let stamp = stamp(path);
                    if directory_stamps.insert(path.clone(), stamp) != Some(stamp) {
                        changed_dirs.mark(path.clone());
                    }
                }
                if changed_dirs.all || !changed_dirs.paths.is_empty() {
                    let mut listings = listings.lock().expect("listing changes");
                    if changed_dirs.all {
                        listings.invalidate();
                    } else {
                        for path in changed_dirs.paths {
                            listings.mark(path);
                        }
                    }
                }
                ticks = ticks.wrapping_add(1);
                if ticks.is_multiple_of(20) && stamps.iter().any(|(p, old)| stamp(p) != *old) {
                    rescan = true;
                }
                let publish = if rescan {
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
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    *published.lock().expect("snapshot") = Some(tree.snapshot());
                }
            }
        });
        Self {
            latest,
            directory_changes,
            expanded,
            stop,
        }
    }
    /// Returns the newest complete snapshot without waiting.
    pub fn poll(&self) -> Option<Snapshot> {
        self.latest.lock().expect("snapshot").take()
    }
    /// Tracks only the directories whose listings may need updating.
    pub fn watch_directories(&self, paths: Arc<HashSet<PathBuf>>) {
        *self.expanded.lock().expect("expanded directories") = paths;
    }
    /// Drains listing invalidations even when the searchable paths haven't changed.
    pub fn directory_changes(&self) -> DirectoryChanges {
        self.directory_changes.try_lock().map_or_else(
            |_| DirectoryChanges::default(),
            |mut changes| std::mem::take(&mut *changes),
        )
    }
}
impl Drop for Index {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_pattern_reference_table() {
        // https://www.sublimetext.com/docs/file_patterns.html (current, 4066+ examples).
        for (pattern, path, directory, expected) in [
            ("abc", "/p/abc", false, true),
            ("abc", "/p/abcd", false, false),
            ("a?c", "/p/abc", false, true),
            ("a?c", "/p/ac", false, false),
            ("a*c", "/p/abc", false, true),
            ("a*c", "/p/ac", false, true),
            ("a*c", "/p/abdc", false, true),
            ("mydir/one", "/parent/mydir/one", false, true),
            ("mydir/one", "/mydir/one", true, true),
            ("mydir/one", "/mydir/one/sub", false, true),
            ("mydir/two", "/parent/mydir/two", false, true),
            ("mydir/two", "/parent/mydir/two_sub", false, false),
            ("/mydir/three", "/mydir/three", false, true),
            ("/mydir/three", "/nested/mydir/three", false, false),
            ("mydir/three/", "/parent/mydir/three/sub", false, true),
            ("mydir/three/", "/parent/mydir/three", false, false),
            ("mydir/four/", "/parent/mydir/four", true, true),
            ("mydir/four/", "/parent/mydir/four/sub", false, true),
            ("//mydir/five", "/project1/mydir/five", false, true),
            ("//mydir/five", "/project2/mydir/five", false, true),
            ("//mydir/five", "/project1/nested/mydir/five", false, false),
            ("foo*", "/root/foo/bar/x.txt", false, false),
            ("a/*/z", "/root/a/b/c/z", false, true),
            ("a/*/z", "/root/a/z", true, false),
            ("a*/*/a*", "/repo/[a]/x/a\nb/a", false, false),
            ("x/**/gen", "/root/x/gen", true, false),
            ("a/?/z", "/root/a/b/c/z", false, false),
            ("?", "/root/好", false, true),
            ("[abc]", "/root/[abc]", false, true),
            ("{a,b}", "/root/a", false, false),
            ("a/**/z", "/root/a/b/z", false, true),
            ("a/**/z", "/root/a/**/z", false, true),
            ("**/node_modules", "/project1/node_modules", true, true),
            ("**/*.min.js", "/project1/public/app.min.js", false, true),
        ] {
            let root = if path.starts_with("/project2/") {
                "/project2"
            } else {
                "/project1"
            };
            assert_eq!(
                Patterns::new(&[pattern.into()]).matches(
                    Path::new(root),
                    Path::new(path),
                    directory
                ),
                expected,
                "{pattern:?} {path:?}"
            );
        }
    }
    #[test]
    #[ignore = "400,000 deterministic comparisons against the scalar oracle"]
    fn round20_random_file_pattern_comparison() {
        // Test every candidate component substring using simple wildcard DP,
        // independently of the production regex or greedy basename matcher.
        fn whole(pattern: &str, candidate: &str, path: bool) -> bool {
            let chars: Vec<_> = candidate.chars().collect();
            let mut reachable = vec![false; chars.len() + 1];
            reachable[0] = true;
            for token in pattern.chars() {
                if token == '*' {
                    for (i, &c) in chars.iter().enumerate() {
                        reachable[i + 1] |= reachable[i] && (path || c != '/');
                    }
                } else {
                    for (i, &c) in chars.iter().enumerate().rev() {
                        reachable[i + 1] = reachable[i] && (token == c || token == '?' && c != '/');
                    }
                    reachable[0] = false;
                }
            }
            reachable[chars.len()]
        }
        fn oracle(pattern: &str, path: &str, directory: bool) -> bool {
            if !pattern.contains('/') {
                let name = path.rsplit('/').next().unwrap();
                return !name.is_empty() && whole(pattern, name, false);
            }
            let relative = pattern.starts_with("//");
            let anchored = pattern.starts_with('/');
            let mut path = if relative {
                let Some(path) = path.strip_prefix("/repo/") else {
                    return false;
                };
                path
            } else {
                path
            };
            if pattern.ends_with('/') && !directory {
                let Some((parent, _)) = path.rsplit_once('/') else {
                    return false;
                };
                path = if parent.is_empty() && !relative {
                    "/"
                } else {
                    parent
                };
            }
            let body = pattern.strip_prefix("//").unwrap_or(pattern);
            let body = if body == "/" {
                body
            } else {
                body.strip_suffix('/').unwrap_or(body)
            };
            if body.is_empty() && relative || body == "/" && path.starts_with('/') {
                return true;
            }
            for start in std::iter::once(0).chain(
                path.match_indices('/')
                    .map(|(i, _)| i + 1)
                    .filter(|_| !anchored),
            ) {
                for end in path.match_indices('/').map(|(i, _)| i).chain([path.len()]) {
                    if end >= start && whole(body, &path[start..end], true) {
                        return true;
                    }
                }
            }
            false
        }
        let mut seed = 0x2a7f_32c6_a021_4badu64;
        let mut random = |n| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as usize % n
        };
        let components = [
            "a", "b", "c", "ab", "世", "x", "[a]", "{a,b}", "gen", "b.c", "a\nb",
        ];
        let source = [
            "a", "b", "a*", "**", "*?", "?", "[a]", "{a,b}", "世?", "gen", "x",
        ];
        for p in 0..800 {
            let pattern = match p % 8 {
                0 => format!(
                    "{}/{}*/{}",
                    source[random(3)],
                    ["", "*"][random(2)],
                    source[random(3)]
                ),
                1 => format!("//{}", source[random(source.len())]),
                2 => format!(
                    "//{}/{}",
                    source[random(source.len())],
                    source[random(source.len())]
                ),
                3 => format!("/repo/{}/", source[random(source.len())]),
                4 => source[random(source.len())].to_string(),
                5 => format!("//{}/", source[random(source.len())]),
                6 => format!(
                    "{}/{}/",
                    source[random(source.len())],
                    source[random(source.len())]
                ),
                _ => format!(
                    "{}/{}/{}",
                    source[random(source.len())],
                    source[random(source.len())],
                    source[random(source.len())]
                ),
            };
            let compiled = Patterns::new(std::slice::from_ref(&pattern));
            for _ in 0..500 {
                let mut path = if random(5) == 0 { "/outer" } else { "/repo" }.to_owned();
                for _ in 0..random(6) {
                    path.push('/');
                    path.push_str(components[random(components.len())]);
                }
                let directory = random(2) == 0;
                assert_eq!(
                    compiled.matches(Path::new("/repo"), Path::new(&path), directory),
                    oracle(&pattern, &path, directory),
                    "{pattern:?}, {path:?}, directory={directory}"
                );
            }
        }
    }
    #[test]
    fn sidebar_sort_directories_then_folded_names() {
        let mut entries = [
            ("README.md", false),
            ("a.txt", false),
            ("z", true),
            ("A.txt", false),
            ("file2", false),
            ("file10", false),
            ("A", true),
        ]
        .map(|(p, directory)| Entry {
            path: p.into(),
            directory,
        });
        sort_sidebar_entries(&mut entries);
        assert_eq!(
            entries.map(|e| e.path),
            ["A", "z", "A.txt", "a.txt", "file10", "file2", "README.md"].map(PathBuf::from)
        );
    }
    #[test]
    fn dot_git_visibility_is_a_folder_rule() {
        let dir = tempfile::tempdir().unwrap();
        let git = dir.path().join(".git");
        std::fs::create_dir(&git).unwrap();
        std::fs::write(git.join("hello"), "allowed by explicit empty rules").unwrap();
        let mut folder = Folder {
            path: dir.path().into(),
            name: "root".into(),
            follow_links: false,
            rules: Rules::default(),
        };
        let stop = AtomicBool::new(false);
        assert_eq!(build(&[folder.clone()], &stop).0.files.len(), 1);
        folder.rules = Rules::new(&[], &[".git".into()], &[]).unwrap();
        assert!(build(&[folder], &stop).0.files.is_empty());
    }
    #[test]
    fn project_session_stages_are_hidden_from_the_sidebar_and_index() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "document.txt",
            ".velocity-save-old",
            ".velocity-session-16acf-crash",
        ] {
            std::fs::write(dir.path().join(name), "fixture").unwrap();
        }
        let folder = Folder {
            path: dir.path().into(),
            name: "root".into(),
            follow_links: false,
            rules: Rules::default(),
        };
        let stop = AtomicBool::new(false);
        let rows = folder.sidebar_children(dir.path(), &stop).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].path, dir.path().join("document.txt"));
        let (index, _) = build(&[folder], &stop);
        assert_eq!(index.files.len(), 1);
    }
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
            files,
            text::{Options, Search},
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        std::fs::write(&path, "cat\n").unwrap();
        let search = Search::new("cat", Options::default()).unwrap();
        // A legacy backup, or a recovery copy retained after a write error.
        let backup = dir.path().join("source.txt.velocity-backup-1");
        std::fs::write(&backup, "cat\n").unwrap();
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
                    assert_eq!(line.path.as_ref(), path);
                }
                files::Event::Error(error) => panic!("{error}"),
                files::Event::Done { matches, files } => {
                    assert_eq!((matches, files), (1, 1));
                    break;
                }
            }
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
