//! Encoding-aware streaming IO, atomic saves and parent-directory file watching.
#![forbid(unsafe_code)]
mod progressive;
use encoding_rs::{CoderResult, Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};
pub use progressive::{LoadTask, content_kind, load_background};
use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};
use vel_buffer::{Buffer, Change, ChangeSet, Edit, EditKind, ViewId};
use vel_config::settings::EditorSettings;
use vel_text::Text;

/// Detected/selected line-ending convention.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineEnding {
    /// LF, also the buffer's internal convention.
    #[default]
    Lf,
    /// CRLF.
    CrLf,
    /// CR.
    Cr,
}
impl LineEnding {
    /// Parses Sublime's public setting/command spelling.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.to_lowercase().as_str() {
            // The macOS system convention is LF.
            "unix" | "lf" | "system" => Ok(Self::Lf),
            "windows" | "crlf" => Ok(Self::CrLf),
            "mac" | "cr" => Ok(Self::Cr),
            _ => Err(format!("unknown line ending {name}")),
        }
    }
    /// Short display name for the source convention.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::CrLf => "CRLF",
            Self::Cr => "CR",
        }
    }
    /// Bytes to write for an internal LF.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }
}
/// Encoding and BOM/line ending metadata retained per document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Format {
    /// Canonical encoding label.
    pub encoding: String,
    /// Preserve a detected BOM.
    pub bom: bool,
    /// Majority line ending; ties prefer LF, then CRLF.
    pub line_ending: LineEnding,
}
impl Default for Format {
    fn default() -> Self {
        Self {
            encoding: "UTF-8".into(),
            bom: false,
            line_ending: LineEnding::Lf,
        }
    }
}
/// Maps display names and standard encoding labels to encoding_rs codecs.
pub fn encoding(name: &str) -> Result<&'static Encoding, String> {
    let name = name.trim();
    match name.to_ascii_lowercase().as_str() {
        "western (windows 1252)" | "western (windows-1252)" => Ok(WINDOWS_1252),
        "utf-8 with bom" => Ok(UTF_8),
        "utf-16 le" => Ok(UTF_16LE),
        "utf-16 be" => Ok(UTF_16BE),
        _ => Encoding::for_label(name.as_bytes()).ok_or_else(|| format!("unknown encoding {name}")),
    }
}
/// New-file metadata from effective settings.
pub fn default_format(settings: &EditorSettings) -> Result<Format, String> {
    let enc = encoding(&settings.default_encoding)?;
    Ok(Format {
        encoding: enc.name().into(),
        bom: settings
            .default_encoding
            .eq_ignore_ascii_case("utf-8 with bom")
            || enc == UTF_16LE
            || enc == UTF_16BE,
        line_ending: LineEnding::parse(&settings.default_line_ending)?,
    })
}
/// A decoded LF-normalized document.
#[derive(Debug)]
pub struct Loaded {
    /// Rope constructed directly from the streaming decoder.
    pub text: Text,
    /// Source representation.
    pub format: Format,
    /// Original disk size.
    pub source_bytes: u64,
}
/// Progress in original file bytes; may advance before UTF-8 tree construction completes.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    /// Bytes consumed.
    pub read: u64,
    /// Total disk size.
    pub total: u64,
}
/// Loads a file, trying BOM/UTF-8 before the configured fallback. NUL input is rejected
/// except when a UTF-16 BOM establishes a text encoding. Invalid sequences are errors.
pub fn load(
    path: &Path,
    fallback: &str,
    mut progress: impl FnMut(Progress),
) -> Result<Loaded, String> {
    load_cancellable(path, fallback, &mut progress, None)
}
fn load_cancellable(
    path: &Path,
    fallback: &str,
    progress: &mut dyn FnMut(Progress),
    cancel: Option<&vel_largefile::Cancellation>,
) -> Result<Loaded, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let total = file.metadata().map_err(|e| e.to_string())?.len();
    let mut prefix = [0; 8192];
    let n = BufReader::new(&file)
        .read(&mut prefix)
        .map_err(|e| e.to_string())?;
    let (codec, bom, skip) = if prefix[..n].starts_with(&[0xef, 0xbb, 0xbf]) {
        (UTF_8, true, 3)
    } else if prefix[..n].starts_with(&[0xff, 0xfe]) {
        (UTF_16LE, true, 2)
    } else if prefix[..n].starts_with(&[0xfe, 0xff]) {
        (UTF_16BE, true, 2)
    } else {
        (UTF_8, false, 0)
    };
    if codec != UTF_16LE && codec != UTF_16BE && prefix[..n].contains(&0) {
        return Err("binary file: NUL in first 8 KiB".into());
    }
    let attempt = |codec: &'static Encoding,
                   progress: &mut dyn FnMut(Progress)|
     -> Result<(Text, LineEnding), String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let mut ignored = vec![0; skip];
        file.read_exact(&mut ignored).map_err(|e| e.to_string())?;
        let mut reader = DecodeReader::new(
            progressive::CheckedReader {
                input: BufReader::with_capacity(128 * 1024, file),
                cancel,
            },
            codec,
            total,
            progress,
        );
        let text = Text::from_reader(&mut reader).map_err(|e| e.to_string())?;
        let ending = reader.ending();
        Ok((text, ending))
    };
    let (text, ending, codec) = match attempt(codec, progress) {
        Ok((text, ending)) => (text, ending, codec),
        Err(first) if !bom => {
            let fallback = encoding(fallback)?;
            if fallback == UTF_8 {
                return Err(first);
            }
            let (text, ending) = attempt(fallback, progress)?;
            (text, ending, fallback)
        }
        Err(e) => return Err(e),
    };
    progress(Progress { read: total, total });
    Ok(Loaded {
        text,
        format: Format {
            encoding: codec.name().into(),
            bom,
            line_ending: ending,
        },
        source_bytes: total,
    })
}
struct DecodeReader<'a, R> {
    input: R,
    decoder: encoding_rs::Decoder,
    raw: Vec<u8>,
    start: usize,
    end: usize,
    eof: bool,
    finished: bool,
    ready: Vec<u8>,
    cursor: usize,
    pending_cr: bool,
    counts: [u64; 3],
    read: u64,
    total: u64,
    progress: &'a mut dyn FnMut(Progress),
}
impl<'a, R: Read> DecodeReader<'a, R> {
    fn new(
        input: R,
        codec: &'static Encoding,
        total: u64,
        progress: &'a mut dyn FnMut(Progress),
    ) -> Self {
        Self {
            input,
            decoder: codec.new_decoder_without_bom_handling(),
            raw: vec![0; 128 * 1024],
            start: 0,
            end: 0,
            eof: false,
            finished: false,
            ready: Vec::new(),
            cursor: 0,
            pending_cr: false,
            counts: [0; 3],
            read: 0,
            total,
            progress,
        }
    }
    fn ending(&self) -> LineEnding {
        if self.counts[1] > self.counts[0] && self.counts[1] >= self.counts[2] {
            LineEnding::CrLf
        } else if self.counts[2] > self.counts[0] && self.counts[2] > self.counts[1] {
            LineEnding::Cr
        } else {
            LineEnding::Lf
        }
    }
}
impl<R: Read> Read for DecodeReader<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.cursor == self.ready.len() && !self.finished {
            self.ready.clear();
            self.cursor = 0;
            if self.start == self.end && !self.eof {
                let n = loop {
                    match self.input.read(&mut self.raw) {
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        result => break result?,
                    }
                };
                self.start = 0;
                self.end = n;
                self.eof = n == 0;
                self.read += n as u64;
                (self.progress)(Progress {
                    read: self.read,
                    total: self.total,
                });
            }
            let mut decoded = vec![0; 512 * 1024];
            let (result, read, written, errors) = self.decoder.decode_to_utf8(
                &self.raw[self.start..self.end],
                &mut decoded,
                self.eof,
            );
            self.start += read;
            if errors {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid encoded text",
                ));
            }
            for byte in &decoded[..written] {
                if self.pending_cr {
                    self.ready.push(b'\n');
                    self.pending_cr = false;
                    if *byte == b'\n' {
                        self.counts[1] += 1;
                        continue;
                    }
                    self.counts[2] += 1;
                }
                if *byte == b'\r' {
                    self.pending_cr = true;
                } else {
                    self.ready.push(*byte);
                    if *byte == b'\n' {
                        self.counts[0] += 1;
                    }
                }
            }
            if self.eof && result == CoderResult::InputEmpty {
                self.finished = true;
                if self.pending_cr {
                    self.ready.push(b'\n');
                    self.counts[2] += 1;
                    self.pending_cr = false;
                }
            }
        }
        let n = out.len().min(self.ready.len() - self.cursor);
        out[..n].copy_from_slice(&self.ready[self.cursor..self.cursor + n]);
        self.cursor += n;
        Ok(n)
    }
}
/// Saved snapshot plus the buffer's monotonic change identity. Line starts map
/// through buffer history, so inserting lines does not mark shifted original lines.
#[derive(Clone, Debug)]
pub struct ChangeTracker {
    saved: Text,
    at: vel_buffer::ChangeId,
}
impl ChangeTracker {
    /// Records the saved/load revision.
    pub fn new(buffer: &Buffer) -> Self {
        Self {
            saved: buffer.text().clone(),
            at: buffer.change_id(),
        }
    }
    /// Whether a decoded disk revision equals the saved snapshot.
    pub fn is_saved(&self, text: &Text) -> bool {
        self.saved == *text
    }
    /// Updates the saved snapshot and change identity after a successful write/reload.
    pub fn mark_saved(&mut self, buffer: &Buffer) {
        self.saved = buffer.text().clone();
        self.at = buffer.change_id();
    }
    /// Lines changed since save, using historic position mapping and content checks.
    /// If history has expired, remaining uncertain lines are conservatively modified.
    pub fn modified_lines(&self, buffer: &Buffer) -> Vec<bool> {
        let text = buffer.text();
        if self.saved == *text {
            return vec![false; text.len_lines()];
        }
        let mut modified = vec![true; text.len_lines()];
        for old_row in 0..self.saved.len_lines() {
            let start = self.saved.line_to_byte(old_row);
            if let Some(mapped) =
                buffer.transform_point_from(start, self.at, vel_buffer::Bias::Right)
            {
                let row = text.byte_to_line(mapped);
                if text.line_to_byte(row) == mapped && text.line(row) == self.saved.line(old_row) {
                    modified[row] = false;
                }
            }
        }
        modified
    }
}
/// Applies save cleanups as one undoable transaction, preserving every selection.
pub fn prepare_save(
    buffer: &mut Buffer,
    id: ViewId,
    settings: &EditorSettings,
    tracker: &ChangeTracker,
) {
    let trim = settings.trim_trailing_white_space_on_save.as_bool() == Some(true)
        || matches!(
            settings.trim_trailing_white_space_on_save.as_str(),
            Some("all" | "not_on_caret")
        );
    let modified = if trim && settings.trim_only_modified_white_space {
        Some(tracker.modified_lines(buffer))
    } else {
        None
    };
    let caret_rows: std::collections::BTreeSet<_> = buffer
        .view(id)
        .sel()
        .iter()
        .map(|r| buffer.text().byte_to_line(r.b))
        .collect();
    let mut owned = Vec::new();
    if trim {
        for row in 0..buffer.text().len_lines() {
            if modified.as_ref().is_some_and(|m| !m[row]) {
                continue;
            }
            if settings.trim_trailing_white_space_on_save.as_str() == Some("not_on_caret")
                && caret_rows.contains(&row)
            {
                continue;
            }
            let line = buffer.text().line(row).to_string();
            // Spaces and tabs only: no-break and ideographic spaces are content.
            let keep = line.trim_end_matches([' ', '\t']).len();
            if keep < line.len() {
                let start = buffer.text().line_to_byte(row);
                owned.push((start + keep..start + line.len(), String::new()));
            }
        }
    }
    if settings.ensure_newline_at_eof_on_save
        && buffer.text().len_bytes() > 0
        && buffer.text().byte_at(buffer.text().len_bytes() - 1) != b'\n'
    {
        let end = buffer.text().len_bytes();
        owned.push((end..end, "\n".into()));
    }
    if owned.is_empty() {
        return;
    }
    let changes = ChangeSet::new(
        owned
            .iter()
            .map(|(r, s)| Change {
                old_range: r.clone(),
                new_len: s.len(),
            })
            .collect(),
    );
    let selection = changes.transform_selection(buffer.view(id).sel());
    let edits: Vec<_> = owned
        .iter()
        .map(|(r, s)| Edit {
            range: r.clone(),
            text: s,
        })
        .collect();
    buffer.transact(id, EditKind::Other, &edits, selection);
}
/// Encodes `text` with `codec`, restoring line endings and the BOM, into `writer`.
fn write_encoded(
    writer: &mut impl Write,
    text: &Text,
    format: &Format,
    codec: &'static Encoding,
) -> Result<(), String> {
    if format.bom {
        writer
            .write_all(if codec == UTF_8 {
                &[0xef, 0xbb, 0xbf]
            } else if codec == UTF_16LE {
                &[0xff, 0xfe]
            } else if codec == UTF_16BE {
                &[0xfe, 0xff]
            } else {
                return Err("BOM unsupported for encoding".into());
            })
            .map_err(|e| e.to_string())?;
    }
    for chunk in text.chunks() {
        let source = if format.line_ending == LineEnding::Lf {
            std::borrow::Cow::Borrowed(chunk)
        } else {
            std::borrow::Cow::Owned(chunk.replace('\n', format.line_ending.as_str()))
        };
        if codec == UTF_16LE || codec == UTF_16BE {
            for unit in source.encode_utf16() {
                writer
                    .write_all(&if codec == UTF_16LE {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    })
                    .map_err(|e| e.to_string())?;
            }
        } else {
            let (bytes, _, errors) = codec.encode(&source);
            if errors {
                return Err(format!(
                    "characters cannot be represented in {}",
                    codec.name()
                ));
            }
            writer.write_all(&bytes).map_err(|e| e.to_string())?;
        }
    }
    writer.flush().map_err(|e| e.to_string())
}
/// Whether replacing `target` by renaming a new file over it would lose something the user
/// expects to keep: other hard links, an owner other than us, extended attributes (Finder tags,
/// quarantine, ACL-backed metadata), or a read-only mode that must make the save fail.
#[cfg(unix)]
fn rename_would_lose(target: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() > 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().readonly()
        || has_user_xattrs(target)
}
/// Extended attributes a rename would drop. `com.apple.provenance` is ignored: macOS attaches it
/// to nearly every file and gives the replacement its own.
#[cfg(unix)]
fn has_user_xattrs(target: &Path) -> bool {
    let Ok(size) = rustix::fs::listxattr_query(target) else {
        return true;
    };
    if size == 0 {
        return false;
    }
    let mut names = vec![0; size + 64];
    let Ok(len) = rustix::fs::listxattr(target, &mut names[..]) else {
        return true;
    };
    names[..len]
        .split(|&b| b == 0)
        .any(|name| !name.is_empty() && name != b"com.apple.provenance")
}
#[cfg(not(unix))]
fn rename_would_lose(_target: &Path, metadata: &fs::Metadata) -> bool {
    metadata.permissions().readonly()
}
/// Renames `from` to `to` without ever replacing an existing destination.
///
/// The check and the rename are one system call (`renameatx_np(RENAME_EXCL)` on macOS), so a
/// file created at `to` in the meantime is never overwritten. A rename that only changes the
/// letter case of the name on a case-insensitive volume (`foo.txt` → `Foo.txt`, where `to`
/// resolves to `from` itself) is allowed.
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let from_meta = fs::symlink_metadata(from)?;
    if let Ok(to_meta) = fs::symlink_metadata(to) {
        if same_entry(&from_meta, &to_meta) && case_only_change(from, to) {
            return fs::rename(from, to);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    exclusive_rename(from, to)
}
/// Both names lead to one directory entry: same parent, names differing only in case.
fn case_only_change(from: &Path, to: &Path) -> bool {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    from.parent() == to.parent() && name(from) == name(to) && from != to
}
#[cfg(unix)]
fn same_entry(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}
#[cfg(not(unix))]
fn same_entry(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    false
}
#[cfg(unix)]
fn exclusive_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(std::io::Error::from)
}
#[cfg(not(unix))]
fn exclusive_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(to).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    fs::rename(from, to)
}
/// Re-encodes, restores endings and writes the file. With `atomic` an existing file is replaced
/// by renaming a complete temporary file over it, keeping its permissions; when that would lose
/// hard links, ownership, extended attributes or a read-only mode the file is rewritten in place
/// instead (Sublime disables atomic saves adaptively in the same cases). New files are created
/// directly so they get the process umask. Unrepresentable characters fail before the
/// destination changes.
pub fn save(path: &Path, text: &Text, format: &Format, atomic: bool) -> Result<(), String> {
    let codec = encoding(&format.encoding)?;
    // Follow a symlink rather than replacing the link itself.
    let target = if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        fs::canonicalize(path).map_err(|e| e.to_string())?
    } else {
        path.to_owned()
    };
    let error = |e: io::Error| format!("{}: {e}", target.display());
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let existing = fs::metadata(&target).ok();
    let in_place = match &existing {
        None => true,
        Some(metadata) => !atomic || rename_would_lose(&target, metadata),
    };
    if in_place {
        // Encoding errors must surface before the file is truncated.
        if codec != UTF_8 && codec != UTF_16LE && codec != UTF_16BE {
            write_encoded(&mut io::sink(), text, format, codec)?;
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o666);
        let file = options.open(&target).map_err(error)?;
        let mut writer = BufWriter::with_capacity(256 * 1024, &file);
        write_encoded(&mut writer, text, format, codec)?;
        drop(writer);
        file.sync_all().map_err(error)?;
    } else {
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(error)?;
        if let Some(metadata) = &existing {
            temp.as_file()
                .set_permissions(metadata.permissions())
                .map_err(error)?;
        }
        let mut writer = BufWriter::with_capacity(256 * 1024, temp.as_file_mut());
        write_encoded(&mut writer, text, format, codec)?;
        drop(writer);
        temp.as_file().sync_all().map_err(error)?;
        temp.persist(&target).map_err(|e| error(e.error))?;
    }
    #[cfg(unix)]
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}
/// Parent-directory watcher so atomic renames do not detach file monitoring.
#[derive(Debug)]
pub struct Watcher {
    watcher: notify::RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    directories: std::collections::BTreeSet<PathBuf>,
    files: std::cell::RefCell<
        std::collections::BTreeMap<PathBuf, Option<(u64, std::time::SystemTime)>>,
    >,
}
impl Watcher {
    /// Starts native notifications; polling never blocks the UI.
    pub fn new() -> Result<Self, String> {
        let (tx, events) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })
        .map_err(|e| e.to_string())?;
        Ok(Self {
            watcher,
            events,
            directories: Default::default(),
            files: Default::default(),
        })
    }
    /// Watches a file's parent once, covering save/replacement/delete/recreate.
    pub fn watch(&mut self, path: &Path) -> Result<(), String> {
        use notify::Watcher as _;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = fs::canonicalize(parent).map_err(|e| e.to_string())?;
        let file = parent.join(path.file_name().ok_or("watch requires a file path")?);
        self.files
            .borrow_mut()
            .entry(file.clone())
            .or_insert_with(|| file_stamp(&file));
        if self.directories.insert(parent.clone()) {
            self.watcher
                .watch(&parent, notify::RecursiveMode::NonRecursive)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// Drains content/name notifications and checks watched metadata as a fallback
    /// when native event delivery is unavailable (including restricted sessions).
    /// Call at a bounded rate; this does one stat per file and never reads its body.
    pub fn changed(&self) -> Vec<Result<Vec<PathBuf>, String>> {
        let mut changed: Vec<_> = self
            .events
            .try_iter()
            .filter_map(|event| match event {
                Ok(e) if e.kind.is_modify() || e.kind.is_create() || e.kind.is_remove() => {
                    Some(Ok(e.paths))
                }
                Ok(_) => None,
                Err(e) => Some(Err(e.to_string())),
            })
            .collect();
        for (path, previous) in self.files.borrow_mut().iter_mut() {
            let current = file_stamp(path);
            if current != *previous {
                *previous = current;
                changed.push(Ok(vec![path.clone()]));
            }
        }
        changed
    }
}

fn file_stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}
