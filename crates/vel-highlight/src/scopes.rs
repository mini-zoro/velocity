//! Compact scope runs and immutable editor query snapshots.
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    ops::Range,
    sync::{Arc, OnceLock},
};
use vel_buffer::ChangeId;
use vel_scope::{MatchScore, Scope, ScopeStack, ScopeStackOp, Selector};
use vel_syntax::{MetadataSet, Symbol, SymbolMetadata};
use vel_text::Text;

/// Document-local interned active scope stack. IDs remain valid for the document's lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ScopeStackId(pub u32);
/// One line-local half-open UTF-8 range, exactly 12 bytes on all supported targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct ScopeRun {
    /// Inclusive byte offset within the line.
    pub start: u32,
    /// Exclusive byte offset within the line.
    pub end: u32,
    /// Interned active scope stack.
    pub stack: ScopeStackId,
}
impl ScopeRun {
    /// Converts compact offsets to an ordinary range.
    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}
/// Immutable, shared stack table. Palette changes do not invalidate IDs.
#[derive(Debug, Clone, Default)]
pub struct ScopeStacks(pub(crate) Arc<Vec<Arc<[Scope]>>>);
impl ScopeStacks {
    /// Resolves an ID from this document, never a different buffer's ID.
    pub fn get(&self, id: ScopeStackId) -> Option<&[Scope]> {
        self.0.get(id.0 as usize).map(AsRef::as_ref)
    }
}
/// Scope output for one line; unavailable lines have no runs and `exact == false`.
#[derive(Debug, Clone)]
pub struct ScopedLine {
    /// Zero-based document line.
    pub line: usize,
    /// Shared compact scope runs (including LF).
    pub runs: Arc<[ScopeRun]>,
    /// Stack at the end of the line, after all end-of-line operations.
    pub end_stack: ScopeStackId,
    /// Whether parsing has validated this line at the snapshot version.
    pub exact: bool,
}
/// Line records with an incrementally accumulated completeness count.
///
/// Collect an iterator of [`ScopedLine`]s to construct this collection. Records
/// use a cached count for immutable published snapshots. Legacy mutable iteration
/// invalidates that count; its next completeness query rebuilds it once. Slice
/// indexing and iteration remain available through [`std::ops::Deref`].
#[derive(Debug, Clone, Default)]
pub struct ScopeLines {
    records: Vec<ScopedLine>,
    inexact: OnceLock<usize>,
}
impl FromIterator<ScopedLine> for ScopeLines {
    fn from_iter<T: IntoIterator<Item = ScopedLine>>(iter: T) -> Self {
        let mut inexact = 0;
        // Preserve Vec's exact-size/trusted-length collection specialization;
        // counting must not add a capacity check to every collected line.
        let records = iter
            .into_iter()
            .inspect(|line| {
                inexact += usize::from(!line.exact);
            })
            .collect();
        Self {
            records,
            inexact: OnceLock::from(inexact),
        }
    }
}
impl ScopeLines {
    /// Mutable iteration for legacy snapshot consumers. Invalidates completeness;
    /// the first query after mutation scans once, then caches its result again.
    /// Prefer collecting new records so their count is accumulated during construction.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, ScopedLine> {
        self.inexact.take();
        self.records.iter_mut()
    }
}
impl std::ops::Deref for ScopeLines {
    type Target = [ScopedLine];
    fn deref(&self) -> &Self::Target {
        &self.records
    }
}
impl<'a> IntoIterator for &'a ScopeLines {
    type Item = &'a ScopedLine;
    type IntoIter = std::slice::Iter<'a, ScopedLine>;
    fn into_iter(self) -> Self::IntoIter {
        self.records.iter()
    }
}
impl<'a> IntoIterator for &'a mut ScopeLines {
    type Item = &'a mut ScopedLine;
    type IntoIter = std::slice::IterMut<'a, ScopedLine>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
/// Read-only scope input shared by editor features and the completion engine.
pub trait ScopeProvider: Send + Sync {
    /// Buffer transaction identity.
    fn version(&self) -> ChangeId;
    /// Active scopes at an absolute UTF-8 byte point, or None while unavailable.
    fn scopes_at(&self, point: usize) -> Option<&[Scope]>;
}
/// Versioned scope output for a requested line interval. Cloning text/stacks/runs shares storage.
#[derive(Debug, Clone)]
pub struct ScopeSnapshot {
    /// Buffer transaction identity.
    pub version: ChangeId,
    /// O(1) immutable rope snapshot used to interpret absolute points.
    pub text: Text,
    /// Half-open line interval represented by `lines`.
    pub range: Range<usize>,
    /// One scope record per requested line.
    pub lines: ScopeLines,
    /// Table resolving the document-local stack IDs.
    pub stacks: ScopeStacks,
}
impl ScopeSnapshot {
    /// True when every requested line has exact scopes; O(1) for published snapshots.
    /// Legacy mutable iteration through `lines` requires one scan to refresh its count.
    pub fn complete(&self) -> bool {
        *self
            .lines
            .inexact
            .get_or_init(|| self.lines.iter().filter(|line| !line.exact).count())
            == 0
    }
    /// Returns scopes without parsing, with UTF-8 and line bounds checked.
    pub fn scope_at(&self, line: usize, byte: usize) -> Option<ScopeStackId> {
        let record = self.lines.get(line.checked_sub(self.range.start)?)?;
        if !record.exact || line >= self.text.len_lines() {
            return None;
        }
        let len = crate::complete_line_range(&self.text, line).len();
        let point = self.text.line_to_byte(line).checked_add(byte)?;
        if byte > len || !self.text.is_char_boundary(point) {
            return None;
        }
        if byte == len {
            return Some(record.end_stack);
        }
        let index = record.runs.partition_point(|run| run.end as usize <= byte);
        record
            .runs
            .get(index)
            .filter(|run| run.start as usize <= byte)
            .map(|run| run.stack)
    }
    /// Maximal equal-stack region within this snapshot's available interval.
    /// Returns None if the region touches an unavailable line; request a larger interval to
    /// extend a result that reaches `range`'s boundary.
    pub fn extract_scope(&self, point: usize) -> Option<Range<usize>> {
        self.scopes_at(point)?;
        let mut current: Option<(ScopeStackId, Range<usize>)> = None;
        for line in &self.lines {
            if !line.exact {
                continue;
            }
            let offset = self.text.line_to_byte(line.line);
            for run in line.runs.iter() {
                let range = offset + run.start as usize..offset + run.end as usize;
                if let Some((id, region)) = &mut current {
                    if *id == run.stack && region.end == range.start {
                        region.end = range.end;
                        continue;
                    }
                    if region.contains(&point) {
                        return Some(region.clone());
                    }
                }
                current = Some((run.stack, range));
            }
        }
        if let Some((_, range)) = current
            && range.contains(&point)
        {
            return Some(range);
        }
        Some(point..point)
    }
    /// Selector specificity at an absolute byte point; no parse or selector recompilation.
    pub fn match_selector(&self, point: usize, selector: &Selector) -> Option<MatchScore> {
        selector.does_match(self.scopes_at(point)?)
    }
}
impl ScopeProvider for ScopeSnapshot {
    fn version(&self) -> ChangeId {
        self.version
    }
    fn scopes_at(&self, point: usize) -> Option<&[Scope]> {
        if point > self.text.len_bytes() || !self.text.is_char_boundary(point) {
            return None;
        }
        let row = self.text.byte_to_line(point);
        let id = self.scope_at(row, point - self.text.line_to_byte(row))?;
        self.stacks.get(id)
    }
}
#[derive(Debug)]
pub(crate) struct ScopeInterner {
    ids: HashMap<Arc<[Scope]>, ScopeStackId, fnv::FnvBuildHasher>,
    pub(crate) stacks: ScopeStacks,
    pub(crate) metadata: Arc<MetadataSet>,
    selected: Vec<Option<SymbolMetadata>>,
    styles: Vec<Option<vel_color_scheme::Style>>,
    transitions: HashMap<u64, ScopeTransition, fnv::FnvBuildHasher>,
    single: HashMap<(ScopeStackId, ScopeStackOp), ScopeStackId, fnv::FnvBuildHasher>,
}
#[derive(Debug)]
struct ScopeTransition {
    before: ScopeStackId,
    ops: Box<[ScopeStackOp]>,
    after: ScopeStackId,
}
impl ScopeInterner {
    pub(crate) fn new(metadata: Arc<MetadataSet>) -> Self {
        let mut value = Self {
            ids: HashMap::default(),
            stacks: ScopeStacks::default(),
            metadata,
            selected: Vec::new(),
            styles: Vec::new(),
            transitions: HashMap::default(),
            single: HashMap::default(),
        };
        value.intern(&ScopeStack::new());
        value
    }
    pub(crate) fn intern(&mut self, stack: &ScopeStack) -> ScopeStackId {
        if let Some(id) = self.ids.get(stack.as_slice()) {
            return *id;
        }
        let id = ScopeStackId(
            self.stacks
                .0
                .len()
                .try_into()
                .expect("scope stack ID overflow"),
        );
        self.selected
            .push(self.metadata.symbol_metadata(stack.as_slice()));
        self.styles.push(None);
        let scopes: Arc<[Scope]> = Arc::from(stack.as_slice());
        Arc::make_mut(&mut self.stacks.0).push(Arc::clone(&scopes));
        self.ids.insert(scopes, id);
        id
    }
    pub(crate) fn apply_group(
        &mut self,
        id: ScopeStackId,
        stack: &mut ScopeStack,
        ops: &[(usize, ScopeStackOp)],
    ) -> Result<ScopeStackId, vel_scope::ScopeError> {
        const MAX_TRANSITIONS: usize = 65_536;
        if let [(_, op)] = ops {
            match *op {
                ScopeStackOp::Noop | ScopeStackOp::Pop(0) => return Ok(id),
                ScopeStackOp::Push(scope) => stack.push(scope),
                ScopeStackOp::Pop(n) => {
                    let keep = stack.len().saturating_sub(n);
                    stack.scopes.truncate(keep);
                    if keep == 0 {
                        return Ok(ScopeStackId(0));
                    }
                }
                _ => {
                    stack.apply(op)?;
                    return Ok(self.intern(stack));
                }
            }
            let key = (id, *op);
            if let Some(&next) = self.single.get(&key) {
                return Ok(next);
            }
            let next = self.intern(stack);
            if self.single.len() + self.transitions.len() < MAX_TRANSITIONS {
                self.single.insert(key, next);
            }
            return Ok(next);
        }
        let mut hash = fnv::FnvHasher::default();
        id.hash(&mut hash);
        let mut restore = false;
        for (_, op) in ops {
            stack.apply(op)?;
            op.hash(&mut hash);
            restore |= *op == ScopeStackOp::Restore;
        }
        // Saved clear stacks are intentionally not encoded by ScopeStackId.
        // Restore depends on that hidden history; every other operation's
        // resulting active scopes depend only on (active ID, operation).
        if restore {
            return Ok(self.intern(stack));
        }
        let key = hash.finish();
        if let Some(next) = self.transitions.get(&key)
            && next.before == id
            && next.ops.len() == ops.len()
            && next.ops.iter().zip(ops).all(|(a, (_, b))| a == b)
        {
            return Ok(next.after);
        }
        let next = self.intern(stack);
        if self.single.len() + self.transitions.len() < MAX_TRANSITIONS {
            self.transitions.insert(
                key,
                ScopeTransition {
                    before: id,
                    ops: ops.iter().map(|(_, op)| *op).collect(),
                    after: next,
                },
            );
        }
        Ok(next)
    }
    pub(crate) fn invalidate_styles(&mut self) {
        self.styles.fill(None);
    }
    pub(crate) fn style(
        &mut self,
        id: ScopeStackId,
        stack: &ScopeStack,
        resolver: &mut vel_color_scheme::Highlighter,
    ) -> vel_color_scheme::Style {
        *self.styles[id.0 as usize].get_or_insert_with(|| resolver.style_for(stack))
    }
    pub(crate) fn has_symbols(&self, runs: &[ScopeRun]) -> bool {
        runs.iter()
            .any(|run| self.selected[run.stack.0 as usize].is_some())
    }
    pub(crate) fn symbols<'a>(
        &self,
        text: &Text,
        lines: impl Iterator<Item = (usize, &'a [ScopeRun])>,
    ) -> Result<Vec<Symbol>, crate::Error> {
        let mut groups: Vec<(Range<usize>, &SymbolMetadata)> = Vec::new();
        for (row, runs) in lines {
            if runs.is_empty() {
                continue;
            }
            let offset = text.line_to_byte(row);
            for run in runs {
                let Some(m) = self.selected[run.stack.0 as usize].as_ref() else {
                    continue;
                };
                let range = offset + run.start as usize..offset + run.end as usize;
                if let Some((old, metadata)) = groups.last_mut()
                    && old.end == range.start
                    && *metadata == m
                {
                    old.end = range.end;
                } else {
                    groups.push((range, m));
                }
            }
        }
        groups
            .into_iter()
            .map(|(range, m)| {
                let name = text.slice(range.clone()).to_string();
                Ok(Symbol {
                    range,
                    name: self
                        .metadata
                        .transform_symbol(m, false, &name)
                        .map_err(|e| crate::Error::Parse(e.to_string()))?,
                    index_name: self
                        .metadata
                        .transform_symbol(m, true, &name)
                        .map_err(|e| crate::Error::Parse(e.to_string()))?,
                    local: m.local,
                    indexed: m.indexed,
                    reference: m.reference,
                })
            })
            .collect()
    }
}
/// Per-file symbols, assembled from incrementally maintained line candidates without parsing.
#[derive(Debug, Clone)]
pub struct SymbolList {
    /// Buffer transaction identity.
    pub version: ChangeId,
    /// False while a suffix still awaits parsing.
    pub complete: bool,
    /// All metadata-selected symbols; use `local` for the document's Goto Symbol list.
    pub symbols: Vec<Symbol>,
}
