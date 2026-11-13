# velocity

Native macOS editor demonstration for lane A/P1. MIT OR Apache-2.0. Requires macOS
14+ for view-bound CADisplayLink. See [RUNBOOK.md](RUNBOOK.md) for hardware review.

```sh
cargo build --release -p velocity
./target/release/velocity
./target/release/velocity path/to/file.rs
```

The default is a bundled original sample repeated to exactly 5,000 lines. An explicit
file loads through vel_text::Text::from_reader; missing/invalid UTF-8 files are errors.
`--lines N` optionally caps the input; zero (default) reads the whole explicit file.
There is no dependency on vel-buffer. The toy editor owns one selection, persistent
text snapshots for bounded Undo/Redo, grapheme movement/deletion, inline composition
and its original snapshot for atomic IME undo/cancellation. Edits remain in memory.

The view lays out only visible rows, with line numbers, a fixed gutter, current-line
highlight, token colors, selection, caret and marked-text underline. Fractional scroll
positions preserve precise trackpad/momentum movement. Tabs use four-column stops;
Unicode grapheme widths drive hit-testing and candidate geometry. Line/run caches are
bounded. This is not a full syntax engine, wrapped/bidi layout, file-save UI or multi-
cursor buffer. Token coloring and measurement/statistics plumbing are adapted from the
repository's MIT/Apache P0 prototype; native lifecycle, UTF-16 IME editing and vel-text
integration are new.

`--bench-startup`, `--bench-scroll`, `--bench-typing`, and `--bench-memory` all run and
exit automatically. Schema-1 JSONL preserves P0's kind/milestones/statistics fields
while adding actual scanout and independent footprint observations. Samples are
buffered outside measured loops. Automatic runs have an independent watchdog, reject
uncompleted output, and report outside input. `--log-input` records native keys,
selectors, UTF-16 ranges and receipt→present-return; its synchronous I/O affects timing.
`--headless-smoke` exercises edits/layout/statistics/kernel telemetry and never calls
Platform::run or creates a GPU device.

Only the three new crate directories and workspace Cargo.lock belong to this lane.
Hardware budgets are pending; [RESULTS.md](RESULTS.md) is the Chinese review template,
and [IME_TEST.md](IME_TEST.md) is the manual native-input checklist.
