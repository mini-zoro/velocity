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
