# Highlight service measurements

Release, macOS 26.1 arm64, rustc 1.99.1, shared public Rust grammar and original Velocity Dark palette. No sysctl. Five runs per case; another lane was active, so scheduler noise is retained in the raw observations.

| Case | Result | Budget |
| --- | ---: | --- |
| Worker service edit submission → exact visible batch, pooled p95 of 500 samples | **0.11500 ms** | ≤0.2 ms |
| Stable one-letter edit, 10k lines, visible 100, pooled p95 of 500 samples | **0.00258 ms** | ≤0.2 ms |
| Median of the five per-run edit p95s | 0.00242 ms | ≤2 ms |
| Worst individual edit sample | 0.05454 ms | all samples completed their requested viewport |
| Warm scope-stack style resolution (prior cache benchmark) | **63.78M/s** | ≥20M/s; zero allocations in every 5M-lookup run |
| Full 10 MB Rust, core including styles/checkpoints | 4.682 s | report only |
| Full 10 MB Rust, actual background service | **4.709 s** | report only |

Worker per-run edit p95 ms: `[0.109166, 0.10775, 0.109166, 0.109333, 0.668292]`; worst worker delivery: 2.13958 ms. Worker tests start from a completely highlighted 10k-line snapshot, submit a real ChangeSet through HighlightService, and time until the immediate or asynchronous result is exact. Polling/yielding is confined to the benchmark; production typing never waits for it.

The edit timer includes apply_change and the returned 100-line batch. Text replacement happens before timing. All 500 edits reparse exactly one line, then converge and reuse the validated suffix. This tests stable token edits, not multiline delimiter changes or a first request at a distant unparsed viewport.

Per-run edit p95 ms: `[0.0026249999999999997, 0.003542, 0.002417, 0.0024159999999999997, 0.002375]`. Core full-file seconds: `[4.681539167, 4.882282708, 5.148992417, 4.621402583, 4.55742825]`. Background full-file seconds: `[4.790597709, 4.736842208, 4.626526, 4.708506292, 4.6868191249999995]`.

The background measurement starts a real HighlightService thread, polls its nonblocking progress once per millisecond, and waits for every line to validate. It includes thread startup, 1 ms work slicing, yielding and completion observation; it excludes syntax-set and Text construction. The first background run includes initially cold regex paths; following runs share compiled patterns. It does not request a 297k-line viewport or allocate a renderer-sized result. Core measurements warm Rust paths first and call finish directly.

timings-before-interning.json is the five-run comparison before repeated operation arrays and end-of-line scope stacks were interned. Dense N=1 checkpoints remain proportional to line count. The warmed parser allocation fixture is allocation-free with and without memoization; cache misses, speculative snapshots, styles and returned batches can allocate. The 2 ms synchronous deadline is cooperative between lines: a single pathological regex call is not preemptible. The parser-only checkpoint-window measurements are in vel-syntax/tests/timings.json; they now reuse the shared stable-line cache. Missing viewports still fall back and complete asynchronously.

The regression tests compare against complete fresh highlighting after Unicode/newline edits, disjoint pending changes, branch backtracking, and palette swaps. A background rewind republishes an already completed visible window; scheme generations and buffer versions filter stale results. The original scope corpus is 125,877/0, symbol assertions 688/0; reindent has known failures described in ../vel-syntax/RESULTS.md.

Reproduce: cargo run --release -p vel-highlight --example highlight_bench, then cargo run --release -p vel-highlight --example highlight_bench -- --background-only and --worker-edits-only; cargo run --release -p vel-color-scheme --example style_cache_bench; python3 crates/vel-syntax/scripts/report.py. Integration: README.md.
