# Round 11 engine budgets and paired evidence

2026-11-15. No commits. Baseline is `760baf9`, the clean worktree at the start of this task
(including accepted round 10). Compiler/architecture/OS and baseline executable hashes are in
[measurement-environment.json](measurement-environment.json). No OS file-cache flush was
performed; independent processes do not imply cold disk caches. The run verified arm64, macOS
26.1 and Rust 1.99.1; it does not infer system load from timing variability. The existing text/
buffer runners record unavailable sysctl load averages, CPU time and faults in the raw samples.

## What was measured

All final timings use release production builds, serial independent processes, alternating AB/BA
order. Feature probes use five pairs; the final legacy pass uses 25, extended to 101 for
undo_50mb. Initial observations are retained (counts are in paired-summary.json). This lane
stopped its builds/tests during the benchmark suites.
The old text/buffer bench executables were saved before changing source. Existing hot-path source
and node layout were not changed; the new text API is additive. New budget probes compare the
ordinary Text loader/operations with the progressive/indexed engine in the same build. These
new operations deliberately do more work: loading also publishes/provides metrics, and typing
also updates grapheme/column/word metadata. They are not claimed faster than unindexed typing.

Every raw process record includes pair and order. [paired-summary.json](paired-summary.json)
uses a deterministic 20,000-resample paired bootstrap over process-level median ratios; queries
inside one process are not treated as independent CI samples. CI is for the median paired B/A
ratio, which need not equal the ratio of the two marginal medians. P95 is nearest rank. Five
load/viewport observations give only a coarse tail estimate (P95 equals their maximum).

## New feature budgets

MB/GB are decimal bytes. Times below are wall time; setup/index construction and destruction
are excluded from the typing/query measurements. All COW edits retain the input root, as history
would. Layout is a 200-cell geometry query, not GPU drawing or font shaping.

| Operation | Budget | New median / P95 | Observed maximum | Assessment |
|---|---:|---:|---:|---|
| First 2 MiB prefix, real 4 GB ASCII file | <=100 ms | 0.875 / 1.846 ms | 1.846 ms | all observed pass |
| Complete 1 GB ASCII, progress enabled | <=1 s | 631.141 / 657.016 ms | 657.016 ms | all observed pass |
| Load peak RSS, 1 GB | <=1.15x | maximum 1.09478x (1,094,778,880 B) | same | passes on this fixture |
| Insert in middle of 50 MB line, including index maintenance | <=2 ms | 0.266250 / 0.279958 ms | 0.920917 ms | all observed pass |
| Word motion, 50 MB identifier | <=1 ms | <0.001 / <0.001 ms | 0.003125 ms | all observed pass |
| Move to line end | <=1 ms | below timer resolution / <0.001 ms | 0.000166 ms | all observed pass; no zero-time claim |
| Visible horizontal window, 200 cells | <=0.5 ms | 0.000125 / 0.000834 ms | 0.001542 ms | all observed pass |

Sources: [budgets-pairs.jsonl](budgets-pairs.jsonl), [first-pairs.jsonl](first-pairs.jsonl).
The first-prefix fixture is 4,000,000,000 actual ASCII bytes written before measurement, not a
sparse NUL-filled file or an in-memory simulated reader. B cancels after observing the first
prefix; A has to finish its ordinary non-progressive full load before any Text can be returned.
Its median first-available time was 1410.233 ms; the paired B/A CI is [0.000434, 0.001373].
This measures the engine's prefix reaching its consumer, not first rendered pixels.

The 1 GB corpus is the existing `text-1gb.txt`; each side opens the file independently and
retains the result through measurement. Every progressive run observed 61 publications. Its
paired time ratio CI against ordinary full loading is [1.1073, 2.6911]; progressive loading is
not a throughput improvement claim. Both variants have about 1.07517x estimated reachable
allocation (new: 1,075,172,597 B, old: 1,075,171,757 B). Peak RSS is captured before diagnostic
allocation accounting; the top-level RSS may include accounting scratch. Some runs had much
lower resident peaks than allocation estimates; RSS alone does not establish the storage bound.
The pipeline stages at most 8 MiB of source input plus bounded leaf descriptors/tree assembly.
This memory result covers this ASCII/LF corpus, not every possible LF density or optional index.

The 50 MB fixture is one ASCII identifier, exposing the full-suffix cost of a scanning word
reference. The reference horizontal layout scans to the middle of the line; Unicode correctness
is tested separately against a flat segmentation/width model. Word/window each have 8 queries
per process, typing/end 200 (40/1000 observations per side across five processes). Line-end
reads approach the clock's granularity; no speedup ratio is interpreted for that case. The
index introduces metadata-update cost into typing but leaves the existing Text API untouched.

## Sparse highlighting

[highlight-pairs.jsonl](highlight-pairs.jsonl) compares the existing dense DocumentHighlighter
with the new ViewportHighlighter using a fixed small syntax, with setup outside request timing.

| Workload | Dense median | Sparse median / P95 | Paired B/A 95% CI |
|---|---:|---:|---|
| Construct for 10M LF lines (20 MB text) | 104.034 ms | 0.05500 / 0.05850 ms | [0.000504, 0.000549] |
| Request 10 lines at line 9M, 2 ms cooperative budget | 2.006 ms | 0.09525 / 0.10237 ms | [0.045945, 0.051040] |
| Request one 50 MB line | 23.962 ms | 0.00404 / 0.00429 ms | [0.000152, 0.000176] |

The deep sparse request parses only its ten requested lines and marks them approximate. The
dense request spends its budget on prefix catch-up and returns fallback. The long-line sparse
request returns a plain approximate span without copying/parsing the body. These comparisons
show bounded foreground degradation, not equal full-parse output or a parser throughput win.
Background/explicit worker repair uses real complete lines, restores comment/string state and
branch rewinds, and can exceed a deadline. Stable oversized transitions bypass memoization.
Regular `advance` stops before them; `repair_long_line` / `repair_lines` are explicit worker-only
APIs. Exact scopes are never published from skipped text. Tests compare repaired output with
the dense engine and exercise checkpoint eviction, edits, palettes and branch fail/read-ahead.

## Existing text/buffer regression gate

The complete original benches were rerun against the final isolated-module implementation:
25 independent pairs for all three loads, all 21 text operations and all eight buffer scenarios;
undo_50mb was extended to 101 pairs. Raw observations: [legacy-pairs.jsonl](legacy-pairs.jsonl).
All paired median changes and all ratios of marginal medians are within the 5% regression
limit. The worst paired increase is undo_50mb, +4.59% (875 -> 917 ns marginal medians, +4.80%).
The 95% ratio interval for that sub-microsecond case is [1.000, 1.09486], so the confidence
interval does not certify a universal <=5% bound. Other intervals spanning 5% are visible below.
No blanket speedup or zero-noise claim is made. Gate details: [regression-gate.json](regression-gate.json).

| Existing metric | Pairs | Paired median change | B/A 95% CI |
|---|---:|---:|---|
| load ascii-100mb | 25 | +1.41% | [0.99926, 1.05553] |
| load ascii-1gb | 25 | -2.02% | [0.91098, 1.01081] |
| load unicode-100mb | 25 | +1.06% | [0.99977, 1.02401] |
| byte_char | 25 | +0.21% | [0.99307, 1.01093] |
| char_byte | 25 | -0.17% | [0.98983, 1.00203] |
| byte_utf16 | 25 | -0.11% | [0.99046, 1.00319] |
| utf16_byte | 25 | +0.03% | [0.99504, 1.00406] |
| byte_line | 25 | +0.55% | [0.95744, 1.01712] |
| line_byte | 25 | -1.22% | [0.94620, 1.01425] |
| byte_point | 25 | -1.16% | [0.94790, 1.01352] |
| byte_point_utf16 | 25 | -4.32% | [0.91237, 0.99045] |
| point_byte | 25 | -1.20% | [0.95014, 1.00130] |
| point_utf16_byte | 25 | +0.40% | [0.95841, 1.07513] |
| clone | 25 | -1.30% | [0.60524, 1.01254] |
| insert | 25 | -12.76% | [0.65751, 0.99983] |
| delete | 25 | -1.62% | [0.92966, 1.09526] |
| batch_lines_10000 | 25 | -4.79% | [0.90229, 1.02594] |
| sequential_lines_10000 | 25 | +0.93% | [1.00107, 1.01503] |
| batch_lines_100000 | 25 | -4.80% | [0.91449, 1.02587] |
| sequential_lines_100000 | 25 | +0.92% | [1.00415, 1.01635] |
| batch_uniform_100000 | 25 | -2.18% | [0.92448, 1.03589] |
| sequential_uniform_100000 | 25 | -0.15% | [0.94978, 1.01660] |
| chunks | 25 | -0.66% | [0.96975, 1.01258] |
| lines | 25 | -0.10% | [0.80270, 1.01776] |
| type_10000 | 25 | -2.12% | [0.95189, 1.05566] |
| type_100000 | 25 | +0.41% | [0.99710, 1.01081] |
| find_10000 | 25 | -0.31% | [0.97824, 1.04724] |
| undo_10000 | 25 | -0.22% | [0.99111, 1.00262] |
| delete_50mb | 25 | -0.28% | [0.98727, 1.01524] |
| undo_50mb | 101 | +4.59% | [1.00000, 1.09486] |
| map_100000 | 25 | -0.33% | [0.98922, 1.01394] |
| undo_metadata | 25 | +0.00% | [1.00000, 1.00000] |

The first implementation had several >5% estimates and was not accepted as the final gate.
The unused append/owned-slice APIs were removed and progressive construction moved out of the
ordinary tree module. Earlier measurements, including all outliers, remain in
[experiments](experiments/README.md). A 51-pair baseline-against-itself control
([undo-aa.jsonl](undo-aa.jsonl)) has CI [0.86258, 1.12528], illustrating this one-operation probe's
noise floor. That control is not used to subtract or hide candidate measurements.

## Experiments and practical limits

An initial one-MiB-at-a-time serial builder and then a variant restarting the parallel loader
for each large chunk were investigated. Their five-pair experiment
([raw data](loader-experiment.jsonl); A=one-MiB serial, B=per-chunk parallel) was inconsistent
and did not establish the load budget. The retained implementation publishes completed subtree
batches inside one bounded pipeline, preserving ordinary loader paths. The final structural
cleanup was followed by a complete fresh paired pass; earlier tail misses (1.258 s load, 3.765 ms typing) are preserved in experiments.
The last mailbox fix makes prefix polling nonblocking and drops retired trees after releasing
the publisher lock; its focused contention test and full workspace checks pass. All feature
probes above were retaken after that fix, with earlier results in experiments/pre-mailbox-*.
The final five feature pairs do not establish hard latency guarantees or cold-cache bounds.
No optional C++ parser/cache optimization was attempted in this round.

Encoding fallback remains a complete vel-fileio decode, with provisional UTF-8 roots invalidated
on failure. CR/CRLF bytes are preserved by the raw loader. Initial line-index construction and
Unicode edits that propagate unbounded grapheme context remain background work; the
interactive measurements do not establish a worst-case bound for those inputs. Exact syntax
repair of long lines is uncapped and may need contiguous allocation. Hex mapping requires the
caller's file-stability invariant. These are API limits described in [README.md](README.md), not
silently accepted data corruption or falsely exact output.

## Reproduction

Build the baseline before changing code, or export commit 760baf9 to a temporary directory with
`git archive` and build there. Save its `vel-text` and `vel-buffer` perf executables as
`/tmp/velocity-r11/text-before` and `buffer-before`. Build the candidate and save `text-after`
and `buffer-after` beside them. Both use the workspace release/bench profile without custom
allocator, target-cpu, LTO or priority changes. The original bench's parent can prepare its
Unicode fixture with `--impl vel-text --only prepare`.

```sh
cargo bench -p vel-text -p vel-buffer --bench perf --no-run --message-format=json
cargo build --release -p vel-largefile -p vel-highlight --examples
python3 crates/vel-largefile/scripts/measure.py --suite legacy --pairs 25 --output crates/vel-largefile/legacy-pairs.jsonl
python3 crates/vel-largefile/scripts/measure.py --suite legacy --pairs 101 --resume --case undo_50mb --output crates/vel-largefile/legacy-pairs.jsonl
python3 crates/vel-largefile/scripts/measure.py --suite budgets --output crates/vel-largefile/budgets-pairs.jsonl
python3 crates/vel-largefile/scripts/measure.py --suite first --output crates/vel-largefile/first-pairs.jsonl
python3 crates/vel-largefile/scripts/measure.py --suite highlight --output crates/vel-largefile/highlight-pairs.jsonl
python3 crates/vel-largefile/scripts/report.py crates/vel-largefile/*-pairs.jsonl --output crates/vel-largefile/paired-summary.json
```

`--data DIRECTORY` selects the existing text fixtures. The first suite generates its 4 GB file
outside timing; it can be removed afterward. `--pairs N --resume --case NAME` extends selected
paired cases without discarding initial observations. Do not overlap these suites with builds
or tests. The scripts fail on worker errors; they do not omit failed/slower observations.

## Validation

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace --release` (421 passed, zero failed/ignored), and
`RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` passed after the module cleanup and nonblocking-mailbox fix.
[validation.json](validation.json) records the commands; all final timings were taken with
this lane's compilation/tests stopped. No commit was made.
