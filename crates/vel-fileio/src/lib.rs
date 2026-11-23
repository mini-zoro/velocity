//! Encoding-aware streaming IO, atomic saves and parent-directory file watching.
#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]
pub mod atomic;
mod decoding;
mod identity;
mod save;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(windows)]
pub use windows::trash;
#[cfg(unix)]
mod save_journal;
pub use identity::content_identity;
mod progressive;
pub mod regular;
pub mod search;
mod workers;
pub use decoding::{
    ContentIdentity, DecodeAttempt, DecodeNotice, DecodingDecision, PrefixFacts, decide_decoding,
    decoding_for_path,
};
use encoding_rs::{CoderResult, Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};
mod codecs;
pub use codecs::Codec;
pub use progressive::{
    LoadTask, content_kind, load_background, load_background_for_reload,
    load_background_with_options, load_on_workers, with_cancellation,
};
use std::{
    fs,
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};
use vel_buffer::{Buffer, Change, ChangeSet, Edit, EditKind, ViewId};
use vel_config::settings::EditorSettings;
use vel_text::Text;
pub use workers::IoWorkers;

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
    /// Operating system convention for new files without ending evidence.
    pub const fn system() -> Self {
        if cfg!(windows) { Self::CrLf } else { Self::Lf }
    }
    /// Parses Sublime's public setting/command spelling.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.to_lowercase().as_str() {
            "system" => Ok(Self::system()),
            "unix" | "lf" => Ok(Self::Lf),
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
    /// Human-readable convention for status text and menu captions.
    pub fn display_name(self) -> &'static str {
        // The current community menus guide demonstrates these platform captions:
        // https://docs.sublimetext.io/guide/customization/menus.html#available-menus
        // (status-bar menu animation). Wire/session names stay LF/CRLF/CR.
        match self {
            Self::Lf => "Unix",
            Self::CrLf => "Windows",
            Self::Cr => "Mac OS 9",
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
    /// Chosen caption when multiple menu names share the same decoder.
    pub display_name: Option<String>,
    /// Preserve a detected BOM.
    pub bom: bool,
    /// Majority line ending; ties prefer LF, then CRLF.
    pub line_ending: LineEnding,
}
impl Default for Format {
    fn default() -> Self {
        Self {
            encoding: "UTF-8".into(),
            display_name: None,
            bom: false,
            line_ending: LineEnding::system(),
        }
    }
}
impl Format {
    /// Independently authored display caption, including the document's BOM choice.
    pub fn display_encoding(&self) -> &str {
        if let Some(name) = &self.display_name
            && encoding(name).is_ok_and(|codec| codec.name().eq_ignore_ascii_case(&self.encoding))
            && name.ends_with("with BOM") == self.bom
        {
            return name;
        }
        ENCODINGS
            .iter()
            .find(|(caption, label)| {
                encoding(label).is_ok_and(|codec| codec.name().eq_ignore_ascii_case(&self.encoding))
                    && caption.ends_with("with BOM") == self.bom
            })
            .map_or(&self.encoding, |(caption, _)| *caption)
    }
}
/// Decode policy for an open, reload or restored disk reference.
#[derive(Clone, Debug)]
pub struct DecodeOptions {
    /// Explicit user choice only; detected codecs must leave this unset.
    /// A conflicting BOM or binary sample takes precedence over this choice.
    pub encoding: Option<String>,
    /// Codec of the last complete load/save, independently of the preference.
    pub effective_encoding: Option<String>,
    /// Legacy session field; retained for compatibility, never changes the explicit choice.
    pub saved_in_effective_encoding: bool,
    /// Raw identity of the saved/loaded revision, never normalized text.
    pub saved_content: Option<ContentIdentity>,
    /// Byte length and modification nanoseconds of that revision. A matching stamp
    /// permits progressive decoding immediately; the streamed digest validates it.
    pub revision_stamp: Option<(u64, u128)>,
    /// Legacy sessions may prove an unchanged revision with their disk stamp.
    pub legacy_stamp_matches: bool,
    /// Saved text recovered from a legacy dirty history, checked privately before publication.
    pub legacy_saved_text: Option<Text>,
    /// Native legacy clean histories retain the normalized text length and BLAKE3 digest.
    pub legacy_text_fingerprint: Option<(usize, String)>,
    /// BOM and newline convention represented by a legacy normalized history.
    /// Without this information its text cannot prove an unchanged raw revision.
    pub legacy_format: Option<Format>,
    /// Decoder used after automatic Unicode detection fails.
    pub fallback_encoding: String,
    /// Convention for a document with no line breaks.
    pub default_line_ending: LineEnding,
}
impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            encoding: None,
            effective_encoding: None,
            saved_in_effective_encoding: false,
            saved_content: None,
            revision_stamp: None,
            legacy_stamp_matches: false,
            legacy_saved_text: None,
            legacy_text_fingerprint: None,
            legacy_format: None,
            fallback_encoding: "Western (Windows 1252)".into(),
            default_line_ending: LineEnding::system(),
        }
    }
}
impl DecodeOptions {
    /// Opening policy from effective editor preferences.
    pub fn from_settings(settings: &EditorSettings) -> Self {
        Self {
            fallback_encoding: settings.fallback_encoding.clone(),
            default_line_ending: LineEnding::parse(&settings.default_line_ending)
                .unwrap_or_else(|_| LineEnding::system()),
            ..Self::default()
        }
    }
}
/// Display names accepted by file settings and encoding menus, with encoding_rs labels.
/// Captions are independently authored; no application package resources are used.
pub const ENCODINGS: &[(&str, &str)] = &[
    ("UTF-8", "utf-8"),
    ("UTF-8 with BOM", "utf-8"),
    ("UTF-16 LE", "utf-16le"),
    ("UTF-16 LE with BOM", "utf-16le"),
    ("UTF-16 BE", "utf-16be"),
    ("UTF-16 BE with BOM", "utf-16be"),
    ("Western (Windows 1252)", "windows-1252"),
    ("Western (ISO 8859-1)", "windows-1252"),
    ("Western (ISO 8859-3)", "iso-8859-3"),
    ("Western (ISO 8859-15)", "iso-8859-15"),
    ("Western (Mac Roman)", "macintosh"),
    ("Arabic (Windows 1256)", "windows-1256"),
    ("Arabic (ISO 8859-6)", "iso-8859-6"),
    ("Baltic (Windows 1257)", "windows-1257"),
    ("Baltic (ISO 8859-4)", "iso-8859-4"),
    ("Celtic (ISO 8859-14)", "iso-8859-14"),
    ("Central European (Windows 1250)", "windows-1250"),
    ("Central European (ISO 8859-2)", "iso-8859-2"),
    ("Cyrillic (Windows 1251)", "windows-1251"),
    ("Cyrillic (Windows 866)", "ibm866"),
    ("Cyrillic (ISO 8859-5)", "iso-8859-5"),
    ("Cyrillic (KOI8-R)", "koi8-r"),
    ("Cyrillic (KOI8-U)", "koi8-u"),
    ("Cyrillic (Mac Cyrillic)", "x-mac-cyrillic"),
    ("Greek (Windows 1253)", "windows-1253"),
    ("Greek (ISO 8859-7)", "iso-8859-7"),
    ("Hebrew (Windows 1255)", "windows-1255"),
    ("Hebrew (ISO 8859-8)", "iso-8859-8"),
    ("Hebrew (DOS CP862)", "cp862"),
    ("Nordic (ISO 8859-10)", "iso-8859-10"),
    ("Nordic (DOS CP865)", "cp865"),
    ("Romanian (ISO 8859-16)", "iso-8859-16"),
    ("Turkish (Windows 1254)", "windows-1254"),
    ("Turkish (ISO 8859-9)", "windows-1254"),
    ("Vietnamese (Windows 1258)", "windows-1258"),
    ("Thai (Windows 874)", "windows-874"),
    ("Japanese (Shift JIS)", "shift_jis"),
    ("Japanese (EUC-JP)", "euc-jp"),
    ("Japanese (ISO 2022-JP)", "iso-2022-jp"),
    ("Korean (EUC-KR)", "euc-kr"),
    ("Chinese Simplified (GBK)", "gbk"),
    ("Chinese Simplified (GB18030)", "gb18030"),
    ("Chinese Traditional (Big5)", "big5"),
];
/// Maps display names and encoding labels to supported codecs.
pub fn encoding(name: &str) -> Result<Codec, String> {
    let name = name.trim();
    let name = if name.eq_ignore_ascii_case("Western (Windows-1252)") {
        "Western (Windows 1252)"
    } else {
        name
    };
    let label = ENCODINGS
        .iter()
        .find(|(display, _)| display.eq_ignore_ascii_case(name))
        .map_or(name, |(_, label)| *label);
    if ["862", "cp862", "ibm862"]
        .iter()
        .any(|l| l.eq_ignore_ascii_case(label))
    {
        Ok(Codec::Cp862)
    } else if ["865", "cp865", "ibm865"]
        .iter()
        .any(|l| l.eq_ignore_ascii_case(label))
    {
        Ok(Codec::Cp865)
    } else {
        Encoding::for_label(label.as_bytes())
            .map(Codec::Standard)
            .ok_or_else(|| format!("unknown encoding {name}"))
    }
}
/// Resolves a fallback, returning a diagnostic once per invalid name per process.
/// Invalid settings use Windows-1252 so a single bad preference cannot block every open.
pub fn fallback_encoding(name: &str) -> (Codec, Option<String>) {
    match encoding(name) {
        Ok(codec) => (codec, None),
        Err(error) => {
            static REPORTED: std::sync::OnceLock<
                std::sync::Mutex<std::collections::BTreeSet<String>>,
            > = std::sync::OnceLock::new();
            let first = REPORTED
                .get_or_init(Default::default)
                .lock()
                .expect("encoding diagnostics")
                .insert(name.into());
            (
                Codec::Standard(WINDOWS_1252),
                first.then(|| format!("{error}; using Western (Windows 1252)")),
            )
        }
    }
}
/// Format for an explicit encoding choice; only names ending in "with BOM" add a BOM.
pub fn encoding_format(name: &str, line_ending: LineEnding) -> Result<Format, String> {
    let codec = encoding(name)?;
    Ok(Format {
        encoding: codec.name().into(),
        display_name: ENCODINGS
            .iter()
            .find(|(caption, _)| caption.eq_ignore_ascii_case(name.trim()))
            .filter(|(caption, _)| {
                ENCODINGS
                    .iter()
                    .find(|(other, label)| {
                        encoding(label).is_ok_and(|candidate| candidate == codec)
                            && other.ends_with("with BOM") == caption.ends_with("with BOM")
                    })
                    .is_some_and(|(first, _)| first != caption)
            })
            .map(|(caption, _)| (*caption).into()),
        bom: name.trim().to_ascii_lowercase().ends_with("with bom"),
        line_ending,
    })
}
/// New-file metadata from effective settings.
pub fn default_format(settings: &EditorSettings) -> Result<Format, String> {
    encoding_format(
        &settings.default_encoding,
        LineEnding::parse(&settings.default_line_ending)?,
    )
}
/// A decoded LF-normalized document.
#[derive(Debug)]
pub struct Loaded {
    /// Identity of the complete raw file; absent for provisional/synthetic text.
    pub disk_content: Option<ContentIdentity>,
    /// This decode used the explicit codec. This is attempt metadata, not the
    /// persistent tab choice: falling back never clears or changes that choice.
    pub encoding_chosen: bool,
    /// Raw bytes match the previous complete save/load.
    pub unchanged: bool,
    /// Non-modal diagnostic when disk decoding overrides a stored user choice.
    pub warning: Option<String>,
    /// Rope constructed directly from the streaming decoder.
    pub text: Text,
    /// Source representation.
    pub format: Format,
    /// Original disk size.
    pub source_bytes: u64,
}
/// Decodes with an explicit codec for Reopen with Encoding, preserving the line convention.
pub fn load_with_encoding(path: &Path, name: &str) -> Result<Loaded, String> {
    load_with_encoding_default(path, name, LineEnding::Lf)
}
/// Explicit decoding with the configured convention for files without line breaks.
pub fn load_with_encoding_default(
    path: &Path,
    name: &str,
    default_line_ending: LineEnding,
) -> Result<Loaded, String> {
    load_with_options(
        path,
        &DecodeOptions {
            encoding: Some(name.into()),
            default_line_ending,
            ..Default::default()
        },
        |_| {},
    )
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
/// except for UTF-16 established by a BOM or the bounded heuristic. Malformed
/// fallback sequences are replaced with a non-modal diagnostic.
pub fn load(
    path: &Path,
    fallback: &str,
    mut progress: impl FnMut(Progress),
) -> Result<Loaded, String> {
    load_with_options(
        path,
        &DecodeOptions {
            fallback_encoding: fallback.into(),
            ..Default::default()
        },
        &mut progress,
    )
}
/// Loads with document encoding and no-line-break defaults, streaming into the rope.
pub fn load_with_options(
    path: &Path,
    options: &DecodeOptions,
    mut progress: impl FnMut(Progress),
) -> Result<Loaded, String> {
    load_cancellable(path, options, &mut progress, None).map_err(|error| load_error(path, error))
}
/// Validates a restored revision without replacing its live text/history.
/// Callers bound synchronous use to small files; large comparisons use LoadTask.
pub fn revision_unchanged(path: &Path, options: &DecodeOptions) -> Result<bool, String> {
    let (_, decision) = decoding_for_path(path, options)?;
    if decision.unchanged {
        return Ok(decision.unchanged);
    }
    // Old sessions can prove a revision by saved decoded text instead of raw bytes.
    load_with_options(path, options, |_| {}).map(|loaded| loaded.unchanged)
}
fn load_error(path: &Path, error: String) -> String {
    regular::error(path, error)
}

struct DecodedContent {
    text: Text,
    ending: LineEnding,
    line_counts: [u64; 3],
    content: ContentIdentity,
}
fn finish_decoding(
    path: &Path,
    options: &DecodeOptions,
    mut facts: PrefixFacts,
    attempt: &DecodeAttempt,
    decoded: DecodedContent,
) -> Result<Loaded, String> {
    let DecodedContent {
        text,
        ending,
        line_counts,
        content,
    } = decoded;
    facts.content = Some(content);
    // A main-format history can prove its saved revision even when BOM routing
    // selected the codec directly. Raw identities, when present, remain authoritative.
    facts.legacy_text_matches = options.saved_content.is_none()
        && options
            .effective_encoding
            .as_deref()
            .is_some_and(|name| encoding(name).is_ok_and(|codec| codec == attempt.encoding))
        && legacy_revision_matches(options, &text, facts.bom.is_some(), line_counts);
    let unchanged = decide_decoding(&facts, options)?.unchanged;
    let display_name = options
        .encoding
        .as_deref()
        .filter(|name| encoding(name).is_ok_and(|codec| codec == attempt.encoding))
        .and_then(|name| encoding_format(name, ending).ok())
        .and_then(|format| format.display_name)
        .filter(|name| name.ends_with("with BOM") == attempt.bom);
    Ok(Loaded {
        disk_content: Some(content),
        encoding_chosen: attempt.user_choice,
        unchanged,
        warning: if unchanged && !attempt.replacement {
            None
        } else {
            attempt.notice.as_ref().map(|notice| notice.message(path))
        },
        text,
        source_bytes: content.bytes,
        format: Format {
            encoding: attempt.encoding.name().into(),
            display_name,
            bom: attempt.bom,
            line_ending: ending,
        },
    })
}
fn legacy_revision_matches(
    options: &DecodeOptions,
    text: &Text,
    bom: bool,
    line_counts: [u64; 3],
) -> bool {
    // Legacy history normalized CRLF/CR and removed the BOM. It can only prove
    // the original representation if every decoded break and the BOM match its
    // stored format. Mixed endings cannot be reconstructed, so fail conservatively.
    let Some(format) = &options.legacy_format else {
        return false;
    };
    let expected = match format.line_ending {
        LineEnding::Lf => 0,
        LineEnding::CrLf => 1,
        LineEnding::Cr => 2,
    };
    if format.bom != bom
        || line_counts
            .iter()
            .enumerate()
            .any(|(i, n)| i != expected && *n != 0)
    {
        return false;
    }
    if let Some(saved) = &options.legacy_saved_text {
        return text == saved;
    }
    options
        .legacy_text_fingerprint
        .as_ref()
        .is_some_and(|(len, hash)| {
            if text.len_bytes() != *len {
                return false;
            }
            let mut digest = blake3::Hasher::new();
            for chunk in text.chunks() {
                digest.update(chunk.as_bytes());
            }
            digest.finalize().to_hex().as_str() == hash
        })
}
fn load_cancellable(
    path: &Path,
    options: &DecodeOptions,
    progress: &mut dyn FnMut(Progress),
    cancel: Option<&vel_largefile::Cancellation>,
) -> Result<Loaded, String> {
    let scoped = progressive::current_cancellation();
    let cancel = cancel.or(scoped.as_ref());
    let mut plan = decoding::DecodePlan::new(path, options)?;
    let mut failure = "invalid encoded text".to_owned();
    while let Some(attempt) = plan.next() {
        let file = regular::read(path)?;
        let total = file.metadata().map_err(|e| e.to_string())?.len();
        let mut raw = identity::DigestReader::new(BufReader::with_capacity(128 * 1024, file));
        let mut ignored = vec![0; attempt.skip];
        raw.read_exact(&mut ignored).map_err(|e| e.to_string())?;
        let mut reader = DecodeReader::new(
            progressive::CheckedReader {
                input: &mut raw,
                cancel,
            },
            attempt.encoding,
            total,
            progress,
            options.default_line_ending,
        );
        reader.allow_replacement = attempt.replacement;
        match Text::from_reader(&mut reader) {
            Ok(text) => {
                let ending = reader.ending();
                let line_counts = reader.counts;
                drop(reader);
                if !plan.read_matches_decision(options, &raw.prefix)? {
                    continue;
                }
                progress(Progress { read: total, total });
                let loaded = finish_decoding(
                    path,
                    options,
                    plan.facts.clone(),
                    &attempt,
                    DecodedContent {
                        text,
                        ending,
                        line_counts,
                        content: raw.digest.identity(),
                    },
                )?;
                if plan.accept(options, &attempt, &loaded)? {
                    return Ok(loaded);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => failure = error.to_string(),
            Err(error) => return Err(error.to_string()),
        }
    }
    Err(plan.binary().map_or(failure, |notice| notice.message(path)))
}
/// Decodes in-memory text input with the file codec, BOM and fallback policy.
pub fn decode_bytes(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded, String> {
    let path = Path::new("stdin");
    let facts = PrefixFacts::inspect(bytes);
    let decision = decide_decoding(&facts, options)?;
    for attempt in decision.attempts {
        let total = bytes.len() as u64;
        let mut progress = |_| {};
        let mut reader = DecodeReader::new(
            &bytes[attempt.skip..],
            attempt.encoding,
            total,
            &mut progress,
            options.default_line_ending,
        );
        reader.allow_replacement = attempt.replacement;
        if let Ok(text) = Text::from_reader(&mut reader) {
            let line_ending = reader.ending();
            return Ok(Loaded {
                disk_content: None,
                encoding_chosen: false,
                unchanged: false,
                warning: attempt.notice.as_ref().map(|notice| notice.message(path)),
                text,
                source_bytes: total,
                format: Format {
                    encoding: attempt.encoding.name().into(),
                    display_name: None,
                    bom: attempt.bom,
                    line_ending,
                },
            });
        }
    }
    Err(decision.binary.map_or_else(
        || "invalid encoded stdin".into(),
        |notice| notice.message(path),
    ))
}

/// Encodes a buffer for stream output using the ordinary file representation.
pub fn encode_to(writer: &mut impl Write, text: &Text, format: &Format) -> Result<(), String> {
    write_encoded(writer, text, format, encoding(&format.encoding)?)
}

struct DecodeReader<'a, R> {
    input: R,
    decoder: codecs::Decoder,
    allow_replacement: bool,
    raw: Vec<u8>,
    start: usize,
    end: usize,
    eof: bool,
    finished: bool,
    ready: Vec<u8>,
    cursor: usize,
    pending_cr: bool,
    counts: [u64; 3],
    default_line_ending: LineEnding,
    read: u64,
    total: u64,
    progress: &'a mut dyn FnMut(Progress),
}
impl<'a, R: Read> DecodeReader<'a, R> {
    fn new(
        input: R,
        codec: Codec,
        total: u64,
        progress: &'a mut dyn FnMut(Progress),
        default_line_ending: LineEnding,
    ) -> Self {
        Self {
            input,
            decoder: codec.decoder(),
            allow_replacement: false,
            raw: vec![0; 128 * 1024],
            start: 0,
            end: 0,
            eof: false,
            finished: false,
            ready: Vec::new(),
            cursor: 0,
            pending_cr: false,
            counts: [0; 3],
            default_line_ending,
            read: 0,
            total,
            progress,
        }
    }
    fn ending(&self) -> LineEnding {
        // No existing convention: use the setting's documented newline convention.
        // https://docs.sublimetext.io/reference/settings.html#system-and-miscellaneous-settings
        if self.counts == [0; 3] {
            self.default_line_ending
        } else if self.counts[1] > self.counts[0] && self.counts[1] >= self.counts[2] {
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
            if errors && !self.allow_replacement {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid encoded text",
                ));
            }
            let decoded = &decoded[..written];
            if !self.pending_cr && !decoded.contains(&b'\r') {
                self.ready.extend_from_slice(decoded);
                self.counts[0] += decoded.iter().filter(|&&b| b == b'\n').count() as u64;
            } else {
                for &byte in decoded {
                    if self.pending_cr {
                        self.ready.push(b'\n');
                        self.pending_cr = false;
                        if byte == b'\n' {
                            self.counts[1] += 1;
                            continue;
                        }
                        self.counts[2] += 1;
                    }
                    if byte == b'\r' {
                        self.pending_cr = true;
                    } else {
                        self.ready.push(byte);
                        if byte == b'\n' {
                            self.counts[0] += 1;
                        }
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
    /// Cheap immutable reference for off-thread reload comparisons.
    pub fn saved_text(&self) -> Text {
        self.saved.clone()
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
    codec: Codec,
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
/// Renames `from` to `to` without ever replacing an existing destination.
///
/// The check and the rename are one system call (`renameatx_np(RENAME_EXCL)` on macOS), so a
/// file created at `to` in the meantime is never overwritten. A rename that only changes the
/// letter case of the name on a case-insensitive volume (`foo.txt` → `Foo.txt`, where `to`
/// resolves to `from` itself) is allowed.
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    // The filesystem distinguishes a case change of the same entry from a
    // separate name (including a hard link). Never fall back to replacing rename.
    exclusive_rename(from, to)
}
#[cfg(unix)]
fn same_entry(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}
#[cfg(not(any(unix, windows)))]
fn same_entry(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    false
}
#[cfg(unix)]
fn exclusive_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(std::io::Error::from)
}
#[cfg(windows)]
fn exclusive_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    windows::rename_no_replace(from, to)
}
#[cfg(not(any(unix, windows)))]
fn exclusive_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(to).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    fs::rename(from, to)
}
pub use save::{Saved, save, save_report, save_report_with_expected, save_with_identity};
#[cfg(unix)]
pub use save_journal::{RecoveredSave, recover_pending_save, recover_saves};
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
