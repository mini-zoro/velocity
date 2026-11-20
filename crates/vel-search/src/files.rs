//! Parallel file traversal, streaming result lines and reviewable replacements.
use crate::{
    project::{Folder, Patterns, Rules},
    text::{Search, apply},
};
use ignore::WalkState;
use std::{
    collections::{BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    io,
    ops::Range,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, TrySendError},
    },
    thread,
    time::Duration,
};
use vel_fileio::DecodeOptions;
use vel_text::Text;
mod multiline;
mod ordered;
pub use ordered::Rows;

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
    /// Filter disk traversal by Git's ignore rules; explicit live scopes remain searchable.
    pub use_gitignore: bool,
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
    pub path: Arc<Path>,
    /// One-based line number.
    pub number: usize,
    /// Text without line endings.
    pub text: String,
    /// UTF-8 match columns; empty for context.
    pub highlights: Vec<Range<usize>>,
    /// Scalar columns skipped before the bounded text preview.
    pub column_offset: usize,
    /// All matches starting on this line, including outside its preview.
    pub match_count: usize,
}
impl Line {
    /// Sublime-style line prefix. File headers are emitted by the result view on path changes.
    pub fn formatted(&self) -> String {
        format!(
            "  {:>5}{} {}\n",
            self.number,
            if self.match_count == 0 { ' ' } else { ':' },
            self.text
        )
    }
    /// One-based scalar column of the first match.
    pub fn column(&self) -> usize {
        self.column_offset
            + self.highlights.first().map_or(1, |r| {
                self.text[..r.start.min(self.text.len())].chars().count() + 1
            })
    }
}
/// Device-free Find Results document: file headers, numbered context and match rows.
#[derive(Clone, Debug, Default)]
pub struct Results {
    /// Ordered display rows, ready for virtualization.
    pub rows: Rows,
    roots: Vec<PathBuf>,
    last_file: Option<u32>,
    key: Vec<u8>,
    parent_keys: HashMap<OsString, u16>,
}
/// The rows added by one streamed line. Resolve the rank only when a selection
/// or navigation cursor needs to survive this insertion.
#[derive(Clone, Copy, Debug)]
pub struct Insertion {
    file: u32,
    offset: usize,
    /// Number of new display rows (including a header or gap if needed).
    pub added: usize,
}
/// One Find Results display row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row<'a> {
    /// One header per file.
    Header(&'a Path),
    /// Nonadjacent context separator.
    Gap,
    /// Numbered match/context line and its jump target.
    Line(&'a Line),
}
impl Results {
    /// Starts ordered output using the project's folder order.
    pub fn ordered(folders: &[Folder]) -> Self {
        Self {
            roots: folders.iter().map(|f| f.path.clone()).collect(),
            ..Self::default()
        }
    }
    /// Inserts a streaming line. The display index is available through `rank`.
    /// Each worker publishes an individual file in line order.
    pub fn push(&mut self, line: Line) -> Insertion {
        // https://www.sublimetext.com/download (sorted Find in Files results).
        let known = self.last_file.filter(|&i| {
            let path = self.rows.path(i);
            Arc::ptr_eq(path, &line.path) || path.as_os_str() == line.path.as_os_str()
        });
        self.key.clear();
        let prefix = if let Some(file) = known {
            self.rows.copy_key(file, &mut self.key)
        } else {
            self.order_key(&line.path)
        };
        let inserted = self.rows.push(&self.key, prefix, line, known);
        self.last_file = Some(inserted.file);
        inserted
    }
    /// The first inserted display row's current rank, for a caller preserving selection.
    pub fn rank(&self, inserted: Insertion) -> usize {
        self.rows.rank(inserted)
    }
    /// Number of distinct files which have published a match, not just context.
    pub fn matched_files(&self) -> usize {
        self.rows.matched
    }

    fn order_key(&mut self, path: &Path) -> u16 {
        if let Some(parent) = path.parent()
            && let Some(name) = path.file_name()
            && let Some(&prefix) = self.parent_keys.get(parent.as_os_str())
            // The root itself uses a shorter relative key if the scope is one file.
            && !self.roots.iter().any(|root| root.as_os_str() == path.as_os_str())
        {
            self.key.extend_from_slice(self.rows.prefix(prefix));
            append_name(&mut self.key, name);
            return prefix;
        }
        let (root, relative) = self
            .roots
            .iter()
            .enumerate()
            .find_map(|(i, p)| path.strip_prefix(p).ok().map(|relative| (i, relative)))
            .unwrap_or((self.roots.len(), path));
        self.key.extend_from_slice(&(root as u32).to_be_bytes());
        let mut parts = relative.components().peekable();
        let mut prefix = 0;
        while let Some(part) = parts.next() {
            let directory = parts.peek().is_some();
            if !directory
                && path.file_name() == Some(part.as_os_str())
                && let Some(parent) = path.parent()
            {
                // Bound cold directory keys. A separate allocation is shared by
                // each cached directory, independent of its number of files.
                if self.parent_keys.len() < 4096 {
                    self.key.push(1);
                    prefix = self.rows.push_prefix(&self.key);
                    self.parent_keys
                        .insert(parent.as_os_str().to_owned(), prefix);
                    append_name(&mut self.key, part.as_os_str());
                    break;
                }
            }
            self.key.push(u8::from(!directory));
            append_name(&mut self.key, part.as_os_str());
        }
        prefix
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

fn append_name(key: &mut Vec<u8>, name: &OsStr) {
    // Same keys as project::sidebar_key: dirs first, lowercase then original.
    // NUL is not a valid path component byte. Delimit both names so a prefix
    // (e.g. "a" before "ab") sorts correctly even across component boundaries.
    let bytes = name.as_encoded_bytes();
    if bytes.is_ascii() {
        let start = key.len();
        key.extend_from_slice(bytes);
        key[start..].make_ascii_lowercase();
    } else {
        key.extend_from_slice(name.to_string_lossy().to_lowercase().as_bytes());
    }
    key.push(0);
    key.extend_from_slice(bytes);
    key.push(0);
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
fn send_text_lines(
    tx: &mpsc::SyncSender<Event>,
    stop: &AtomicBool,
    matches: &AtomicUsize,
    search: &Search,
    path: &Path,
    text: &Text,
    context: usize,
) -> bool {
    let hits = search.find(text, &[]);
    let path: Arc<Path> = path.into();
    let mut rows = BTreeSet::new();
    for hit in &hits {
        let first = text.byte_to_line(hit.start);
        let last = text.byte_to_line(hit.end.saturating_sub(1).max(hit.start));
        rows.extend(first.saturating_sub(context)..=(last + context).min(text.len_lines() - 1));
    }
    for row in rows {
        let start = text.line_to_byte(row);
        let len = text.line_len_bytes(row);
        let end = if row + 1 < text.len_lines() {
            text.line_to_byte(row + 1)
        } else {
            text.len_bytes() + 1
        };
        let highlights: Vec<_> = hits[hits.partition_point(|h| h.start < start)..]
            .iter()
            .take_while(|h| h.start < end)
            .map(|h| (h.start - start).min(len)..(h.end - start).min(len))
            .collect();
        let match_count = highlights.len();
        matches.fetch_add(match_count, Ordering::Relaxed);
        if !send(
            tx,
            stop,
            Event::Line(Line {
                path: path.clone(),
                number: row + 1,
                text: text.slice(start..start + len).to_string(),
                highlights,
                column_offset: 0,
                match_count,
            }),
        ) {
            return false;
        }
    }
    true
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
        Self::start_with_decode(request, DecodeOptions::default())
    }

    /// Runs a search using the automatic decode policy of the owning window.
    pub fn start_with_decode(request: Request, decode: DecodeOptions) -> Result<Self, String> {
        if request.search.query.is_empty() {
            return Err("Find in Files needs a nonempty query".into());
        }
        let cross_line = multiline::Multiline::for_search(&request.search);
        let greps = cross_line
            .is_none()
            .then(|| multiline::Greps::new(&request.search))
            .flatten();
        let required = multiline::Prefilter::new(&request.search);
        let contiguous = request.search.contiguous()?;
        let (tx, events) = mpsc::sync_channel(128);
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let searched = Arc::new(AtomicUsize::new(0));
        let files = searched.clone();
        thread::spawn(move || {
            let matches = AtomicUsize::new(0);
            let result = vel_crash::catch_recoverable(AssertUnwindSafe(|| {
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
                        if !send_text_lines(
                            &tx,
                            &cancelled,
                            &matches,
                            &request.search,
                            &open.path,
                            &open.text,
                            request.context,
                        ) {
                            return;
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
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    folder
                        .walker_with_gitignore(
                            &folder.path,
                            None,
                            true,
                            request.scope.use_gitignore,
                        )
                        .build_parallel()
                        .run(|| {
                            let mut reader = vel_fileio::search::Reader::default();
                            let contiguous = contiguous.clone();
                            let cross_line = cross_line.as_ref();
                            let greps = greps.as_ref();
                            let required = required.as_ref();
                            let decode = &decode;
                            let tx = &tx;
                            let stop = &cancelled;
                            let scope = &request.scope;
                            let seen = &seen;
                            let matches = &matches;
                            let files = &files;
                            let search = &request.search;
                            let context = request.context;
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
                                            && scope.prunes_at(
                                                entry.path(),
                                                root,
                                                project_root,
                                            ) =>
                                    {
                                        return WalkState::Skip;
                                    }
                                    Ok(entry)
                                        if entry.file_type().is_some_and(|t| t.is_file())
                                            && scope.allows_at(
                                                entry.path(),
                                                root,
                                                project_root,
                                            ) =>
                                    {
                                        if !seen.lock().expect("seen").insert(entry.path().into()) {
                                            return WalkState::Continue;
                                        }
                                        files.fetch_add(1, Ordering::Relaxed);
                                        let result =
                                            vel_crash::catch_recoverable(AssertUnwindSafe(|| {
                                                let mut contains = |bytes: &[u8]| {
                                                    required
                                                        .is_none_or(|filter| filter.contains(bytes))
                                                };
                                                let filter = required.map(|filter| {
                                                    vel_fileio::search::Filter {
                                                        width: filter.width(),
                                                        needs_lf: filter.needs_lf(),
                                                        line_local: cross_line.is_none(),
                                                        contains: &mut contains,
                                                    }
                                                });
                                                reader
                                                    .open_with_options(
                                                        entry.path(),
                                                        decode,
                                                        || stop.load(Ordering::Relaxed),
                                                        filter,
                                                    )
                                                    .and_then(|input| {
                                                        if let Some(input) = input {
                                                            let query = multiline::Query {
                                                                search,
                                                                regex: &contiguous,
                                                                multiline: cross_line,
                                                            };
                                                            multiline::scan_prepared(
                                                                input,
                                                                entry.path(),
                                                                query,
                                                                greps,
                                                                context,
                                                                stop,
                                                                |line| {
                                                                    matches.fetch_add(
                                                                        line.match_count,
                                                                        Ordering::Relaxed,
                                                                    );
                                                                    send(
                                                                        tx,
                                                                        stop,
                                                                        Event::Line(line),
                                                                    )
                                                                },
                                                            )?;
                                                        }
                                                        Ok(())
                                                    })
                                            }))
                                            .unwrap_or_else(|_| {
                                                Err(io::Error::other("search worker failed"))
                                            });
                                        if let Err(e) = result {
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
            }));
            if result.is_err() {
                send(
                    &tx,
                    &cancelled,
                    Event::Error("file traversal failed".into()),
                );
            }
            // Cancelled producers stop filling the queue; the terminal event
            // still has to reach a live consumer. Dropping it disconnects this send.
            let _ = tx.send(Event::Done {
                matches: matches.load(Ordering::Relaxed),
                files: files.load(Ordering::Relaxed),
            });
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
    /// Stops the workers. A consumer that keeps draining the queue receives Done.
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Release);
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel();
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
    identity: Option<vel_fileio::ContentIdentity>,
    decode: DecodeOptions,
    edits: Vec<crate::text::Replacement>,
    /// Number of replacements to show before confirmation.
    pub count: usize,
}
/// A completed write, with any non-fatal recovery cleanup error.
#[derive(Debug)]
pub struct Committed {
    /// Raw bytes written by the shared save helper.
    pub identity: vel_fileio::ContentIdentity,
    /// The file was saved even if its recovery copy could not be removed.
    pub warning: Option<String>,
}
impl FileChange {
    /// Prepares replacements without writing. The application confirms the complete plan first.
    pub fn prepare(path: PathBuf, search: &Search, replacement: &str) -> Result<Self, String> {
        Self::prepare_with_decode(path, search, replacement, &DecodeOptions::default())
    }
    /// Prepares and later rechecks the file with the owning window's decode options.
    pub fn prepare_with_decode(
        path: PathBuf,
        search: &Search,
        replacement: &str,
        decode: &DecodeOptions,
    ) -> Result<Self, String> {
        let loaded = vel_fileio::load_with_options(&path, decode, |_| {})?;
        let replacements = search.replacements(&loaded.text, replacement, &[]);
        let mut buffer = vel_buffer::Buffer::new(loaded.text.clone());
        let view = buffer.new_view();
        apply(&mut buffer, view, &replacements);
        Ok(Self {
            path,
            before: loaded.text,
            format: loaded.format,
            after: buffer.text().clone(),
            identity: loaded.disk_content,
            decode: decode.clone(),
            count: replacements.len(),
            edits: replacements,
        })
    }
    /// Original decoded text, for matching an already-open clean buffer.
    pub fn before(&self) -> &Text {
        &self.before
    }
    /// The original-coordinate edits, for undo in an open buffer.
    pub fn edits(&self) -> &[crate::text::Replacement] {
        &self.edits
    }
    /// Rechecks the preview and keeps the raw original until saving succeeds.
    pub fn commit(&self) -> Result<Committed, String> {
        self.commit_with(vel_fileio::save_with_identity)
    }
    fn commit_with(
        &self,
        save: impl FnOnce(
            &Path,
            &Text,
            &vel_fileio::Format,
            bool,
        ) -> Result<vel_fileio::ContentIdentity, String>,
    ) -> Result<Committed, String> {
        let current = vel_fileio::load_with_options(&self.path, &self.decode, |_| {})?;
        if current.disk_content != self.identity
            || current.text != self.before
            || current.format != self.format
        {
            return Err(format!(
                "{} changed after replacement preview",
                self.path.display()
            ));
        }
        // https://www.sublimetext.com/3 (adaptive atomic saves). The current
        // shared helper may fall back to in-place writes, e.g. for hard links.
        // Keep exact original bytes on any write failure, but no sidecar on success.
        let dir = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let error = |e: io::Error| format!("{}: {e}", self.path.display());
        let filename = self.path.file_name().unwrap_or_default().to_string_lossy();
        let suffix = format!(
            "-{}",
            &filename[..filename.floor_char_boundary(filename.len().min(180))]
        );
        let mut temporary = tempfile::Builder::new()
            .prefix(".velocity-backup-")
            .suffix(&suffix)
            .tempfile_in(dir)
            .map_err(error)?;
        let mut source = vel_fileio::regular::read(&self.path)?;
        io::copy(&mut source, &mut temporary).map_err(error)?;
        let original = same_file::Handle::from_file(source).map_err(error)?;
        if vel_fileio::content_identity(temporary.path())? != self.identity {
            return Err(format!(
                "{} changed during replacement recovery copy",
                self.path.display()
            ));
        }
        temporary.as_file().sync_all().map_err(error)?;
        #[cfg(unix)]
        std::fs::File::open(dir)
            .and_then(|dir| dir.sync_all())
            .map_err(error)?;
        let (file, recovery) = temporary.keep().map_err(|e| error(e.error))?;
        drop(file);
        let identity = match save(&self.path, &self.after, &self.format, true) {
            Ok(identity) => identity,
            Err(e) => {
                // An open/encoding error can leave the original completely
                // untouched. Retain the copy only if that cannot be proved.
                let same_target = || {
                    vel_fileio::regular::read(&self.path)
                        .ok()
                        .and_then(|file| same_file::Handle::from_file(file).ok())
                        .is_some_and(|current| current == original)
                };
                if same_target()
                    && matches!(vel_fileio::content_identity(&self.path), Ok(Some(id)) if Some(id) == self.identity)
                    && same_target()
                {
                    return match std::fs::remove_file(&recovery) {
                        Ok(()) => Err(e),
                        Err(error) => Err(format!(
                            "{e}; could not remove {}: {error}",
                            recovery.display()
                        )),
                    };
                }
                return Err(format!("{e}; original retained at {}", recovery.display()));
            }
        };
        let warning = std::fs::remove_file(&recovery)
            .err()
            .map(|e| format!("could not remove {}: {e}", recovery.display()));
        Ok(Committed { identity, warning })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_mutation_and_cancellation_always_send_done_without_crash_reports() {
        const NAME: &str =
            "files::tests::task_mutation_and_cancellation_always_send_done_without_crash_reports";
        const CHILD: &str = "VELOCITY_TEST_SEARCH_CRASH_HOME";
        let Some(home) = std::env::var_os(CHILD) else {
            let home = tempfile::tempdir().unwrap();
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env(CHILD, home.path())
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            return;
        };
        let home = Path::new(&home);
        let reporter =
            vel_crash::CrashReporter::new(vel_crash::Config::for_home(home, "test")).unwrap();
        reporter.install_panic_hook();
        // Verify the same process-wide hook that the App installs.
        assert!(vel_crash::catch_recoverable(|| panic!("contained search panic")).is_err());
        let dir = tempfile::tempdir_in(home).unwrap();
        let root = dir.path();
        let file = root.join("source.txt");
        let text = "alpha gamma needle\nbeta\n".repeat(230_000);
        let mut rng = 357_u64;
        for (n, query) in [r"needle\nbeta", r"\bgam+a\b", r"(?s)alpha.*"]
            .into_iter()
            .cycle()
            .take(12)
            .enumerate()
        {
            std::fs::write(&file, &text).unwrap();
            let task = Task::start(Request {
                search: Search::new(
                    query,
                    crate::text::Options {
                        regex: true,
                        ..Default::default()
                    },
                )
                .unwrap(),
                scope: Where::parse("", root).unwrap(),
                folders: vec![Folder {
                    path: root.into(),
                    name: "fixture".into(),
                    rules: Rules::default(),
                    follow_links: false,
                }],
                open: vec![],
                context: 0,
            })
            .unwrap();
            let timeout = Duration::from_secs(10);
            assert!(matches!(
                task.events.recv_timeout(timeout),
                Ok(Event::Line(_))
            ));
            // The bounded event queue prevents a busy scan from finishing.
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            thread::sleep(Duration::from_micros(rng >> 53));
            if n % 4 < 2 {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&file)
                    .unwrap()
                    .set_len(130_000)
                    .unwrap();
            } else {
                use std::io::{Seek, SeekFrom, Write};
                let mut fd = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
                fd.seek(SeekFrom::Start(150_000)).unwrap();
                fd.write_all(b"replaced in-place").unwrap();
            }
            let cancelled = n % 2 == 0;
            if cancelled {
                task.cancel();
            }
            let mut errors = Vec::new();
            loop {
                match task
                    .events
                    .recv_timeout(timeout)
                    .expect("search must finish")
                {
                    Event::Line(_) => {}
                    Event::Error(error) => errors.push(error),
                    Event::Done { files, .. } => {
                        assert_eq!(files, 1);
                        break;
                    }
                }
            }
            if !cancelled {
                assert_eq!(errors.len(), 1, "{query}, {errors:?}");
                assert!(
                    errors[0].contains("file changed during search"),
                    "{errors:?}"
                );
            }
            assert!(reporter.pending_reports().unwrap().is_empty());
        }
    }

    #[test]
    fn line_local_search_across_large_batches_preserves_anchors_and_context() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("large.txt");
        // A hit near a block boundary followed by mixed editor line endings;
        // the two absolute anchors must not reappear at later batch boundaries.
        let padding = "filler\r\n".repeat(16_382);
        let mut text = format!("start\r\n{padding}return\rnext\r\n");
        text += &padding.repeat(34);
        text += "last\r\n";
        std::fs::write(&file, text).unwrap();
        let search = Search::new(
            r"\Astart|^ret\w*n\b|last$|\z",
            crate::text::Options {
                regex: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut reader = vel_fileio::search::Reader::default();
        let input = reader.open(&file, || false).unwrap().unwrap();
        assert!(matches!(input, vel_fileio::search::Text::Windows(_)));
        let mut got = Vec::new();
        multiline::scan(
            input,
            &file,
            multiline::Query {
                search: &search,
                regex: &search.contiguous().unwrap(),
                multiline: None,
            },
            1,
            &AtomicBool::new(false),
            |line| {
                got.push((line.number, line.text, line.match_count));
                true
            },
        )
        .unwrap();
        assert_eq!(
            got,
            [
                (1, "start".into(), 1),
                (2, "filler".into(), 0),
                (16_383, "filler".into(), 0),
                (16_384, "return".into(), 1),
                (16_385, "next".into(), 0),
                (16_385 + 34 * 16_382, "filler".into(), 0),
                (16_386 + 34 * 16_382, "last".into(), 1),
            ],
        );
    }

    #[test]
    fn grep_with_context_keeps_every_match_around_several_long_lines() {
        // Each long line stays below the bounded-cursor threshold, so the file takes the
        // grep path; with context the searcher also keeps preceding lines, which once hit
        // a fixed 8 MiB heap limit and silently dropped the later matches.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long-lines.txt");
        let mut text = String::new();
        for i in 0..300 {
            text += &if i % 5 == 1 {
                format!("café {i} return value\n")
            } else {
                format!("short {i} here\n")
            };
        }
        let long = "abé ".repeat(2_800_000 / 5);
        for _ in 0..3 {
            text += &long;
            text.push('\n');
        }
        for i in 300..900 {
            text += &if i % 5 == 1 {
                format!("café {i} return value\n")
            } else {
                format!("short {i} here\n")
            };
        }
        std::fs::write(&path, text).unwrap();
        let search = Search::new(
            r"ret\w*n\b",
            crate::text::Options {
                regex: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut reader = vel_fileio::search::Reader::default();
        let input = reader.open(&path, || false).unwrap().unwrap();
        let mut matches = 0;
        multiline::scan(
            input,
            &path,
            multiline::Query {
                search: &search,
                regex: &search.contiguous().unwrap(),
                multiline: None,
            },
            1,
            &AtomicBool::new(false),
            |line| {
                matches += line.match_count;
                true
            },
        )
        .unwrap();
        assert_eq!(matches, 180);
    }

    #[test]
    fn grep_long_line_preview_rebases_an_end_of_line_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.txt");
        let prefix = "日".repeat(90_000);
        std::fs::write(
            &path,
            format!("{}{}é\r\n", "other\r\n".repeat(600_000), prefix),
        )
        .unwrap();
        let task = Task::start(Request {
            search: Search::new(
                "é$",
                crate::text::Options {
                    regex: true,
                    case_sensitive: true,
                    ..Default::default()
                },
            )
            .unwrap(),
            scope: Where::parse("", dir.path()).unwrap(),
            folders: vec![Folder {
                path: dir.path().into(),
                name: "fixture".into(),
                rules: Rules::default(),
                follow_links: false,
            }],
            open: vec![],
            context: 0,
        })
        .unwrap();
        let timeout = Duration::from_secs(10);
        let Event::Line(hit) = task.events.recv_timeout(timeout).unwrap() else {
            panic!("expected one result");
        };
        assert_eq!(hit.number, 600_001);
        assert_eq!(hit.column(), 90_001);
        assert!(hit.text.len() <= 4096);
        assert_eq!(&hit.text[hit.highlights[0].clone()], "é");
        assert!(matches!(
            task.events.recv_timeout(timeout),
            Ok(Event::Done {
                matches: 1,
                files: 1
            })
        ));
    }
    #[test]
    fn ranked_file_buckets_match_sidebar_order_while_streaming() {
        let mut results = Results::default();
        let dirs = [
            "a", "Ä", "A", "a10", "ä", "ab", "aa", "É", "é", "A1", "ß", "ss", "a-0",
        ];
        let paths: Vec<_> = (0..1601)
            .map(|n| {
                let dir = dirs[n % dirs.len()];
                if n % 3 == 0 {
                    format!("/p/{dir}/sub/file{n}.txt")
                } else if n % 3 == 1 {
                    format!("/p/{dir}/File{n}.txt")
                } else {
                    format!("/p/{dir}{n}.txt")
                }
            })
            .map(|path| Arc::<Path>::from(PathBuf::from(path)))
            .collect();
        let mut expected = paths.clone();
        expected.sort_by_cached_key(|p| {
            let mut components = p.components().peekable();
            let mut key = Vec::new();
            while let Some(component) = components.next() {
                key.push(crate::project::sidebar_key(
                    component.as_os_str(),
                    components.peek().is_some(),
                ));
            }
            key
        });
        let make_line = |path: Arc<Path>, number, match_count| Line {
            path,
            number,
            text: "result".into(),
            highlights: Vec::new(),
            column_offset: 0,
            match_count,
        };
        for (pass, number) in [1, 4].into_iter().enumerate() {
            for i in 0..paths.len() {
                let path = &paths[(37 + i * 337 + pass * 29) % paths.len()];
                let index = results.push(make_line(path.clone(), number, pass));
                assert_eq!(index.added, 2);
                let rank = results.rank(index);
                if pass == 0 {
                    assert_eq!(results.rows.get(rank), Some(Row::Header(path)));
                } else {
                    assert_eq!(results.rows.get(rank), Some(Row::Gap));
                }
                let Some(Row::Line(line)) = results.rows.get(rank + 1) else {
                    panic!("missing line")
                };
                assert!(Arc::ptr_eq(path, &line.path));
                assert_eq!(line.number, number);
            }
        }
        assert_eq!(results.matched_files(), paths.len());
        let mut actual = results.rows.iter().filter_map(|r| match r {
            Row::Header(path) => Some(path),
            _ => None,
        });
        for i in 0..expected.len() / 2 {
            assert_eq!(actual.next(), Some(expected[i].as_ref()));
            assert_eq!(
                actual.next_back(),
                Some(expected[expected.len() - i - 1].as_ref())
            );
        }
        assert_eq!(actual.next(), Some(expected[expected.len() / 2].as_ref()));
        assert!(actual.next_back().is_none());
    }
    #[test]
    fn where_stream_formats_context_and_removes_successful_recovery() {
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
        assert!(plan.commit().unwrap().warning.is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(std::fs::read_to_string(&path).unwrap().contains("dog"));
        let plan = FileChange::prepare(path.clone(), &search, "x").unwrap();
        std::fs::write(path, "new contents").unwrap();
        assert!(plan.commit().is_err());
    }
    #[test]
    fn replacement_leaves_byte_exact_recovery_if_in_place_save_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        let original = b"hello cat\r\nsecond line\r\n";
        std::fs::write(&path, original).unwrap();
        let search = Search::new("cat", crate::text::Options::default()).unwrap();
        let plan = FileChange::prepare(path.clone(), &search, "dog").unwrap();
        let error = plan
            .commit_with(|path, _, _, _| {
                std::fs::write(path, "partial write").unwrap();
                Err("simulated write failure".into())
            })
            .unwrap_err();
        let recovery = error.split_once("original retained at ").unwrap().1;
        assert_eq!(std::fs::read(recovery).unwrap(), original);
    }
    #[test]
    fn replacement_retains_named_recovery_if_failed_save_swaps_identical_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        let original = b"hello cat\r\n";
        std::fs::write(&path, original).unwrap();
        let search = Search::new("cat", crate::text::Options::default()).unwrap();
        let plan = FileChange::prepare(path, &search, "dog").unwrap();
        let error = plan
            .commit_with(|path, _, _, _| {
                std::fs::rename(path, path.with_extension("old")).unwrap();
                std::fs::write(path, original).unwrap();
                Err("simulated failure after path swap".into())
            })
            .unwrap_err();
        let recovery = error.split_once("original retained at ").unwrap().1;
        assert!(recovery.ends_with("-sample.txt"));
        assert_eq!(std::fs::read(recovery).unwrap(), original);
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
                    assert_eq!(line.path.as_ref(), Path::new("unsaved.rs"));
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
