//! Seekable UTF-8/LF search input using the editor's automatic decode policy.
use crate::{DecodeAttempt, DecodeOptions, PrefixFacts, decide_decoding, decoding};
use encoding_rs::{UTF_8, UTF_16BE, UTF_16LE};
use memchr::{memchr, memchr2};
use std::{
    fs::{File, Metadata},
    io::{self, BufReader, Read, Write},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

const IN_MEMORY: usize = 4 * 1024 * 1024;
const BLOCK: usize = 128 * 1024;

/// A worker's reusable small-file buffers.
#[derive(Debug, Default)]
pub struct Reader {
    raw: Vec<u8>,
    decoded: Vec<u8>,
}

/// The complete, validated search input, normalized to LF as in the editor.
#[derive(Debug)]
pub enum Text<'a> {
    /// Ordinary small files do not need an index or scratch file.
    Slice(&'a str),
    /// A compact index of restartable source windows for large files.
    Windows(Box<Windows>),
}

/// A required, finite substring on normalized UTF-8. A negative answer must
/// rule out a match. Windows also test the overlap at each block boundary.
pub struct Filter<'a> {
    /// Maximum byte length of the required substring.
    pub width: usize,
    /// Whether the substring includes line endings and needs normalization.
    pub needs_lf: bool,
    /// On a definite line-local miss there is no need to index CRLF offsets.
    pub line_local: bool,
    /// Tests the substring on one window or overlap.
    pub contains: &'a mut dyn FnMut(&[u8]) -> bool,
}
impl std::fmt::Debug for Filter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Filter")
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl Reader {
    /// Prepares a candidate for search. Binary and cancelled inputs return `None`.
    pub fn open(
        &mut self,
        path: &Path,
        cancelled: impl Fn() -> bool,
    ) -> io::Result<Option<Text<'_>>> {
        self.open_with_options(path, &DecodeOptions::default(), cancelled, None)
    }

    /// Prepares disk text with the same automatic decoding options as a new tab.
    pub fn open_with_options(
        &mut self,
        path: &Path,
        options: &DecodeOptions,
        cancelled: impl Fn() -> bool,
        filter: Option<Filter<'_>>,
    ) -> io::Result<Option<Text<'_>>> {
        self.open_with_limit(path, options, cancelled, filter, 0)
    }

    /// Prepares text only if the opened file fits the byte limit (zero means unlimited).
    pub fn open_with_limit(
        &mut self,
        path: &Path,
        options: &DecodeOptions,
        cancelled: impl Fn() -> bool,
        filter: Option<Filter<'_>>,
        max_size: u64,
    ) -> io::Result<Option<Text<'_>>> {
        let (mut file, bytes) = crate::regular::read_search_file(path)?;
        if max_size != 0 && bytes > max_size {
            return Ok(None);
        }
        let len = usize::try_from(bytes).map_err(io::Error::other)?;
        if len > IN_MEMORY {
            return Windows::open(file, len, options, cancelled, filter)
                .map(|w| w.map(|w| Text::Windows(Box::new(w))));
        }
        if cancelled() {
            return Ok(None);
        }
        self.raw.resize(len, 0);
        file.read_exact(&mut self.raw)?;
        if options.encoding.is_none()
            && let Ok(s) = simdutf8::basic::from_utf8(&self.raw)
            && memchr2(0, b'\r', s.as_bytes()).is_none()
            && !decoding::binary_signature(s.as_bytes())
        {
            return Ok(Some(Text::Slice(s.strip_prefix('\u{feff}').unwrap_or(s))));
        }
        let policy = plan(&self.raw, options)?;
        for attempt in policy {
            if let Some(s) = decode(&self.raw[attempt.skip..], &attempt) {
                if memchr(0, s.as_bytes()).is_some() {
                    return Ok(None);
                }
                normalize(&s, &mut self.decoded);
                return Ok(Some(Text::Slice(
                    std::str::from_utf8(&self.decoded).expect("normalized UTF-8"),
                )));
            }
        }
        Ok(None)
    }
}

fn plan(prefix: &[u8], options: &DecodeOptions) -> io::Result<Vec<DecodeAttempt>> {
    decide_decoding(&PrefixFacts::inspect(prefix), options)
        .map(|decision| decision.attempts)
        .map_err(io::Error::other)
}

fn decode<'a>(bytes: &'a [u8], attempt: &DecodeAttempt) -> Option<std::borrow::Cow<'a, str>> {
    if attempt.encoding == UTF_8 && !attempt.replacement {
        simdutf8::basic::from_utf8(bytes).ok().map(Into::into)
    } else if attempt.replacement {
        Some(attempt.encoding.decode_without_bom_handling(bytes).0)
    } else {
        attempt
            .encoding
            .decode_without_bom_handling_and_without_replacement(bytes)
    }
}

fn normalize(text: &str, output: &mut Vec<u8>) {
    output.clear();
    let mut bytes = text.as_bytes();
    while let Some(at) = memchr(b'\r', bytes) {
        output.extend_from_slice(&bytes[..at]);
        output.push(b'\n');
        bytes = &bytes[at + 1..];
        bytes = bytes.strip_prefix(b"\n").unwrap_or(bytes);
    }
    output.extend_from_slice(bytes);
}

/// Seekable source windows. The index uses two offsets per 128 KiB of input.
/// Each window begins on a scalar and CRLF boundary, so it can be decoded on
/// demand with a fresh decoder; neither disk nor memory holds the decoded file.
#[derive(Debug)]
pub struct Windows {
    file: File,
    revision: Metadata,
    offsets: Vec<(usize, usize)>, // Encoded and normalized offsets, including an EOF sentinel.
    attempt: DecodeAttempt,
    plain: bool,
    lone_cr: bool,
    decoded: Option<Replay>,
    long_line: bool,
    first_candidate: usize,
    line_index: Vec<(usize, bool)>, // Completed line breaks and whether the block starts a row.
}

impl Windows {
    fn open(
        file: File,
        len: usize,
        options: &DecodeOptions,
        cancelled: impl Fn() -> bool,
        mut filter: Option<Filter<'_>>,
    ) -> io::Result<Option<Self>> {
        let revision = file.metadata()?;
        if revision.len() != len as u64 {
            return Err(changed());
        }
        let mut raw = vec![0; BLOCK + 4];
        read_at(&file, &mut raw[..8192], 0)?;
        'attempts: for attempt in plan(&raw[..8192], options)? {
            let mut required = Required::new(filter.as_mut());
            if !attempt.encoding.is_single_byte()
                && ![UTF_8, UTF_16BE, UTF_16LE]
                    .into_iter()
                    .any(|codec| attempt.encoding == codec)
                || attempt.encoding == UTF_8 && attempt.replacement
            {
                // Stateful/multibyte legacy codecs cannot restart at arbitrary
                // source offsets. Decode once with the editor's streaming decoder.
                let mut snapshot = Replay::new()?;
                let mut source = BufReader::with_capacity(BLOCK, file.try_clone()?);
                use io::Seek;
                source.seek(io::SeekFrom::Start(attempt.skip as u64))?;
                let mut progress = |_| {};
                let mut decoder = crate::DecodeReader::new(
                    source.take((len - attempt.skip) as u64),
                    attempt.encoding,
                    len as u64,
                    &mut progress,
                    options.default_line_ending,
                );
                decoder.allow_replacement = attempt.replacement;
                loop {
                    if cancelled() {
                        return Ok(None);
                    }
                    match decoder.read(&mut raw) {
                        Ok(0) => break,
                        Ok(n) => snapshot.append(&raw[..n])?,
                        Err(e) if e.kind() == io::ErrorKind::InvalidData => continue 'attempts,
                        Err(e) => return Err(e),
                    }
                }
                verify_revision(&file, &revision)?;
                let mut offsets = Vec::new();
                let mut at = 0;
                let mut lines = LineWidth::default();
                while at < snapshot.len {
                    if cancelled() {
                        return Ok(None);
                    }
                    let n = (snapshot.len - at).min(raw.len());
                    snapshot.read(at, &mut raw[..n])?;
                    let cut = utf8_boundary(&raw[..n]);
                    if memchr(0, &raw[..cut]).is_some() {
                        return Ok(None);
                    }
                    required.push(&raw[..cut], at);
                    lines.push(&raw[..cut]);
                    offsets.push((at, at));
                    at += cut;
                }
                offsets.push((at, at));
                return Ok(required.found.then_some(Self {
                    file,
                    revision,
                    offsets,
                    attempt,
                    plain: true,
                    lone_cr: false,
                    decoded: Some(snapshot),
                    long_line: lines.long,
                    first_candidate: required.first,
                    line_index: Vec::new(),
                }));
            }
            let mut offsets = Vec::with_capacity(len.div_ceil(BLOCK) + 2);
            let mut at = attempt.skip;
            let mut normalized = 0;
            let mut plain = attempt.encoding == UTF_8;
            let mut lone_cr = false;
            let mut text = Vec::new();
            let mut lines = LineWidth::default();
            let mut deferred = false;
            let project = required.filter.as_ref().is_some_and(|f| !f.line_local);
            let mut line_index = if project {
                Vec::with_capacity(offsets.capacity())
            } else {
                Vec::new()
            };
            let mut completed = 0;
            let mut starts_row = true;
            while at < len {
                if cancelled() {
                    return Ok(None);
                }
                let n = (len - at).min(raw.len());
                read_at(&file, &mut raw[..n], at)?;
                let cut = boundary(&raw[..n], &attempt);
                let Some(s) = decode(&raw[..cut], &attempt) else {
                    continue 'attempts;
                };
                if memchr(0, s.as_bytes()).is_some() {
                    return Ok(None);
                }
                offsets.push((at, normalized));
                if project {
                    line_index.push((completed, starts_row));
                }
                at += cut;
                let bytes = s.as_bytes();
                lines.push(bytes);
                required.push_encoded(&s, normalized, &mut text);
                if memchr(b'\r', bytes).is_some() {
                    plain = false;
                    if !required.found && required.filter.as_ref().is_some_and(|f| f.line_local) {
                        deferred = true;
                        // We only use offsets after a hit. In that uncommon
                        // case, finish the index from the stable source below.
                        continue;
                    }
                    let mut pairs = 0;
                    let mut lone = 0;
                    for cr in memchr::memchr_iter(b'\r', bytes) {
                        let pair = bytes.get(cr + 1) == Some(&b'\n');
                        pairs += usize::from(pair);
                        lone += usize::from(!pair);
                    }
                    lone_cr |= lone != 0;
                    completed += lone;
                    normalized += bytes.len() - pairs;
                } else {
                    normalized += bytes.len();
                }
                if project {
                    completed += memchr::memchr_iter(b'\n', bytes).count();
                    starts_row = matches!(bytes.last(), Some(b'\n' | b'\r'));
                }
            }
            verify_revision(&file, &revision)?;
            if required.found && deferred {
                let source = Self::open(file, len, options, cancelled, None)?;
                if let Some(source) = &source {
                    verify_revision(&source.file, &revision)?;
                }
                return Ok(source);
            }
            offsets.push((len, normalized));
            return Ok(required.found.then_some(Self {
                file,
                revision,
                offsets,
                attempt,
                plain,
                lone_cr,
                decoded: None,
                long_line: lines.long,
                first_candidate: required.first,
                line_index,
            }));
        }
        Ok(None)
    }

    /// Number of uncompressed windows.
    pub fn blocks(&self) -> usize {
        self.offsets.len() - 1
    }

    /// Number of normalized UTF-8 bytes.
    pub fn len_bytes(&self) -> usize {
        self.offsets[self.blocks()].1
    }

    /// Whether contiguous line buffering would need more than a few windows.
    pub fn has_long_line(&self) -> bool {
        self.long_line
    }

    /// Provides the original bytes for fast line-oriented grep, when UTF-8 and
    /// the raw separators agree with the editor's normalized line model.
    pub fn grep_input(&self) -> io::Result<Option<(io::Take<File>, bool)>> {
        if self.attempt.encoding != UTF_8 || self.lone_cr || self.decoded.is_some() {
            return Ok(None);
        }
        use io::Seek;
        let mut file = self.file.try_clone()?;
        file.seek(io::SeekFrom::Start(self.attempt.skip as u64))?;
        Ok(Some((
            file.take(self.revision.len() - self.attempt.skip as u64),
            !self.plain,
        )))
    }

    /// Earliest block that a bounded match could start in. Retain enough of
    /// the real prefix for the full width and look-behind of the expression.
    pub fn first_candidate_block(&self, max_width: usize) -> usize {
        let first = self
            .first_candidate
            .saturating_sub(max_width.saturating_add(4));
        self.offsets
            .partition_point(|&(_, offset)| offset <= first)
            .saturating_sub(1)
    }

    /// Starts result projection with enough complete rows for leading context.
    /// Returns the block, the number of preceding breaks and whether it starts mid-row.
    pub fn projection_start(&self, first: usize, context: usize) -> (usize, usize, bool) {
        let Some(&(lines, _)) = self.line_index.get(first) else {
            return (0, 0, false);
        };
        let wanted = lines.saturating_sub(context.saturating_add(1));
        if wanted == 0 {
            return (0, 0, false);
        }
        let block = self.line_index[..first]
            .partition_point(|&(lines, _)| lines <= wanted)
            .saturating_sub(1);
        let (lines, starts_row) = self.line_index[block];
        (block, lines, !starts_row)
    }

    /// Normalized byte range of this block, for the regex cursor's replay cache.
    pub fn block_range(&self, block: usize) -> std::ops::Range<usize> {
        self.offsets[block].1..self.offsets[block + 1].1
    }

    /// Rejects a revision changed since the window index was built.
    pub fn verify(&self) -> io::Result<()> {
        verify_revision(&self.file, &self.revision)
    }

    /// Fills a window and returns its normalized offset. The caller may reuse
    /// `scratch` and `text` across windows. A failed read leaves `text` intact.
    pub fn read_block(
        &self,
        block: usize,
        scratch: &mut Vec<u8>,
        text: &mut Vec<u8>,
    ) -> io::Result<usize> {
        self.read_block_reuse(block, scratch, &mut Vec::new(), text)
    }

    /// Like `read_block`, reusing a second staging buffer for transcoding.
    pub fn read_block_reuse(
        &self,
        block: usize,
        scratch: &mut Vec<u8>,
        normal: &mut Vec<u8>,
        text: &mut Vec<u8>,
    ) -> io::Result<usize> {
        let (offset, decoded) = self.offsets[block];
        let (end, decoded_end) = self.offsets[block + 1];
        scratch.resize(end - offset, 0);
        if let Some(snapshot) = &self.decoded {
            snapshot.read(offset, scratch)?;
        } else {
            read_at(&self.file, scratch, offset).map_err(|e| {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    changed()
                } else {
                    e
                }
            })?;
        }
        if self.plain {
            if simdutf8::basic::from_utf8(scratch).is_err() || memchr2(0, b'\r', scratch).is_some()
            {
                return Err(changed());
            }
            std::mem::swap(scratch, text);
        } else {
            let s = decode(scratch, &self.attempt).ok_or_else(changed)?;
            if memchr(0, s.as_bytes()).is_some() {
                return Err(changed());
            }
            normalize(&s, normal);
            if normal.len() != decoded_end - decoded {
                return Err(changed());
            }
            std::mem::swap(normal, text);
        }
        Ok(decoded)
    }
}

#[derive(Default)]
struct LineWidth {
    previous: usize,
    long: bool,
}
impl LineWidth {
    fn push(&mut self, bytes: &[u8]) {
        if self.long {
            return;
        }
        if let Some(first) = memchr2(b'\r', b'\n', bytes) {
            self.long = self.previous + first > IN_MEMORY;
            self.previous =
                bytes.len() - 1 - memchr::memrchr2(b'\r', b'\n', bytes).unwrap_or(first);
        } else {
            self.previous += bytes.len();
            self.long = self.previous > IN_MEMORY;
        }
    }
}

struct Required<'a, 'b> {
    filter: Option<&'a mut Filter<'b>>,
    tail: Vec<u8>,
    joined: Vec<u8>,
    found: bool,
    first: usize,
}
impl<'a, 'b> Required<'a, 'b> {
    fn new(filter: Option<&'a mut Filter<'b>>) -> Self {
        Self {
            found: filter.is_none(),
            filter,
            tail: vec![],
            joined: vec![],
            first: 0,
        }
    }
    fn push_encoded(&mut self, text: &str, offset: usize, scratch: &mut Vec<u8>) {
        if self.found {
            return;
        }
        if self.filter.as_ref().is_some_and(|filter| filter.needs_lf)
            && memchr(b'\r', text.as_bytes()).is_some()
        {
            normalize(text, scratch);
            self.push(scratch, offset);
        } else {
            self.push(text.as_bytes(), offset);
        }
    }
    fn push(&mut self, bytes: &[u8], offset: usize) {
        let Some(filter) = self.filter.as_mut().filter(|_| !self.found) else {
            return;
        };
        if (filter.contains)(bytes) {
            self.found = true;
            // The overlap could contain an earlier occurrence than this block.
            self.first = offset.saturating_sub(filter.width.saturating_sub(1));
            return;
        }
        let width = filter.width.saturating_sub(1);
        if width == 0 {
            return;
        }
        self.joined.clear();
        self.joined.extend_from_slice(&self.tail);
        self.joined
            .extend_from_slice(&bytes[..bytes.len().min(width)]);
        self.found = (filter.contains)(&self.joined);
        if self.found {
            self.first = offset.saturating_sub(width);
            return;
        }
        if bytes.len() >= width {
            self.tail.clear();
            self.tail.extend_from_slice(&bytes[bytes.len() - width..]);
        } else {
            let drop = (self.tail.len() + bytes.len()).saturating_sub(width);
            self.tail.drain(..drop);
            self.tail.extend_from_slice(bytes);
        }
    }
}

fn verify_revision(file: &File, before: &Metadata) -> io::Result<()> {
    let now = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if now.ctime() != before.ctime() || now.ctime_nsec() != before.ctime_nsec() {
            return Err(changed());
        }
    }
    if now.len() != before.len() || now.modified().ok() != before.modified().ok() {
        return Err(changed());
    }
    Ok(())
}

fn changed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "file changed during search")
}

fn boundary(bytes: &[u8], attempt: &DecodeAttempt) -> usize {
    if bytes.len() <= BLOCK {
        return bytes.len();
    }
    let mut cut = BLOCK;
    if attempt.encoding == UTF_16LE || attempt.encoding == UTF_16BE {
        let unit = |at| {
            let pair = [bytes[at], bytes[at + 1]];
            if attempt.encoding == UTF_16LE {
                u16::from_le_bytes(pair)
            } else {
                u16::from_be_bytes(pair)
            }
        };
        if (0xd800..=0xdbff).contains(&unit(cut - 2))
            || (bytes.len() >= cut + 2 && unit(cut - 2) == 13 && unit(cut) == 10)
        {
            cut -= 2;
        }
    } else {
        if attempt.encoding == UTF_8 {
            cut = utf8_boundary(bytes);
        }
        if bytes[cut - 1] == b'\r' && bytes[cut] == b'\n' {
            cut -= 1;
        }
    }
    cut
}

fn utf8_boundary(bytes: &[u8]) -> usize {
    let mut cut = BLOCK.min(bytes.len());
    while cut < bytes.len() && cut > BLOCK - 3 && bytes[cut] & 0xc0 == 0x80 {
        cut -= 1;
    }
    cut
}

// Ordinary search never spills. The bounded shared quota is for stateful
// legacy decoding and exceptional regex attempts that need long-distance replay.
static SPILL_BYTES: AtomicUsize = AtomicUsize::new(0);
const SPILL_LIMIT: usize = 768 * 1024 * 1024;

/// Anonymous, size-limited immutable replay bytes for a search worker.
#[derive(Debug)]
pub struct Replay {
    file: File,
    len: usize,
}
impl Replay {
    /// Allocates an unlinked temporary file, with no persistent recovery artifact.
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            file: tempfile::tempfile()?,
            len: 0,
        })
    }
    /// Extends the cached region within the aggregate search spill budget.
    pub fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        SPILL_BYTES
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes.len()).filter(|&n| n <= SPILL_LIMIT)
            })
            .map_err(|_| io::Error::other("search replay limit exceeded"))?;
        self.len += bytes.len();
        // On Windows positional reads update the descriptor's seek position.
        use io::Seek;
        self.file
            .seek(io::SeekFrom::Start((self.len - bytes.len()) as u64))?;
        self.file.write_all(bytes)
    }
    /// Reads bytes already committed to the replay cache.
    pub fn read(&self, offset: usize, bytes: &mut [u8]) -> io::Result<()> {
        read_at(&self.file, bytes, offset)
    }
}
impl Drop for Replay {
    fn drop(&mut self) {
        SPILL_BYTES.fetch_sub(self.len, Ordering::Relaxed);
    }
}

fn read_at(file: &File, mut out: &mut [u8], mut offset: usize) -> io::Result<()> {
    while !out.is_empty() {
        #[cfg(unix)]
        let result = std::os::unix::fs::FileExt::read_at(file, out, offset as u64);
        #[cfg(windows)]
        // The two logical cursors use the same worker; every access sets the position.
        let result = std::os::windows::fs::FileExt::seek_read(file, out, offset as u64);
        match result {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                offset += n;
                out = &mut out[n..];
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_read_does_not_overwrite_the_current_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        for prefix in ["hello\n", "hello\r\n"] {
            std::fs::write(&path, prefix.repeat(IN_MEMORY / prefix.len() + 800)).unwrap();
            let mut reader = Reader::default();
            let Some(Text::Windows(source)) = reader.open(&path, || false).unwrap() else {
                panic!("expected windows");
            };
            let mut scratch = Vec::new();
            let mut current = Vec::new();
            source.read_block(0, &mut scratch, &mut current).unwrap();
            let before = current.clone();
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len((BLOCK + 1) as u64)
                .unwrap();
            assert!(source.read_block(1, &mut scratch, &mut current).is_err());
            assert_eq!(current, before);
        }
    }

    #[test]
    fn window_prefilter_checks_overlaps_after_newline_normalization() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let mut text = "x".repeat(BLOCK - 4);
        text += "one\r\ntwo\n";
        text += &"other\n".repeat(IN_MEMORY / 6);
        std::fs::write(&path, text).unwrap();
        let mut reader = Reader::default();
        for (needle, expect) in [(b"one\ntwo", true), (b"one\nzzz", false)] {
            let mut contains = |bytes: &[u8]| memchr::memmem::find(bytes, needle).is_some();
            let input = reader
                .open_with_options(
                    &path,
                    &DecodeOptions::default(),
                    || false,
                    Some(Filter {
                        width: needle.len(),
                        needs_lf: true,
                        line_local: false,
                        contains: &mut contains,
                    }),
                )
                .unwrap();
            assert_eq!(input.is_some(), expect);
        }
    }

    #[test]
    fn seekable_windows_match_editor_decoding_without_a_decoded_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        for (voice, pattern, pad) in [
            ("utf8", "😀\r\ncafé\rvalue\n", BLOCK - 1),
            ("legacy", "café\r\nvalue\r", BLOCK - 4),
            ("utf16le", "😀\r\ncafé\rvalue\n", BLOCK / 2 - 1),
            ("utf16be", "a\r\n😀\rvalue\n", BLOCK / 2 - 2),
            ("sjis", "日本語\r\nvalue\r", BLOCK - 1),
        ] {
            let normalized = format!("{}{pattern}", "x".repeat(pad));
            let mut bytes = match voice {
                "utf16le" => vec![0xff, 0xfe],
                "utf16be" => vec![0xfe, 0xff],
                _ => vec![],
            };
            let part: Vec<u8> = match voice {
                "utf16le" => normalized
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
                "utf16be" => normalized
                    .encode_utf16()
                    .flat_map(u16::to_be_bytes)
                    .collect(),
                "legacy" => normalized.chars().map(|c| c as u8).collect(),
                "sjis" => encoding_rs::SHIFT_JIS.encode(&normalized).0.into_owned(),
                _ => normalized.into_bytes(),
            };
            while bytes.len() <= IN_MEMORY {
                bytes.extend_from_slice(&part);
            }
            std::fs::write(&path, bytes).unwrap();
            let options = DecodeOptions {
                fallback_encoding: if voice == "sjis" {
                    "Japanese (Shift JIS)"
                } else {
                    "Western (Windows 1252)"
                }
                .into(),
                ..DecodeOptions::default()
            };
            let expected = crate::load_with_options(&path, &options, |_| {})
                .unwrap()
                .text
                .to_string();
            let mut reader = Reader::default();
            let Some(Text::Windows(input)) = reader
                .open_with_options(&path, &options, || false, None)
                .unwrap()
            else {
                panic!("{voice} did not use windows");
            };
            let mut raw = Vec::new();
            let mut text = Vec::new();
            assert_eq!(input.len_bytes(), expected.len());
            let mut end = expected.len();
            for block in (0..input.blocks()).rev() {
                let start = input.read_block(block, &mut raw, &mut text).unwrap();
                assert_eq!(
                    text,
                    expected.as_bytes()[start..end],
                    "{voice}, block {block}"
                );
                end = start;
            }
            assert_eq!(end, 0);
        }
    }
}
