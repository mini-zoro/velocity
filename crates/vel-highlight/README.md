# vel-highlight

Versioned, incremental code highlighting over vel-text snapshots and vel-buffer ChangeSets.
The service returns line-local UTF-8 byte ranges and resolved colors/decorations, including
hashed token colors. A shared Arc<SyntaxSet> and immutable Arc<ColorScheme> can serve multiple
buffers. The editor integration belongs to lane A; this crate does not change the application.

## Editor integration

Load one SyntaxSet at startup (prefer `SyntaxSet::load_cached`), load/merge a ColorScheme with
vel-color-scheme, then create one service per buffer and share it among that buffer's views:

```rust
use std::sync::Arc;
use vel_buffer::Buffer;
use vel_color_scheme::ColorScheme;
use vel_highlight::{detect_syntax, DocumentHighlighter, HighlightService};
use vel_syntax::SyntaxSet;

# fn attach(buffer: &Buffer, set: Arc<SyntaxSet>) -> Result<HighlightService, Box<dyn std::error::Error>> {
let first_line = buffer.text().line(0).to_string();
let syntax = detect_syntax(&set, "untitled.rs", &first_line, None)
    .unwrap_or_else(|| set.find_syntax_plain_text());
let engine = DocumentHighlighter::new(
    buffer.text().clone(), buffer.change_id(), set.clone(), syntax,
    Arc::new(ColorScheme::velocity_dark()),
);
let service = HighlightService::new(engine)?;
# Ok(service)
# }
```

For each committed transaction, preserve the same old-coordinate batch passed to
`Buffer::transact`, and construct its ChangeSet from the Edit descriptors:

```rust
# use vel_buffer::{Buffer, Change, ChangeSet};
# use vel_highlight::HighlightService;
# use vel_text::Edit;
# fn after_transaction(buffer: &Buffer, service: &mut HighlightService, edits: &[Edit<'_>]) -> Result<(), vel_highlight::Error> {
let changes = ChangeSet::new(edits.iter().map(|edit| Change {
    old_range: edit.range.clone(), new_len: edit.text.len(),
}).collect());
service.apply_change(buffer.text().clone(), buffer.change_id(), changes)?;
# Ok(())
# }
```

A ChangeSet carries lengths, not replacement text: always supply the post-transaction Text
snapshot too. Submit every transaction in order, including each undo/redo step. The current
Buffer does not expose an undo-step ChangeSet observer; lane A must surface that descriptor at
its transaction boundary or recreate the service from the current snapshot after undo/redo.
Do not infer an incremental change from selection movement or reuse a map from another buffer.
When syntax selection changes or transaction history is unavailable, create a fresh engine.

On layout/scroll/redraw, call `service.request_lines(buffer.change_id(), first..last)` with a
half-open visible line range. It tries at most 2 ms of synchronous catch-up using try_lock;
when the worker owns the state, it immediately returns default-colored, current-text spans
with `StyledLine::exact == false`. Render these and poll `service.try_recv()` from the idle or
redraw loop; a complete worker batch should invalidate/redraw the returned interval. Later branch rewinds
republish a corrected visible batch automatically, even after the first viewport batch completed. Polling
filters stale buffer versions and scheme generations. The immediate batch is also tagged with
both. Do not apply results to another buffer or version. Ranges include LF; strip the LF when
feeding a renderer that lays out line content separately. An empty last line has no spans.

`set_color_scheme` recolors the validated prefix from saved operations and schedules remaining
work; old-generation batches are discarded. Resolve `selected_scheme(settings, os_is_dark)`
and merged package/User resources outside typing/layout, then submit the new Arc<ColorScheme>.
`detect_syntax` supports explicit resource path/name/scope/token overrides, whole filenames,
compound and hidden extensions, then first_line_match; it does no file I/O. Plain text fallback
is the caller's policy. Returning None for an invalid explicit override is intentional.

## Checkpoints, scheduling, and limits

The v1 service uses dense checkpoints (N=1), including every visible line. ParseState is shared
and copy-on-write; repeated operation arrays and scope stacks are interned in bounded caches
(4096 entries each). A reusable String collects only the current rope line, and parse_line_into retains LineOps capacity. Stable line transitions use the parser's bounded document cache; speculative corrections still replay normally. Newline changes
splice line records; an ordinary character replacement does not scan the document tail.
Multiple pending edits retain the furthest invalidated region, preventing premature convergence.
After the final changed line, equal stable parser state plus equal full scope stack validates
the saved suffix. Stable checkpoints relocate their absolute line counter after line insertions;
active speculative checkpoints are reparsed. Branch corrections replace prior operation lists,
rebuild scopes/styles from the rewind anchor, and invalidate obsolete speculative checkpoints.

The worker prioritizes a pending viewport over the rest of the file, runs background work in
1 ms slices and yields between slices. This is cooperative low-priority scheduling, not a
platform QoS guarantee. Deadlines are checked between lines: a single pathological line or
Oniguruma call can exceed 2 ms; the worker's regex resource limits still apply. UI requests use
try_lock and never wait for such worker work. Dropping the service cancels without joining a
potentially slow regex; the detached thread exits after its current call. Dense checkpoint
memory is proportional to line count; a sparse checkpoint policy is a future optimization.
A scheme swap while an edit is pending reparses the unvalidated suffix, so convergence cannot
reuse spans from an old palette.

`DocumentHighlighter` exposes the same core for editor-owned scheduling/tests: `apply_change`,
`request_lines(version, range, budget)`, `advance`, `finish`, and convergence/rewind statistics.
`HighlightService::progress` observes current-version completion without waiting for its lock.
Use `finish` only in background jobs; it deliberately has no foreground deadline.

## Validation and performance

Round 10 shares rendered line shapes by the full starting scope stack (including
saved clears), exact operation list and byte length. The cache retains at most
8,192 shapes and 4 MiB of operation/span/run arrays; entry/stack metadata is
separately bounded by the entry count. Hash collisions are checked against the
complete key. UTF-8 operation boundaries remain validated on reuse. Hashed token
colors bypass shape sharing, and palette/metadata changes discard its entries.
Push/pop/clear sequences at one byte also reuse document-local IDs, up to
65,536 transitions, without interning invisible intermediate stacks. Groups
containing Restore always consult their actual saved scope history.

The optional `profile` feature adds component timers for rope input, parser,
rendering (including corrected lines) and checkpoints, plus render reuse counts.
It is compiled out of production and throughput builds. The vel-syntax
`worker_probe` example forwards this feature and adds per-context regex timers
and Rust allocation counts. Those diagnostic times include timer/atomic overhead;
allocation totals omit C allocations. Paired production results and reproduction
commands are in [../vel-regex/PERF.md](../vel-regex/PERF.md).

Tests compare incremental output with a fresh parse over 100 deterministic Unicode/newline/
comment edits, multiple queued edits, multi-range edits, branch rewinds after line insertion,
scheme changes during pending edits, stale results, zero-budget fallback, and syntax detection.
The official syntax corpus and metadata/reindent evidence remain in vel-syntax/RESULTS.md.

Measurements use release on arm64 macOS, five runs, the actual 10 MB Rust fixture, no sysctl. The default matcher is now MultiDfa, and stable parser transitions are memoized. Backend-isolated comparisons and compile/memory evidence are in ../vel-regex/PERF.md.
`timings.json` contains 500 core edit samples, another 500 edits through the real worker
service, and five runs each of core and actual background full-file highlighting. The edit case starts
with a fully highlighted 10k-line snapshot, changes one ASCII letter on line 5000, then
requests lines 4950..5050. Timing includes applying the ChangeSet and constructing the batch;
Text replacement happens before the timer. All 500 edits converged after one parsed line.
This stable edit case is not a worst-case multiline syntax change.

The final medians/p95s are recorded in RESULTS.md. `timings-before-interning.json` preserves the
five-run comparison preceding operation/scope interning. Style-cache-only throughput and its
allocation check live in vel-color-scheme/timings.jsonl; parser-only comparisons live in
vel-syntax/tests/timings.json.

```sh
cargo fmt -p vel-highlight --check
cargo clippy -p vel-highlight --all-targets -- -D warnings
cargo test -p vel-highlight --release
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-highlight --no-deps
cargo run --release -p vel-highlight --example highlight_bench
cargo run --release -p vel-highlight --example highlight_bench -- --background-only
cargo run --release -p vel-highlight --example highlight_bench -- --worker-edits-only
```

## Round-six editing scopes and symbols: remove the second parser

Every `HighlightBatch` now carries `batch.scopes: ScopeSnapshot`, from the same corrected
LineOps used for colors. `ScopedLine.runs` stores `(start: u32, end: u32, ScopeStackId)` in
12 bytes per run. The shared stack table resolves IDs to active outermost-first scopes.
Styles and symbol flags use the same interned ID; colors can merge spans without losing scope
boundaries. Stack IDs are document-local and stable across edits and palette changes. Saved
snapshots own their table version, so later interning cannot change their meaning.

```rust
use vel_buffer::ChangeId;
use vel_highlight::{HighlightService, ScopeProvider};
use vel_scope::Selector;

# fn editing(service: &HighlightService, version: ChangeId) -> Result<(), vel_highlight::Error> {
let batch = service.request_lines(version, 0..50)?; // call from the existing dispatcher
if let Some(id) = batch.scopes.scope_at(3, 5) {
    let stack = batch.scopes.stacks.get(id).unwrap();
    let is_comment = Selector::parse("comment").unwrap().matches(stack);
    let score = batch.scopes.match_selector(5, &Selector::parse("source").unwrap());
    let region = batch.scopes.extract_scope(5);
    let active = batch.scopes.scopes_at(5); // absolute UTF-8 point
#   let _ = (is_comment, score, region, active);
}
# Ok(())
# }
```

`scope_snapshot(version, lines)` returns a nonblocking, current-version read without advancing
the parser; it is None while the worker owns the state or has not applied the requested version.
Unavailable lines have no runs and `exact == false`; scope queries return None there, including
in color-fallback batches. `scope_at`, `extract_scope` and `match_selector` also have service
conveniences with explicit versions, and DocumentHighlighter equivalents for owned scheduling.
`extract_scope` describes the maximal equal active stack in the validated/requested interval;
request a larger interval if its result reaches an interval boundary. EOF can be an empty region.

Load the shared public/User MetadataSet once and call
`DocumentHighlighter::new(...).with_metadata(metadata.clone())` before starting the service.
`service.symbols(version)` returns a versioned `SymbolList`; filter `symbol.local` and use
`symbol.name` for Goto Symbol in File. `index_name`/indexed/reference flags remain available.
Line records incrementally maintain candidate membership through edits, line splices,
convergence and branch repairs. List assembly joins contiguous candidates across lines and
applies the cached `symbolTransformation` / `symbolIndexTransformation` programs without
parsing again. Assembling a whole-file list/scope snapshot is a background operation proportional
to validated lines/symbols; do it on the existing dispatcher/analysis worker, then publish it.

Lane A's concrete migration from `velocity/src/language.rs`:

1. Remove its `ScopeCache`, per-line ParseState/ScopeStack checkpoints, local parse/replay path
   and the second-parser part of `AnalysisTask`.
2. Forward scopes together with each viewport batch; keep the current version and branch-repair
   invalidation protocol. For the whole-file analysis pass, acquire a validated full scope
   snapshot on the dispatcher and pass it to the existing bracket/fold/overview sweep.
3. Replace `LanguageState::scopes` and selector-context lookups with ScopeProvider/scoped runs.
   Convert the returned scopes to ScopeStack only when calling `metadata_for` for comment or
   indentation variables. Skip unavailable scopes and retry after matching batches arrive.
4. Use `service.symbols(version)` instead of `metadata.symbols` over a separately parsed file.
   Bracket matching, folding and minimap geometry remain lane A's analysis, reading the published
   runs and existing colors. Preserve edit/undo/redo ChangeSets as described above; recreate
   the service if that transaction history is unavailable.
5. Resource loading: add vel-index/vel-complete/vel-snippet dependencies; publish project edits,
   prepared buffer words and scope snapshots to their workers. For highlighted buffers, pass
   complete snapshots/SymbolList to `vel_index::analyze_scopes` on the analysis worker, then
   call `update_buffer_analysis`; this also avoids reparsing unsaved text in the project index.
   Apply completion/snippet edit
   plans once at the normal Buffer transaction boundary and retain snippet Session in the view.

Runs and style spans include LF. Round six corrects a previous implementation mismatch:
`Text::line()` excludes LF, so the service now slices start-of-line through the next line start
before parsing/recoloring. Renderers that slice `Text::line()` must clip run/style ends to the
body length. This also restores newline-sensitive context transitions and contiguous multiline
scope/symbol regions. A line above 4 GiB is rejected explicitly instead of truncating run offsets.

New regression/performance evidence is in [../vel-index/ROUND6.md](../vel-index/ROUND6.md), with
raw five-run measurements in `round6-timings.json`. Previous round evidence remains intact.

## Round 11: sparse huge-document integration

When `vel_largefile::LargeDocumentPolicy` selects viewport highlighting, construct a
`ViewportHighlighter` instead of the ordinary dense DocumentHighlighter/HighlightService.
Construction does not allocate a record per document line. This is an editor-owned scheduling
core; keep it on the existing analysis dispatcher. The current HighlightService continues to
wrap the dense engine for normal documents.

`request_lines(version, a..b, budget)` starts from a nearby exact stable checkpoint, or directly
at a with a best-effort initial state. It never parses a distant prefix on that request. It clips
requests to max_viewport_lines (default 512), checks the cooperative deadline between lines, and
returns version/scheme tags, colors and ScopeSnapshot just like the dense engine. Approximate
styles have `StyledLine::exact=false`; their scope queries return None. Deadline exhaustion uses
plain spans. Lines over max_line_bytes (default 16 KiB, including LF) are not copied or parsed
in this path. Skipping keeps the incoming state and marks dependent results approximate.

On a worker, call `advance(budget)` for exact background slices. Its progress reports the exact
prefix and `long_line` when it stops before an oversized line. Schedule `repair_long_line()` on
that worker, then continue advancing. This explicit repair is uncapped and may block in a regex;
never perform it while holding a UI/render lock. A general grammar cannot derive the state after
arbitrary skipped text without inspecting it. The repair uses real complete-line parsing, including
branch corrections, and bypasses transition memoization for oversized lines so stable multi-MB
strings do not remain cached. Speculative parser history can still retain text required for replay.
A line over the compact scope-run limit (4 GiB) still returns an explicit error during exact repair;
viewport fallback for it remains available.

Call request_lines again after background progress/rewinds to redraw the watched viewport from
corrected records. If its exact checkpoint was evicted or is outside the foreground lookback,
`repair_lines(version, range)` is the worker-only eventual-correction path. It parses from the
nearest retained exact checkpoint without a deadline and reads ahead through the 128-line rewind
horizon. Publish its returned batch only for the matching buffer version/palette. Even very old
viewports can therefore recover exact scopes; the UI never treats a skipped prefix as validated.
Worker repair and ordinary requests must remain serialized on this engine's owner. Off-thread
repair should publish batches to UI through its existing channel; do not move this core behind a
mutex that the UI waits on.

The engine retains bounded viewport/replay history, at most max_checkpoints stable states (128
by default) and 1024 memoized transitions. `apply_change(after, new_version, changes)` validates
lengths/boundaries, drops checkpoints after the first changed line, and restarts exact work from
a retained stable state, without a document-tail scan. A palette swap increments scheme_revision,
discards styled records and reparses. All old-coordinate/undo/redo integration rules above still
apply. New tests and paired construction/viewport measurements are linked from
[vel-largefile/PERF.md](../vel-largefile/PERF.md).
