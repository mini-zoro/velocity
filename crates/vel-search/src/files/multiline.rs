//! Exact matching on bounded UTF-8 windows, including unbounded regex repeats.
use super::Line;
use crate::text::Search;
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, SearcherBuilder, Sink, SinkContext, SinkMatch};
use memchr::{memchr, memmem};
use regex_cursor::Cursor;
use regex_syntax::hir::{Class, Hir, HirKind};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    io::{self, BufRead, Read},
    ops::Range,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct Multiline {
    pub(super) required: Option<RegexMatcher>,
    max_width: Option<usize>,
}

pub(super) enum Prefilter {
    Literal(memmem::Finder<'static>),
    AsciiFolded { needle: Vec<u8>, pivot: usize },
    Folded(RegexMatcher, usize),
}
impl Prefilter {
    pub(super) fn new(search: &Search) -> Option<Self> {
        let hir = regex_syntax::ParserBuilder::new()
            .build()
            .parse(&Search::pattern(&search.query, search.options))
            .ok()?;
        let required = match hir.kind() {
            HirKind::Literal(_) => hir,
            _ => required(&hir)?,
        };
        let width = required.properties().maximum_len()?;
        if !(3..=4096).contains(&width) {
            return None;
        }
        if let HirKind::Literal(literal) = required.kind() {
            Some(Self::Literal(memmem::Finder::new(&literal.0).into_owned()))
        } else {
            let mut needle = Vec::new();
            if ascii_folded(&required, &mut needle) {
                // A rare byte is useful even when every other byte in the
                // candidate is common. The full regex still decides each hit.
                let order = b"zqxjvkwyfgpbmhdulcrantsioe ";
                let pivot = (0..needle.len())
                    .min_by_key(|&i| order.iter().position(|&c| c == needle[i]).unwrap_or(0))
                    .expect("nonempty required literal");
                Some(Self::AsciiFolded { needle, pivot })
            } else {
                Some(Self::Folded(
                    RegexMatcherBuilder::new()
                        .build(&required.to_string())
                        .ok()?,
                    width,
                ))
            }
        }
    }
    pub(super) fn width(&self) -> usize {
        match self {
            Self::Literal(finder) => finder.needle().len(),
            Self::AsciiFolded { needle, .. } => needle.len(),
            Self::Folded(_, width) => *width,
        }
    }
    pub(super) fn needs_lf(&self) -> bool {
        match self {
            Self::Literal(finder) => memchr::memchr2(b'\n', b'\r', finder.needle()).is_some(),
            Self::AsciiFolded { .. } | Self::Folded(_, _) => false,
        }
    }
    pub(super) fn contains(&self, bytes: &[u8]) -> bool {
        match self {
            Self::Literal(finder) => finder.find(bytes).is_some(),
            Self::AsciiFolded { needle, pivot } => {
                if bytes.len() < needle.len() {
                    return false;
                }
                let lower = needle[*pivot];
                let upper = lower.to_ascii_uppercase();
                let possible = bytes.len() - needle.len() + 1;
                memchr::memchr2_iter(lower, upper, &bytes[*pivot..*pivot + possible])
                    .any(|i| bytes[i..i + needle.len()].eq_ignore_ascii_case(needle))
            }
            Self::Folded(regex, _) => regex.is_match(bytes).unwrap_or(true),
        }
    }
}

fn ascii_folded(hir: &Hir, out: &mut Vec<u8>) -> bool {
    let folded = |c: char| {
        u8::try_from(c)
            .ok()
            .filter(u8::is_ascii)
            .map(|b| b.to_ascii_lowercase())
    };
    match hir.kind() {
        HirKind::Literal(literal) if literal.0.is_ascii() => {
            out.extend(literal.0.iter().map(u8::to_ascii_lowercase));
            true
        }
        HirKind::Class(Class::Unicode(class)) => {
            let Some(c) = class.ranges().first().and_then(|r| folded(r.start())) else {
                return false;
            };
            if class
                .iter()
                .any(|r| r.start() != r.end() || folded(r.start()) != Some(c))
            {
                return false;
            }
            out.push(c);
            true
        }
        HirKind::Concat(parts) => parts.iter().all(|h| ascii_folded(h, out)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_with_newline_subtracted_are_line_local() {
        let options = crate::text::Options {
            regex: true,
            ..Default::default()
        };
        for (query, multiline) in [
            (r"return[^\S\n]+value", false),
            (r"return[\W&&[^\n]]value", false),
            (r"return[^\n]+value", false),
            (r"return\s+value", true),
            (r"return\Wvalue", true),
            (r"return[^;]value", true),
        ] {
            let search = Search::new(query, options).unwrap();
            assert_eq!(
                Multiline::for_search(&search).is_some(),
                multiline,
                "{query}"
            );
        }
    }

    #[test]
    fn cancellation_and_a_failed_frontier_leave_admitted_windows_replayable() {
        use std::io::{Seek, SeekFrom, Write};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let bytes = b"alpha beta\n".repeat(6 * 1024 * 1024 / 11);
        for cancel in [true, false] {
            std::fs::write(&path, &bytes).unwrap();
            let mut reader = vel_fileio::search::Reader::default();
            let Some(vel_fileio::search::Text::Windows(source)) =
                reader.open(&path, || false).unwrap()
            else {
                panic!("expected windows");
            };
            let stop = AtomicBool::new(false);
            let accepted = Cell::new(0);
            let error = RefCell::new(None);
            let mut cursor = Windows::new(&source, &stop, &error, &accepted).unwrap();
            let first = cursor.chunk().to_vec();
            assert!(cursor.advance());
            let second = cursor.chunk().to_vec();
            if cancel {
                stop.store(true, Ordering::Relaxed);
            } else {
                let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                f.seek(SeekFrom::Start(0)).unwrap();
                f.write_all(b"rewritten").unwrap();
                f.set_len((2 * second.len() + 17) as u64).unwrap();
            }
            assert!(!cursor.advance());
            assert_eq!(error.borrow().is_some(), !cancel);
            assert_eq!(cursor.chunk(), second);
            assert!(cursor.backtrack());
            assert_eq!(cursor.chunk(), first);
            assert!(cursor.advance());
            assert_eq!(cursor.chunk(), second);
        }
    }

    #[test]
    fn greedy_fallback_replays_more_than_the_memory_cache() {
        // The first branch reads beyond the cache. Only then can the engine
        // return the shorter alternative and continue near the beginning.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let bytes = format!("alpha\nbeta\n{}\ndone", "x".repeat(5 * 1024 * 1024));
        std::fs::write(&path, &bytes).unwrap();
        let search = Search::new(
            r"(?s:alpha.*missing)|alpha|beta|done",
            crate::text::Options {
                regex: true,
                case_sensitive: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut reader = vel_fileio::search::Reader::default();
        let input = reader.open(&path, || false).unwrap().unwrap();
        let mut actual = Vec::new();
        scan(
            input,
            &path,
            Query {
                search: &search,
                regex: &search.contiguous().unwrap(),
                multiline: Multiline::for_search(&search).as_ref(),
            },
            0,
            &AtomicBool::new(false),
            |line| {
                actual.push((line.number, line.text, line.highlights));
                true
            },
        )
        .unwrap();
        let highlight = |end| std::iter::once(0..end).collect::<Vec<_>>();
        assert_eq!(
            actual,
            [
                (1, "alpha".into(), highlight(5)),
                (2, "beta".into(), highlight(4)),
                (4, "done".into(), highlight(4)),
            ]
        );
    }
}
impl Multiline {
    pub(super) fn for_search(search: &Search) -> Option<Self> {
        let query = &search.query;
        let (required, fixed, max_width) = if search.options.regex {
            let hir = regex_syntax::ParserBuilder::new()
                .multi_line(true)
                .case_insensitive(!search.options.case_sensitive)
                .build()
                .parse(query)
                .ok()?;
            if !consumes_lf(&hir) {
                return None;
            }
            (
                required(&hir).map(|h| h.to_string()),
                false,
                hir.properties().maximum_len(),
            )
        } else {
            if !query.contains('\n') {
                return None;
            }
            (
                query
                    .split(['\r', '\n'])
                    .filter(|s| !s.is_empty() && s.is_ascii())
                    .max_by_key(|s| s.len())
                    .map(str::to_owned),
                true,
                Some(query.len().saturating_mul(4)),
            )
        };
        let required = required.and_then(|fragment| {
            RegexMatcherBuilder::new()
                .case_insensitive(fixed && !search.options.case_sensitive)
                .fixed_strings(fixed)
                .line_terminator(Some(b'\n'))
                .build(&fragment)
                .ok()
        });
        Some(Self {
            required,
            max_width,
        })
    }
}

pub(super) struct Greps {
    lf: RegexMatcher,
    crlf: Option<RegexMatcher>,
}
impl Greps {
    pub(super) fn new(search: &Search) -> Option<Self> {
        use regex_syntax::hir::Look;
        let pattern = Search::pattern(&search.query, search.options);
        let hir = regex_syntax::Parser::new().parse(&pattern).ok()?;
        let look = hir.properties().look_set();
        // grep matches a physical line at a time. Keep absolute anchors,
        // and possible empty matches at the final separator, on the batch path.
        if look.contains(Look::Start)
            || look.contains(Look::End)
            || hir.properties().minimum_len() == Some(0)
        {
            return None;
        }
        let mut builder = RegexMatcherBuilder::new();
        let lf = builder.line_terminator(Some(b'\n')).build(&pattern).ok()?;
        let crlf = builder.crlf(true).build(&pattern).ok();
        Some(Self { lf, crlf })
    }

    fn grep(
        &self,
        source: &vel_fileio::search::Windows,
        path: &Path,
        context: usize,
        stop: &AtomicBool,
        emit: impl FnMut(Line) -> bool,
    ) -> io::Result<()> {
        let grep = source.grep_input()?;
        let is_crlf = grep.as_ref().is_some_and(|(_, crlf)| *crlf) && self.crlf.is_some();
        let matcher = if is_crlf {
            self.crlf.as_ref().expect("CRLF matcher")
        } else {
            &self.lf
        };
        let mut searcher = SearcherBuilder::new()
            .line_terminator(if is_crlf {
                grep_matcher::LineTerminator::crlf()
            } else {
                grep_matcher::LineTerminator::byte(b'\n')
            })
            .bom_sniffing(false)
            .line_number(true)
            .before_context(context)
            .after_context(context)
            .binary_detection(BinaryDetection::quit(0))
            // Every line here is at most `IN_MEMORY` (4 MiB; longer files take the
            // bounded cursor), but with context the searcher keeps the preceding
            // lines too, and its buffer grows by doubling. Bound it by the lines it
            // can hold at once instead of a fixed 8 MiB, which silently truncated
            // results after a few ~3 MB lines with context 1.
            .heap_limit(Some((2 * context + 3) * 2 * 4 * 1024 * 1024))
            .build();
        let sink = GrepSink {
            path,
            shared: None,
            stop,
            emit,
            matcher,
            is_crlf,
        };
        if let Some((raw, crlf)) = grep
            && crlf == is_crlf
        {
            searcher.search_reader(matcher, CancelRead(raw, stop), sink)
        } else {
            searcher.search_reader(
                matcher,
                CancelRead(
                    BlockReader {
                        source: Some(source),
                        ..Default::default()
                    },
                    stop,
                ),
                sink,
            )
        }
    }
}

struct CancelRead<'a, R>(R, &'a AtomicBool);
impl<R: Read> Read for CancelRead<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.1.load(Ordering::Relaxed) {
            return Ok(0);
        }
        self.0.read(out)
    }
}

struct GrepSink<'a, F> {
    path: &'a Path,
    shared: Option<Arc<Path>>,
    stop: &'a AtomicBool,
    emit: F,
    matcher: &'a RegexMatcher,
    is_crlf: bool,
}
impl<F: FnMut(Line) -> bool> GrepSink<'_, F> {
    fn emit(&mut self, bytes: &[u8], number: Option<u64>, matched: bool) -> io::Result<bool> {
        if self.stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let bytes = if self.is_crlf {
            bytes.strip_suffix(b"\r").unwrap_or(bytes)
        } else {
            bytes
        };
        let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
        let mut highlights = Vec::new();
        if matched {
            self.matcher
                .find_iter(bytes, |m| {
                    highlights.push(m.start()..m.end());
                    true
                })
                .map_err(io::Error::other)?;
        }
        let match_count = highlights.len();
        let (span, column_offset) = clip_highlights(text, &mut highlights);
        Ok((self.emit)(Line {
            path: self.shared.get_or_insert_with(|| self.path.into()).clone(),
            number: number.unwrap_or(1) as usize,
            text: text[span].to_owned(),
            highlights,
            column_offset,
            match_count,
        }))
    }
}
impl<F: FnMut(Line) -> bool> Sink for GrepSink<'_, F> {
    type Error = io::Error;
    fn matched(&mut self, _: &grep_searcher::Searcher, m: &SinkMatch<'_>) -> io::Result<bool> {
        self.emit(m.bytes(), m.line_number(), true)
    }
    fn context(&mut self, _: &grep_searcher::Searcher, row: &SinkContext<'_>) -> io::Result<bool> {
        self.emit(row.bytes(), row.line_number(), false)
    }
}

fn preview_span(text: &str, focus: usize) -> Range<usize> {
    const LIMIT: usize = 4096;
    if text.len() <= LIMIT {
        return 0..text.len();
    }
    let start = if focus < LIMIT / 2 {
        0
    } else {
        focus - LIMIT / 4
    };
    let start = text.ceil_char_boundary(start);
    start..text.floor_char_boundary((start + LIMIT).min(text.len()))
}

fn clip_highlights(text: &str, highlights: &mut Vec<Range<usize>>) -> (Range<usize>, usize) {
    let span = preview_span(text, highlights.first().map_or(0, |r| r.start));
    if span.start != 0 || span.end != text.len() {
        highlights.retain_mut(|r| {
            if r.start > span.end || r.end < span.start {
                return false;
            }
            *r = r.start.saturating_sub(span.start)..r.end.min(span.end) - span.start;
            true
        });
    }
    (span.clone(), text[..span.start].chars().count())
}

fn consumes_lf(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Literal(l) => l.0.contains(&b'\n'),
        HirKind::Class(Class::Unicode(c)) => c.iter().any(|r| r.start() <= '\n' && r.end() >= '\n'),
        HirKind::Class(Class::Bytes(c)) => c.iter().any(|r| r.start() <= b'\n' && r.end() >= b'\n'),
        _ => hir.kind().subs().iter().any(consumes_lf),
    }
}

// Only choose text that every match must contain. Optional and alternate
// branches don't qualify. Restrict literals to ASCII so the fast prefilter
// also works on the default legacy codec; fold classes preserve regex flags.
fn literal_token(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Literal(l) => l.0.is_ascii() && !l.0.contains(&b'\n') && !l.0.contains(&b'\r'),
        HirKind::Class(Class::Unicode(c)) => {
            c.ranges().len() <= 4
                && c.iter()
                    .all(|r| r.start() == r.end() && r.start().is_ascii_alphanumeric())
        }
        HirKind::Class(Class::Bytes(c)) => {
            c.ranges().len() <= 4
                && c.iter()
                    .all(|r| r.start() == r.end() && r.start().is_ascii_alphanumeric())
        }
        _ => false,
    }
}
fn required(hir: &Hir) -> Option<Hir> {
    if literal_token(hir) {
        return Some(hir.clone());
    }
    match hir.kind() {
        HirKind::Literal(l) => {
            l.0.split(|b| matches!(b, b'\r' | b'\n'))
                .filter(|s| !s.is_empty() && s.is_ascii())
                .max_by_key(|s| s.len())
                .map(Hir::literal)
        }
        HirKind::Capture(c) => required(&c.sub),
        HirKind::Repetition(r) if r.min != 0 => required(&r.sub),
        HirKind::Concat(parts) => {
            let mut best = None;
            let mut run = Vec::new();
            let mut keep = |h: Hir| {
                if h.properties().minimum_len()
                    > best
                        .as_ref()
                        .and_then(|b: &Hir| b.properties().minimum_len())
                {
                    best = Some(h);
                }
            };
            for h in parts {
                if literal_token(h) {
                    run.push(h.clone());
                } else {
                    if !run.is_empty() {
                        keep(Hir::concat(std::mem::take(&mut run)));
                    }
                    if let Some(h) = required(h) {
                        keep(h);
                    }
                }
            }
            if !run.is_empty() {
                keep(Hir::concat(run));
            }
            best
        }
        _ => None,
    }
}

// The regex engine may replay past EOF, both to recover the start of a match
// and to resume after a greedy alternative. Only the advancing frontier reads
// mutable source bytes. Keep one look-behind block before the last accepted hit.
struct Windows<'a> {
    source: &'a vel_fileio::search::Windows,
    stop: &'a AtomicBool,
    error: &'a RefCell<Option<io::Error>>,
    accepted: &'a Cell<usize>,
    block: usize,
    offset: usize,
    first: usize,
    cache: VecDeque<Vec<u8>>,
    cached: usize,
    spill: Option<(usize, vel_fileio::search::Replay)>,
    replay: Vec<u8>,
    raw: Vec<u8>,
    spare: Vec<u8>,
}
impl<'a> Windows<'a> {
    #[cfg(test)]
    fn new(
        source: &'a vel_fileio::search::Windows,
        stop: &'a AtomicBool,
        error: &'a RefCell<Option<io::Error>>,
        accepted: &'a Cell<usize>,
    ) -> io::Result<Self> {
        Self::from_block(source, stop, error, accepted, 0)
    }
    fn from_block(
        source: &'a vel_fileio::search::Windows,
        stop: &'a AtomicBool,
        error: &'a RefCell<Option<io::Error>>,
        accepted: &'a Cell<usize>,
        start: usize,
    ) -> io::Result<Self> {
        let first = start.saturating_sub(1);
        let mut result = Self {
            source,
            stop,
            error,
            accepted,
            block: 0,
            offset: 0,
            first,
            cache: VecDeque::new(),
            cached: 0,
            spill: None,
            replay: Vec::new(),
            raw: Vec::new(),
            spare: Vec::new(),
        };
        result.admit(first)?;
        if first != start {
            result.admit(start)?;
        }
        Ok(result)
    }
    fn admit(&mut self, block: usize) -> io::Result<()> {
        const CACHE: usize = 4 * 1024 * 1024;
        self.source.verify()?;
        let mut bytes = Vec::new();
        let offset =
            self.source
                .read_block_reuse(block, &mut self.raw, &mut self.spare, &mut bytes)?;
        self.source.verify()?;
        // Retire only blocks older than the current attempt, including its
        // look-behind. A rare attempt longer than RAM can replay anonymously.
        let keep = self.accepted.get().saturating_sub(4);
        if self.spill.is_some() && self.source.block_range(self.first).start <= keep {
            self.spill = None;
        }
        while self.cached + bytes.len() > CACHE && self.cache.len() > 1 {
            let start = self.source.block_range(self.first);
            if start.end > keep {
                if self.spill.is_none() {
                    self.spill = Some((start.start, vel_fileio::search::Replay::new()?));
                }
                let (_, spill) = self.spill.as_mut().expect("spill initialized");
                spill.append(self.cache.front().expect("retained window"))?;
            }
            let retired = self.cache.pop_front().expect("retained window");
            self.cached -= retired.len();
            self.first += 1;
        }
        self.cached += bytes.len();
        self.cache.push_back(bytes);
        self.offset = offset;
        self.block = block;
        Ok(())
    }

    fn replay(&mut self, block: usize) {
        let range = self.source.block_range(block);
        if block < self.first {
            let (start, cache) = self.spill.as_ref().expect("window remains replayable");
            self.replay.resize(range.len(), 0);
            if let Err(error) = cache.read(range.start - start, &mut self.replay) {
                // An I/O failure of our anonymous backing cannot impersonate
                // BOF under the infallible Cursor API. Unwind to scan's error boundary.
                std::panic::resume_unwind(Box::new(error));
            }
        }
        self.block = block;
        self.offset = range.start;
    }
}
fn continuation(b: u8) -> bool {
    b & 0xc0 == 0x80
}
impl Cursor for Windows<'_> {
    fn chunk(&self) -> &[u8] {
        if self.block < self.first {
            &self.replay
        } else {
            &self.cache[self.block - self.first]
        }
    }
    fn advance(&mut self) -> bool {
        let block = self.block + 1;
        if block < self.first + self.cache.len() {
            self.replay(block);
            return true;
        }
        if block == self.source.blocks()
            || self.stop.load(Ordering::Relaxed)
            || self.error.borrow().is_some()
        {
            return false;
        }
        match self.admit(block) {
            Ok(()) => true,
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                false
            }
        }
    }
    fn backtrack(&mut self) -> bool {
        if self.block == 0 {
            return false;
        }
        self.replay(self.block - 1);
        true
    }
    fn total_bytes(&self) -> Option<usize> {
        Some(self.source.len_bytes())
    }
    fn offset(&self) -> usize {
        self.offset
    }
}

#[derive(Default)]
struct BlockReader<'a> {
    source: Option<&'a vel_fileio::search::Windows>,
    block: usize,
    raw: Vec<u8>,
    spare: Vec<u8>,
    text: Vec<u8>,
    at: usize,
}

impl Read for BlockReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let buf = self.fill_buf()?;
        let n = buf.len().min(out.len());
        out[..n].copy_from_slice(&buf[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for BlockReader<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.at == self.text.len()
            && let Some(source) = self.source
            && self.block < source.blocks()
        {
            source.read_block_reuse(self.block, &mut self.raw, &mut self.spare, &mut self.text)?;
            self.block += 1;
            self.at = 0;
        }
        Ok(&self.text[self.at..])
    }

    fn consume(&mut self, amount: usize) {
        self.at += amount;
    }
}

struct Lines<'a, B> {
    file: B,
    path: &'a Path,
    shared: Option<Arc<Path>>,
    offset: usize,
    number: usize,
}
impl<B: BufRead> Lines<'_, B> {
    fn discard_partial_row(&mut self) -> io::Result<()> {
        loop {
            let bytes = self.file.fill_buf()?;
            if bytes.is_empty() {
                return Ok(());
            }
            let newline = memchr(b'\n', bytes);
            let n = newline.map_or(bytes.len(), |i| i + 1);
            self.file.consume(n);
            self.offset += n;
            if newline.is_some() {
                self.number += 1;
                return Ok(());
            }
        }
    }
    // Jump over unmatched lines but keep the requested context before the
    // first matching row. This uses vectorized newline counts on full windows.
    fn skip_before(&mut self, focus: usize, context: usize) -> io::Result<bool> {
        let bytes = self.file.fill_buf()?;
        let prefix = &bytes[..(focus - self.offset).min(bytes.len())];
        if let Some(last) = memchr::memrchr_iter(b'\n', prefix).nth(context) {
            self.number += memchr::memchr_iter(b'\n', &prefix[..last]).count() + 1;
            self.file.consume(last + 1);
            self.offset += last + 1;
            return Ok(true);
        }
        Ok(false)
    }
    // Keep a bounded preview around the first hit, or the start of a context
    // line. Count preceding Unicode scalars so F4 still jumps to the hit.
    fn next(&mut self, focus: usize) -> io::Result<(usize, Range<usize>, Line, bool)> {
        const PREVIEW: usize = 4096;
        let start = self.offset;
        let bytes = self.file.fill_buf()?;
        if let Some(n) = memchr(b'\n', &bytes[..bytes.len().min(PREVIEW + 1)]) {
            let text = std::str::from_utf8(&bytes[..n])
                .expect("validated line")
                .to_owned();
            self.file.consume(n + 1);
            self.offset += n + 1;
            self.number += 1;
            return Ok((start, start..self.offset, self.line(text, 0), true));
        }
        let from = if focus.saturating_sub(start) >= PREVIEW / 2 {
            focus.saturating_sub(PREVIEW / 4)
        } else {
            start
        };
        let mut prefix = Vec::new();
        let mut preview = Vec::new();
        let mut column = 0;
        let mut terminated = false;
        loop {
            let bytes = self.file.fill_buf()?;
            if bytes.is_empty() {
                break;
            }
            let newline = memchr(b'\n', bytes);
            let slice = &bytes[..newline.unwrap_or(bytes.len())];
            let x = from.saturating_sub(self.offset).min(slice.len());
            if from > start {
                prefix.extend_from_slice(&slice[..slice.len().min(PREVIEW - prefix.len())]);
                column += slice[..x].iter().filter(|&&b| !continuation(b)).count();
            }
            preview.extend_from_slice(&slice[x..slice.len().min(x + PREVIEW - preview.len())]);
            let take = slice.len() + usize::from(newline.is_some());
            self.file.consume(take);
            self.offset += take;
            if newline.is_some() {
                terminated = true;
                break;
            }
        }
        let (bytes, origin, column) =
            if from > start && (focus > self.offset || (focus == self.offset && terminated)) {
                (prefix, start, 0)
            } else {
                let skip = preview.iter().take_while(|&&b| continuation(b)).count();
                if skip != 0 {
                    preview.drain(..skip);
                }
                (preview, from + skip, column)
            };
        let len = std::str::from_utf8(&bytes).map_or_else(|e| e.valid_up_to(), str::len);
        let mut bytes = bytes;
        bytes.truncate(len);
        self.number += 1;
        Ok((
            origin,
            start..self.offset,
            self.line(String::from_utf8(bytes).expect("validated preview"), column),
            terminated,
        ))
    }

    fn line(&mut self, text: String, column_offset: usize) -> Line {
        let path = self.shared.get_or_insert_with(|| self.path.into()).clone();
        Line {
            path,
            number: self.number,
            text,
            highlights: Vec::new(),
            column_offset,
            match_count: 0,
        }
    }
}

pub(super) struct Query<'a> {
    pub(super) search: &'a Search,
    pub(super) regex: &'a regex::Regex,
    pub(super) multiline: Option<&'a Multiline>,
}

struct Projection<'a> {
    path: &'a Path,
    eof_row: bool,
    offset: usize,
    number: usize,
    partial_row: bool,
}
impl<'a> Projection<'a> {
    fn new(path: &'a Path, eof_row: bool) -> Self {
        Self {
            path,
            eof_row,
            offset: 0,
            number: 0,
            partial_row: false,
        }
    }
}

#[cfg(test)]
pub(super) fn scan(
    input: vel_fileio::search::Text<'_>,
    path: &Path,
    query: Query<'_>,
    context: usize,
    stop: &AtomicBool,
    emit: impl FnMut(Line) -> bool,
) -> io::Result<()> {
    let greps = query
        .multiline
        .is_none()
        .then(|| Greps::new(query.search))
        .flatten();
    scan_prepared(input, path, query, greps.as_ref(), context, stop, emit)
}

pub(super) fn scan_prepared(
    input: vel_fileio::search::Text<'_>,
    path: &Path,
    Query {
        search,
        regex,
        multiline,
    }: Query<'_>,
    greps: Option<&Greps>,
    context: usize,
    stop: &AtomicBool,
    emit: impl FnMut(Line) -> bool,
) -> io::Result<()> {
    match input {
        vel_fileio::search::Text::Slice(s) => {
            if multiline
                .and_then(|m| m.required.as_ref())
                .is_some_and(|filter| filter.is_match(s.as_bytes()).is_ok_and(|found| !found))
            {
                return Ok(());
            }
            project(
                s.len(),
                s.as_bytes(),
                regex.find_iter(s).map(|m| m.range()),
                Projection::new(path, multiline.is_some()),
                context,
                stop,
                emit,
            )
        }
        vel_fileio::search::Text::Windows(source) => {
            source.verify()?;
            if stop.load(Ordering::Relaxed) || source.len_bytes() == 0 {
                return Ok(());
            }
            if multiline.is_none() && !source.has_long_line() {
                if let Some(greps) = greps {
                    greps.grep(&source, path, context, stop, emit)?;
                } else {
                    scan_lines(&source, path, regex, context, stop, emit)?;
                }
                return source.verify();
            }
            let error = RefCell::new(None);
            let start = multiline
                .and_then(|m| m.max_width)
                .map_or(0, |width| source.first_candidate_block(width));
            let accepted = Cell::new(source.block_range(start).start);
            let windows = Windows::from_block(&source, stop, &error, &accepted, start)?;
            let (block, number, partial_row) = source.projection_start(start, context);
            let projection = Projection {
                offset: source.block_range(block).start,
                number,
                partial_row,
                ..Projection::new(path, multiline.is_some())
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                project(
                    source.len_bytes(),
                    StableReader(
                        &source,
                        BlockReader {
                            source: Some(&source),
                            block,
                            ..Default::default()
                        },
                    ),
                    search.matches_in(windows).map_while(|hit| {
                        if stop.load(Ordering::Relaxed) || error.borrow().is_some() {
                            return None;
                        }
                        accepted.set(hit.end);
                        Some(hit)
                    }),
                    projection,
                    context,
                    stop,
                    emit,
                )
            }));
            match result {
                Ok(result) => result?,
                Err(payload) => match payload.downcast::<io::Error>() {
                    Ok(error) => return Err(*error),
                    Err(payload) => std::panic::resume_unwind(payload),
                },
            }
            if let Some(error) = error.into_inner() {
                return Err(error);
            }
            source.verify()
        }
    }
}

// Projection must not apply a hit to bytes from a different file revision.
struct StableReader<'a>(&'a vel_fileio::search::Windows, BlockReader<'a>);
impl Read for StableReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let bytes = self.fill_buf()?;
        let n = bytes.len().min(out.len());
        out[..n].copy_from_slice(&bytes[..n]);
        self.consume(n);
        Ok(n)
    }
}
impl BufRead for StableReader<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let next_block = self.1.at == self.1.text.len();
        if next_block {
            self.0.verify()?;
        }
        let bytes = self.1.fill_buf()?;
        if next_block {
            self.0.verify()?;
        }
        Ok(bytes)
    }
    fn consume(&mut self, amount: usize) {
        self.1.consume(amount);
    }
}

// A line-local pattern can't consume the separator. Search batches of whole
// lines with the contiguous engine (and its literal prefilter), rather than
// running the rope engine for every match. Keep absolute anchors anchored to
// file edges: the leading LF is real left context, but isn't searched.
fn scan_lines(
    source: &vel_fileio::search::Windows,
    path: &Path,
    regex: &regex::Regex,
    context: usize,
    stop: &AtomicBool,
    mut emit: impl FnMut(Line) -> bool,
) -> io::Result<()> {
    let mut reader = BlockReader {
        source: Some(source),
        ..Default::default()
    };
    let mut batch = Vec::new();
    let mut number = 1;
    let mut recent = VecDeque::new();
    let mut after = 0;
    let mut shared = None::<Arc<Path>>;
    while !stop.load(Ordering::Relaxed) {
        let bytes = reader.fill_buf()?;
        let eof = bytes.is_empty();
        batch.extend_from_slice(bytes);
        let n = bytes.len();
        reader.consume(n);
        let start = usize::from(number != 1);
        let end = if eof {
            batch.len()
        } else {
            memchr::memrchr(b'\n', &batch[start..]).map_or(start, |i| start + i + 1)
        };
        if start == end {
            if eof {
                break;
            }
            continue;
        }
        let text = std::str::from_utf8(&batch[..end]).expect("validated search batch");
        let mut hits = regex
            .find_iter(text)
            .filter(|m| m.start() >= start)
            .peekable();
        if context == 0 && hits.peek().is_none() {
            number += memchr::memchr_iter(b'\n', &batch[start..end]).count();
        } else {
            let mut left = start;
            for right in memchr::memchr_iter(b'\n', &batch[start..end])
                .map(|i| start + i)
                .chain((batch[end - 1] != b'\n').then_some(end))
            {
                let mut highlights = Vec::new();
                while hits.peek().is_some_and(|m| m.start() <= right) {
                    let hit = hits.next().expect("peeked match");
                    highlights.push((hit.start() - left)..(hit.end() - left));
                }
                let matched = !highlights.is_empty();
                if matched || after != 0 || context != 0 {
                    let text = &text[left..right];
                    let match_count = highlights.len();
                    let (span, column_offset) = clip_highlights(text, &mut highlights);
                    let row = Line {
                        path: shared.get_or_insert_with(|| path.into()).clone(),
                        number,
                        text: text[span].to_owned(),
                        match_count,
                        highlights,
                        column_offset,
                    };
                    if matched {
                        for preceding in recent.drain(..) {
                            if !emit(preceding) {
                                return Ok(());
                            }
                        }
                        after = context;
                        if !emit(row) {
                            return Ok(());
                        }
                    } else if after > 0 {
                        after -= 1;
                        if !emit(row) {
                            return Ok(());
                        }
                    } else {
                        if recent.len() == context {
                            recent.pop_front();
                        }
                        recent.push_back(row);
                    }
                }
                left = right + 1;
                number += 1;
            }
        }
        if eof {
            break;
        }
        batch.drain(..end - 1); // retain the LF for anchors and word boundaries
    }
    Ok(())
}

fn project(
    total: usize,
    input: impl BufRead,
    hits: impl Iterator<Item = Range<usize>>,
    projection: Projection<'_>,
    context: usize,
    stop: &AtomicBool,
    mut publish: impl FnMut(Line) -> bool,
) -> io::Result<()> {
    let mut hits = hits.peekable();
    let mut lines = Lines {
        file: input,
        path: projection.path,
        shared: None,
        offset: projection.offset,
        number: projection.number,
    };
    if projection.partial_row {
        lines.discard_partial_row()?;
    }
    let mut recent = VecDeque::new();
    let mut last_match_end = 0;
    let mut after = 0;
    while !stop.load(Ordering::Relaxed) {
        if !projection.eof_row && lines.offset == total {
            break;
        }
        let focus = hits.peek().map(|m| m.start);
        if focus.is_none() && after == 0 && last_match_end <= lines.offset {
            break;
        }
        if let Some(focus) = focus
            && focus > lines.offset
            && after == 0
            && last_match_end <= lines.offset
            && lines.skip_before(focus, context)?
        {
            recent.clear();
        }
        let (origin, bytes, mut line, terminated) = lines.next(focus.unwrap_or(usize::MAX))?;
        // EOF belongs to the last real row, empty only after a final LF.
        let final_row = bytes.end == total && !terminated;
        let limit = bytes.end + usize::from(final_row);
        let continuation = bytes.start < last_match_end;
        while hits.peek().is_some_and(|m| m.start < limit) {
            let hit = hits.next().expect("peeked match");
            last_match_end = last_match_end.max(hit.end);
            line.match_count += 1;
            if hit.start <= origin + line.text.len() {
                line.highlights.push(
                    hit.start.saturating_sub(origin)
                        ..(hit.end - origin.min(hit.end)).min(line.text.len()),
                );
            }
        }
        if line.match_count > 0 {
            for preceding in recent.drain(..) {
                if !publish(preceding) {
                    return Ok(());
                }
            }
        }
        if line.match_count > 0 || continuation || after > 0 {
            if line.match_count > 0 || continuation {
                after = context;
            } else {
                after -= 1;
            }
            if !publish(line) {
                return Ok(());
            }
        } else if context > 0 {
            if recent.len() == context {
                recent.pop_front();
            }
            recent.push_back(line);
        }
        if final_row {
            break;
        }
    }
    Ok(())
}
