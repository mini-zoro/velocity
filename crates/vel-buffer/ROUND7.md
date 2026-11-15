# Round seven: buffer regression evidence

arm64 macOS, Rust 1.99.1, release; same 100 MB fixture and unchanged original perf harness.
No sysctl values are available; every raw record preserves the blocked-command diagnostic.
No buffer transaction, selection mapping, grouping or undo hot path was changed.

| Existing scenario | Before median | After median | Median change |
|---|---:|---:|---:|
| Type, 10,000 carets | 0.908542 ms | 0.893479 ms | -1.66% |
| Type, 100,000 carets | 9.750938 ms | 9.840917 ms | +0.92% |
| Add 10,000th match | 0.036395 ms | 0.030354 ms | -16.60% |
| Undo, 10,000 carets | 1.132667 ms | 1.108980 ms | -2.09% |
| Delete 50 MB | 0.730896 ms | 0.707313 ms | -3.23% |
| Undo 50 MB delete | 0.001646 ms | 0.001708 ms | +3.80% |
| 10k edits / 100k regions | 3.961542 ms | 4.153688 ms | +4.85% |
| Undo metadata per caret | 48.048900 B/caret | 48.048900 B/caret | +0.00% |

All 16 observations per side are included: the original seven-run sequential before/after
passes, followed by nine alternating old/new pairs for each scenario. The first sequential
pass had a noisy >5% typing result, so the saved baseline and current executables were
alternated to check it. Pooling includes every observation without exclusions; the largest
median increase is **4.85%**, below the 5% gate. The paired-only and sequential-only subsets
are both retained and visibly noisier. This is a median comparison, not a claim of identical
p95 tails under concurrent host activity; nearest-rank p95s are in the summary.

Raw evidence: [before](perf-round7-before.jsonl), [after](perf-round7.jsonl),
[paired before](perf-round7-paired-before.jsonl), [paired after](perf-round7-paired-after.jsonl),
[summary with all p95s](perf-round7-summary.json). Each worker is a separate process.
The original [PERF.md](PERF.md) and round-two data remain intact.

Reproduce ordinary passes with:

```sh
cargo bench -p vel-buffer --bench perf -- --runs 7 > crates/vel-buffer/perf-round7.jsonl
```

For paired comparisons, save the pre-change benchmark executable; alternate its
`--worker SCENARIO spikes/data/text-100mb.txt` invocation with the current executable,
setting `VEL_BUFFER_BENCH_RUN` for each pair. The harness remains unchanged.

History capture/serialized-size evidence and integration links are in
[the round-seven engine report](../vel-git/ROUND7.md).
