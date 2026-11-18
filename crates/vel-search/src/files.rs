//! Parallel grep-searcher traversal, streaming result lines and reviewable replacements.
use crate::{
    project::{Folder, Patterns, Rules},
    text::{Search, apply},
};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkState;
use std::{
    collections::BTreeSet,
    io,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, TrySendError},
    },
    thread,
    time::Duration,
};
use vel_text::Text;

/// Parsed Where field. Commas separate locations, include patterns and `-exclude` patterns.
#[derive(Clone, Debug, Default)]
pub struct Where {
    /// Explicit folders/files.
    pub roots: Vec<PathBuf>,
    /// Include current project/open folder roots.
    pub project: bool,
    /// Search live buffers instead of their on-disk contents.
    pub open_files: bool,
    /// Search the first live buffer supplied in the request (the active file).
    pub current_file: bool,
    include: Patterns,
    exclude: Patterns,
}
impl Where {
    /// Parses symbolic locations and name/path patterns; an empty location uses open folders.
    pub fn parse(input: &str, base: &Path) -> Result<Self, String> {
        let mut result = Self::default();
        let (mut include, mut exclude) = (Vec::new(), Vec::new());
        let mut quoted = false;
        let fields = input.split(|c| {
            if c == '"' {
                quoted = !quoted;
            }
            c == ',' && !quoted
        });
        for part in fields.map(str::trim).filter(|p| !p.is_empty()) {
            let part = part.trim_matches('"');
            match part {
                "<project>" | "<open folders>" => result.project = true,
                "<open files>" => result.open_files = true,
                "<current file>" => result.current_file = true,
                _ => {
                    if let Some(pattern) = part.strip_prefix('-') {
                        exclude.push(pattern.trim_matches('"').to_owned());
                    } else {
                        let path = Path::new(part);
                        let resolved = base.join(path);
                        if !part.starts_with("//")
                            && !part.contains(['*', '?'])
                            && (path.is_absolute()
                                || (part.contains('/')
                                    && !part.ends_with('/')
                                    && resolved.exists()))
                        {
                            result.roots.push(resolved);
                        } else {
                            include.push(part.to_owned());
                        }
                    }
                }
            }
        }
        // https://www.sublimetext.com/docs/file_patterns.html#uses includes the
        // Find in Files Where field; only its location and exclusion syntax is separate.
        result.include = Patterns::new(&include);
        result.exclude = Patterns::new(&exclude);
        result.project |= !result.open_files && !result.current_file && result.roots.is_empty();
        Ok(result)
    }
    /// Whether a directory and its complete subtree should be skipped by the walker.
    pub fn prunes(&self, path: &Path, root: &Path) -> bool {
        self.prunes_at(path, root, root)
    }
    /// Applies name/path patterns to the file and every containing directory.
    pub fn allows(&self, path: &Path, root: &Path) -> bool {
        self.allows_at(path, root, root)
    }
    fn prunes_at(&self, path: &Path, root: &Path, project_root: &Path) -> bool {
        self.exclude.matches_within(project_root, root, path, true)
    }
    fn allows_at(&self, path: &Path, root: &Path, project_root: &Path) -> bool {
        (self.include.is_empty() || self.include.matches_within(project_root, root, path, false))
            && !self.exclude.matches_within(project_root, root, path, false)
    }
}
/// A searchable live buffer. Path may be a user-facing Untitled caption.
#[derive(Clone, Debug)]
pub struct OpenFile {
    /// Display/jump identity.
    pub path: PathBuf,
    /// O(1) current buffer snapshot, including unsaved edits.
    pub text: Text,
}
/// Worker search description.
#[derive(Clone, Debug)]
pub struct Request {
    /// Compiled rope semantics; disk grep uses the same regex flags.
    pub search: Search,
    /// Where field.
    pub scope: Where,
    /// Ignore-aware project roots.
    pub folders: Vec<Folder>,
    /// Live buffers used by symbolic file locations; the active buffer must be first.
    pub open: Vec<OpenFile>,
    /// Context lines before/after each matching line.
    pub context: usize,
}
/// One result/context line with a stable jump location and highlight spans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Absolute file path/live-buffer identity.
    pub path: PathBuf,
    /// One-based line number.
    pub number: usize,
    /// Text without line endings.
    pub text: String,
    /// UTF-8 match columns; empty for context.
    pub highlights: Vec<Range<usize>>,
}
impl Line {
    /// Sublime-style line prefix. File headers are emitted by the result view on path changes.
    pub fn formatted(&self) -> String {
        format!(
            "  {:>5}{} {}\n",
            self.number,
            if self.highlights.is_empty() { ' ' } else { ':' },
            self.text
        )
    }
    /// One-based scalar column of the first match.
    pub fn column(&self) -> usize {
        self.highlights.first().map_or(1, |r| {
            self.text[..r.start.min(self.text.len())].chars().count() + 1
        })
    }
}
/// Device-free Find Results document: file headers, numbered context and match rows.
#[derive(Clone, Debug, Default)]
pub struct Results {
    /// Append-only display rows, ready for virtualization.
    pub rows: Vec<Row>,
    last: Option<(PathBuf, usize)>,
}
/// One Find Results display row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A file header; repeated when parallel streams switch files.
    Header(PathBuf),
    /// Nonadjacent context separator.
    Gap,
    /// Numbered match/context line and its jump target.
    Line(Line),
}
impl Results {
    /// Appends streaming lines without sorting/copying previous output.
    pub fn push(&mut self, line: Line) {
        if self
            .last
            .as_ref()
            .is_none_or(|(path, _)| *path != line.path)
        {
            self.rows.push(Row::Header(line.path.clone()));
        } else if self.last.as_ref().is_some_and(|(_, n)| line.number > n + 1) {
            self.rows.push(Row::Gap);
        }
        self.last = Some((line.path.clone(), line.number));
        self.rows.push(Row::Line(line));
    }
    /// Sublime-style plain-text representation, useful for copying/export/tests.
    pub fn formatted(&self) -> String {
        self.rows
            .iter()
            .map(|row| match row {
                Row::Header(path) => format!("\n{}:\n", path.display()),
                Row::Gap => "        …\n".into(),
                Row::Line(line) => line.formatted(),
            })
            .collect()
    }
}
/// Streaming events. Channels are bounded so the consumer controls memory growth.
#[derive(Debug)]
pub enum Event {
    /// One match or context line.
    Line(Line),
    /// Traversal/decoding/IO failure, never silently counted as a successful file.
    Error(String),
    /// Terminal event with matching lines and searched files.
    Done {
        /// Number of matching lines.
        matches: usize,
        /// Files actually searched.
        files: usize,
    },
}
fn send(tx: &mpsc::SyncSender<Event>, stop: &AtomicBool, mut event: Event) -> bool {
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(value)) => {
                event = value;
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
}
struct GrepSink<'a> {
    path: &'a Path,
    matcher: &'a RegexMatcher,
    tx: &'a mpsc::SyncSender<Event>,
    stop: &'a AtomicBool,
    matches: &'a AtomicUsize,
}
impl Sink for GrepSink<'_> {
    type Error = io::Error;
    fn matched(&mut self, _: &Searcher, found: &SinkMatch<'_>) -> io::Result<bool> {
        let text = std::str::from_utf8(found.bytes())
            .map_err(io::Error::other)?
            .trim_end_matches(['\n', '\r'])
            .to_owned();
        let mut highlights = Vec::new();
        self.matcher
            .find_iter(text.as_bytes(), |m| {
                highlights.push(m.start()..m.end());
                true
            })
            .map_err(io::Error::other)?;
        self.matches.fetch_add(1, Ordering::Relaxed);
        Ok(send(
            self.tx,
            self.stop,
            Event::Line(Line {
                path: self.path.into(),
                number: found.line_number().unwrap_or(1) as usize,
                text,
                highlights,
            }),
        ))
    }
    fn context(&mut self, _: &Searcher, context: &SinkContext<'_>) -> io::Result<bool> {
        let text = std::str::from_utf8(context.bytes())
            .map_err(io::Error::other)?
            .trim_end_matches(['\n', '\r'])
            .to_owned();
        Ok(send(
            self.tx,
            self.stop,
            Event::Line(Line {
                path: self.path.into(),
                number: context.line_number().unwrap_or(1) as usize,
                text,
                highlights: Vec::new(),
            }),
        ))
    }
}
/// Cancellable parallel file search. Dropping cancels blocked producers as well as walkers.
#[derive(Debug)]
pub struct Task {
    /// Bounded streaming channel (consume regularly).
    pub events: mpsc::Receiver<Event>,
    stop: Arc<AtomicBool>,
    searched: Arc<AtomicUsize>,
}
impl Task {
    /// Validates regex and spawns workers; no GUI or full-project result buffer is needed.
    pub fn start(request: Request) -> Result<Self, String> {
        if request.search.query.is_empty() {
            return Err("Find in Files needs a nonempty query".into());
        }
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!request.search.options.case_sensitive)
            .word(request.search.options.whole_word)
            .fixed_strings(!request.search.options.regex)
            .multi_line(true)
            .build(&request.search.query)
            .map_err(|e| e.to_string())?;
        let (tx, events) = mpsc::sync_channel(128);
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let searched = Arc::new(AtomicUsize::new(0));
        let files = searched.clone();
        thread::spawn(move || {
            let matches = AtomicUsize::new(0);
            let seen = std::sync::Mutex::new(BTreeSet::new());
            // Live buffers take precedence over on-disk copies when both scope tokens are present.
            if request.scope.open_files || request.scope.current_file {
                let count = if request.scope.open_files {
                    request.open.len()
                } else {
                    1
                };
                for open in request.open.iter().take(count) {
                    let root = request
                        .folders
                        .iter()
                        .find(|f| open.path.starts_with(&f.path))
                        .map_or(Path::new(""), |f| f.path.as_path());
                    if !request.scope.allows(&open.path, root) {
                        continue;
                    }
                    seen.lock().expect("seen").insert(open.path.clone());
                    files.fetch_add(1, Ordering::Relaxed);
                    let hits = request.search.find(&open.text, &[]);
                    let mut rows = BTreeSet::new();
                    for hit in &hits {
                        let row = open.text.byte_to_line(hit.start);
                        for row in row.saturating_sub(request.context)
                            ..=(row + request.context).min(open.text.len_lines() - 1)
                        {
                            rows.insert(row);
                        }
                    }
                    for row in rows {
                        let start = open.text.line_to_byte(row);
                        let text = open.text.line(row).to_string();
                        let highlights: Vec<_> = hits
                            .iter()
                            .filter(|h| h.start >= start && h.start <= start + text.len())
                            .map(|h| h.start - start..(h.end - start).min(text.len()))
                            .collect();
                        if !highlights.is_empty() {
                            matches.fetch_add(1, Ordering::Relaxed);
                        }
                        if !send(
                            &tx,
                            &cancelled,
                            Event::Line(Line {
                                path: open.path.clone(),
                                number: row + 1,
                                text,
                                highlights,
                            }),
                        ) {
                            return;
                        }
                    }
                }
            }
            let mut folders = if request.scope.project {
                request.folders.clone()
            } else {
                Vec::new()
            };
            for path in &request.scope.roots {
                let rules = request
                    .folders
                    .iter()
                    .find(|f| path.starts_with(&f.path))
                    .map_or_else(Rules::default, |f| f.rules.clone());
                folders.push(Folder {
                    path: path.clone(),
                    name: path.display().to_string(),
                    rules,
                    follow_links: false,
                });
            }
            for folder in folders {
                folder
                    .walker(&folder.path, None, true)
                    .build_parallel()
                    .run(|| {
                        let mut searcher = SearcherBuilder::new()
                            .line_number(true)
                            .before_context(request.context)
                            .after_context(request.context)
                            .binary_detection(BinaryDetection::quit(b'\0'))
                            .build();
                        let tx = &tx;
                        let stop = &cancelled;
                        let scope = &request.scope;
                        let matcher = &matcher;
                        let seen = &seen;
                        let matches = &matches;
                        let files = &files;
                        let root = if folder.path.is_file() {
                            folder.path.parent().unwrap_or(Path::new(""))
                        } else {
                            folder.path.as_path()
                        };
                        let project_root = request
                            .folders
                            .iter()
                            .filter(|project| folder.path.starts_with(&project.path))
                            .max_by_key(|project| project.path.components().count())
                            .map_or(root, |project| project.path.as_path());
                        Box::new(move |result| {
                            if stop.load(Ordering::Relaxed) {
                                return WalkState::Quit;
                            }
                            match result {
                                Ok(entry)
                                    if entry.depth() > 0
                                        && entry.file_type().is_some_and(|t| t.is_dir())
                                        && scope.prunes_at(entry.path(), root, project_root) =>
                                {
                                    return WalkState::Skip;
                                }
                                Ok(entry)
                                    if entry.file_type().is_some_and(|t| t.is_file())
                                        && scope.allows_at(entry.path(), root, project_root) =>
                                {
                                    if !seen.lock().expect("seen").insert(entry.path().into()) {
                                        return WalkState::Continue;
                                    }
                                    files.fetch_add(1, Ordering::Relaxed);
                                    let sink = GrepSink {
                                        path: entry.path(),
                                        matcher,
                                        tx,
                                        stop,
                                        matches,
                                    };
                                    if let Err(e) =
                                        searcher.search_path(matcher, entry.path(), sink)
                                    {
                                        send(
                                            tx,
                                            stop,
                                            Event::Error(format!(
                                                "{}: {e}",
                                                entry.path().display()
                                            )),
                                        );
                                    }
                                }
                                Err(e) => {
                                    send(tx, stop, Event::Error(e.to_string()));
                                }
                                _ => {}
                            }
                            WalkState::Continue
                        })
                    });
            }
            send(
                &tx,
                &cancelled,
                Event::Done {
                    matches: matches.load(Ordering::Relaxed),
                    files: files.load(Ordering::Relaxed),
                },
            );
        });
        Ok(Self {
            events,
            stop,
            searched,
        })
    }
    /// Number of live/disk files visited so far, without blocking the UI consumer.
    pub fn searched_files(&self) -> usize {
        self.searched.load(Ordering::Relaxed)
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// A read-only prepared change. Confirmation belongs to the native application.
#[derive(Debug)]
pub struct FileChange {
    /// Original file.
    pub path: PathBuf,
    before: Text,
    format: vel_fileio::Format,
    after: Text,
    /// Number of replacements to show before confirmation.
    pub count: usize,
}
impl FileChange {
    /// Prepares replacements without writing. The application confirms the complete plan first.
    pub fn prepare(path: PathBuf, search: &Search, replacement: &str) -> Result<Self, String> {
        let loaded = vel_fileio::load(&path, "Western (Windows 1252)", |_| {})?;
        let replacements = search.replacements(&loaded.text, replacement, &[]);
        let mut buffer = vel_buffer::Buffer::new(loaded.text.clone());
        let view = buffer.new_view();
        apply(&mut buffer, view, &replacements);
        Ok(Self {
            path,
            before: loaded.text,
            format: loaded.format,
            after: buffer.text().clone(),
            count: replacements.len(),
        })
    }
    /// After confirmation, rechecks original content/format, backs up original bytes, then saves atomically.
    pub fn commit(self) -> Result<PathBuf, String> {
        let current = vel_fileio::load(&self.path, "Western (Windows 1252)", |_| {})?;
        if current.text != self.before || current.format != self.format {
            return Err(format!(
                "{} changed after replacement preview",
                self.path.display()
            ));
        }
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let backup = self.path.with_file_name(format!(
            "{}.velocity-backup-{suffix}",
            self.path.file_name().unwrap_or_default().to_string_lossy()
        ));
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
            .map_err(|e| e.to_string())?;
        let mut source = std::fs::File::open(&self.path).map_err(|e| e.to_string())?;
        io::copy(&mut source, &mut target).map_err(|e| e.to_string())?;
        target.sync_all().map_err(|e| e.to_string())?;
        vel_fileio::save(&self.path, &self.after, &self.format, true)?;
        Ok(backup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn where_stream_format_context_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.rs");
        std::fs::write(&path, "before\nhello cat\nafter\n").unwrap();
        let search = Search::new("cat", crate::text::Options::default()).unwrap();
        let scope = Where::parse("<project>, *.rs, -ignored*", dir.path()).unwrap();
        let folder = Folder {
            path: dir.path().into(),
            name: "root".into(),
            rules: Rules::default(),
            follow_links: false,
        };
        let task = Task::start(Request {
            search: search.clone(),
            scope,
            folders: vec![folder],
            open: vec![],
            context: 1,
        })
        .unwrap();
        let mut lines = Vec::new();
        loop {
            match task.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::Line(line) => lines.push(line),
                Event::Error(e) => panic!("{e}"),
                Event::Done { matches, files } => {
                    assert_eq!((matches, files), (1, 1));
                    break;
                }
            }
        }
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].formatted(), "      2: hello cat\n");
        assert_eq!(lines[1].highlights, std::slice::from_ref(&(6..9)));
        let mut view = Results::default();
        for line in &lines {
            view.push(line.clone());
        }
        assert_eq!(
            view.formatted(),
            format!(
                "\n{}:\n      1  before\n      2: hello cat\n      3  after\n",
                path.display()
            )
        );
        let plan = FileChange::prepare(path.clone(), &search, "dog").unwrap();
        assert_eq!(plan.count, 1);
        let backup = plan.commit().unwrap();
        assert!(std::fs::read_to_string(backup).unwrap().contains("cat"));
        assert!(std::fs::read_to_string(&path).unwrap().contains("dog"));
        let plan = FileChange::prepare(path.clone(), &search, "x").unwrap();
        std::fs::write(path, "new contents").unwrap();
        assert!(plan.commit().is_err());
    }
    #[test]
    fn open_file_scope_uses_unsaved_text_and_exclude_patterns() {
        let scope = Where::parse("<open files>, *.rs, -skip*", Path::new(".")).unwrap();
        let search = Search::new("changed", crate::text::Options::default()).unwrap();
        let task = Task::start(Request {
            search,
            scope,
            folders: vec![],
            open: vec![
                OpenFile {
                    path: "unsaved.rs".into(),
                    text: Text::from("changed in memory"),
                },
                OpenFile {
                    path: "skip.rs".into(),
                    text: Text::from("changed too"),
                },
            ],
            context: 0,
        })
        .unwrap();
        let mut found = 0;
        loop {
            match task.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::Line(line) => {
                    assert_eq!(line.path, Path::new("unsaved.rs"));
                    found += 1;
                }
                Event::Done { matches, files } => {
                    assert_eq!((matches, files, found), (1, 1, 1));
                    break;
                }
                Event::Error(e) => panic!("{e}"),
            }
        }
    }
    #[test]
    fn where_symbolic_locations_names_paths_and_directory_pruning() {
        let root = Path::new("/project");
        for token in ["", "<project>", "<open folders>"] {
            let scope = Where::parse(token, root).unwrap();
            assert!(scope.project);
            assert!(scope.roots.is_empty());
        }
        for (token, open, current) in [
            ("<open files>", true, false),
            ("<current file>", false, true),
            ("<current file>, <open files>", true, true),
        ] {
            let scope = Where::parse(token, root).unwrap();
            assert_eq!(
                (scope.open_files, scope.current_file, scope.project),
                (open, current, false)
            );
        }
        for token in ["-node_modules", "-node_modules/"] {
            let scope = Where::parse(token, root).unwrap();
            assert!(scope.prunes(Path::new("/project/sub/node_modules"), root));
            assert!(!scope.allows(Path::new("/project/sub/node_modules/pkg/file.rs"), root));
            assert!(scope.allows(Path::new("/project/sub/node_modules_other/file.rs"), root));
        }
        for (pattern, path, expected) in [
            ("src", "/project/sub/src/file.rs", true),
            ("src", "/project/src", true),
            ("src/", "/project/src", false),
            ("src/", "/project/src/file.rs", true),
            ("//src/", "/project/src/file.rs", true),
            ("//src/", "/project/sub/src/file.rs", false),
            ("//*.rs", "/project/file.rs", true),
            ("//*.rs", "/project/sub/file.rs", true),
            ("sub/*.rs", "/project/sub/deep/file.rs", true),
            ("*.rs", "/project/sub/file.rs", true),
            (
                "//src, -//src/vendor/",
                "/project/src/vendor/file.rs",
                false,
            ),
            ("//src, -//src/vendor/", "/project/src/file.rs", true),
        ] {
            assert_eq!(
                Where::parse(pattern, root)
                    .unwrap()
                    .allows(Path::new(path), root),
                expected,
                "{pattern}: {path}"
            );
        }
        let absolute = Where::parse("/project/src/*.rs", root).unwrap();
        assert!(absolute.allows(Path::new("/project/src/file.rs"), root));
        assert!(!absolute.allows(Path::new("/other/project/src/file.rs"), root));
    }
    #[test]
    fn where_walk_excludes_entire_named_directories_and_explicit_root_spaces() {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "src/one.rs",
            "src/deep/two.rs",
            "src/node_modules/no.rs",
            "nested/node_modules/no.rs",
            "node_modules/no.rs",
            "other/no.txt",
        ] {
            let file = dir.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "needle").unwrap();
        }
        for exclude in ["-node_modules", "-node_modules/"] {
            let task = Task::start(Request {
                search: Search::new("needle", crate::text::Options::default()).unwrap(),
                scope: Where::parse(&format!("<open folders>, *.rs, {exclude}"), dir.path())
                    .unwrap(),
                folders: vec![Folder {
                    path: dir.path().into(),
                    name: "root".into(),
                    rules: Rules::default(),
                    follow_links: false,
                }],
                open: vec![],
                context: 0,
            })
            .unwrap();
            let mut found = Vec::new();
            loop {
                match task.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    Event::Line(line) => found.push(line.path),
                    Event::Error(e) => panic!("{e}"),
                    Event::Done { matches, files } => {
                        assert_eq!((matches, files), (2, 2));
                        break;
                    }
                }
            }
            assert_eq!(found.len(), 2);
            assert_eq!(task.searched_files(), 2);
        }
        let path = dir.path().join("folder with spaces");
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            Where::parse(&format!("\"{}\"", path.display()), dir.path())
                .unwrap()
                .roots,
            [path]
        );
    }
    #[test]
    fn where_path_patterns_are_suffix_anchored_and_name_filters_skip_the_root() {
        let root = Path::new("/Users/me/src/app");
        for (pattern, path, expected) in [
            ("src/*.rs", "/Users/me/src/app/main.rs", true),
            ("src/*.rs", "/Users/me/src/app/src/main.rs", true),
            ("src/*.rs", "/Users/me/src/app/nested/src/main.rs", true),
            ("-src/*.rs", "/Users/me/src/app/main.rs", false),
            ("-src/*.rs", "/Users/me/src/app/src/main.rs", false),
            ("-me/*", "/Users/me/src/app/main.rs", false),
        ] {
            assert_eq!(
                Where::parse(pattern, root)
                    .unwrap()
                    .allows(Path::new(path), root),
                expected,
                "{pattern}: {path}"
            );
        }
        assert!(!Where::parse("-me/*", root).unwrap().prunes(root, root));
        let hidden = Path::new("/Users/me/.dotfiles");
        let scope = Where::parse("-.*", hidden).unwrap();
        assert!(!scope.prunes(hidden, hidden));
        assert!(scope.allows(&hidden.join("main.rs"), hidden));
        assert!(!scope.allows(&hidden.join(".hidden/main.rs"), hidden));
        // Explicit nested search roots and // project anchors have distinct meanings.
        let project = Path::new("/Users/me");
        assert!(Where::parse("//src/app/*.rs", project).unwrap().allows_at(
            &root.join("main.rs"),
            root,
            project
        ));
        assert!(Where::parse("src/app/*.rs", project).unwrap().allows_at(
            &root.join("main.rs"),
            root,
            project
        ));
    }
    #[test]
    fn hidden_search_root_is_walked_with_full_path_and_project_relative_filters() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("src/.dotfiles");
        for name in ["one.rs", "src/two.rs", ".hidden/no.rs"] {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "needle").unwrap();
        }
        for (patterns, expected) in [
            ("-.*", 2),
            ("src/*.rs", 3),
            ("-src/*.rs, -.*", 0),
            ("-src/*", 0),
            ("//src/.dotfiles/src/*.rs", 1),
        ] {
            let task = Task::start(Request {
                search: Search::new("needle", crate::text::Options::default()).unwrap(),
                scope: Where::parse(&format!("{}, {patterns}", root.display()), dir.path())
                    .unwrap(),
                folders: vec![Folder {
                    path: dir.path().into(),
                    name: "project".into(),
                    rules: Rules::default(),
                    follow_links: false,
                }],
                open: vec![],
                context: 0,
            })
            .unwrap();
            let mut found = 0;
            loop {
                match task.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                    Event::Line(_) => found += 1,
                    Event::Error(error) => panic!("{error}"),
                    Event::Done { matches, files } => {
                        assert_eq!(
                            (found, matches, files),
                            (expected, expected, expected),
                            "{patterns}"
                        );
                        break;
                    }
                }
            }
        }
    }
}
