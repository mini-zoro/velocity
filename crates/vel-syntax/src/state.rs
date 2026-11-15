//! Editor-facing snapshots; context stacks are shared and hash-consed by the interpreter.
use crate::{
    ParsingError, SyntaxReference, SyntaxSet,
    parser::{InterpreterState, ScopeOperations},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
};
/// Earliest previously emitted line whose highlighting changed. Lines are zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rewind {
    /// Rebuild scope stacks and spans starting here.
    pub from_line: usize,
}
/// Byte-indexed scope operations and any corrections from branch backtracking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineOps {
    /// Operations for the supplied line, in application order.
    pub ops: ScopeOperations,
    /// Earliest corrected earlier line.
    pub rewind: Option<Rewind>,
    /// Replacement operations for already returned lines (zero-based).
    pub corrections: Vec<(usize, ScopeOperations)>,
    /// Resource and branch-expiry diagnostics.
    pub warnings: Vec<String>,
}
/// Cheap, immutable-to-clones parser checkpoint bound to one syntax set.
///
/// Feed complete lines including `\n`. Keep a [`crate::ScopeStack`] alongside each
/// checkpoint. When `rewind` is returned, replace cached operations with `corrections`
/// and rebuild scope stacks from that earlier line. A branch can revise at most 128
/// preceding lines, matching Sublime's documented limit.
///
/// Equality and hashing omit absolute line numbers when no branch is active, so
/// incremental highlighting can converge across inserted/deleted lines. Active
/// branches conservatively include their replay history in equality.
#[derive(Debug, Clone)]
pub struct ParseState {
    set: Arc<SyntaxSet>,
    engine: Arc<InterpreterState>,
    fingerprint: u64,
    next_line: usize,
    lines: Arc<Mutex<LineCache>>,
    cache_enabled: bool,
}
#[derive(Debug)]
struct CachedLine {
    // Clock replacement keeps repeated transitions resident without an LRU
    // allocation or queue update on every hit. Cold admissions get no extra pass.
    referenced: bool,
    text: Box<str>,
    before: Arc<InterpreterState>,
    after: Arc<InterpreterState>,
    fingerprint: u64,
    ops: Arc<[(usize, crate::ScopeStackOp)]>,
}
#[derive(Debug)]
struct LineCache {
    capacity: usize,
    bytes: usize,
    entries: HashMap<u64, CachedLine>,
    order: VecDeque<u64>,
    operations: HashSet<Arc<[(usize, crate::ScopeStackOp)]>>,
}
impl LineCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
            operations: HashSet::new(),
        }
    }
    fn insert(&mut self, key: u64, line: CachedLine) {
        const BYTES: usize = 4 * 1024 * 1024;
        if line.text.len() > BYTES || self.entries.contains_key(&key) {
            return;
        }
        while self.entries.len() >= self.capacity || self.bytes + line.text.len() > BYTES {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.get_mut(&old)
                && entry.referenced
            {
                entry.referenced = false;
                self.order.push_back(old);
                continue;
            }
            if let Some(entry) = self.entries.remove(&old) {
                self.bytes -= entry.text.len();
            }
        }
        self.bytes += line.text.len();
        self.order.push_back(key);
        self.entries.insert(key, line);
    }
}
impl ParseState {
    /// Binds a new interpreter to a syntax belonging to `set`.
    pub fn new(set: Arc<SyntaxSet>, syntax: &SyntaxReference) -> Self {
        let engine = Arc::new(InterpreterState::new(syntax));
        let fingerprint = engine.fingerprint();
        Self {
            set,
            engine,
            fingerprint,
            next_line: 0,
            lines: Arc::new(Mutex::new(LineCache::new(16384))),
            cache_enabled: true,
        }
    }
    /// Bounds memoized stable line transitions. Zero disables memoization, which is
    /// useful for backend differentials and isolated matcher throughput measurements.
    /// Cloned checkpoints share the cache; it is excluded from semantic equality.
    pub fn with_line_cache_capacity(mut self, capacity: usize) -> Self {
        self.lines = Arc::new(Mutex::new(LineCache::new(capacity)));
        self.cache_enabled = capacity != 0;
        self
    }
    /// Parses a contiguous UTF-8 line, preserving all speculative branch state.
    pub fn parse_line(&mut self, line: &str) -> Result<LineOps, ParsingError> {
        let mut output = LineOps::default();
        self.parse_line_into(line, &mut output)?;
        Ok(output)
    }
    /// Parses without retaining this line in the shared transition cache. Useful
    /// for explicit background repair of oversized lines: the exact interpreter
    /// state is preserved, but a multi-MB string is not cached after a stable parse.
    /// Speculative branch replay may still retain its required source history.
    pub fn parse_line_uncached(&mut self, line: &str) -> Result<LineOps, ParsingError> {
        let enabled = self.cache_enabled;
        self.cache_enabled = false;
        let result = self.parse_line(line);
        self.cache_enabled = enabled;
        result
    }
    /// Parses a line while retaining the operation and correction vector capacities.
    /// Use this for streaming parsing; ordinary, non-speculative lines reuse storage.
    pub fn parse_line_into(&mut self, line: &str, out: &mut LineOps) -> Result<(), ParsingError> {
        let stable = !self.engine.is_speculative();
        let key = if stable && self.cache_enabled {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            self.fingerprint.hash(&mut hash);
            line.hash(&mut hash);
            let key = hash.finish();
            let mut cache = self.lines.lock().unwrap_or_else(|e| e.into_inner());
            if cache.capacity == 0 {
                None
            } else {
                if let Some(entry) = cache.entries.get_mut(&key)
                    && entry.text.as_ref() == line
                    && entry.before.semantic_eq(&self.engine)
                {
                    entry.referenced = true;
                    out.ops.clear();
                    out.ops.extend_from_slice(&entry.ops);
                    out.corrections.clear();
                    out.warnings.clear();
                    out.rewind = None;
                    self.engine = entry.after.clone();
                    self.fingerprint = entry.fingerprint;
                    self.next_line += 1;
                    return Ok(());
                }
                Some(key)
            }
        } else {
            None
        };
        let before = key.map(|_| self.engine.clone());
        let engine = Arc::make_mut(&mut self.engine);
        if stable {
            engine.relocate(self.next_line);
        }
        let output = engine.parse_line_reusing(line, &self.set, std::mem::take(&mut out.ops));
        self.fingerprint = engine.fingerprint();
        let output = output?;
        out.corrections.clear();
        out.corrections
            .extend(output.replayed.into_iter().map(|(n, ops)| (n - 1, ops)));
        out.rewind = out
            .corrections
            .iter()
            .map(|(n, _)| *n)
            .min()
            .map(|from_line| Rewind { from_line });
        out.ops = output.ops;
        out.warnings = output.warnings;
        self.next_line += 1;
        if let Some(key) = key
            && !self.engine.is_speculative()
            && out.rewind.is_none()
            && out.warnings.is_empty()
        {
            let mut cache = self.lines.lock().unwrap_or_else(|e| e.into_inner());
            let ops = if let Some(shared) = cache.operations.get(out.ops.as_slice()) {
                shared.clone()
            } else {
                let ops: Arc<[_]> = Arc::from(out.ops.as_slice());
                if cache.operations.len() < cache.capacity {
                    cache.operations.insert(ops.clone());
                }
                ops
            };
            cache.insert(
                key,
                CachedLine {
                    referenced: false,
                    text: line.into(),
                    before: before.expect("memoization enabled"),
                    after: self.engine.clone(),
                    fingerprint: self.fingerprint,
                    ops,
                },
            );
        }
        Ok(())
    }
    /// Whether a later `fail` may correct previously emitted operations.
    pub fn is_speculative(&self) -> bool {
        self.engine.is_speculative()
    }

    /// Relocates a stable checkpoint after inserted/deleted lines. Returns false if an
    /// active branch's absolute replay history would need rebasing; reparse that region.
    pub fn resume_at(&mut self, next_line: usize) -> bool {
        if self.engine.is_speculative() && self.next_line != next_line {
            return false;
        }
        self.next_line = next_line;
        true
    }
    /// The shared immutable syntax collection this state references.
    pub fn syntax_set(&self) -> &Arc<SyntaxSet> {
        &self.set
    }
}
impl PartialEq for ParseState {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.set, &other.set)
            && self.fingerprint == other.fingerprint
            && (Arc::ptr_eq(&self.engine, &other.engine) || self.engine.semantic_eq(&other.engine))
    }
}
impl Eq for ParseState {}
impl Hash for ParseState {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.set) as usize).hash(state);
        self.fingerprint.hash(state);
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::{SyntaxDefinition, SyntaxSetBuilder};
    #[test]
    fn frequently_reused_transition_survives_cold_admissions() {
        let mut builder = SyntaxSetBuilder::new();
        builder.add(
            SyntaxDefinition::load_from_str(
                "name: Clock\nscope: source.clock\ncontexts:\n  main: []\n",
                true,
                None,
            )
            .unwrap(),
        );
        let set = Arc::new(builder.build());
        let mut state =
            ParseState::new(set.clone(), &set.syntaxes()[0]).with_line_cache_capacity(2);
        state.parse_line("bootstrap\n").unwrap();
        state.parse_line("hot\n").unwrap();
        state.parse_line("cold-a\n").unwrap();
        state.parse_line("hot\n").unwrap();
        state.parse_line("cold-b\n").unwrap();
        let cache = state.lines.lock().unwrap();
        assert!(
            cache
                .entries
                .values()
                .any(|entry| entry.text.as_ref() == "hot\n")
        );
        assert_eq!(cache.entries.len(), 2);
    }
}
