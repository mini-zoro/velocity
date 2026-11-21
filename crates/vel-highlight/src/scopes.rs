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
/// One decoded, line-local half-open UTF-8 range.
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
/// Adjacent runs cover the line, so only ends are stored. A narrow word
/// holds a 16-bit byte end and a 16-bit scope ID. The wide header preserves
/// the full 32-bit domains when either field doesn't fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeRuns(Arc<[u32]>);
impl ScopeRuns {
    const WIDE: u32 = 1 << 31;
    const SYMBOLS: u32 = 1 << 30;

    pub(crate) fn new(
        runs: &[(u32, ScopeStackId)],
        end_stack: ScopeStackId,
        has_symbols: bool,
    ) -> Self {
        let flags = if has_symbols { Self::SYMBOLS } else { 0 };
        if end_stack.0 < Self::SYMBOLS
            && runs
                .last()
                .is_none_or(|&(end, _)| end <= u32::from(u16::MAX))
            && runs
                .iter()
                .all(|&(_, stack)| stack.0 <= u32::from(u16::MAX))
        {
            Self(Arc::from_iter(std::iter::once(flags | end_stack.0).chain(
                runs.iter().map(|&(end, stack)| (end << 16) | stack.0),
            )))
        } else {
            Self(Arc::from_iter(
                [flags | Self::WIDE, end_stack.0]
                    .into_iter()
                    .chain(runs.iter().flat_map(|&(end, stack)| [end, stack.0])),
            ))
        }
    }

    pub(crate) fn end_stack(&self) -> ScopeStackId {
        ScopeStackId(if self.wide() {
            self.0[1]
        } else {
            self.0[0] & (Self::SYMBOLS - 1)
        })
    }

    pub(crate) fn has_symbols(&self) -> bool {
        self.0[0] & Self::SYMBOLS != 0
    }

    fn wide(&self) -> bool {
        self.0[0] & Self::WIDE != 0
    }

    pub(crate) fn storage(&self) -> &Arc<[u32]> {
        &self.0
    }

    pub(crate) fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = ScopeRun> + ExactSizeIterator + '_ {
        self.view().iter()
    }

    fn view(&self) -> RunView<'_> {
        if self.wide() {
            RunView::Wide(self.0[2..].as_chunks::<2>().0)
        } else {
            RunView::Narrow(&self.0[1..])
        }
    }
}

#[derive(Clone, Copy)]
enum RunView<'a> {
    Narrow(&'a [u32]),
    Wide(&'a [[u32; 2]]),
}
impl<'a> RunView<'a> {
    fn len(self) -> usize {
        match self {
            Self::Narrow(runs) => runs.len(),
            Self::Wide(runs) => runs.len(),
        }
    }
    fn end(self, i: usize) -> u32 {
        match self {
            Self::Narrow(runs) => runs[i] >> 16,
            Self::Wide(runs) => runs[i][0],
        }
    }
    fn run(self, i: usize) -> ScopeRun {
        let (end, stack) = match self {
            Self::Narrow(runs) => (runs[i] >> 16, runs[i] & 0xffff),
            Self::Wide(runs) => (runs[i][0], runs[i][1]),
        };
        ScopeRun {
            start: i.checked_sub(1).map_or(0, |prev| self.end(prev)),
            end,
            stack: ScopeStackId(stack),
        }
    }
    fn iter(self) -> impl DoubleEndedIterator<Item = ScopeRun> + ExactSizeIterator + 'a {
        self.span(0..self.len())
    }
    fn span(
        self,
        range: Range<usize>,
    ) -> impl DoubleEndedIterator<Item = ScopeRun> + ExactSizeIterator + 'a {
        range.map(move |i| self.run(i))
    }
    fn end_partition(self, pred: impl Fn(u32) -> bool) -> usize {
        match self {
            Self::Narrow(runs) => runs.partition_point(|word| pred(word >> 16)),
            Self::Wide(runs) => runs.partition_point(|pair| pred(pair[0])),
        }
    }
}

/// Scope output for one row. The snapshot range determines its row number.
#[derive(Debug, Clone, Default)]
pub struct ScopedLine {
    runs: Option<ScopeRuns>,
}
impl ScopedLine {
    pub(crate) fn new(runs: Option<ScopeRuns>) -> Self {
        Self { runs }
    }
    /// An exact empty line, including the stack after all end-of-line operations.
    pub fn empty(end_stack: ScopeStackId) -> Self {
        Self::new(Some(ScopeRuns::new(&[], end_stack, false)))
    }
    /// Whether parsing has validated this line at the snapshot version.
    pub fn exact(&self) -> bool {
        self.runs.is_some()
    }
    fn run_view(&self) -> RunView<'_> {
        self.runs
            .as_ref()
            .map_or(RunView::Narrow(&[]), ScopeRuns::view)
    }
    /// Shared scope runs (including LF), decoded without a new allocation.
    pub fn runs(&self) -> impl DoubleEndedIterator<Item = ScopeRun> + ExactSizeIterator + '_ {
        self.run_view().iter()
    }
    /// Runs starting before the requested local offset.
    pub fn runs_before(
        &self,
        byte: usize,
    ) -> impl DoubleEndedIterator<Item = ScopeRun> + ExactSizeIterator + '_ {
        let view = self.run_view();
        let stop = (view.end_partition(|end| (end as usize) < byte) + usize::from(byte != 0))
            .min(view.len());
        view.span(0..stop)
    }
    /// Runs ending after the requested local offset.
    pub fn runs_after(
        &self,
        byte: usize,
    ) -> impl DoubleEndedIterator<Item = ScopeRun> + ExactSizeIterator + '_ {
        let view = self.run_view();
        view.span(view.end_partition(|end| end as usize <= byte)..view.len())
    }
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
                inexact += usize::from(!line.exact());
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
    /// Line table bytes, excluding the shared rope, scope runs and interned stacks.
    pub fn line_table_bytes(&self) -> usize {
        self.lines.records.capacity() * size_of::<ScopedLine>()
    }
    /// True when every requested line has exact scopes; O(1) for published snapshots.
    /// Legacy mutable iteration through `lines` requires one scan to refresh its count.
    pub fn complete(&self) -> bool {
        *self
            .lines
            .inexact
            .get_or_init(|| self.lines.iter().filter(|line| !line.exact()).count())
            == 0
    }
    /// Returns scopes without parsing, with UTF-8 and line bounds checked.
    pub fn scope_at(&self, line: usize, byte: usize) -> Option<ScopeStackId> {
        let runs = self
            .lines
            .get(line.checked_sub(self.range.start)?)?
            .runs
            .as_ref()?;
        if line >= self.text.len_lines() {
            return None;
        }
        let len = crate::complete_line_range(&self.text, line).len();
        let point = self.text.line_to_byte(line).checked_add(byte)?;
        if byte > len || !self.text.is_char_boundary(point) {
            return None;
        }
        if byte == len {
            return Some(runs.end_stack());
        }
        let view = runs.view();
        let index = view.end_partition(|end| end as usize <= byte);
        (index < view.len()).then(|| view.run(index).stack)
    }
    /// Maximal equal-stack region within this snapshot's available interval.
    /// Returns None if the region touches an unavailable line; request a larger interval to
    /// extend a result that reaches `range`'s boundary.
    pub fn extract_scope(&self, point: usize) -> Option<Range<usize>> {
        self.scopes_at(point)?;
        let mut current: Option<(ScopeStackId, Range<usize>)> = None;
        for (row, line) in self.range.clone().zip(&self.lines) {
            if !line.exact() {
                continue;
            }
            let offset = self.text.line_to_byte(row);
            for run in line.runs() {
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
    styles: Vec<Option<u32>>,
    palette: Vec<vel_color_scheme::Style>,
    palette_ids: HashMap<vel_color_scheme::Style, u32, fnv::FnvBuildHasher>,
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
    pub(crate) fn estimated_bytes(&self) -> usize {
        self.stacks.0.capacity() * size_of::<Arc<[Scope]>>()
            + self
                .stacks
                .0
                .iter()
                .map(|s| size_of_val(s.as_ref()) + 2 * size_of::<usize>())
                .sum::<usize>()
            + table_bytes(&self.ids)
            + self.selected.capacity() * size_of::<Option<SymbolMetadata>>()
            + self.styles.capacity() * size_of::<Option<u32>>()
            + self.palette.capacity() * size_of::<vel_color_scheme::Style>()
            + table_bytes(&self.palette_ids)
            + table_bytes(&self.transitions)
            + table_bytes(&self.single)
    }
    pub(crate) fn new(metadata: Arc<MetadataSet>) -> Self {
        let mut value = Self {
            ids: HashMap::default(),
            stacks: ScopeStacks::default(),
            metadata,
            selected: Vec::new(),
            styles: Vec::new(),
            palette: Vec::new(),
            palette_ids: HashMap::default(),
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
        self.palette.clear();
        self.palette_ids.clear();
    }
    pub(crate) fn style_id(
        &mut self,
        id: ScopeStackId,
        resolver: &mut vel_color_scheme::Highlighter,
    ) -> u32 {
        if let Some(style) = self.styles[id.0 as usize] {
            return style;
        }
        let style = resolver.resolve_scopes(self.stacks.get(id).unwrap());
        let style_id = self.intern_style(style);
        self.styles[id.0 as usize] = Some(style_id);
        style_id
    }
    pub(crate) fn intern_style(&mut self, style: vel_color_scheme::Style) -> u32 {
        *self.palette_ids.entry(style).or_insert_with(|| {
            let id = self.palette.len().try_into().expect("style ID overflow");
            self.palette.push(style);
            id
        })
    }
    pub(crate) fn style(&self, id: u32) -> vel_color_scheme::Style {
        self.palette[id as usize]
    }
    pub(crate) fn has_symbol(&self, stack: ScopeStackId) -> bool {
        self.selected[stack.0 as usize].is_some()
    }
    pub(crate) fn symbols<'a>(
        &self,
        text: &Text,
        lines: impl Iterator<Item = (usize, &'a ScopeRuns)>,
    ) -> Result<Vec<Symbol>, crate::Error> {
        let mut groups: Vec<(Range<usize>, &SymbolMetadata)> = Vec::new();
        for (row, runs) in lines {
            if runs.view().len() == 0 {
                continue;
            }
            let offset = text.line_to_byte(row);
            for run in runs.iter() {
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

fn table_bytes<K, V, S>(map: &HashMap<K, V, S>) -> usize {
    map.capacity() * (size_of::<(K, V)>() + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_runs_preserve_boundaries_order_and_full_width_stack_ids() {
        for (last_end, id, end_stack, words) in [
            (u16::MAX.into(), ScopeStackId(17), ScopeStackId(3), 3),
            (70_000, ScopeStackId(17), ScopeStackId(3), 6),
            (20, ScopeStackId(u32::MAX), ScopeStackId(3), 6),
            (20, ScopeStackId(17), ScopeStackId(u32::MAX), 6),
        ] {
            let first = ScopeRun {
                start: 0,
                end: 5,
                stack: ScopeStackId(1),
            };
            let second = ScopeRun {
                start: 5,
                end: last_end,
                stack: id,
            };
            let runs = ScopeRuns::new(
                &[(first.end, first.stack), (second.end, second.stack)],
                end_stack,
                true,
            );
            assert_eq!(runs.storage().len(), words);
            assert!(runs.has_symbols());
            assert_eq!(runs.end_stack(), end_stack);
            let line = ScopedLine::new(Some(runs));
            assert!(line.exact());
            assert!(line.runs().eq([first, second]));
            assert!(line.runs_before(0).next().is_none());
            assert!(line.runs_before(5).rev().eq([first]));
            assert!(line.runs_before(6).rev().eq([second, first]));
            assert!(line.runs_after(4).eq([first, second]));
            assert!(line.runs_after(5).eq([second]));
            assert!(line.runs_after(last_end as usize).next().is_none());
        }
        assert_eq!(size_of::<ScopedLine>(), 2 * size_of::<usize>());
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
