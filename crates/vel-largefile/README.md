# vel-largefile

Engine APIs for progressive huge documents, long horizontal lines, feature policy and
read-only mapped hex rows. No UI, buffer history, encoding preferences or file-save ownership
moves into this crate. See [PERF.md](PERF.md) for paired results and remaining limits.

## Lane A: opening a huge file

```rust
use vel_largefile::{load_utf8, LoadError, LoadOptions};

let task = load_utf8(path, LoadOptions::default())?;
// In the existing dispatcher/idle loop:
if let Some(prefix) = task.try_snapshot() {
    // Install a provisional read-only view of prefix.text, then redraw.
    // prefix.progress counts source bytes; prefix.metrics describes decoded text.
}
match task.result.try_recv() {
    Ok(Ok(loaded)) => { /* install final Text; restore normal transaction ownership */ }
    Ok(Err(LoadError::DecodeRequired)) => { /* discard prefixes; vel_fileio::load fallback */ }
    Ok(Err(LoadError::Binary)) => { /* discard prefixes; offer the hex model below */ }
    Ok(Err(error)) => { /* discard provisional prefixes; surface error/cancellation */ }
    Err(std::sync::mpsc::TryRecvError::Empty) => {}
    Err(error) => { /* worker disconnected; invalidate the provisional view */ }
}
// task.cancel(), or dropping task, requests cancellation. Drop never joins.
```

`LoadTask` has a single coalescing prefix mailbox and an independent terminal result. Poll the
prefix before the terminal result. Polling never waits: a busy mailbox returns None for retry
on the next tick, and retired prefix cleanup happens outside its lock. Associate both with the editor's open/reload generation;
a late result must not replace a newer buffer. Do not allow edits/saves on provisional prefixes.
The first completed 2 MiB is published immediately; subsequent prefixes follow 16 MiB / 100 ms
cadence and EOF. These are construction boundaries, not hard I/O deadlines. A blocked Read call
cannot be interrupted. Cancellation is checked before source reads and prefix publication;
error paths close the pipeline and join its at most four workers. At most 8 MiB of input is in
flight. Snapshots share persistent trees; polling does not queue every intermediate root.

`stream_utf8(reader, total_hint, options, cancellation, callback)` is the same synchronous core
for an existing worker/reader. The reader and callback need not be Send or static. `chunk_bytes`
limits source Read requests (default 2 MiB), not the pipeline's staging capacity. The total is
an initial hint; EOF determines the actual length. `Text::from_reader_snapshots` is the additive
lower-level UTF-8 pipeline. The ordinary `Text::from_reader` path is unchanged.

Encoding contract: sniff the first 8 KiB, recognize UTF-16 BOMs before checking NUL, strip a
UTF-8 BOM, and strictly validate all UTF-8, including seams/truncation. UTF-16 and other encodings
return `DecodeRequired`; lane A must discard all provisional roots and use its full
`vel_fileio::load(path, fallback, progress)` decoder. This also applies to an invalid byte found
late in a file. No replacement characters are inserted. This crate deliberately has no dependency
on vel-fileio (its decoder is private). Raw UTF-8 CR/CRLF/LF bytes are preserved, like vel-text;
line metrics count LF, and CR contributes one body byte. Lane A should keep using vel-fileio
when it needs normalized line endings/complete Format metadata, or expose its streaming decoder
as Read and feed that decoded, normalized stream to `Text::from_reader_snapshots` directly.
That lower-level API performs UTF-8 validation without re-sniffing BOM/NUL; using the raw loader
again could strip a real leading U+FEFF or misclassify a decoded NUL. The decoder adapter owns
original byte progress and Format metadata. The `Loaded::bom` flag applies only to
the raw UTF-8 loader.

Before the complete file is available, evaluate policy using at least the known UTF-8 source
length for `metrics.bytes` and the loaded prefix's line/long-line metrics. Otherwise a 4 GB file
could briefly schedule full indexing while only its first 2 MiB is available.

## Feature settings

`LargeDocumentPolicy::evaluate(DocumentMetrics)` is pure. Limits are inclusive. These are the
integration setting names/defaults; lane A maps its effective settings into the public fields
(the current vel-config/UI is not modified here). Explicit user wrapping overrides stay in UI.

| Setting | Default | Result at/above threshold |
|---|---:|---|
| `large_file.viewport_bytes` | 67,108,864 | viewport highlighting; indexing/wrap/spelling/disk diff off |
| `large_file.viewport_lines` | 1,000,000 | same |
| `large_file.long_line_bytes` | 16,384 | same, even in a small file |
| `large_file.indexing_bytes` | 67,108,864 | indexing off |
| `large_file.wrap_bytes` | 16,777,216 | wrapping off by default |
| `large_file.spell_bytes` | 16,777,216 | spelling off |
| `large_file.diff_bytes` | 67,108,864 | disk diff off |
| `large_file.minimap_lines` | 10,000 | sample every ceil(lines / limit) lines; zero permits at most one sample |

Viewport parser settings are `ViewportOptions` in vel-highlight: `max_line_bytes=16384`
(complete bytes including LF), `max_viewport_lines=512`, `max_lookback_lines=256`,
`checkpoint_interval=1024`, `max_checkpoints=128`. These limits describe explicit degradation,
not approximate results silently presented as exact. See its README for worker repair APIs.

## Lane A: long horizontal lines

Build `LongLineIndex::new(buffer.text().clone(), row, tab_size)` on the analysis worker. Retain
its matching buffer ChangeId at the integration boundary; an index from another version is
unavailable, not reusable. Until it arrives, lay out just a bounded prefix around the current
caret; do not flatten the entire line for layout. The index keeps ~4 KiB grapheme-aligned blocks,
compact prefix sums and word-class runs; no per-character table or duplicate full body is stored.

```rust
let window = index.visible_window(first_cell, 200); // line-local UTF-8 range
let absolute = index.line_start() + window.start..index.line_start() + window.end;
let visible_chunks = index.text().slice(absolute);
// Feed only these chunks into the existing renderer/shaper.
let cell = index.byte_to_column(caret_byte_in_line);
let next = index.next_word_boundary(caret_byte_in_line);
let end = index.len_bytes();
```

Use `byte_to_grapheme` / `grapheme_to_byte` and `next_grapheme_boundary` /
`prev_grapheme_boundary` for extended Unicode clusters. Columns are terminal cells from
unicode-width 0.2 (ambiguous width 1), with configurable tab stops; font shaping/bidi remains the
renderer's responsibility. Word motion uses Unicode alphanumeric/underscore, whitespace and
punctuation runs, not language-specific token boundaries. Zero-width clusters and wide glyphs
are handled; a horizontal window includes whole edge clusters. EOF and zero-width requests clip.

After one ordinary line-local Buffer transaction, call
`index.update_after_edit(buffer.text().clone(), old_local_range, inserted_utf8_bytes)` and retain
the new ChangeId. This adopts the already committed snapshot; it does not edit Buffer again.
The snapshot must result from exactly that replacement. For an index-owned fixture,
`index.replace(range, utf8)` edits both text and metrics. Newline changes, edits before the indexed
line, multi-range edits and tab-size changes require rebuilding the affected index on a worker.
Queries are O(log blocks) plus one bounded block; giant clusters are isolated and use constant size
metadata for queries. Ordinary edits resegment nearby blocks and update O(blocks) prefix metadata.
An edit that changes unbounded Unicode context (e.g. a huge RI run/combining cluster) can require
linear resegmentation; that case has no interactive worst-case guarantee. Construction scans the line on a worker; repeated Unicode context queries can add work.
Setup is excluded from typing/query budgets.

## Lane A: binaries

`detect_content(prefix)` is a NUL/BOM/UTF-8 heuristic, matching vel-fileio's first-8-KiB NUL rule.
Non-UTF-8 without NUL routes to full decode rather than being assumed binary. UTF-16 BOMs take
precedence. Binary classification is not a MIME decision and does not scan the entire file.

`HexView::open(path)` is **unsafe** because memmap2 requires externally stable contents/length
for the mapping's full lifetime. The integration must establish that invariant; a watcher alone
cannot make arbitrary external truncation safe. Release the map before permitted writes/reloads.
An immutable application-owned file is suitable. The model exposes only reads and has no edit
API. `len_rows()` and `row(u64)` use 16-byte rows; each row has an absolute offset, borrowed bytes
and a fixed ASCII column (nonprintable -> '.', unused cells -> space). Empty files have no map;
partial EOF rows and out-of-range/u64::MAX queries are checked. Only requested mapped pages are
touched; tests read the tail of a sparse 4,000,000,003-byte fixture without copying its body.

## Validation and reproduction

Run workspace fmt, clippy with `-D warnings`, release tests, and rustdoc with `-D warnings`.
The new tests compare Unicode edits/columns against a flat grapheme/width model, exercise short
reads, UTF-8 errors after published prefixes, cancellation, coalescing, binary routing and mapped
rows. vel-highlight compares corrected sparse output against the dense parser, including branch
rewinds, skipped long lines, edits, palette changes and evicted checkpoints. Full paired commands,
raw samples, confidence intervals and budgets are in [PERF.md](PERF.md).
