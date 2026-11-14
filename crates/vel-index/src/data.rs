use crate::names::{NONE, Names, mask};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    path::{Path, PathBuf},
};

/// Public scope-derived symbol category. Unknown custom scopes stay `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SymbolKind {
    /// Function, method or callable reference.
    Function,
    /// Class, struct, enum, trait, interface or alias.
    Type,
    /// Module, package or namespace.
    Namespace,
    /// Variable, property, parameter or constant.
    Variable,
    /// Document heading, tag or other markup.
    Markup,
    /// Unclassified metadata-selected symbol.
    #[default]
    Other,
}
impl SymbolKind {
    /// Infers a category from public scope naming conventions, most specific scope first.
    pub fn from_scopes(scopes: &[vel_scope::Scope]) -> Self {
        for scope in scopes.iter().rev() {
            let name = scope.build_string();
            if name.contains("function") || name.contains("method") {
                return Self::Function;
            }
            if [
                "class",
                "struct",
                "enum",
                "union",
                "trait",
                "interface",
                "type",
            ]
            .iter()
            .any(|part| name.split('.').any(|p| p == *part))
            {
                return Self::Type;
            }
            if name.contains("namespace") || name.contains("module") || name.contains("package") {
                return Self::Namespace;
            }
            if name.starts_with("variable") || name.starts_with("constant") {
                return Self::Variable;
            }
            if name.contains("heading") || name.contains("toc-list") || name.contains("tag") {
                return Self::Markup;
            }
        }
        Self::Other
    }
}
/// Metadata-selected name and its original position in one file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedSymbol {
    /// Transformed index name.
    pub name: String,
    /// Zero-based row.
    pub row: u32,
    /// Zero-based UTF-8 byte column.
    pub column: u32,
    /// Original absolute UTF-8 byte range.
    pub range: Range<u32>,
    /// Public category inferred from scopes.
    pub kind: SymbolKind,
    /// Definition index membership (can coexist with reference membership).
    pub definition: bool,
    /// Reference index membership.
    pub reference: bool,
}
/// Per-file word, sequence and punctuation counters, replaceable without stale contributions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WordStatistics {
    /// Case-sensitive words and occurrence counts.
    pub words: BTreeMap<String, u32>,
    /// Consecutive word sequences of length two or three, reset at newlines.
    pub sequences: BTreeMap<Vec<String>, u32>,
    /// Word and directly trailing punctuation, e.g. ("println", "!(").
    pub trailing: BTreeMap<(String, String), u32>,
}
impl WordStatistics {
    /// Tokenizes UTF-8 words, retaining word sequences and trailing punctuation.
    pub fn from_text(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            let mut words = Vec::new();
            let mut start = None;
            for (at, c) in line
                .char_indices()
                .chain(std::iter::once((line.len(), ' ')))
            {
                if c.is_alphanumeric() || c == '_' {
                    start.get_or_insert(at);
                } else if let Some(start) = start.take() {
                    words.push((start, at));
                }
            }
            for (i, &(a, b)) in words.iter().enumerate() {
                let word = &line[a..b];
                *out.words.entry(word.into()).or_default() += 1;
                for n in [2, 3] {
                    if i + 1 >= n {
                        let sequence = words[i + 1 - n..=i]
                            .iter()
                            .map(|&(a, b)| line[a..b].into())
                            .collect();
                        *out.sequences.entry(sequence).or_default() += 1;
                    }
                }
                let end = line[b..]
                    .char_indices()
                    .find(|(_, c)| c.is_whitespace() || c.is_alphanumeric() || *c == '_')
                    .map_or(line.len(), |(at, _)| b + at);
                if end > b {
                    *out.trailing
                        .entry((word.into(), line[b..end].into()))
                        .or_default() += 1;
                }
            }
        }
        out
    }
}
/// Analysis payload accepted by the in-memory index and stored in the journal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileAnalysis {
    /// Definitions and references.
    pub symbols: Vec<IndexedSymbol>,
    /// Completion statistics.
    pub words: WordStatistics,
}
/// Owned query result. Coordinates are zero-based UTF-8; paths are absolute for project files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolLocation {
    /// Source file.
    pub file: PathBuf,
    /// Transformed index name.
    pub name: String,
    /// Zero-based row.
    pub row: usize,
    /// Zero-based UTF-8 byte column.
    pub column: usize,
    /// Original UTF-8 byte range.
    pub range: Range<usize>,
    /// Scope-derived category.
    pub kind: SymbolKind,
}
/// Ranked project symbol.
#[derive(Debug, Clone)]
pub struct SymbolMatch {
    /// Source location.
    pub location: SymbolLocation,
    /// Higher is a better fuzzy match.
    pub score: i64,
}
/// A word candidate enriched by project context.
#[derive(Debug, Clone)]
pub struct WordSuggestion {
    /// Word spelling.
    pub word: String,
    /// Project occurrence count.
    pub frequency: u32,
    /// Matching preceding bigram/trigram count.
    pub context_count: u32,
    /// Most frequent directly trailing punctuation for this word.
    pub trailing: String,
    /// Fuzzy quality plus bounded frequency/context bonuses.
    pub score: i64,
    /// Scope-derived category when this word is also a definition.
    pub kind: SymbolKind,
}
#[derive(Debug, Clone, Copy)]
struct Slot {
    name: u32,
    file: u32,
    row: u32,
    column: u32,
    start: u32,
    end: u32,
    next: u32,
    flags: u8,
    kind: SymbolKind,
}
#[derive(Debug, Default)]
struct Contributions {
    symbols: Vec<u32>,
    words: Vec<(u32, u32)>,
    sequences: Vec<([u32; 3], u32)>,
    trailing: Vec<((u32, String), u32)>,
}
#[derive(Debug)]
struct File {
    path: PathBuf,
    data: Contributions,
}
/// Compact, mutable in-memory index. The background service serializes writes; queries need only
/// a shared lock. Deleted slots are reused and exact lookup follows name-local posting chains.
#[derive(Debug, Default)]
pub struct Index {
    names: Names,
    slots: crate::slab::Slab<Slot>,
    free: Vec<u32>,
    files: Vec<File>,
    paths: HashMap<PathBuf, u32>,
    sequences: crate::sequences::Sequences,
    trailing: HashMap<u32, Vec<(String, u32)>>,
    symbols: usize,
}
impl Index {
    /// New empty index (also useful for embedded callers/tests).
    pub fn new() -> Self {
        Self::default()
    }
    /// Replaces one file's complete contributions, removing old definitions, references and stats.
    pub fn replace_file(&mut self, path: impl Into<PathBuf>, analysis: FileAnalysis) {
        let path = path.into();
        self.remove_file(&path);
        let file = if let Some(&id) = self.paths.get(&path) {
            id
        } else {
            let id = self.files.len() as u32;
            self.files.push(File {
                path: path.clone(),
                data: Contributions::default(),
            });
            self.paths.insert(path, id);
            id
        };
        let mut data = Contributions::default();
        for symbol in analysis.symbols {
            if !symbol.definition && !symbol.reference || symbol.name.trim().is_empty() {
                continue;
            }
            let name = self.names.intern(&symbol.name);
            let entry = &mut self.names.records[name as usize];
            let id = self.free.pop().unwrap_or(self.slots.len() as u32);
            let slot = Slot {
                name,
                file,
                row: symbol.row,
                column: symbol.column,
                start: symbol.range.start,
                end: symbol.range.end,
                next: entry.head,
                flags: u8::from(symbol.definition) | (u8::from(symbol.reference) * 2),
                kind: symbol.kind,
            };
            if id as usize == self.slots.len() {
                self.slots.push(slot);
            } else {
                self.slots[id as usize] = slot;
            }
            entry.head = id;
            entry.definitions += u32::from(symbol.definition);
            data.symbols.push(id);
            self.symbols += 1;
        }
        for (word, count) in analysis.words.words {
            let id = self.names.intern(&word);
            self.names.records[id as usize].frequency = self.names.records[id as usize]
                .frequency
                .saturating_add(count);
            data.words.push((id, count));
        }
        for (sequence, count) in analysis.words.sequences {
            if !(2..=3).contains(&sequence.len()) {
                continue;
            }
            let mut ids = [NONE; 3];
            for (i, word) in sequence.iter().enumerate() {
                ids[i] = self.names.intern(word);
            }
            self.sequences.add(ids, count);
            data.sequences.push((ids, count));
        }
        for ((word, punct), count) in analysis.words.trailing {
            let key = (self.names.intern(&word), punct);
            let variants = self.trailing.entry(key.0).or_default();
            if let Some((_, total)) = variants.iter_mut().find(|(punct, _)| punct == &key.1) {
                *total += count;
            } else {
                variants.push((key.1.clone(), count));
            }
            data.trailing.push((key, count));
        }
        self.files[file as usize].data = data;
    }
    /// Removes file contributions; other files containing the same name remain indexed.
    pub fn remove_file(&mut self, path: &Path) {
        let Some(&file) = self.paths.get(path) else {
            return;
        };
        let data = std::mem::take(&mut self.files[file as usize].data);
        let names: std::collections::HashSet<_> = data
            .symbols
            .iter()
            .map(|&id| self.slots[id as usize].name)
            .collect();
        for name in names {
            let record = &mut self.names.records[name as usize];
            let mut cursor = record.head;
            let mut previous = NONE;
            while cursor != NONE {
                let old = self.slots[cursor as usize];
                if old.file == file {
                    if previous == NONE {
                        record.head = old.next;
                    } else {
                        self.slots[previous as usize].next = old.next;
                    }
                    record.definitions -= u32::from(old.flags & 1 != 0);
                    self.slots[cursor as usize].flags = 0;
                    self.free.push(cursor);
                    self.symbols -= 1;
                } else {
                    previous = cursor;
                }
                cursor = old.next;
            }
        }
        for (id, count) in data.words {
            self.names.records[id as usize].frequency -= count;
        }
        for (key, count) in data.sequences {
            self.sequences.subtract(key, count);
        }
        for ((word, punct), count) in data.trailing {
            if let Some(variants) = self.trailing.get_mut(&word) {
                if let Some(at) = variants.iter().position(|(p, _)| p == &punct) {
                    variants[at].1 -= count;
                    if variants[at].1 == 0 {
                        variants.swap_remove(at);
                    }
                }
                if variants.is_empty() {
                    self.trailing.remove(&word);
                }
            }
        }
    }
    /// Number of active definition/reference locations.
    pub fn symbol_count(&self) -> usize {
        self.symbols
    }
    /// Estimated retained allocation bytes, excluding allocator overhead and temporary queries.
    pub fn storage_bytes(&self) -> usize {
        self.names.storage_bytes()
            + self.slots.capacity() * std::mem::size_of::<Slot>()
            + self.free.capacity() * 4
            + self
                .files
                .iter()
                .map(|file| {
                    file.data.symbols.capacity() * 4
                        + file.data.words.capacity() * 8
                        + file.data.sequences.capacity() * 16
                        + file.data.trailing.capacity()
                            * std::mem::size_of::<((u32, String), u32)>()
                        + file
                            .data
                            .trailing
                            .iter()
                            .map(|((_, punct), _)| punct.capacity())
                            .sum::<usize>()
                        + file.path.as_os_str().len()
                })
                .sum::<usize>()
            + self.sequences.storage_bytes()
            + self.trailing.capacity() * 64
    }
    fn location(&self, slot: Slot) -> SymbolLocation {
        SymbolLocation {
            file: self.files[slot.file as usize].path.clone(),
            name: self.names.text(slot.name).into(),
            row: slot.row as usize,
            column: slot.column as usize,
            range: slot.start as usize..slot.end as usize,
            kind: slot.kind,
        }
    }
    fn lookup(&self, name: &str, flag: u8) -> Vec<SymbolLocation> {
        let Some(id) = self.names.find(name) else {
            return Vec::new();
        };
        let mut slot = self.names.records[id as usize].head;
        let mut out = Vec::new();
        while slot != NONE {
            let record = self.slots[slot as usize];
            if record.flags & flag != 0 {
                out.push(self.location(record));
            }
            slot = record.next;
        }
        out.sort_by(|a, b| a.file.cmp(&b.file).then(a.range.start.cmp(&b.range.start)));
        out
    }
    /// Exact, case-sensitive definition lookup.
    pub fn lookup_definition(&self, name: &str) -> Vec<SymbolLocation> {
        self.lookup(name, 1)
    }
    /// Exact, case-sensitive reference lookup.
    pub fn lookup_references(&self, name: &str) -> Vec<SymbolLocation> {
        self.lookup(name, 2)
    }
    /// Top-K fuzzy definitions, ordered by score and stable source location.
    pub fn fuzzy_symbols(&self, query: &str, k: usize) -> Vec<SymbolMatch> {
        self.fuzzy_symbols_cancel(query, k, || false)
            .unwrap_or_default()
    }
    /// Cancellable top-K lookup, checking cancellation at least every 256 interned names.
    pub fn fuzzy_symbols_cancel(
        &self,
        query: &str,
        k: usize,
        cancel: impl Fn() -> bool + Sync,
    ) -> Option<Vec<SymbolMatch>> {
        if cancel() {
            return None;
        }
        if k == 0 {
            return Some(Vec::new());
        }
        let required = mask(query);
        let scan = |range: Range<usize>| -> Option<Vec<(i64, u32)>> {
            use std::{cmp::Reverse, collections::BinaryHeap};
            let mut scorer = crate::names::FuzzyScorer::new(query);
            let mut best: BinaryHeap<std::cmp::Reverse<(i64, Reverse<u32>)>> =
                BinaryHeap::with_capacity(k.min(1024));
            for id in range {
                if id % 256 == 0 && cancel() {
                    return None;
                }
                let name = &self.names.records[id];
                if name.definitions == 0 || name.mask & required != required {
                    continue;
                }
                if let Some(score) = scorer.score(self.names.text(id as u32)) {
                    let key = (score, Reverse(id as u32));
                    if best.len() < k {
                        best.push(Reverse(key));
                    } else if best.peek().is_some_and(|worst| key > worst.0) {
                        *best.peek_mut().unwrap() = Reverse(key);
                    }
                }
                if query.is_empty() && best.len() == k {
                    break;
                }
            }
            Some(
                best.into_iter()
                    .map(|Reverse((score, Reverse(id)))| (score, id))
                    .collect(),
            )
        };
        let count = self.names.records.len();
        let mut hits = if count >= 200_000 && !query.is_empty() {
            std::thread::scope(|scope| {
                let scan = &scan;
                let step = count.div_ceil(8);
                let handles: Vec<_> = (0..count)
                    .step_by(step)
                    .map(|start| scope.spawn(move || scan(start..(start + step).min(count))))
                    .collect();
                let mut hits = Vec::new();
                for handle in handles {
                    hits.extend(handle.join().ok()??);
                }
                Some(hits)
            })?
        } else {
            scan(0..count)?
        };
        let cmp = |a: &(i64, u32), b: &(i64, u32)| b.0.cmp(&a.0).then(a.1.cmp(&b.1));
        hits.sort_unstable_by(cmp);
        hits.truncate(k);
        let mut out = Vec::new();
        for (score, id) in hits {
            let mut slot = self.names.records[id as usize].head;
            let remaining = k - out.len();
            let mut locations = std::collections::BinaryHeap::new();
            while slot != NONE {
                if slot.is_multiple_of(256) && cancel() {
                    return None;
                }
                let record = self.slots[slot as usize];
                if record.flags & 1 != 0 {
                    let key = (
                        self.files[record.file as usize].path.as_path(),
                        record.start,
                        slot,
                    );
                    if locations.len() < remaining {
                        locations.push(key);
                    } else if locations.peek().is_some_and(|worst| key < *worst) {
                        *locations.peek_mut().unwrap() = key;
                    }
                }
                slot = record.next;
            }
            for (_, _, slot) in locations.into_sorted_vec() {
                out.push(SymbolMatch {
                    location: self.location(self.slots[slot as usize]),
                    score,
                });
                if out.len() == k {
                    return Some(out);
                }
            }
        }
        Some(out)
    }
    /// Word-sequence count; accepts two or three case-sensitive words.
    pub fn sequence_count(&self, sequence: &[&str]) -> u32 {
        if !(2..=3).contains(&sequence.len()) {
            return 0;
        }
        let mut ids = [NONE; 3];
        for (i, word) in sequence.iter().enumerate() {
            let Some(id) = self.names.find(word) else {
                return 0;
            };
            ids[i] = id;
        }
        self.sequences.count(ids)
    }
    /// Exact project word frequency.
    pub fn word_count(&self, word: &str) -> u32 {
        self.names
            .find(word)
            .map_or(0, |id| self.names.records[id as usize].frequency)
    }
    /// Top-K word candidates using up to two preceding words and directly trailing punctuation.
    pub fn suggest_words(
        &self,
        query: &str,
        preceding: &[&str],
        k: usize,
        cancel: impl Fn() -> bool,
    ) -> Option<Vec<WordSuggestion>> {
        self.suggest_words_for(std::slice::from_ref(&query), preceding, k, cancel)
    }
    /// Like `suggest_words`, requiring compatibility with every caret prefix before top-K
    /// selection. Duplicate prefixes can be removed by the caller.
    pub fn suggest_words_for(
        &self,
        queries: &[&str],
        preceding: &[&str],
        k: usize,
        cancel: impl Fn() -> bool,
    ) -> Option<Vec<WordSuggestion>> {
        let required = queries.iter().fold(0, |bits, query| bits | mask(query));
        let context: Vec<_> = preceding
            .iter()
            .rev()
            .take(2)
            .rev()
            .map(|word| self.names.find(word))
            .collect();
        let context_counts = self
            .sequences
            .context_counts(&context, self.names.records.len());
        use std::{cmp::Reverse, collections::BinaryHeap};
        if k == 0 {
            return Some(Vec::new());
        }
        let mut scorers: Vec<_> = queries
            .iter()
            .map(|query| crate::names::FuzzyScorer::new(query))
            .collect();
        let mut best: BinaryHeap<Reverse<(i64, Reverse<u32>, u32)>> =
            BinaryHeap::with_capacity(k.min(1024));
        for (id, name) in self.names.records.iter().enumerate() {
            if id % 256 == 0 && cancel() {
                return None;
            }
            if name.frequency == 0 && name.definitions == 0 || name.mask & required != required {
                continue;
            }
            let word = self.names.text(id as u32);
            let score = if scorers.len() == 1 {
                scorers[0].score(word)
            } else {
                let count = scorers.len().max(1);
                scorers
                    .iter_mut()
                    .try_fold(0i64, |total, scorer| {
                        scorer.score(word).map(|score| total + score)
                    })
                    .map(|score| score / count as i64)
            };
            if let Some(mut score) = score {
                let context_count = context_counts[id];
                score +=
                    name.frequency.max(1).ilog2() as i64 * 3 + context_count.min(50) as i64 * 8;
                let key = (score, Reverse(id as u32), context_count);
                if best.len() < k {
                    best.push(Reverse(key));
                } else if best.peek().is_some_and(|worst| key > worst.0) {
                    *best.peek_mut().unwrap() = Reverse(key);
                }
            }
        }
        let mut hits: Vec<_> = best
            .into_iter()
            .map(|Reverse((score, Reverse(id), context_count))| (score, id, context_count))
            .collect();
        hits.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        Some(
            hits.into_iter()
                .map(|(score, id, context_count)| {
                    let trailing = self
                        .trailing
                        .get(&id)
                        .and_then(|variants| {
                            variants
                                .iter()
                                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                        })
                        .map_or_else(String::new, |(punct, _)| punct.clone());
                    let mut slot = self.names.records[id as usize].head;
                    let mut kind = SymbolKind::Other;
                    while slot != NONE {
                        let record = self.slots[slot as usize];
                        if record.flags & 1 != 0 {
                            kind = record.kind;
                            break;
                        }
                        slot = record.next;
                    }
                    WordSuggestion {
                        word: self.names.text(id).into(),
                        frequency: self.names.records[id as usize].frequency,
                        context_count,
                        kind,
                        trailing,
                        score,
                    }
                })
                .collect(),
        )
    }
}
