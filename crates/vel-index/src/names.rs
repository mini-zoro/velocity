use std::hash::{Hash, Hasher};
pub(crate) const NONE: u32 = u32::MAX;
const PLAIN_ASCII: u32 = 1 << 31;
const SHORT_LENGTH_BITS: u32 = 10;
const SHORT_LENGTH_MASK: u32 = (1 << SHORT_LENGTH_BITS) - 1;
#[derive(Debug, Default)]
pub(crate) struct Name {
    pub(crate) start: u32,
    pub(crate) mask: u64,
    pub(crate) head: u32,
    pub(crate) definitions: u32,
    pub(crate) frequency: u32,
}
/// Compact string interner: a UTF-8 arena and open-addressed u32 IDs, no per-name allocations.
#[derive(Debug, Default)]
pub(crate) struct Names {
    arena: Vec<String>,
    lengths: crate::slab::Slab<u32>,
    pub(crate) records: crate::slab::Slab<Name>,
    table: Vec<u32>,
}
fn hash(text: &str) -> usize {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish() as usize
}
pub(crate) fn mask(text: &str) -> u64 {
    text.chars()
        .flat_map(char::to_lowercase)
        .fold(0, |m, c| m | 1 << (c as u32 % 64))
}
impl Names {
    pub(crate) fn text(&self, id: u32) -> &str {
        self.label(id).0
    }
    pub(crate) fn label(&self, id: u32) -> (&str, Option<usize>) {
        let n = &self.records[id as usize];
        let chunk = (n.start >> 20) as usize;
        let offset = (n.start & ((1 << 20) - 1)) as usize;
        let packed = self.lengths[id as usize];
        let (len, core) = if packed & PLAIN_ASCII != 0 {
            (
                (packed & SHORT_LENGTH_MASK) as usize,
                Some(((packed & !PLAIN_ASCII) >> SHORT_LENGTH_BITS) as usize),
            )
        } else {
            (packed as usize, None)
        };
        (&self.arena[chunk][offset..offset + len], core)
    }
    pub(crate) fn find(&self, text: &str) -> Option<u32> {
        if self.table.is_empty() {
            return None;
        }
        let mut slot = hash(text) & (self.table.len() - 1);
        loop {
            let id = self.table[slot];
            if id == NONE {
                return None;
            }
            if self.text(id) == text {
                return Some(id);
            }
            slot = (slot + 1) & (self.table.len() - 1);
        }
    }
    pub(crate) fn intern(&mut self, text: &str) -> u32 {
        if let Some(id) = self.find(text) {
            return id;
        }
        if (self.records.len() + 1) * 10 > self.table.len() * 7 {
            self.table = vec![NONE; (self.table.len() * 2).max(32)];
            for id in 0..self.records.len() as u32 {
                let mut slot = hash(self.text(id)) & (self.table.len() - 1);
                while self.table[slot] != NONE {
                    slot = (slot + 1) & (self.table.len() - 1);
                }
                self.table[slot] = id;
            }
        }
        const CHUNK: usize = 1 << 20;
        if self
            .arena
            .last()
            .is_none_or(|chunk| chunk.len().saturating_add(text.len()) > CHUNK)
        {
            assert!(
                self.arena.len() < 4096,
                "index spelling arena exceeds 4 GiB of chunks"
            );
            self.arena
                .push(String::with_capacity(CHUNK.max(text.len())));
        }
        let chunk = self.arena.len() - 1;
        let offset = self.arena[chunk].len();
        let start = ((chunk as u32) << 20) | offset as u32;
        assert!(
            text.len() < PLAIN_ASCII as usize,
            "index name exceeds 2 GiB"
        );
        let length = text.len() as u32;
        let id = u32::try_from(self.records.len()).expect("index name ID overflow");
        self.arena[chunk].push_str(text);
        // Use the spare bits of a short label's length for its numeric
        // suffix boundary. Querying the interner needs no extra allocation.
        let packed = if length <= SHORT_LENGTH_MASK
            && text
                .bytes()
                .all(|c| c.is_ascii() && !matches!(c, b'/' | b'\\' | b'.'))
        {
            let core = text.trim_end_matches(|c: char| c.is_ascii_digit()).len() as u32;
            PLAIN_ASCII | length | (core << SHORT_LENGTH_BITS)
        } else {
            length
        };
        self.lengths.push(packed);
        self.records.push(Name {
            start,
            mask: mask(text),
            head: NONE,
            ..Name::default()
        });
        let mut slot = hash(text) & (self.table.len() - 1);
        while self.table[slot] != NONE {
            slot = (slot + 1) & (self.table.len() - 1);
        }
        self.table[slot] = id;
        id
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        self.arena.iter().map(String::capacity).sum::<usize>()
            + self.lengths.capacity() * 4
            + self.records.capacity() * std::mem::size_of::<Name>()
            + self.table.capacity() * 4
    }
}
/// Original Unicode-aware subsequence score shared by symbols and completions.
/// Prefixes, adjacent matches and word/CamelCase starts receive bounded bonuses.
pub fn fuzzy_score(label: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    if label.is_ascii() && query.is_ascii() {
        let mut at = 0;
        let mut score = 0;
        let bytes = label.as_bytes();
        for (i, needle) in query.bytes().enumerate() {
            let pos = bytes[at..]
                .iter()
                .position(|c| c.eq_ignore_ascii_case(&needle))?
                + at;
            score += 20;
            if pos == at && i > 0 {
                score += 38;
            }
            if pos == 0
                || !bytes[pos - 1].is_ascii_alphanumeric()
                || bytes[pos - 1].is_ascii_lowercase() && bytes[pos].is_ascii_uppercase()
            {
                score += 32;
            }
            score -= (pos - at) as i64;
            at = pos + 1;
        }
        if label.len() >= query.len() && label[..query.len()].eq_ignore_ascii_case(query) {
            score += 150;
        }
        if label.eq_ignore_ascii_case(query) {
            score += 1000;
        }
        return Some(score - label.len() as i64);
    }
    let lower = label.to_lowercase();
    let query = query.to_lowercase();
    let mut chars = lower.char_indices();
    let mut score = 0;
    let mut previous = None;
    for c in query.chars() {
        let (at, value) = chars.find(|(_, v)| *v == c)?;
        score += 20;
        if previous == Some(at) {
            score += 38;
        }
        previous = Some(at + value.len_utf8());
    }
    if lower.starts_with(&query) {
        score += 150;
    }
    if lower == query {
        score += 1000;
    }
    Some(score - label.chars().count() as i64)
}

#[derive(Debug, Clone, Copy, Default)]
struct PrefixScore {
    matched: usize,
    at: usize,
    score: i64,
    prefix: bool,
}
/// Query-local reuse of a candidate's first 16 bytes, common for namespace-qualified symbols.
/// Greedy match state is cached, never a final answer: every remaining byte is still scored.
pub(crate) struct FuzzyScorer {
    query: Vec<u8>,
    ascii: bool,
    original: String,
    last: Option<([u8; 16], PrefixScore)>,
}
impl FuzzyScorer {
    pub(crate) fn new(query: &str) -> Self {
        Self {
            query: query.bytes().map(|b| b.to_ascii_lowercase()).collect(),
            ascii: query.is_ascii(),
            original: query.into(),
            last: None,
        }
    }
    pub(crate) fn score(&mut self, label: &str) -> Option<i64> {
        if !self.ascii || label.len() < 16 || !label.is_ascii() || self.query.is_empty() {
            return fuzzy_score(label, &self.original);
        }
        let bytes = label.as_bytes();
        let key: [u8; 16] = bytes[..16].try_into().unwrap();
        let mut state = if let Some((old, state)) = self.last
            && old == key
        {
            state
        } else {
            let mut state = PrefixScore::default();
            while state.matched < self.query.len() {
                let Some(pos) = bytes[state.at..16]
                    .iter()
                    .position(|c| c.to_ascii_lowercase() == self.query[state.matched])
                    .map(|p| p + state.at)
                else {
                    break;
                };
                advance_score(&mut state, bytes, pos);
            }
            state.prefix = self.query.len() <= 16
                && bytes[..self.query.len()].eq_ignore_ascii_case(&self.query);
            self.last = Some((key, state));
            state
        };
        while state.matched < self.query.len() {
            let pos = bytes[state.at.max(16)..]
                .iter()
                .position(|c| c.to_ascii_lowercase() == self.query[state.matched])?
                + state.at.max(16);
            advance_score(&mut state, bytes, pos);
        }
        if state.prefix
            || self.query.len() > 16
                && bytes.len() >= self.query.len()
                && bytes[..self.query.len()].eq_ignore_ascii_case(&self.query)
        {
            state.score += 150;
        }
        if bytes.len() == self.query.len() && bytes.eq_ignore_ascii_case(&self.query) {
            state.score += 1000;
        }
        Some(state.score - bytes.len() as i64)
    }
}
fn advance_score(state: &mut PrefixScore, bytes: &[u8], pos: usize) {
    state.score += 20;
    if pos == state.at && state.matched > 0 {
        state.score += 38;
    }
    if pos == 0
        || !bytes[pos - 1].is_ascii_alphanumeric()
        || bytes[pos - 1].is_ascii_lowercase() && bytes[pos].is_ascii_uppercase()
    {
        state.score += 32;
    }
    state.score -= (pos - state.at) as i64;
    state.at = pos + 1;
    state.matched += 1;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_prefix_scores_match_the_scalar_reference() {
        let mut seed = 71u64;
        for query in [
            "rw99",
            "widget42",
            "render",
            "XY",
            "",
            "é",
            "abcdefghijklmnopq",
            "00",
            "::",
        ] {
            let mut scorer = FuzzyScorer::new(query);
            for n in 0..5000 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let value = match n % 4 {
                    0 => format!("render_widget_{n:07}"),
                    1 => format!("render_widget_{:07}", seed % 1_000_000),
                    2 => format!("Foo::BarClassX{}_{n}", seed % 100),
                    _ => format!("é你好{n}"),
                };
                assert_eq!(
                    scorer.score(&value),
                    fuzzy_score(&value, query),
                    "{query}/{value}"
                );
            }
        }
    }
}
