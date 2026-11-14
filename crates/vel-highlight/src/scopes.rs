//! Compact scope runs and immutable editor query snapshots.
use std::{collections::HashMap, ops::Range, sync::Arc};
use vel_buffer::ChangeId;
use vel_scope::{MatchScore, Scope, ScopeStack, Selector};
use vel_syntax::{Metadata, MetadataSet, Symbol};
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
    pub lines: Vec<ScopedLine>,
    /// Table resolving the document-local stack IDs.
    pub stacks: ScopeStacks,
}
impl ScopeSnapshot {
    /// True when every requested line has exact scopes.
    pub fn complete(&self) -> bool {
        self.lines.iter().all(|line| line.exact)
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
    ids: HashMap<Vec<Scope>, ScopeStackId, fnv::FnvBuildHasher>,
    pub(crate) stacks: ScopeStacks,
    pub(crate) metadata: Arc<MetadataSet>,
    selected: Vec<Option<Arc<Metadata>>>,
    styles: Vec<Option<vel_color_scheme::Style>>,
}
impl ScopeInterner {
    pub(crate) fn new(metadata: Arc<MetadataSet>) -> Self {
        let mut value = Self {
            ids: HashMap::default(),
            stacks: ScopeStacks::default(),
            metadata,
            selected: Vec::new(),
            styles: Vec::new(),
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
        let metadata = Arc::new(self.metadata.metadata_for(stack));
        let selected = [
            "showInSymbolList",
            "showInIndexedSymbolList",
            "showInIndexedReferenceList",
        ]
        .iter()
        .any(|key| metadata.boolean(key).unwrap_or(false))
        .then_some(metadata);
        self.selected.push(selected);
        self.styles.push(None);
        Arc::make_mut(&mut self.stacks.0).push(Arc::from(stack.as_slice()));
        self.ids.insert(stack.as_slice().to_vec(), id);
        id
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
        let mut groups: Vec<(Range<usize>, &Metadata)> = Vec::new();
        for (row, runs) in lines {
            if runs.is_empty() {
                continue;
            }
            let offset = text.line_to_byte(row);
            for run in runs {
                let Some(m) = self.selected[run.stack.0 as usize].as_deref() else {
                    continue;
                };
                let range = offset + run.start as usize..offset + run.end as usize;
                let flags = |m: &Metadata| {
                    [
                        "showInSymbolList",
                        "showInIndexedSymbolList",
                        "showInIndexedReferenceList",
                    ]
                    .map(|key| m.boolean(key).unwrap_or(false))
                };
                if let Some((old, metadata)) = groups.last_mut()
                    && old.end == range.start
                    && flags(metadata) == flags(m)
                    && metadata.string("symbolTransformation") == m.string("symbolTransformation")
                    && metadata.string("symbolIndexTransformation")
                        == m.string("symbolIndexTransformation")
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
                        .transform(m, false, &name)
                        .map_err(|e| crate::Error::Parse(e.to_string()))?,
                    index_name: self
                        .metadata
                        .transform(m, true, &name)
                        .map_err(|e| crate::Error::Parse(e.to_string()))?,
                    local: m.boolean("showInSymbolList").unwrap_or(false),
                    indexed: m.boolean("showInIndexedSymbolList").unwrap_or(false),
                    reference: m.boolean("showInIndexedReferenceList").unwrap_or(false),
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
