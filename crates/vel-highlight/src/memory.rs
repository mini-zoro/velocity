//! Reachable allocations for the offline memory probe; never used during parsing.
use super::*;

/// Counted Rust allocation payloads. Hash tables use an occupied-capacity estimate;
/// allocator headers, shared grammars, thread scratch and theme data are excluded.
#[derive(Debug, Default, Clone, Copy)]
pub struct HighlightMemory {
    /// Per-line metadata, including inline checkpoint handles.
    pub records: usize,
    /// Distinct interpreter states and their reachable context stacks.
    pub parser_states: usize,
    /// Line memo entries, operation-dedup table and cached text (not operations).
    pub parser_memo: usize,
    /// Shared operation sequences, counted once across records and caches.
    pub operations: usize,
    /// Scope interner including metadata selection and transition maps.
    pub interned_scopes: usize,
    /// Shared exact scope runs (not the editor's line table).
    pub scope_runs: usize,
    /// Shared palette-ID runs and cached viewport style arrays.
    pub style_spans: usize,
    /// Render-cache table and visual shape keys.
    pub render_cache: usize,
}

#[derive(Default)]
struct SharedBytes {
    visited: HashSet<usize>,
    bytes: usize,
}
impl SharedBytes {
    fn slice<T>(&mut self, values: &Arc<[T]>) {
        if self.visited.insert(values.as_ptr() as usize) {
            self.bytes += size_of_val(values.as_ref()) + 2 * size_of::<usize>();
        }
    }
    fn operations(&mut self, ops: &SharedScopeOperations) {
        if self.visited.insert(ops.address()) {
            self.bytes += ops.payload_bytes() + 2 * size_of::<usize>();
        }
    }
}

impl DocumentHighlighter {
    /// Counts retained payload bytes in the full-file highlighter.
    pub fn memory_usage(&self) -> HighlightMemory {
        let mut operations = SharedBytes::default();
        let mut scope_runs = SharedBytes::default();
        let mut style_spans = SharedBytes::default();
        let mut visual_shapes = SharedBytes::default();
        let mut checkpoint_handles = SharedBytes::default();
        let (parser_states, parser_memo) = self.initial.measure_document(
            std::iter::once(&self.state).chain(self.records.iter().flatten().filter_map(|r| {
                let checkpoint = r.checkpoint.as_ref()?;
                if checkpoint_handles
                    .visited
                    .insert(Arc::as_ptr(checkpoint) as usize)
                {
                    checkpoint_handles.bytes += size_of::<ParseState>() + 2 * size_of::<usize>();
                }
                Some(checkpoint.as_ref())
            })),
            |ops| operations.operations(ops),
        );
        for record in self.records.iter().flatten() {
            operations.operations(&record.ops);
            scope_runs.slice(&record.runs);
            visual_shapes.slice(&record.spans);
        }
        let cache = &self.render_cache;
        for entry in cache.entries.values() {
            operations.operations(&entry.ops);
            scope_runs.slice(&entry.output.runs);
            visual_shapes.slice(&entry.output.spans);
        }
        let render_cache = cache.entries.capacity() * (size_of::<(RenderKey, CachedRender)>() + 1)
            + cache.visual.shapes.capacity() * (size_of::<Arc<VisualShape>>() + 1)
            + cache.visual.expanded.capacity() * (size_of::<(usize, ExpandedShape)>() + 1);
        for shape in &cache.visual.shapes {
            visual_shapes.slice(shape);
        }
        for ExpandedShape { shape, spans } in cache.visual.expanded.values() {
            visual_shapes.slice(shape);
            style_spans.slice(spans);
        }
        HighlightMemory {
            records: self.records.capacity() * size_of::<Option<LineRecord>>(),
            parser_states: parser_states + checkpoint_handles.bytes,
            parser_memo,
            operations: operations.bytes,
            interned_scopes: self.scope_interner.estimated_bytes(),
            scope_runs: scope_runs.bytes,
            style_spans: style_spans.bytes + visual_shapes.bytes,
            render_cache,
        }
    }
}
