//! Scope/metadata-driven project symbols, persistent background indexing and completion statistics.
#![doc = include_str!("../README.md")]
mod data;
mod names;
mod sequences;
mod service;
mod slab;
mod store;
pub use data::{
    FileAnalysis, Index, IndexedSymbol, SymbolKind, SymbolLocation, SymbolMatch, WordStatistics,
    WordSuggestion,
};
pub use names::fuzzy_score;
pub use service::{IndexOptions, IndexStatus, ProjectIndex};
mod temporary;
use std::{path::Path, sync::Arc};
pub use temporary::sweep_stale;
use vel_syntax::{MetadataSet, SyntaxSet};
/// Indexing, cache, resource or syntax error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// File, worker or persistent-store I/O error.
    #[error("index I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid exclusion pattern or parse/metadata error.
    #[error("index analysis: {0}")]
    Analysis(String),
}
/// Extracts corrected definitions/references and context-aware statistics from UTF-8 source.
/// Unknown syntaxes yield words only. Disk size/binary filtering is the service's responsibility.
pub fn analyze(
    path: &Path,
    source: &str,
    syntaxes: &Arc<SyntaxSet>,
    metadata: &MetadataSet,
) -> Result<FileAnalysis, Error> {
    let mut result = FileAnalysis {
        words: WordStatistics::from_text(source),
        ..FileAnalysis::default()
    };
    let Some(syntax) = vel_highlight::detect_syntax(
        syntaxes,
        &path.to_string_lossy(),
        source.lines().next().unwrap_or(""),
        None,
    ) else {
        return Ok(result);
    };
    let spans =
        vel_syntax::indent::scoped_spans(syntaxes, syntax, source).map_err(Error::Analysis)?;
    let symbols = metadata
        .symbols(source, &spans)
        .map_err(|e| Error::Analysis(e.to_string()))?;
    let mut line_starts = vec![0];
    line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
    for symbol in symbols {
        if !symbol.indexed && !symbol.reference {
            continue;
        }
        let row = line_starts.partition_point(|&at| at <= symbol.range.start) - 1;
        let span = spans.get(spans.partition_point(|span| span.range.end <= symbol.range.start));
        let n = |n: usize| {
            u32::try_from(n).map_err(|_| Error::Analysis("file coordinates exceed 4 GiB".into()))
        };
        result.symbols.push(IndexedSymbol {
            name: symbol.index_name,
            row: n(row)?,
            column: n(symbol.range.start - line_starts[row])?,
            range: n(symbol.range.start)?..n(symbol.range.end)?,
            kind: span.map_or(SymbolKind::Other, |s| SymbolKind::from_scopes(&s.scopes)),
            definition: symbol.indexed,
            reference: symbol.reference,
        });
    }
    Ok(result)
}

/// Builds index definitions/references and word statistics from a complete, matching highlighter
/// snapshot and SymbolList. No syntax parser runs here. Call on an analysis worker, then submit
/// with `ProjectIndex::update_buffer_analysis` so disk events cannot overwrite unsaved results.
pub fn analyze_scopes(
    scopes: &vel_highlight::ScopeSnapshot,
    symbols: &vel_highlight::SymbolList,
) -> Result<FileAnalysis, Error> {
    use vel_highlight::ScopeProvider;
    if scopes.version != symbols.version
        || !scopes.complete()
        || !symbols.complete
        || scopes.range != (0..scopes.text.len_lines())
    {
        return Err(Error::Analysis(
            "complete matching scope/symbol snapshots required".into(),
        ));
    }
    let mut result = FileAnalysis {
        words: WordStatistics::from_text(&scopes.text.to_string()),
        ..FileAnalysis::default()
    };
    for symbol in &symbols.symbols {
        if !symbol.indexed && !symbol.reference {
            continue;
        }
        if symbol.range.start > symbol.range.end
            || symbol.range.end > scopes.text.len_bytes()
            || !scopes.text.is_char_boundary(symbol.range.start)
            || !scopes.text.is_char_boundary(symbol.range.end)
        {
            return Err(Error::Analysis("invalid shared symbol range".into()));
        }
        let point = scopes.text.byte_to_point(symbol.range.start);
        let n = |n: usize| {
            u32::try_from(n).map_err(|_| Error::Analysis("file coordinates exceed 4 GiB".into()))
        };
        result.symbols.push(IndexedSymbol {
            name: symbol.index_name.clone(),
            row: n(point.row)?,
            column: n(point.column)?,
            range: n(symbol.range.start)?..n(symbol.range.end)?,
            kind: SymbolKind::from_scopes(scopes.scopes_at(symbol.range.start).unwrap_or(&[])),
            definition: symbol.indexed,
            reference: symbol.reference,
        });
    }
    Ok(result)
}
