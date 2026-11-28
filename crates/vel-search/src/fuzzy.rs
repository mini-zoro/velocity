//! Original subsequence matcher: filename, adjacency, word boundaries and recency.
use std::{
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicI64, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// Prepared candidate; normalization and character masks are paid once at indexing time.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// User-facing caption/path.
    pub label: String,
    lower: String,
    name: usize,
    stem_end: usize,
    ascii: bool,
    basename_key: usize,
    prefix_key: usize,
    label_name: usize,
    mask: u64,
    basename_mask: u64,
    directory_mask: u64,
    directory: Option<Arc<AsciiDirectory>>,
    boundaries: Vec<bool>,
    /// Monotonic use counter. Zero means never opened.
    pub recent: u64,
}
fn mask(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0, |m, &b| m | mask_bit(b))
}
fn mask_bit(byte: u8) -> u64 {
    1 << match byte {
        b'a'..=b'z' => byte - b'a',
        b'0'..=b'9' => byte - b'0' + 26,
        _ => 36 + byte % 28,
    }
}
fn spelling_key(bytes: &[u8]) -> usize {
    bytes.iter().fold(0x811c9dc5_u32, |hash, &byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x01000193)
    }) as usize
}

fn ascii_boundary(bytes: &[u8], at: usize) -> bool {
    at == 0
        || !bytes[at - 1].is_ascii_alphanumeric()
        || bytes[at - 1].is_ascii_lowercase() && bytes[at].is_ascii_uppercase()
}
impl Candidate {
    /// Prepares a path or command caption.
    pub fn new(label: impl Into<String>) -> Self {
        let mut item = Self {
            label: String::new(),
            lower: String::new(),
            name: 0,
            stem_end: 0,
            ascii: true,
            basename_key: 0,
            prefix_key: 0,
            label_name: 0,
            mask: 0,
            basename_mask: 0,
            directory_mask: 0,
            directory: None,
            boundaries: Vec::new(),
            recent: 0,
        };
        item.prepare(&label.into());
        item
    }
    fn prepare(&mut self, label: &str) {
        self.prepare_alignment(label);
        self.basename_key = spelling_key(&label.as_bytes()[self.label_name..]);
        self.prefix_key = spelling_key(&label.as_bytes()[..self.label_name]);
        self.basename_mask = mask(&self.lower.as_bytes()[self.name..]);
        self.directory_mask = mask(&self.lower.as_bytes()[..self.name]);
        self.mask = self.basename_mask | self.directory_mask;
        self.directory = (self.ascii && self.name != 0 && self.lower.len() <= MAX_ALIGN)
            .then(|| AsciiDirectory::prepare(self));
    }

    fn prepare_alignment(&mut self, label: &str) {
        self.label.clear();
        self.label.push_str(label);
        self.lower.clear();
        self.ascii = label.is_ascii();
        if self.ascii {
            self.lower.push_str(label);
            self.lower.make_ascii_lowercase();
        } else {
            self.lower
                .extend(label.chars().flat_map(char::to_lowercase));
        }
        self.name = self.lower.rfind(['/', '\\']).map_or(0, |i| i + 1);
        self.label_name = label.rfind(['/', '\\']).map_or(0, |i| i + 1);
        self.stem_end = self.lower[self.name..]
            .rfind('.')
            .map_or(self.lower.len(), |p| self.name + p);
        self.boundaries.clear();
        self.boundaries.resize(self.lower.len(), false);
        let mut offset = 0;
        let mut previous = None::<char>;
        for c in label.chars() {
            self.boundaries[offset] = previous
                .is_none_or(|p| !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()));
            offset += c.to_lowercase().map(char::len_utf8).sum::<usize>();
            previous = Some(c);
        }
    }
}
/// Ranked hit with original UTF-8 byte positions for highlighted Unicode scalars.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Index into the candidate snapshot.
    pub index: usize,
    /// Higher is better; ties retain index order.
    pub score: i64,
    /// Matched scalar positions in the original label.
    pub positions: Vec<usize>,
}
const MAX_ALIGN: usize = 512;
const MISSING: i32 = i32::MIN / 2;
const NO_PAIR: i16 = i16::MIN;

#[derive(Debug)]
struct AsciiDirectory {
    before_slash: [i16; 128],
    after_slash: [i8; 128],
    last_slash: usize,
    short: Mutex<Option<(u64, ShortPrefix)>>,
}

impl AsciiDirectory {
    fn prepare(item: &Candidate) -> Arc<Self> {
        // Indexing supplies many paths with the same directory. An interning
        // miss changes only the preparation cost, not the score; weak entries
        // keep this cache from extending the lifetime of a snapshot.
        type RecentDirectories = RefCell<HashMap<Box<str>, Weak<AsciiDirectory>>>;
        thread_local! {
            static RECENT: RecentDirectories = RefCell::new(HashMap::new());
        }
        let spelling = &item.label[..item.label_name];
        RECENT.with_borrow_mut(|cache| {
            if let Some(pair) = cache.get(spelling).and_then(Weak::upgrade) {
                return pair;
            }
            if cache.len() >= 32_768 {
                cache.clear();
            }
            let mut before_slash = [NO_PAIR; 128];
            let mut after_slash = [i8::MIN; 128];
            let mut last_slash = None;
            let mut rows = [EndScore::EMPTY; 128];
            let mut seen = 0_u128;
            for (at, (&byte, &boundary)) in item.lower.as_bytes()[..item.name]
                .iter()
                .zip(&item.boundaries)
                .enumerate()
            {
                let at = at as i32;
                let bonus = if boundary || at == 0 { 32 } else { 0 };
                if let Some(last) = last_slash {
                    let skipped = at - last - 1;
                    let edge = if skipped == 0 {
                        58
                    } else {
                        20 + bonus - skipped
                    };
                    let bound = edge.max(i32::from(i8::MIN) + 1) as i8;
                    after_slash[byte as usize] = after_slash[byte as usize].max(bound);
                }
                if byte == b'/' {
                    let mut possible = seen;
                    while possible != 0 {
                        let ch = possible.trailing_zeros() as usize;
                        possible &= possible - 1;
                        let score = (rows[ch].extend(at, bonus) + 20) as i16;
                        before_slash[ch] = before_slash[ch].max(score);
                    }
                    last_slash = Some(at);
                }
                rows[byte as usize].push(at, 20 + bonus - 2 * at);
                seen |= 1 << byte;
            }
            let pair = Arc::new(Self {
                before_slash,
                after_slash,
                last_slash: last_slash.unwrap_or_default() as usize,
                short: Mutex::new(None),
            });
            cache.insert(spelling.into(), Arc::downgrade(&pair));
            pair
        })
    }

    fn after_upper(&self, item: &Candidate, byte: u8) -> Option<i64> {
        let mut upper = self.after_slash[byte as usize];
        if item.basename_mask & mask_bit(byte) != 0 {
            // Only the true component start can immediately follow its slash.
            // For all later positions use the most favorable boundary bonus.
            let start = item.lower.as_bytes().get(item.name) == Some(&byte);
            let skipped = item.name - self.last_slash - 1 + usize::from(!start);
            let edge = if skipped == 0 {
                58
            } else {
                52 - skipped as i32
            };
            upper = upper.max(edge.max(i32::from(i8::MIN) + 1) as i8);
        }
        (upper != i8::MIN).then(|| i64::from(upper))
    }
}

fn best_alignment<T: Eq>(
    hay: &[T],
    needle: &[T],
    boundaries: &[bool],
    start: usize,
    end: usize,
    mut positions: Option<&mut Vec<usize>>,
    refine: bool,
) -> Option<i32> {
    let hay = &hay[start..end];
    let boundaries = &boundaries[start..end];
    if needle.len() > hay.len() {
        return None;
    }
    if hay.starts_with(needle) {
        if let Some(out) = positions {
            out.extend(start..start + needle.len());
        }
        return Some(32 + 20 * needle.len() as i32 + 38 * (needle.len() - 1) as i32);
    }
    if let [ch] = needle {
        let first = hay.iter().position(|v| v == ch)?;
        let mut best = first;
        let mut score = 20 + i32::from(boundaries[first]) * 32 - first as i32 * 2;
        if refine && !boundaries[first] {
            // A boundary can beat the first occurrence only within 15 cells.
            if let Some(next) =
                (first + 1..(first + 16).min(hay.len())).find(|&i| boundaries[i] && hay[i] == *ch)
            {
                best = next;
                score = 52 - next as i32 * 2;
            }
        }
        if let Some(out) = positions {
            out.push(start + best);
        }
        return Some(score);
    }
    if positions.is_none()
        && refine
        && hay.len() <= MAX_ALIGN
        && let [first, second] = needle
    {
        return pair_score(hay, boundaries, first, second);
    }
    // Quickly reject non-matches and keep the work bounded for unusually long
    // captions/queries. Such rows still match; the greedy fallback also supplies
    // their highlights so scoring and paint never pick different alignments.
    let mut first = 0;
    let mut greedy = 0;
    let fallback = !refine || hay.len() > MAX_ALIGN || needle.len() > 64;
    let checkpoint = positions.as_ref().map_or(0, |out| out.len());
    for (i, c) in needle.iter().enumerate() {
        let Some(at) = hay[first..]
            .iter()
            .position(|v| v == c)
            .map(|at| at + first)
        else {
            if let Some(out) = positions {
                out.truncate(checkpoint);
            }
            return None;
        };
        greedy += 20
            + if i == 0 {
                i32::from(boundaries[at]) * 32 - at as i32 * 2
            } else if first == at {
                38
            } else {
                i32::from(boundaries[at]) * 32 - (at - first) as i32
            };
        if fallback && let Some(out) = positions.as_deref_mut() {
            out.push(at + start);
        }
        first = at + 1;
    }
    if fallback {
        return Some(greedy);
    }

    // Sparse DP: only positions that match the current character can end an
    // alignment. Maintain the best non-adjacent predecessor as the scan advances.
    let mut previous = Vec::<(usize, i32)>::new();
    let mut next = Vec::new();
    let mut trace = positions
        .as_ref()
        .map(|_| vec![0u16; needle.len() * hay.len()]);
    for (i, ch) in needle.iter().enumerate() {
        let mut gap = (MISSING, 0);
        let mut before = 0;
        let limit = hay.len() - (needle.len() - i - 1);
        let start = previous.first().map_or(0, |(j, _)| j + 1);
        let mut at = start;
        while let Some(j) = hay[at..limit]
            .iter()
            .position(|value| value == ch)
            .map(|j| j + at)
        {
            at = j + 1;
            let boundary = i32::from(j == 0 || boundaries[j]) * 32;
            if i == 0 {
                next.push((j, 20 + boundary - j as i32 * 2));
                continue;
            }
            while let Some(&(position, score)) = previous.get(before) {
                if position + 1 >= j {
                    break;
                }
                let value = score + position as i32 + 1;
                if value > gap.0 {
                    gap = (value, position);
                }
                before += 1;
            }
            let adjacent = previous
                .get(before)
                .filter(|(position, _)| position + 1 == j)
                .map_or(MISSING, |(_, score)| *score);
            let gapped = if gap.0 != MISSING {
                gap.0 - j as i32 + boundary
            } else {
                MISSING
            };
            let (value, predecessor) = if adjacent != MISSING && adjacent + 38 >= gapped {
                (adjacent + 38, j - 1)
            } else {
                (gapped, gap.1)
            };
            if value != MISSING {
                next.push((j, value + 20));
            }
            if let Some(trace) = &mut trace {
                trace[i * hay.len() + j] = predecessor as u16;
            }
        }
        std::mem::swap(&mut previous, &mut next);
        next.clear();
    }
    let (mut j, score) = previous
        .into_iter()
        .max_by_key(|&(index, score)| (score, std::cmp::Reverse(index)))?;
    if let (Some(trace), Some(out)) = (trace, positions) {
        let from = out.len();
        for i in (0..needle.len()).rev() {
            out.push(j + start);
            j = trace[i * hay.len() + j] as usize;
        }
        out[from..].reverse();
    }
    Some(score)
}

fn normalized(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_lowercase()
    } else {
        s.chars().flat_map(char::to_lowercase).collect()
    }
}

fn score_term(
    item: &Candidate,
    term: &str,
    path_tier: bool,
    mut out: Option<&mut Vec<usize>>,
    refine: bool,
) -> Option<i64> {
    let bytes = item.lower.as_bytes();
    let mut align = |start, end| {
        if item.ascii && term.is_ascii() {
            return best_alignment(
                bytes,
                term.as_bytes(),
                &item.boundaries,
                start,
                end,
                out.as_deref_mut(),
                refine,
            );
        }
        let hay: Vec<_> = item.lower.char_indices().collect();
        let needle: Vec<_> = term.chars().collect();
        let char_start = hay.partition_point(|&(byte, _)| byte < start);
        let char_end = hay.partition_point(|&(byte, _)| byte < end);
        let boundaries: Vec<_> = hay
            .iter()
            .map(|(offset, _)| item.boundaries[*offset])
            .collect();
        let chars: Vec<_> = hay.iter().map(|&(_, ch)| ch).collect();
        let mut positions = Vec::new();
        let score = best_alignment(
            &chars,
            &needle,
            &boundaries,
            char_start,
            char_end,
            out.is_some().then_some(&mut positions),
            refine,
        )?;
        if let Some(out) = out.as_deref_mut() {
            let from = out.len();
            out.extend(positions.into_iter().map(|i| hay[i].0));
            let mut lowered = 0;
            let mut matched = from;
            for (offset, c) in item.label.char_indices() {
                lowered += c.to_lowercase().map(char::len_utf8).sum::<usize>();
                while matched < out.len() && out[matched] < lowered {
                    out[matched] = offset;
                    matched += 1;
                }
            }
        }
        Some(score)
    };
    let score = align(item.name, item.stem_end)
        .map(|s| s + 700)
        .or_else(|| align(item.name, bytes.len()).map(|s| s + 500))
        .or_else(|| path_tier.then(|| align(0, bytes.len())).flatten())?;
    let name = &item.lower[item.name..];
    let stem = &item.lower[item.name..item.stem_end];
    let exact = if name == term {
        1000
    } else if stem == term {
        450
    } else {
        0
    };
    Some(i64::from(
        score + exact + if name.starts_with(term) { 150 } else { 0 },
    ))
}

fn length_penalty(length: usize) -> i64 {
    length.min(56) as i64
}

#[derive(Clone, Copy, Default)]
struct Greedy {
    matched: usize,
    end: usize,
    score: i32,
    first: usize,
    first_bonus: i32,
}
impl Greedy {
    fn advance<const FOLD: bool>(&mut self, bytes: &[u8], boundaries: &[bool], needle: &[u8]) {
        while let Some(&ch) = needle.get(self.matched) {
            let Some(gap) = bytes[self.end..].iter().position(|&byte| {
                (if FOLD {
                    byte.to_ascii_lowercase()
                } else {
                    byte
                }) == ch
            }) else {
                break;
            };
            self.push::<FOLD>(bytes, boundaries, self.end + gap);
        }
    }

    #[inline]
    fn push<const FOLD: bool>(&mut self, bytes: &[u8], boundaries: &[bool], at: usize) {
        let gap = (at - self.end) as i32;
        let bonus = || {
            if at == 0
                || if FOLD {
                    ascii_boundary(bytes, at)
                } else {
                    boundaries[at]
                }
            {
                32
            } else {
                0
            }
        };
        self.score += 20
            + if self.matched == 0 {
                self.first = at;
                self.first_bonus = bonus();
                self.first_bonus - 2 * gap
            } else if gap == 0 {
                38
            } else {
                bonus() - gap
            };
        self.end = at + 1;
        self.matched += 1;
    }

    fn bound(self, len: usize) -> Bound {
        if self.matched == 1 {
            let score = i64::from(self.score);
            return if self.first_bonus == 32 {
                Bound::exact(score)
            } else {
                Bound::new(score + 30, false)
            };
        }
        if self.matched > 1 && (len > MAX_ALIGN || self.matched > 64) {
            return Bound::exact(i64::from(self.score));
        }
        let start = self.first as i64;
        let gaps = (self.end - self.first - self.matched) as i64;
        let ideal = ideal_alignment(self.matched) - 2 * start;
        // Every alignment finishes at least as late as greedy. Starting at
        // the same byte costs at least 'gaps' skipped bytes and, if nonzero,
        // one gapped edge (38 adjacency points vs at most 32 for a boundary).
        let same_start =
            ideal - (32 - i64::from(self.first_bonus)) - gaps - if gaps == 0 { 0 } else { 6 };
        // Starting d bytes later costs 2*d and still requires at least
        // gaps-d skipped bytes. The cheapest gapped case starts at the
        // earliest allowed byte; the other possibility is a contiguous run.
        let later_start = ideal - if gaps <= 1 { 2 } else { gaps + 7.min(gaps) };
        let upper = same_start.max(later_start);
        Bound::new(upper, upper == i64::from(self.score))
    }
}

fn ideal_alignment(len: usize) -> i64 {
    58 * len as i64 - 6
}

// Pack the exact bit alongside the bound, keeping candidate lists compact.
#[derive(Clone, Copy)]
struct Bound(i64);
impl Bound {
    fn new(score: i64, exact: bool) -> Self {
        Self(2 * score + i64::from(exact))
    }
    fn exact(score: i64) -> Self {
        Self::new(score, true)
    }
    fn upper(self) -> i64 {
        self.0 >> 1
    }
    fn is_exact(self) -> bool {
        self.0 & 1 != 0
    }
    fn add(self, other: Self) -> Self {
        Self((self.0 & !1) + (other.0 & !1) + (self.0 & other.0 & 1))
    }
    fn bonus(self, score: i64) -> Self {
        Self(self.0 + 2 * score)
    }
}

fn coarse_basename(item: &Candidate, needle: &[u8]) -> Option<Bound> {
    let name = &item.lower.as_bytes()[item.name..];
    if name.len() < needle.len() {
        return None;
    }
    let stem_end = item.stem_end - item.name;
    if let [ch] = needle {
        let at = name.iter().position(|byte| byte == ch)?;
        let score = if at < stem_end { 700 } else { 500 } + 52 - 2 * at as i64;
        return Some(if at == 0 {
            let exact = if name.len() == 1 {
                1000
            } else if stem_end == 1 {
                450
            } else {
                0
            };
            Bound::exact(score + 150 + exact)
        } else if item.boundaries[item.name + at] {
            Bound::exact(score)
        } else {
            Bound::new(score - 2, false)
        });
    }
    if name.starts_with(needle) {
        let tier = if needle.len() <= stem_end { 700 } else { 500 };
        let exact = if name.len() == needle.len() {
            1000
        } else if stem_end == needle.len() {
            450
        } else {
            0
        };
        return Some(Bound::exact(
            ideal_alignment(needle.len()) + tier + exact + 150,
        ));
    }
    let mut matched = Greedy::default();
    matched.advance::<false>(name, &item.boundaries[item.name..], needle);
    if matched.matched != needle.len() {
        return None;
    }
    let (tier, len) = if matched.end <= stem_end {
        (700, stem_end)
    } else {
        (500, name.len())
    };
    Some(matched.bound(len).bonus(tier))
}

type PrefixCache<'a> = [Option<(&'a str, usize, Greedy)>; 256];

fn coarse_path<'a>(
    item: &'a Candidate,
    needle: &[u8],
    tail: &[u64],
    term: usize,
    cache: &mut PrefixCache<'a>,
) -> Option<Bound> {
    coarse_path_end(item, needle, tail, term, cache).map(|(bound, _)| bound)
}

fn coarse_path_end<'a>(
    item: &'a Candidate,
    needle: &[u8],
    tail: &[u64],
    term: usize,
    cache: &mut PrefixCache<'a>,
) -> Option<(Bound, usize)> {
    if item.name == 0 {
        return None;
    }
    let name = &item.lower[..item.name];
    let spelling = &item.label[..item.label_name];
    let slot = &mut cache[(item.prefix_key ^ term.wrapping_mul(101)) % 256];
    let mut matched = match *slot {
        Some((prefix, key, state)) if key == term && prefix == spelling => state,
        _ => {
            let mut state = Greedy::default();
            state.advance::<false>(name.as_bytes(), &item.boundaries[..item.name], needle);
            *slot = Some((spelling, term, state));
            state
        }
    };
    if matched.matched != needle.len() {
        let needed = tail[matched.matched];
        if item.basename_mask & needed != needed {
            return None;
        }
        // Resume at the first unscanned byte, but measure a gap from the last
        // hit, which can be in the prefix. This is exactly the uncached scan.
        let bytes = item.lower.as_bytes();
        let &ch = &needle[matched.matched];
        let first = bytes[item.name..].iter().position(|&byte| byte == ch)? + item.name;
        matched.push::<false>(bytes, &item.boundaries, first);
        matched.advance::<false>(bytes, &item.boundaries, needle);
    }
    (matched.matched == needle.len()).then(|| (matched.bound(item.lower.len()), matched.end))
}

#[derive(Debug)]
struct AsciiTerm {
    rows: [u64; 128],
    len: usize,
    first_suffix_row: usize,
    directory_mask: u64,
    tails: Vec<u64>,
}

impl AsciiTerm {
    fn new(needle: &[u8]) -> Self {
        let mut rows = [0; 128];
        for (row, &byte) in needle.iter().take(64).enumerate() {
            if byte.is_ascii() {
                rows[byte as usize] |= 1 << row;
            }
        }
        let first_suffix_row = needle
            .iter()
            .rposition(|&b| matches!(b, b'/' | b'\\'))
            .map_or(0, |i| i + 1);
        Self {
            rows,
            len: needle.len(),
            first_suffix_row,
            directory_mask: mask(&needle[..first_suffix_row]),
            tails: tail_masks(needle),
        }
    }
}

fn tail_masks(needle: &[u8]) -> Vec<u64> {
    let mut suffix = vec![0; needle.len()];
    let mut needed = 0;
    for (&ch, slot) in needle.iter().zip(&mut suffix).rev() {
        needed |= mask_bit(ch);
        *slot = needed;
    }
    suffix
}

#[derive(Clone, Copy, Debug)]
struct EndScore {
    last: i32,
    score: i32,
    earlier: i32,
}

impl EndScore {
    const EMPTY: Self = Self {
        last: -2,
        score: MISSING,
        earlier: MISSING,
    };

    fn extend(self, at: i32, boundary: i32) -> i32 {
        if self.score == MISSING {
            return MISSING;
        }
        if self.last + 1 == at {
            (self.score + 38).max(self.earlier - at + boundary)
        } else {
            self.earlier.max(self.score + self.last + 1) - at + boundary
        }
    }

    fn push(&mut self, at: i32, score: i32) {
        self.earlier = self.earlier.max(self.score + self.last + 1);
        self.last = at;
        self.score = score;
    }
}

fn pair_score<T: Eq>(bytes: &[T], boundaries: &[bool], first: &T, second: &T) -> Option<i32> {
    let mut row = EndScore::EMPTY;
    let mut best = MISSING;
    for (j, (byte, &boundary)) in bytes.iter().zip(boundaries).enumerate() {
        let at = j as i32;
        let bonus = if boundary || at == 0 { 32 } else { 0 };
        if byte == second {
            let score = row.extend(at, bonus);
            if score != MISSING {
                best = best.max(score + 20);
            }
        }
        if byte == first {
            row.push(at, 20 + bonus - 2 * at);
        }
    }
    (best != MISSING).then_some(best)
}

/// The score-only form of best_alignment, transposed to walk the label once.
/// Each row retains its most recent end and the best earlier gapped end.
#[derive(Clone, Copy, Debug)]
struct AlignmentState<const N: usize = 64> {
    rows: [EndScore; N],
    complete: i32,
}

impl<const N: usize> AlignmentState<N> {
    fn new() -> Self {
        Self {
            rows: [EndScore::EMPTY; N],
            complete: MISSING,
        }
    }

    fn advance(&mut self, term: &AsciiTerm, bytes: &[u8], boundaries: &[bool], start: usize) {
        self.walk::<false>(term, bytes, boundaries, start);
    }

    fn walk<const FOLD: bool>(
        &mut self,
        term: &AsciiTerm,
        bytes: &[u8],
        boundaries: &[bool],
        start: usize,
    ) {
        let rows_usable = if start == 0 {
            u64::MAX
        } else {
            u64::MAX
                .checked_shl(term.first_suffix_row as u32)
                .unwrap_or(0)
        };
        for (offset, &byte) in bytes.iter().enumerate() {
            let folded = if FOLD {
                byte.to_ascii_lowercase()
            } else {
                byte
            };
            let rows = term.rows[folded as usize] & rows_usable;
            if rows == 0 {
                continue;
            }
            let at = (start + offset) as i32;
            let boundary = if FOLD {
                ascii_boundary(bytes, offset)
            } else {
                boundaries[offset]
            };
            let bonus = if boundary || at == 0 { 32 } else { 0 };
            self.visit(term.len, rows, at, bonus);
        }
    }

    #[inline]
    fn visit(&mut self, len: usize, mut rows: u64, at: i32, boundary: i32) {
        // Descending order prevents repeated letters from sharing one byte.
        while rows != 0 {
            let i = 63 - rows.leading_zeros() as usize;
            rows ^= 1 << i;
            let score = if i == 0 {
                boundary - 2 * at
            } else {
                self.rows[i - 1].extend(at, boundary)
            };
            if score == MISSING {
                continue;
            }
            let score = score + 20;
            self.rows[i].push(at, score);
            if i + 1 == len {
                self.complete = self.complete.max(score);
            }
        }
    }

    fn path_upper(&self, term: &AsciiTerm, end: usize) -> i32 {
        // No match starts entirely in the filename (the basename scan
        // established that). A crossing match cannot start its first
        // filename byte sooner than 'end', nor receive more than 58 per
        // subsequent byte. If even that loses, use the cached prefix score.
        (term.first_suffix_row.max(1)..term.len)
            .filter_map(|i| {
                let score = self.rows[i - 1].extend(end as i32, 32);
                (score != MISSING).then_some(score + 20 + 58 * (term.len - i - 1) as i32)
            })
            .fold(self.complete, i32::max)
    }
}

const SHORT_PATH: usize = 6;

type PathSlot<'a, T> = Option<(&'a AsciiDirectory, T)>;

// Cache compact automata by prepared directory identity. A collision or a
// directory evicted from the preparation cache only causes another scan.
struct PathCache<'a, T> {
    slots: Vec<PathSlot<'a, T>>,
}

impl<T> Default for PathCache<'_, T> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

impl<'a, T> PathCache<'a, T> {
    fn get(&mut self, item: &'a Candidate, make: impl FnOnce() -> T) -> &T {
        let directory = item.directory.as_deref().expect("bounded ASCII path");
        if self.slots.is_empty() {
            self.slots.resize_with(4096, || None);
        }
        let slot = &mut self.slots[(item.prefix_key ^ (item.prefix_key >> 13)) & 4095];
        if !slot
            .as_ref()
            .is_some_and(|(key, _)| std::ptr::eq(*key, directory))
        {
            *slot = Some((directory, make()));
        }
        &slot.as_ref().unwrap().1
    }
}

#[derive(Debug, Clone, Copy)]
struct ShortPrefix {
    state: AlignmentState<SHORT_PATH>,
    matched: usize,
}

struct ShortPathCache<'a> {
    directories: PathCache<'a, ShortPrefix>,
    generation: u64,
}

impl<'a> ShortPathCache<'a> {
    fn new(generation: u64) -> Self {
        Self {
            directories: PathCache::default(),
            generation,
        }
    }

    fn prefix(
        &mut self,
        item: &'a Candidate,
        term: &AsciiTerm,
    ) -> (&AlignmentState<SHORT_PATH>, usize) {
        let generation = self.generation;
        let prefix = self.directories.get(item, || {
            let shared = &item.directory.as_ref().unwrap().short;
            if generation != 0
                && let Some((key, prefix)) = *shared.lock().unwrap()
                && key == generation
            {
                return prefix;
            }
            // Compute outside the lock. Shards can duplicate a miss but
            // never block each other on an alignment.
            let mut state = AlignmentState::new();
            let bytes = &item.lower.as_bytes()[..item.name];
            state.advance(term, bytes, &item.boundaries[..item.name], 0);
            let matched = state.rows[..term.len]
                .iter()
                .rposition(|row| row.score != MISSING)
                .map_or(0, |i| i + 1);
            let prefix = ShortPrefix { state, matched };
            if generation != 0 {
                *shared.lock().unwrap() = Some((generation, prefix));
            }
            prefix
        });
        (&prefix.state, prefix.matched)
    }

    fn bound(&mut self, item: &'a Candidate, needle: &[u8], term: &AsciiTerm) -> Option<Bound> {
        if item.directory_mask & term.directory_mask != term.directory_mask {
            return None;
        }
        let (state, matched) = self.prefix(item, term);
        if matched < term.first_suffix_row {
            return None;
        }
        let name = &item.lower.as_bytes()[item.name..];
        let mut lower = state.complete;
        if let Some(tail) = term.tails.get(matched) {
            if item.basename_mask & tail != *tail {
                return None;
            }
            // The furthest reachable directory row leaves the shortest
            // suffix to match, so this one check proves a crossing exists.
            let mut remaining = name;
            for &byte in &needle[matched..] {
                let at = remaining.iter().position(|&value| value == byte)?;
                remaining = &remaining[at + 1..];
            }
        }
        let mut upper = state.complete;
        for row in term.first_suffix_row..term.len {
            let tail = term.tails[row];
            if item.basename_mask & tail != tail {
                continue;
            }
            // The last separator must already have matched in the directory.
            // A crossing can get adjacency at the component start only if
            // that byte matches. Later starts can earn at most a boundary
            // bonus and incur at least one more skipped byte.
            let at_start = name.first() == Some(&needle[row]);
            let at = item.name as i32 + i32::from(!at_start);
            let score = state.rows[row - 1].extend(at, 32);
            if score != MISSING {
                let score = score + 20 + 58 * (term.len - row - 1) as i32;
                upper = upper.max(score);
                if at_start && name.starts_with(&needle[row..]) {
                    lower = lower.max(score);
                }
            }
        }
        Some(Bound::new(i64::from(upper), upper == lower))
    }

    fn score(&mut self, item: &'a Candidate, term: &AsciiTerm) -> Option<i64> {
        let (prefix, _) = self.prefix(item, term);
        let mut state = AlignmentState {
            rows: prefix.rows,
            complete: prefix.complete,
        };
        state.advance(
            term,
            &item.lower.as_bytes()[item.name..],
            &item.boundaries[item.name..],
            item.name,
        );
        (state.complete != MISSING).then_some(i64::from(state.complete))
    }
}

struct RefineCache<'a> {
    directories: Vec<Option<DirectoryScore<'a>>>,
    scratch: AlignmentState,
}

impl Default for RefineCache<'_> {
    fn default() -> Self {
        Self {
            directories: Vec::new(),
            scratch: AlignmentState::new(),
        }
    }
}

struct DirectoryScore<'a> {
    spelling: &'a str,
    term: usize,
    state: AlignmentState,
    upper: i32,
    matched: usize,
}

impl<'a> RefineCache<'a> {
    fn score(
        &mut self,
        item: &'a Candidate,
        term: (&str, u64),
        query: &AsciiTerm,
        term_index: usize,
        cutoff: i64,
    ) -> Option<i64> {
        let (term, mask) = term;
        if !item.ascii || !term.is_ascii() || !(2..=64).contains(&query.len) {
            return score_term(item, term, true, None, true);
        }
        if query.first_suffix_row == 0
            && item.basename_mask & mask == mask
            && let Some(bound) = coarse_basename(item, term.as_bytes())
        {
            if bound.is_exact() {
                return Some(bound.upper());
            }
            // The tier computed by the greedy pass is also the exact tier.
            return score_term(item, term, false, None, true);
        }
        self.path_score(item, term, query, term_index, cutoff)
    }

    fn path_score(
        &mut self,
        item: &'a Candidate,
        term: &str,
        query: &AsciiTerm,
        term_index: usize,
        cutoff: i64,
    ) -> Option<i64> {
        let bytes = item.lower.as_bytes();
        if item.name == 0 {
            return None;
        }
        if bytes.len() > MAX_ALIGN || query.len > 64 {
            return score_term(item, term, true, None, true);
        }
        if self.directories.is_empty() {
            self.directories.resize_with(256, || None);
        }
        let slot = &mut self.directories[(item.prefix_key ^ term_index.wrapping_mul(101)) % 256];
        let spelling = &item.label[..item.label_name];
        if !slot
            .as_ref()
            .is_some_and(|entry| entry.spelling == spelling && entry.term == term_index)
        {
            let mut state = AlignmentState::new();
            state.advance(query, &bytes[..item.name], &item.boundaries[..item.name], 0);
            let upper = state.path_upper(query, item.name);
            let matched = state.rows[..query.len]
                .iter()
                .rposition(|r| r.score != MISSING)
                .map_or(0, |i| i + 1);
            *slot = Some(DirectoryScore {
                spelling,
                term: term_index,
                state,
                upper,
                matched,
            });
        }
        let entry = slot.as_ref().unwrap();
        if i64::from(entry.upper) <= cutoff || entry.matched < query.first_suffix_row {
            return None;
        }
        if let Some(&tail) = query.tails.get(entry.matched)
            && item.basename_mask & tail != tail
        {
            return None;
        }
        if entry.state.complete == entry.upper
            || item.basename_mask & mask_bit(term.as_bytes()[query.len - 1]) == 0
        {
            return (entry.state.complete != MISSING).then_some(i64::from(entry.state.complete));
        }
        self.scratch.rows[..query.len].copy_from_slice(&entry.state.rows[..query.len]);
        self.scratch.complete = entry.state.complete;
        self.scratch.advance(
            query,
            &bytes[item.name..],
            &item.boundaries[item.name..],
            item.name,
        );
        (self.scratch.complete != MISSING).then_some(i64::from(self.scratch.complete))
    }
}

/// Query-local scorer for compact symbol stores. Uses the same score and alignment
/// as the path/palette matcher without retaining a prepared candidate per symbol.
#[derive(Debug)]
pub struct LabelMatcher {
    terms: Vec<String>,
    ascii: bool,
    one_ascii: Option<AsciiTerm>,
    ignore_trailing_digits: bool,
    last_stem: String,
    last_stem_score: Option<i64>,
    item: Candidate,
}
impl LabelMatcher {
    /// Compile a space-separated query (ST3 build 3170, <https://www.sublimetext.com/3>).
    pub fn new(query: &str) -> Self {
        let terms: Vec<_> = query.split_whitespace().map(normalized).collect();
        let ascii = query.is_ascii();
        let one_ascii = match terms.as_slice() {
            [term] if ascii && term.len() <= 64 => Some(AsciiTerm::new(term.as_bytes())),
            _ => None,
        };
        Self {
            terms,
            ascii,
            one_ascii,
            ignore_trailing_digits: !query.bytes().any(|b| b.is_ascii_digit()),
            last_stem: String::new(),
            last_stem_score: None,
            item: Candidate::new(""),
        }
    }
    /// Scores only labels that can beat an existing result. The caller can
    /// provide a strict cutoff when ties favor already visited labels.
    /// For an ASCII label without path/extension separators, `core_end`
    /// gives its length excluding trailing ASCII digits.
    pub fn score_above(
        &mut self,
        label: &str,
        core_end: Option<usize>,
        cutoff: i64,
    ) -> Option<i64> {
        if let Some(core_end) = core_end
            && self.ascii
            && !self.terms.is_empty()
        {
            let bytes = label.as_bytes();
            let penalty = length_penalty(bytes.len());
            let bound = if let [term] = self.terms.as_slice() {
                let needle = term.as_bytes();
                let ideal = ideal_alignment(needle.len()) + 700 - penalty;
                if self.ignore_trailing_digits
                    && core_end < bytes.len()
                    && bytes.len() <= MAX_ALIGN
                    && let Some(query) = &self.one_ascii
                {
                    let stem = label.get(..core_end)?;
                    if stem.len() < needle.len() || ideal + 150 <= cutoff {
                        return None;
                    }
                    // Digit-free queries have the same raw alignment on all
                    // these labels. Cache it before comparing prefixes, and
                    // charge the full label's length separately.
                    if stem != self.last_stem {
                        self.last_stem_score =
                            plain_numeric_core_score(stem.as_bytes(), needle, query);
                        self.last_stem.clear();
                        self.last_stem.push_str(stem);
                    }
                    return self
                        .last_stem_score
                        .map(|score| score - penalty)
                        .filter(|&s| s > cutoff);
                }
                if bytes.get(..needle.len())?.eq_ignore_ascii_case(needle) {
                    let score = ideal + 150 + if bytes.len() == needle.len() { 1000 } else { 0 };
                    return (score > cutoff).then_some(score);
                }
                if ideal <= cutoff {
                    return None;
                }
                let bound = plain_name_bound(bytes, needle)?.bonus(-penalty);
                if bound.is_exact() || bound.upper() <= cutoff {
                    return (bound.upper() > cutoff).then(|| bound.upper());
                }
                if bytes.len() <= MAX_ALIGN
                    && let Some(query) = &self.one_ascii
                {
                    return plain_name_alignment(bytes, needle, query)
                        .map(|s| s + 700 - penalty)
                        .filter(|&s| s > cutoff);
                }
                bound
            } else {
                self.terms
                    .iter()
                    .try_fold(Bound::exact(-penalty), |sum, term| {
                        Some(sum.add(plain_name_bound(bytes, term.as_bytes())?))
                    })?
            };
            if bound.upper() <= cutoff {
                return None;
            }
            if bound.is_exact() {
                return Some(bound.upper());
            }
        }
        self.score(label, None).filter(|&score| score > cutoff)
    }
    /// Scores a label; optionally supplies original byte positions for painting.
    pub fn score(&mut self, label: &str, positions: Option<&mut Vec<usize>>) -> Option<i64> {
        if self.terms.is_empty() {
            return Some(0);
        }
        if let [term] = self.terms.as_slice()
            && label
                .as_bytes()
                .get(..term.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(term.as_bytes()))
            && label
                .bytes()
                .all(|byte| byte.is_ascii() && !matches!(byte, b'.' | b'/' | b'\\'))
        {
            if let Some(out) = positions {
                out.extend(0..term.len());
            }
            return Some(
                700 + 150 + 32 + 58 * term.len() as i64 - 38
                    + if label.len() == term.len() { 1000 } else { 0 }
                    - length_penalty(label.len()),
            );
        }
        self.item.prepare_alignment(label);
        score_terms(
            &self.item,
            self.terms.iter().map(String::as_str),
            true,
            positions,
            true,
        )
    }
}

fn plain_numeric_core_score(bytes: &[u8], needle: &[u8], term: &AsciiTerm) -> Option<i64> {
    if bytes.get(..needle.len())?.eq_ignore_ascii_case(needle) {
        return Some(700 + ideal_alignment(needle.len()) + 150);
    }
    let bound = plain_name_bound(bytes, needle)?;
    if bound.is_exact() {
        return Some(bound.upper());
    }
    plain_name_alignment(bytes, needle, term).map(|score| score + 700)
}

fn plain_name_alignment(bytes: &[u8], needle: &[u8], term: &AsciiTerm) -> Option<i64> {
    if let [ch] = needle {
        let first = bytes
            .iter()
            .position(|&byte| byte.to_ascii_lowercase() == *ch)?;
        if ascii_boundary(bytes, first) {
            return Some(52 - 2 * first as i64);
        }
        let next = (first + 1..(first + 16).min(bytes.len()))
            .find(|&at| bytes[at].to_ascii_lowercase() == *ch && ascii_boundary(bytes, at));
        return Some(next.map_or(20 - 2 * first as i64, |at| 52 - 2 * at as i64));
    }
    let mut state = AlignmentState::<64>::new();
    state.walk::<true>(term, bytes, &[], 0);
    (state.complete != MISSING).then_some(i64::from(state.complete))
}

fn plain_name_bound(bytes: &[u8], needle: &[u8]) -> Option<Bound> {
    let mut matched = Greedy::default();
    matched.advance::<true>(bytes, &[], needle);
    (matched.matched == needle.len()).then(|| {
        let prefix = if matched.end == needle.len() { 150 } else { 0 };
        let exact = if bytes.len() == needle.len() { 1000 } else { 0 };
        matched.bound(bytes.len()).bonus(700 + prefix + exact)
    })
}
fn score_terms<'a>(
    item: &Candidate,
    terms: impl Iterator<Item = &'a str>,
    path_tier: bool,
    mut out: Option<&mut Vec<usize>>,
    refine: bool,
) -> Option<i64> {
    let mut score = 0;
    let mut matches = false;
    for term in terms {
        score += score_term(item, term, path_tier, out.as_deref_mut(), refine)?;
        matches = true;
    }
    if let Some(out) = out {
        out.sort_unstable();
        out.dedup();
    }
    Some(
        score
            - if matches {
                length_penalty(item.lower.len())
            } else {
                0
            },
    )
}

/// Reusable incremental matcher. Backspace/changed queries rescore the complete snapshot.
#[derive(Debug, Default)]
pub struct Matcher {
    previous: String,
    candidates: Vec<Row>,
    latest: u64,
}

#[derive(Clone, Copy, Debug)]
struct Row {
    // The index itself uses u32 IDs. Keep the narrowing records cache-compact.
    index: u32,
    // Raw path-alignment upper bound, independent of recency and length.
    path_upper: i16,
    // End of a proven optimal alignment, or zero when there is no witness.
    path_end: u16,
}

impl Row {
    fn new(index: usize) -> Self {
        Self {
            index: u32::try_from(index).expect("Goto snapshot exceeds the index ID space"),
            path_upper: i16::MAX,
            path_end: 0,
        }
    }

    fn with_path(self, upper: i64, end: usize) -> Self {
        Self {
            path_upper: upper as i16,
            path_end: end as u16,
            ..self
        }
    }

    fn extended_bound(self, appended: Option<&[u8]>) -> Option<i64> {
        let appended = appended?;
        (self.path_upper != i16::MAX)
            .then(|| i64::from(self.path_upper) + 58 * appended.len() as i64)
    }

    fn extend_witness(self, item: &Candidate, appended: Option<&[u8]>) -> Option<(Bound, usize)> {
        let bytes = appended?;
        let end = usize::from(self.path_end);
        if end == 0 || !item.lower.as_bytes()[end..].starts_with(bytes) {
            return None;
        }
        // The old optimal alignment plus adjacent bytes attains the
        // upper bound, so no other alignment of the larger term can win.
        Some((
            Bound::exact(self.extended_bound(appended)?),
            end + bytes.len(),
        ))
    }
}

#[derive(Clone, Copy)]
struct Scan<'a> {
    items: &'a [Candidate],
    extras: &'a [Candidate],
    prior: Option<&'a [Row]>,
    latest: u64,
    appended: Option<&'a [u8]>,
    top_k_floor: Option<&'a AtomicI64>,
    generation: u64,
}

impl Scan<'_> {
    fn row(&self, n: usize) -> Row {
        self.prior.map_or_else(|| Row::new(n), |rows| rows[n])
    }

    fn candidate(&self, i: usize) -> &Candidate {
        self.items
            .get(i)
            .unwrap_or_else(|| &self.extras[i - self.items.len()])
    }

    fn len(&self) -> usize {
        self.prior
            .map_or(self.items.len() + self.extras.len(), <[Row]>::len)
    }
}

struct ScanResult {
    candidates: Vec<Row>,
    latest: u64,
    hits: Vec<Hit>,
}

/// A path with a separator cannot rank at either basename tier. Stream the
/// exact top K, pruning filename refinement with the directory/prefix bound.
fn scan_ascii_path<'a>(
    source: Scan<'a>,
    recent: &std::collections::BTreeMap<usize, u64>,
    needle: &str,
    limit: usize,
    range: Range<usize>,
    cancel: &impl Fn() -> bool,
) -> Result<Option<ScanResult>, ()> {
    use std::{cmp::Reverse, collections::BinaryHeap};
    let term = AsciiTerm::new(needle.as_bytes());
    let required = term.tails[0];
    let slash_pair = matches!(needle.as_bytes(), [_, b'/']);
    let slash_prefix = needle.as_bytes().get(1) == Some(&b'/');
    let short = needle.len() <= SHORT_PATH && term.first_suffix_row <= 2 && !slash_pair;
    let mut paths = ShortPathCache::new(source.generation);
    let mut greedy = [None; 256];
    let mut exact = RefineCache::default();
    let mut retained = Vec::new();
    let mut best = BinaryHeap::with_capacity(limit.min(1024));
    let mut cutoff = (i64::MIN, Reverse(usize::MAX));
    // An exact Kth row from any shard is a valid floor for the global Kth
    // row. Pack the original index into the floor so ties prune safely too.
    let pack =
        |(score, Reverse(index)): (i64, Reverse<usize>)| (score << 32) | i64::from(!(index as u32));
    let mut latest = source.latest;
    let newest = recent.values().copied().max().unwrap_or(latest);
    for n in range {
        if n.is_multiple_of(256) && cancel() {
            return Ok(None);
        }
        let row = source.row(n);
        let i = row.index as usize;
        let item = source.candidate(i);
        if item.recent > latest {
            latest = item.recent;
            if recent.is_empty() {
                // Imported use counters need a stable global reference;
                // the general two-pass scan determines it before ranking.
                return Err(());
            }
        }
        if item.mask & required != required {
            continue;
        }
        let threshold = source.top_k_floor.map_or(cutoff, |floor| {
            let floor = floor.load(Ordering::Relaxed);
            cutoff.max((floor >> 32, Reverse(!floor as u32 as usize)))
        });
        let penalty = length_penalty(item.lower.len());
        let bonus = if newest == 0 && recent.is_empty() {
            -penalty
        } else {
            recency(item, recent, i, newest) - penalty
        };
        let bounded = item.ascii && item.lower.len() <= MAX_ALIGN;
        let mut prior = row.extended_bound(source.appended);
        if slash_prefix
            && !slash_pair
            && let Some(directory) = &item.directory
        {
            let prefix = directory.before_slash[needle.as_bytes()[0] as usize];
            if prefix == NO_PAIR {
                continue;
            }
            // Every full alignment contains this separator-ending prefix,
            // even on a fresh (not narrowed) query.
            let Some(first_step) = directory.after_upper(item, needle.as_bytes()[2]) else {
                continue;
            };
            let upper = i64::from(prefix) + first_step + 58 * (needle.len() - 3) as i64;
            prior = Some(prior.map_or(upper, |previous| previous.min(upper)));
        }
        if !slash_pair
            && let Some(upper) = prior
            && (upper + bonus, Reverse(i)) <= threshold
        {
            // Rows pruned by score remain eligible for a later, longer query.
            retained.push(row.with_path(upper, 0));
            continue;
        }
        let found = if let Some(witness) = row.extend_witness(item, source.appended) {
            Some(witness)
        } else if slash_pair && let Some(directory) = &item.directory {
            let score = directory.before_slash[needle.as_bytes()[0] as usize];
            (score != NO_PAIR).then(|| (Bound::exact(i64::from(score)), 0))
        } else if bounded && item.directory_mask & term.directory_mask != term.directory_mask {
            None
        } else if bounded && short {
            paths
                .bound(item, needle.as_bytes(), &term)
                .map(|bound| (bound, 0))
        } else if bounded {
            coarse_path_end(item, needle.as_bytes(), &term.tails, 0, &mut greedy)
        } else {
            score_term(item, needle, true, None, true).map(|s| (Bound::exact(s), 0))
        };
        let Some((bound, end)) = found else {
            continue;
        };
        let upper = if bound.is_exact() {
            bound.upper()
        } else {
            bound.upper().min(prior.unwrap_or(i64::MAX))
        };
        retained.push(if bounded {
            row.with_path(upper, if bound.is_exact() { end } else { 0 })
        } else {
            Row::new(i)
        });
        if (upper + bonus, Reverse(i)) <= threshold {
            continue;
        }
        let score = if bound.is_exact() {
            upper
        } else {
            let min_score = threshold
                .0
                .saturating_sub(bonus)
                .saturating_sub(i64::from(Reverse(i) > threshold.1));
            let score = if short {
                paths.score(item, &term)
            } else {
                exact.path_score(item, needle, &term, 0, min_score)
            };
            let Some(score) = score else {
                continue;
            };
            retained.last_mut().unwrap().path_upper = score as i16;
            score
        };
        let key = (score + bonus, Reverse(i));
        if best.len() < limit {
            best.push(Reverse(key));
        } else if key > cutoff {
            *best.peek_mut().unwrap() = Reverse(key);
        }
        if best.len() == limit {
            cutoff = best.peek().unwrap().0;
            if cutoff > threshold
                && let Some(floor) = source.top_k_floor
            {
                floor.fetch_max(pack(cutoff), Ordering::Relaxed);
            }
        }
    }
    Ok(Some(ScanResult {
        candidates: retained,
        latest,
        hits: best
            .into_sorted_vec()
            .into_iter()
            .map(|Reverse((score, Reverse(index)))| Hit {
                index,
                score,
                positions: Vec::new(),
            })
            .collect(),
    }))
}

impl Matcher {
    /// Clears narrowing after a snapshot/recency change.
    pub fn reset(&mut self) {
        self.previous.clear();
        self.candidates.clear();
        self.latest = 0;
    }
    /// Ranks at most `limit` items, retaining a superset of matches for narrowing.
    pub fn rank(&mut self, items: &[Candidate], query: &str, limit: usize) -> Vec<Hit> {
        self.rank_with_extras(items, &[], &Default::default(), query, limit)
    }
    /// Ranks one shared snapshot plus a small set of non-indexed candidates.
    pub fn rank_with_extras(
        &mut self,
        items: &[Candidate],
        extras: &[Candidate],
        recent: &std::collections::BTreeMap<usize, u64>,
        query: &str,
        limit: usize,
    ) -> Vec<Hit> {
        self.rank_cancel(items, extras, recent, query, limit, || false)
            .unwrap_or_default()
    }
    fn rank_cancel(
        &mut self,
        items: &[Candidate],
        extras: &[Candidate],
        recent: &std::collections::BTreeMap<usize, u64>,
        query: &str,
        limit: usize,
        cancel: impl Fn() -> bool + Sync,
    ) -> Option<Vec<Hit>> {
        static PATH_QUERIES: AtomicU64 = AtomicU64::new(1);
        let query = normalized(query);
        let incremental = !self.previous.is_empty() && query.starts_with(&self.previous);
        let appended = (incremental
            && query.is_ascii()
            && query.len() <= 64
            && self.previous.contains(['/', '\\'])
            && !query.bytes().any(|byte| byte.is_ascii_whitespace()))
        .then(|| &query.as_bytes()[self.previous.len()..]);
        let source = Scan {
            items,
            extras,
            prior: incremental.then_some(self.candidates.as_slice()),
            latest: if incremental { self.latest } else { 0 },
            appended,
            top_k_floor: None,
            generation: if query.contains(['/', '\\']) {
                PATH_QUERIES.fetch_add(1, Ordering::Relaxed)
            } else {
                0
            },
        };
        let count = source.len();
        // Each range, fresh or narrowed, computes its exact top K over global
        // row IDs; their union contains the overall top K.
        let mut combined = None;
        if limit != 0 && should_split(source, &query) {
            let step = count.div_ceil(4);
            let floor = AtomicI64::new(i64::MIN);
            let shared = Scan {
                top_k_floor: Some(&floor),
                ..source
            };
            let scan = |range| Self::rank_scan(shared, recent, &query, limit, range, &cancel);
            let shards = thread::scope(|scope| {
                let scan = &scan;
                let workers: Vec<_> = (step..count)
                    .step_by(step)
                    .map(|start| scope.spawn(move || scan(start..(start + step).min(count))))
                    .collect();
                let mut shards = vec![scan(0..step)?];
                for worker in workers {
                    shards.push(worker.join().ok()??);
                }
                Some(shards)
            })?;
            let newest = shards.iter().map(|shard| shard.latest).max().unwrap_or(0);
            // A range with older inherited recency may need the actual global
            // newest value. Window-specific overrides already share it.
            if !recent.is_empty()
                || shards
                    .iter()
                    .all(|shard| shard.latest == 0 || shard.latest == newest)
            {
                let mut shards = shards.into_iter();
                let mut result = shards.next().unwrap();
                for shard in shards {
                    result.candidates.extend(shard.candidates);
                    result.hits.extend(shard.hits);
                }
                result.latest = newest;
                result
                    .hits
                    .sort_unstable_by(|a, b| b.score.cmp(&a.score).then(a.index.cmp(&b.index)));
                result.hits.truncate(limit);
                combined = Some(result);
            }
        }
        let mut result = match combined {
            Some(result) => result,
            None => Self::rank_scan(source, recent, &query, limit, 0..count, &cancel)?,
        };
        Self::highlight(source, &query, &mut result.hits, &cancel)?;
        self.candidates = result.candidates;
        self.latest = result.latest;
        self.previous = query;
        Some(result.hits)
    }

    fn rank_scan(
        source: Scan<'_>,
        recent: &std::collections::BTreeMap<usize, u64>,
        query: &str,
        limit: usize,
        range: Range<usize>,
        cancel: &impl Fn() -> bool,
    ) -> Option<ScanResult> {
        let terms: Vec<_> = query.split_whitespace().collect();
        if let [needle] = terms.as_slice()
            && limit != 0
            && needle.is_ascii()
            && needle.len() <= 64
            && needle.contains(['/', '\\'])
            && let Ok(result) =
                scan_ascii_path(source, recent, needle, limit, range.clone(), cancel)
        {
            return result;
        }
        let needles: Vec<_> = terms
            .iter()
            .map(|term| (term.as_bytes(), mask(term.as_bytes())))
            .collect();
        let required = needles.iter().fold(0, |m, &(_, bits)| m | bits);
        let ascii = query.is_ascii();
        let mut newest = source.latest;
        let lookup = |i| source.candidate(i);
        let mut matches = Vec::new();
        let mut candidates = Vec::new();
        let ascii_terms: Vec<_> = needles
            .iter()
            .map(|(term, _)| AsciiTerm::new(term))
            .collect();
        let mut alignments = RefineCache::default();
        let mut basename_cache = [None::<(&str, Option<Bound>)>; 256];
        let mut prefix_cache = [None; 256];
        let mut deferred = Vec::new();
        let mut dominant = 0;
        let one_term = needles.first().filter(|_| needles.len() == 1);
        let path_query = ascii && ascii_terms.len() == 1 && ascii_terms[0].first_suffix_row != 0;
        let defer_paths =
            ascii && one_term.is_some_and(|(needle, _)| needle.len() <= 7 && !path_query);
        for n in range {
            if n % 256 == 0 && cancel() {
                return None;
            }
            let row = source.row(n);
            let i = row.index as usize;
            let item = lookup(i);
            newest = newest.max(item.recent);
            if item.mask & required != required {
                continue;
            }
            if path_query
                && item.ascii
                && item.directory_mask & ascii_terms[0].directory_mask
                    != ascii_terms[0].directory_mask
            {
                continue;
            }
            let score = if terms.is_empty() {
                Some(Bound::exact(0))
            } else if !ascii || !item.ascii {
                score_terms(item, terms.iter().copied(), true, None, true).map(Bound::exact)
            } else {
                // Repeated basenames are common in multi-root projects. The
                // basename tiers only depend on that spelling, not the path.
                let base = if item.basename_mask & required == required {
                    let name = &item.label[item.label_name..];
                    let slot = &mut basename_cache[item.basename_key % 256];
                    match *slot {
                        Some((label, base)) if label == name => base,
                        _ => {
                            let base =
                                needles
                                    .iter()
                                    .try_fold(Bound::exact(0), |score, &(needle, _)| {
                                        Some(score.add(coarse_basename(item, needle)?))
                                    });
                            *slot = Some((name, base));
                            base
                        }
                    }
                } else {
                    None
                };
                if defer_paths {
                    if base.is_none() {
                        deferred.push(i);
                        continue;
                    }
                    if dominant < limit {
                        let m = needles[0].0.len() as i64;
                        let n = (item.lower.len() - item.name) as i64;
                        // Any matching basename earns at least 500 tier
                        // points and 20 per byte. Leading and interior
                        // skips together cost at most 2*(n-m). An entire
                        // path can earn at most the ideal alignment + 200
                        // recency, even before its length penalty.
                        if 500 + 20 * m - 2 * (n - m) - 56 > ideal_alignment(m as usize) + 200 {
                            dominant += 1;
                        }
                    }
                }
                base.or_else(|| {
                    if needles.len() == 1 {
                        coarse_path(
                            item,
                            needles[0].0,
                            &ascii_terms[0].tails,
                            0,
                            &mut prefix_cache,
                        )
                    } else {
                        needles.iter().enumerate().try_fold(
                            Bound::exact(0),
                            |score, (n, &(needle, bits))| {
                                let base = (item.basename_mask & bits == bits)
                                    .then(|| coarse_basename(item, needle))
                                    .flatten();
                                Some(score.add(base.or_else(|| {
                                    coarse_path(
                                        item,
                                        needle,
                                        &ascii_terms[n].tails,
                                        n,
                                        &mut prefix_cache,
                                    )
                                })?))
                            },
                        )
                    }
                })
                .map(|score| score.bonus(-length_penalty(item.lower.len())))
            };
            if let Some(mut score) = score {
                let mut next = Row::new(i);
                if path_query
                    && item.ascii
                    && item.lower.len() <= MAX_ALIGN
                    && needles[0].0.len() <= 64
                {
                    let penalty = length_penalty(item.lower.len());
                    score = score.bonus(penalty);
                    // Any alignment of the extension contains an alignment
                    // of the old query and gains at most 58 per appended byte.
                    if let Some(upper) = row.extended_bound(source.appended)
                        && !score.is_exact()
                    {
                        score = Bound::new(score.upper().min(upper), false);
                    }
                    next.path_upper = score.upper() as i16;
                    score = score.bonus(-penalty);
                }
                candidates.push(next);
                matches.push((i, score));
            }
        }
        if dominant < limit {
            for (n, &index) in deferred.iter().enumerate() {
                if n.is_multiple_of(256) && cancel() {
                    return None;
                }
                let item = lookup(index);
                if let Some(score) = coarse_path(
                    item,
                    needles[0].0,
                    &ascii_terms[0].tails,
                    0,
                    &mut prefix_cache,
                ) {
                    candidates.push(Row::new(index));
                    matches.push((index, score.bonus(-length_penalty(item.lower.len()))));
                }
            }
            deferred.clear();
        }
        // These rows cannot make this top K, but may match after narrowing.
        candidates.extend(deferred.into_iter().map(Row::new));
        let latest = newest;
        // Per-window use counters override inherited candidate use counters.
        let newest = recent.values().copied().max().unwrap_or(newest);
        if newest != 0 || !recent.is_empty() {
            for (index, score) in &mut matches {
                *score = score.bonus(recency(lookup(*index), recent, *index, newest));
            }
        }
        let visible = limit.min(matches.len());
        if visible == 0 {
            return Some(ScanResult {
                candidates,
                latest,
                hits: Vec::new(),
            });
        }
        use std::{cmp::Reverse, collections::BinaryHeap};
        let key = |index, score| (score, Reverse(index));
        let by_bound = |a: &(usize, Bound), b: &(usize, Bound)| {
            key(b.0, b.1.upper()).cmp(&key(a.0, a.1.upper()))
        };
        if visible < matches.len() {
            matches.select_nth_unstable_by(visible, by_bound);
        }
        let mut exact = |index, bound: Bound, cutoff: i64| -> Option<i64> {
            let item = lookup(index);
            if bound.is_exact() {
                return Some(bound.upper());
            }
            let bonus = recency(item, recent, index, newest) - length_penalty(item.lower.len());
            if let [term] = terms.as_slice() {
                return alignments
                    .score(
                        item,
                        (term, needles[0].1),
                        &ascii_terms[0],
                        0,
                        cutoff.saturating_sub(bonus),
                    )
                    .map(|score| score + bonus);
            }
            terms.iter().zip(&ascii_terms).enumerate().try_fold(
                bonus,
                |sum, (n, (&term, ascii))| {
                    Some(sum + alignments.score(item, (term, needles[n].1), ascii, n, i64::MIN)?)
                },
            )
        };
        // Score the highest bounds first to establish a cutoff. Revisit all
        // other rows that could still beat it, including ties by original row
        // index. No candidate is discarded based on its greedy score.
        let mut seeds = Vec::with_capacity(visible);
        for &(index, bound) in &matches[..visible] {
            if cancel() {
                return None;
            }
            seeds.push(Reverse(key(index, exact(index, bound, i64::MIN)?)));
        }
        let mut best = BinaryHeap::from(seeds);
        if let Some(next) = matches.get(visible)
            && key(next.0, next.1.upper()) > best.peek().unwrap().0
        {
            for (n, &(index, bound)) in matches[visible..].iter().enumerate() {
                if n.is_multiple_of(256) && cancel() {
                    return None;
                }
                if key(index, bound.upper()) <= best.peek().unwrap().0 {
                    continue;
                }
                let (cutoff, Reverse(worst_index)) = best.peek().unwrap().0;
                let cutoff = cutoff - i64::from(index < worst_index);
                let Some(score) = exact(index, bound, cutoff) else {
                    continue;
                };
                let candidate = key(index, score);
                if candidate > best.peek().unwrap().0 {
                    *best.peek_mut().unwrap() = Reverse(candidate);
                }
            }
        }
        let mut hits = Vec::with_capacity(visible);
        for Reverse((score, Reverse(index))) in best.into_sorted_vec() {
            if cancel() {
                return None;
            }
            hits.push(Hit {
                index,
                score,
                positions: Vec::new(),
            });
        }
        Some(ScanResult {
            candidates,
            latest,
            hits,
        })
    }

    fn highlight(
        source: Scan<'_>,
        query: &str,
        hits: &mut [Hit],
        cancel: &impl Fn() -> bool,
    ) -> Option<()> {
        for hit in hits {
            if cancel() {
                return None;
            }
            score_terms(
                source.candidate(hit.index),
                query.split_whitespace(),
                true,
                Some(&mut hit.positions),
                true,
            );
        }
        Some(())
    }
}

fn should_split(source: Scan<'_>, query: &str) -> bool {
    let count = source.len();
    let minimum = if query.contains(['/', '\\']) {
        24_576
    } else {
        65_536
    };
    if count < minimum || query.trim().is_empty() {
        return false;
    }
    let required = query
        .split_whitespace()
        .fold(0, |bits, term| bits | mask(term.as_bytes()));
    let mut possible = 0;
    for n in (0..count).step_by(count / 128).take(128) {
        let item = source.candidate(source.row(n).index as usize);
        if item.mask & required == required {
            possible += 1;
            if possible >= 14 {
                return true;
            }
        }
    }
    false
}

fn recency(
    item: &Candidate,
    recent: &std::collections::BTreeMap<usize, u64>,
    index: usize,
    newest: u64,
) -> i64 {
    let used = recent.get(&index).copied().unwrap_or(item.recent);
    if used == 0 {
        0
    } else {
        200 - newest.saturating_sub(used).min(200) as i64
    }
}
/// Completed worker result, tagged to prevent stale-query publication.
#[derive(Debug)]
pub struct Ranked {
    /// Submitted generation.
    pub generation: u64,
    /// End-to-end worker queue/scoring time.
    pub elapsed: Duration,
    /// Current top K.
    pub hits: Vec<Hit>,
}
#[derive(Debug)]
enum Work {
    Items(
        Arc<Vec<Candidate>>,
        Arc<Vec<Candidate>>,
        std::collections::BTreeMap<usize, u64>,
    ),
    Query(u64, String, usize, Instant),
    Stop,
}
/// One scoring worker; new queries cancel stale work and queued queries are coalesced.
#[derive(Debug)]
pub struct Worker {
    tx: mpsc::Sender<Work>,
    rx: mpsc::Receiver<Ranked>,
    generation: Arc<AtomicU64>,
    join: Option<thread::JoinHandle<()>>,
}
impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}
impl Worker {
    /// Starts an idle device-free worker.
    pub fn new() -> Self {
        let (tx, requests) = mpsc::channel();
        let (results, rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        let join = thread::spawn(move || {
            let mut items = Arc::new(Vec::new());
            let mut extras = Arc::new(Vec::new());
            let mut recent = std::collections::BTreeMap::new();
            let mut matcher = Matcher::default();
            while let Ok(work) = requests.recv() {
                match work {
                    Work::Stop => break,
                    Work::Items(next, other, recency) => {
                        items = next;
                        extras = other;
                        recent = recency;
                        matcher.reset();
                    }
                    Work::Query(id, query, limit, at) => {
                        if current.load(Ordering::Acquire) != id {
                            continue;
                        }
                        if let Some(hits) =
                            matcher.rank_cancel(&items, &extras, &recent, &query, limit, || {
                                current.load(Ordering::Relaxed) != id
                            })
                            && current.load(Ordering::Acquire) == id
                        {
                            let _ = results.send(Ranked {
                                generation: id,
                                elapsed: at.elapsed(),
                                hits,
                            });
                        }
                    }
                }
            }
        });
        Self {
            tx,
            rx,
            generation,
            join: Some(join),
        }
    }
    /// Atomically invalidates old queries and supplies a new index.
    pub fn set_items(&self, items: Arc<Vec<Candidate>>) {
        self.set_items_with_extras(items, Arc::default(), Default::default());
    }
    /// Invalidate queries and install a snapshot with per-window recency and extra files.
    pub fn set_items_with_extras(
        &self,
        items: Arc<Vec<Candidate>>,
        extras: Arc<Vec<Candidate>>,
        recent: std::collections::BTreeMap<usize, u64>,
    ) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.send(Work::Items(items, extras, recent));
    }
    /// Returns a monotonically increasing query token.
    pub fn query(&self, query: impl Into<String>, limit: usize) -> u64 {
        let id = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self
            .tx
            .send(Work::Query(id, query.into(), limit, Instant::now()));
        id
    }
    /// Invalidates in-flight results while retaining the last completed narrowing set.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    /// Drains old results; only the newest generation can escape.
    pub fn poll(&self) -> Option<Ranked> {
        self.rx
            .try_iter()
            .filter(|r| r.generation == self.generation.load(Ordering::Acquire))
            .last()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.send(Work::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Hang guard: stops a wait that would otherwise never end. It is not a latency
    /// bound; the test checks the condition it waits for.
    const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    fn full_dp(items: &[Candidate], query: &str, limit: usize) -> Vec<Hit> {
        let query = normalized(query);
        let mut ranked: Vec<_> = items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let mut positions = Vec::new();
                let score = score_terms(
                    item,
                    query.split_whitespace(),
                    true,
                    Some(&mut positions),
                    true,
                )?;
                Some(Hit {
                    index,
                    score,
                    positions,
                })
            })
            .collect();
        ranked.sort_unstable_by_key(|hit| (std::cmp::Reverse(hit.score), hit.index));
        ranked.truncate(limit);
        ranked
    }

    #[test]
    fn exact_top_k_keeps_a_winner_below_two_hundred_greedy_decoys() {
        let mut items: Vec<_> = (0..200)
            .map(|i| Candidate::new(format!("xxxx_Dat_{i:03}.rs")))
            .collect();
        items.push(Candidate::new("xd_Data_000.rs"));
        let mut matcher = Matcher::default();
        let _ = matcher.rank(&items, "da", 10);
        for limit in [1, 10, 100] {
            let actual = matcher.rank(&items, "dat", limit);
            assert_eq!(actual[0].index, 200);
            assert_eq!(actual[0].positions, [3, 4, 5]);
            assert_eq!(actual, full_dp(&items, "dat", limit));
        }
        let files: Vec<_> = [
            "spikes/bench-text/src/main.rs",
            "crates/velocity/src/main.rs",
        ]
        .map(Candidate::new)
        .into();
        let result = Matcher::default().rank(&files, "src/main", 1);
        assert_eq!(result[0].index, 1);
        assert_eq!(result, full_dp(&files, "src/main", 1));
    }

    #[test]
    fn large_snapshot_shards_match_serial_recency_ties_and_narrowing() {
        let mut files: Vec<_> = (0..68_000)
            .map(|i| Candidate::new(format!("project/{:04}/src/main_{:03}.rs", i / 100, i % 100)))
            .collect();
        files[17_500].recent = 73;
        files[67_500].recent = 314;
        let extras = [Candidate::new("extras/alpha/src/main.rs")];
        let fresh = Scan {
            items: &files,
            extras: &extras,
            prior: None,
            latest: 0,
            appended: None,
            top_k_floor: None,
            generation: 0,
        };
        let mut sharded = Matcher::default();
        assert!(should_split(fresh, "src/main"));
        for recent in [Default::default(), [(10, 102), (files.len(), 105)].into()] {
            for query in ["s", "s/", "s/m", "s/ma", "src/main", "src/main.rs"] {
                let mut serial =
                    Matcher::rank_scan(fresh, &recent, query, 10, 0..fresh.len(), &|| false)
                        .unwrap();
                Matcher::highlight(fresh, query, &mut serial.hits, &|| false).unwrap();
                assert_eq!(
                    sharded.rank_with_extras(&files, &extras, &recent, query, 10),
                    serial.hits
                );
                assert!(should_split(
                    Scan {
                        prior: Some(&sharded.candidates),
                        ..fresh
                    },
                    query
                ));
            }
        }
    }

    #[test]
    fn typed_separator_paths_match_full_dp_with_gaps_and_component_starts() {
        let mut seed = 98_723_522_u64;
        let components = [
            "s", "test", "c_Set", "assets", "set", "c", "st", "east", "tTe",
        ];
        let items: Vec<_> = (0..1600)
            .map(|i| {
                let mut next = || {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    components[seed as usize % components.len()]
                };
                let separator = if i % 5 == 0 { '\\' } else { '/' };
                Candidate::new(format!(
                    "{}{separator}{}{separator}{}{separator}{}{}_{i}.rs",
                    next(),
                    next(),
                    next(),
                    next(),
                    next(),
                ))
            })
            .collect();
        let mut matcher = Matcher::default();
        for needle in [
            "s/te", "s/tSe", "e/c", "c/t", "s//t", "s\\te", "/s/t", "s/test", "s/t sc",
        ] {
            for end in 1..=needle.len() {
                let query = &needle[..end];
                for limit in [1, 10, 100] {
                    assert_eq!(
                        matcher.rank(&items, query, limit),
                        full_dp(&items, query, limit),
                        "{query} limit={limit}"
                    );
                }
            }
        }
    }

    #[test]
    fn thousand_repo_queries_match_full_dp_top_ten() {
        use std::path::Path;
        fn paths(root: &Path, dir: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let kind = entry.file_type().unwrap();
                let path = entry.path();
                if kind.is_dir() && !matches!(entry.file_name().to_str(), Some("target" | ".git")) {
                    paths(root, &path, out);
                } else if kind.is_file() {
                    out.push(
                        path.strip_prefix(root)
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .to_owned(),
                    );
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut labels = Vec::new();
        for dir in ["crates", "docs"] {
            paths(&root, &root.join(dir), &mut labels);
        }
        labels.sort_unstable();
        let items: Vec<_> = labels.iter().map(Candidate::new).collect();
        assert!(items.len() > 500);
        let mut seed = 8_341_325_213_u64;
        let mut next = |upper| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as usize % upper
        };
        let mut matcher = Matcher::default();
        for n in 0..1000 {
            let label = &items[next(items.len())];
            let text = if n % 5 < 3 {
                &label.lower[label.name..]
            } else {
                &label.lower
            };
            let letters: Vec<_> = text.chars().collect();
            let start = next(letters.len());
            let mut query: String = letters[start..]
                .iter()
                .step_by(1 + next(3))
                .take(1 + next(11))
                .collect();
            if n % 5 == 0 {
                query = format!("{} {query}", letters[next(letters.len())]);
            }
            assert_eq!(
                matcher.rank(&items, &query, 10),
                full_dp(&items, &query, 10),
                "{query:?}"
            );
        }
    }

    #[test]
    fn multi_term_ranking_and_best_alignment_goldens() {
        let candidates: Vec<_> = [
            "src/maintenance.rs",
            "crates/velocity/src/main.rs",
            "src/analytic.rs",
            "src/AbstractController.rs",
            "src/sparse_fuzzy.rs",
            "crates/vel-search/src/fuzzy.rs",
            "lib.rs",
            "liberal.rs",
            "ssearch_fuzzy",
        ]
        .into_iter()
        .map(Candidate::new)
        .collect();
        let mut matcher = Matcher::default();
        for (query, want) in [
            ("main", 1),
            ("ac", 3),
            ("searchfuz", 8),
            ("lib", 6),
            ("search fuzzy", 8),
            ("fuzzy search", 8),
        ] {
            assert_eq!(
                matcher.rank(&candidates, query, 5)[0].index,
                want,
                "{query}"
            );
        }
        let hits = matcher.rank(&candidates, "searchfuz", 5);
        assert_eq!(hits[0].positions, [1, 2, 3, 4, 5, 6, 8, 9, 10]);
        assert_eq!(
            matcher.rank(&candidates, "search fuzzy", 5),
            matcher.rank(&candidates, "fuzzy search", 5)
        );
    }
    #[test]
    fn ranking_goldens_narrowing_backspace_and_unicode() {
        let items: Vec<_> = [
            "src/main.rs",
            "tests/main_test.rs",
            "docs/main/guide.md",
            "src/market.rs",
            "src/MapReader.rs",
            "src/你好.rs",
            "main.rs",
        ]
        .into_iter()
        .map(Candidate::new)
        .collect();
        let mut matcher = Matcher::default();
        for (q, top) in [
            ("main", [6, 0, 1]),
            ("mr", [4, 3, 6]),
            ("你好", [5, usize::MAX, usize::MAX]),
        ] {
            let actual: Vec<_> = matcher.rank(&items, q, 3).iter().map(|h| h.index).collect();
            let expected: Vec<_> = top.into_iter().filter(|i| *i != usize::MAX).collect();
            assert_eq!(actual, expected, "{q}");
        }
        for q in ["m", "ma", "mai", "main", "ma", "rs", ""] {
            assert_eq!(
                matcher.rank(&items, q, 3),
                Matcher::default().rank(&items, q, 3)
            );
        }
        let mut recent = vec![Candidate::new("aa.rs"), Candidate::new("ab.rs")];
        recent[1].recent = 10;
        assert_eq!(matcher.rank(&recent, "", 1)[0].index, 1);
    }
    #[test]
    fn narrowing_keeps_path_hits_outside_the_refinement_budget() {
        let mut items: Vec<_> = (0..200)
            .map(|i| Candidate::new(format!("src/util{i:03}/m{i:03}.rs")))
            .collect();
        items.push(Candidate::new("src/main/src/entry.rs"));
        let mut matcher = Matcher::default();
        let first = matcher.rank(&items, "m", 2);
        assert!(!first.iter().any(|hit| hit.index == 200));
        let next = matcher.rank(&items, "main", 2);
        assert_eq!(next[0].index, 200);
        assert_eq!(&next[0].positions, &[4, 5, 6, 7]);
        assert_eq!(next, Matcher::default().rank(&items, "main", 2));
    }
    #[test]
    fn stale_worker_queries_never_publish() {
        let worker = Worker::new();
        worker.set_items(Arc::new(
            (0..100_000)
                .map(|i| Candidate::new(format!("src/{i}/main.rs")))
                .collect(),
        ));
        worker.query("m", 20);
        let last = worker.query("unmatchable", 20);
        let until = Instant::now() + HANG_GUARD;
        loop {
            if let Some(result) = worker.poll() {
                assert_eq!(result.generation, last);
                assert!(result.hits.is_empty());
                break;
            }
            assert!(Instant::now() < until);
            thread::yield_now();
        }
    }
}
