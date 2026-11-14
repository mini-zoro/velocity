//! Editor-facing snapshots; context stacks are shared and hash-consed by the interpreter.
use crate::{
    ParsingError, SyntaxReference, SyntaxSet,
    parser::{InterpreterState, ScopeOperations},
};
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};
/// Earliest previously emitted line whose highlighting changed. Lines are zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rewind {
    /// Rebuild scope stacks and spans starting here.
    pub from_line: usize,
}
/// Byte-indexed scope operations and any corrections from branch backtracking.
#[derive(Debug, Clone, Default)]
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
        }
    }
    /// Parses a contiguous UTF-8 line, preserving all speculative branch state.
    pub fn parse_line(&mut self, line: &str) -> Result<LineOps, ParsingError> {
        let engine = Arc::make_mut(&mut self.engine);
        let output = engine.parse_line(line, &self.set);
        self.fingerprint = engine.fingerprint();
        let output = output?;
        let corrections: Vec<_> = output
            .replayed
            .into_iter()
            .map(|(n, ops)| (n - 1, ops))
            .collect();
        Ok(LineOps {
            rewind: corrections
                .iter()
                .map(|(n, _)| *n)
                .min()
                .map(|from_line| Rewind { from_line }),
            ops: output.ops,
            corrections,
            warnings: output.warnings,
        })
    }
    /// Whether a later `fail` may correct previously emitted operations.
    pub fn is_speculative(&self) -> bool {
        self.engine.is_speculative()
    }

    /// Relocates a stable checkpoint after inserted/deleted lines. Returns false if an
    /// active branch's absolute replay history would need rebasing; reparse that region.
    pub fn resume_at(&mut self, next_line: usize) -> bool {
        Arc::make_mut(&mut self.engine).relocate(next_line)
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
