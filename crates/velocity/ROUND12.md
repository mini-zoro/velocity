# Round 12: huge documents, horizontal lines and hex tabs

Source baseline: `4e6a16a2a7b06b70bccaf02034bb1ff276bb44ba`. No commit. Changes are limited
to `velocity`, `vel-fileio`, and root Cargo.lock. The large-file/highlight engines are consumed
read-only. No GUI or Metal initialization was run in this sandbox.

Files at/above `large_file.load_bytes` (64 MiB by default) open on a cancellable worker.
UTF-8 streams through `vel_largefile::stream_utf8` and a bounded CR/CRLF normalizer. The
first shared 2 MiB prefix can be scrolled and selected immediately. Tab captions show name
and progress; the status bar explicitly marks the prefix read-only. Edits and all saves are
blocked until completion. Closing the tab drops/cancels its task without joining the UI.
Each open/reload owns its channels; an old result cannot replace a newer generation.
UTF-16 and invalid UTF-8 invalidate any provisional prefix and retry the existing complete
encoding-aware decoder on that worker, with original-byte progress and cancellation.
BOM, encoding and majority line endings survive save. Session checkpoints retain a disk
reference, never incomplete prefix history; restored huge/binary tabs use the same loader.

`LargeDocumentPolicy` uses known source length while loading, decoded byte/line counts and
worker-computed longest-line metrics. Huge/long documents use `ViewportHighlighter` on a
dispatcher instead of a dense per-line table. Visible approximations are repaired there;
oversized source lines render plain and exact following-line state is repaired on the worker.
Indexing/buffer-word completion, spelling and disk diff are suppressed by policy. The minimap
uses bounded samples. All README thresholds and viewport parser caps are effective settings.
The command palette has **Large File: Enable … for This View** commands for highlighting,
indexing, wrapping, spelling, disk diff and minimap. An explicit boolean `word_wrap` also
overrides automatic degradation. Enabling indexing also enables full highlighting because
the existing index publisher requires exact document scopes.

Unwrapped physical lines at/above 16 KiB use a bounded horizontal slice for rendering.
Worker-built `LongLineIndex` instances are bound to BufferId/ChangeId/tab size, with at most
eight completed indexes and four pending results per layout. A global permit limits actual
index construction to two workers even when edits invalidate in-flight jobs. Before readiness, a bounded local
preview supplies provisional geometry; the idle loop collects results and redraws. Ordinary
single replacements call `update_after_edit`; newline, multiple-range and incompatible
snapshot changes discard/rebuild indexes. Caret/word movement, selection, hit testing,
vertical goals and IME rectangles share the indexed columns. IME preview updates its own
versioned geometry without committing the buffer. Keymap text contexts are capped at 8 KiB,
and newline in an oversized line inserts LF without scanning the line for autoindent.

Binaries detected by the first-8-KiB/BOM heuristic open in a read-only `HexView`: 16 bytes
per row, 64-bit offsets and ASCII. The mapping backs a private application-owned snapshot,
never the externally mutable source. Snapshot creation is worker-only and may require a
complete disk copy on filesystems without cloning. Source modification/reload cannot truncate
the mapped file. Ctrl+G accepts decimal or `0x` offsets. Copy/Hex: Copy Hexadecimal Bytes
copies hexadecimal; Hex: Copy Text uses lossy UTF-8 for non-UTF-8 bytes. Clipboard selections
are capped by `large_file.copy_bytes` (16 MiB default) before allocating, so Select All on a
4 GB binary cannot allocate a multi-GB clipboard on the event thread.

## Measurement boundaries

Final measurements and process-level paired confidence intervals are in
`timings-round12.json`; raw observations and frozen executable hashes are retained under
`/tmp/velocity-round12`. The log fixture contains 4,000,000,000 actual ASCII bytes. The binary
fixture is sparse, 4,000,000,003 bytes with an `END` tail. No cache flush, custom allocator,
priority change, target-cpu, LTO or GPU work was used. Fixtures, indexes and immutable binary
snapshots are constructed outside edit/scroll timing. The prefix timer starts before
`Workspace::open` and ends after the first nonempty production CPU scene. It establishes
editor-path readiness, not displayed pixels or a cold-disk guarantee. Edit/scroll timers are
wall time for the CPU path, including Buffer history/index maintenance and scene construction.

Five final independent processes produced these editor-path results (ms). Typing and scroll
entries are process p95s; the last column is the worst process, not a pooled-frame percentile.

| Operation | Budget | Median process result | Worst process result |
| --- | ---: | ---: | ---: |
| First prefix + CPU scene, real 4 GB log | 100 | 7.347 | 8.036 |
| Input -> scene, middle of 50 MB line | p95 4 | 0.407 | 0.420 |
| Horizontal scroll + scene, 50 MB line | p95 2 | 0.0226 | 0.0230 |
| Offset/reveal/hex scene, sparse 4 GB binary | smooth; no numeric spec | 0.1019 | 0.1046 |

All observed UI budgets pass. Five prefix observations give only a coarse tail estimate;
their nearest-rank p95 is the maximum. A preceding pilot and all independent process records
are retained. An initial normal-file implementation incorrectly ran the plain minimap fallback
for ordinary documents with an empty minimap cache. Its nine completed typing pairs regressed
by +310.50% and were rejected; all remain in `normal-final/rejected-pilot-summary.json` and its
raw JSONL. The corrected fallback runs only under degraded policy, and threshold settings are
parsed once per settings identity. Component profiles locate the removed cost; a fresh full
paired series establishes the normal-file gate, separately from those diagnostic profiles.

The final normal-file gate uses **ten alternating independent AB/BA process pairs** against
`4e6a16a`. Both p50 paired bootstrap upper bounds are below 1.05:

| Metric | Baseline median (ms) | Candidate median (ms) | Paired change | Paired B/A 95% CI |
| --- | ---: | ---: | ---: | --- |
| Typing p50 | 0.4094 | 0.3737 | -8.72% | [0.8971, 0.9207] |
| Typing p95 | 0.4410 | 0.4118 | -6.43% | [0.9216, 0.9407] |
| Scroll p50 | 0.3857 | 0.3404 | -12.16% | [0.8642, 0.9062] |
| Scroll p95 | 0.5038 | 0.4672 | -7.49% | [0.8941, 0.9347] |

The independent ratio-of-medians p50 intervals are [0.9047, 0.9210] for typing and
[0.8632, 0.9038] for scroll. `round12-normal.jsonl` preserves all 40 gating process records,
including their per-frame samples. `round12-features.jsonl` preserves all five feature runs.
Full logs, rejected pilot samples and frozen binaries remain under `/tmp/velocity-round12`.
Final candidate SHA-256 is `715d9b3828baa5bf68aef2f0c7a10e3adee2281ee1afa221ac138848167828da`;
baseline is `09dcac292f70ea6e7647634e39de887440e43f574772147b1f10d63fa28e2ecb`.
The runner verified that no measured runtime source changed after freezing this candidate.

Required fmt, clippy, release tests and rustdoc pass: **464 passed, zero failed/ignored**.
Commands and log hashes are in `validation-round12.json`. The eight editor-path tests cover
prefix publication/cancellation/save safety, encoding fallback, policy/overrides, Unicode
columns, multi-caret movement/deletion, line-boundary transitions, IME reconversion and
version invalidation, sparse repair, hex copy/truncation safety and session restore. The
new file-I/O test also compares progressive/full decoding at BOM/CR/encoding seams.

## Reviewer commands

From this worktree, the frozen baseline used here is `/tmp/velocity-round12/velocity-before`.
To build it afresh without touching a branch, export the commit and provide its submodule/data:

```sh
R12_BASE=$(mktemp -d /tmp/velocity-r12-base.XXXXXX)
git archive 4e6a16a | tar -x -C "$R12_BASE"
rmdir "$R12_BASE/third_party/sublime-packages"
ln -s <lane-checkout>/third_party/sublime-packages "$R12_BASE/third_party/sublime-packages"
ln -s <checkout>/spikes/data "$R12_BASE/spikes/data"
cargo build --release --manifest-path "$R12_BASE/Cargo.toml" -p velocity
cargo build --release -p velocity
python3 crates/velocity/scripts/measure-normal.py \
  --before "$R12_BASE/target/release/velocity" --after target/release/velocity \
  --pairs 10 --output-dir /tmp/velocity-r12-review-normal
```

The output directory must be new. The runner freezes both binaries, alternates AB/BA order,
runs one process at a time, keeps every raw JSONL/log, and bootstraps independent process
p50/p95 pairs with 20,000 resamples. Stop builds/tests while measuring.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
python3 crates/velocity/scripts/prepare-largefiles.py /tmp/velocity-r12-fixtures
target/release/velocity --headless-large-files /tmp/velocity-r12-fixtures/log-4gb.log \
  --data-dir /tmp/velocity-r12-profile --output /tmp/velocity-r12-headless.jsonl --timeout 180
python3 crates/velocity/scripts/measure-largefiles.py --binary target/release/velocity \
  --file /tmp/velocity-r12-fixtures/log-4gb.log --runs 5 --output-dir /tmp/velocity-r12-review-features
```

Outside the sandbox, on an awake/unlocked Mac, automatic native modes exercise all three files:

```sh
zsh crates/velocity/scripts/review-round12-gui.zsh target/release/velocity /tmp/velocity-r12-fixtures
```

The native script is supplied for review and has not been run here. See MANUAL_TEST.md for
IME/clipboard checks and scanout inspection. Explicitly re-enabled dense features/wrapping,
unbounded Unicode-context edits, initial index construction, native shaping and GPU scanout
have no latency guarantee established by these headless results. Full-file load throughput
and the engine's separate 1 GB allocation budget were not measured as editor UI budgets.
