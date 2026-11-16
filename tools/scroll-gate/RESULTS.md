# Round 14 evidence

Base source: `4b65f50e2df98be1ffeb3cdd699e5713ed46e046`, the clean starting
`lane-b` worktree after scroll phase 1. The base executable was copied before source
changes to `/tmp/velocity-scroll-round14/velocity-base`. No commit or packaging run.
All native timing values from the phase-2 spec remain **reviewer measurements of
phase 1**, not measurements of this candidate.

## Removed costs and implementation boundaries

1. `LineLayoutCache::insert`: the 2048-entry minimum scan is replaced with indexed
   oldest/newest links, slot reuse, and a hash index. Lookup/touch/insert/eviction
   take expected O(1) cache operations plus bounded content hashing/equality.
   Normal miss-driven insertions preserve exact LRU hit behavior. Overwriting a
   resident key updates its slot without evicting an unrelated entry. Collision
   checks, 16 KiB line cap and font style keys remain in force.
2. `ScopeSnapshot::complete`: production snapshot builders accumulate an inexact
   count as they collect records; published snapshots query it in O(1). Arbitrary
   raw mutation cannot maintain a boolean-field count without observation, so the
   existing spelling fixture's `for line in &mut snapshot.lines` API invalidates
   the cache and its next query scans once. Subsequent queries are O(1). This
   compatibility escape is explicit in the API docs; no frame path uses it.
   Property tests cover arbitrary/all/single-inexact patterns, intervals, clones
   and legacy mutations, in addition to the existing real edit/rewind tests.
3. `Renderer::prepare` / uploads: fresh per-frame `Queue::write_buffer` instance
   staging is replaced by a reusable `StagingBelt`. Copies and remapping are on
   the render submission; `PollType::Poll` never waits. Resident CPU vectors swap
   only after submit, preserving correctness across skipped acquisitions. Buffer
   growth invalidates the resident baseline. Dirty planning uses whole-buffer and
   64 KiB comparisons, refining changed blocks to 4 KiB upload ranges. It skips
   stable geometry at unchanged offsets and coalesces neighboring dirty ranges.
   A changed instance count can move later geometry and require its re-upload.
   Extra memory is one resident CPU scene plus reusable in-flight belt chunks.
   Fifo and `desired_maximum_frame_latency = 1` are unchanged.

The upload microbenchmark is a CPU allocation/copy **model**; it does not exercise
Metal, driver allocation, asynchronous remapping, draw submission or scanout.
GPU staging correctness and the actual renderer-frame delta still need the native
command in README.md. `atlas_info.instance_upload_bytes` and
`instance_upload_ranges` expose the last submission's copy volume for that review.

## Application/frame inspection and lane A follow-up

The supplied reviewer profile is the native `Renderer::render` evidence: remaining
`prepare → Queue::write_buffer / _platform_memmove` is about 3% wall time after phase
1. No renderer is instantiated in this sandbox. `sample` also denied inspection
of a headless child (`sample cannot examine process ...; try running with sudo`),
so there is no invented candidate main-thread percentage or native App::scene
profile here. The real-event gate collects both outside the sandbox.

Source inspection followed `App::scene → View::scene` and the frame loop's
`Language::busy/ready → scopes_ready → ScopeSnapshot::complete` queries. The
full-file completeness scan is removed from published snapshot queries.
Renderer packing traverses supplied scene instances, not the
rope, line records, or undo history. `paint_minimap` consumes at most roughly 1024
sampled lines (`language.rs`, analysis-worker `step = len_lines.div_ceil(1024)`),
and the huge-file fallback samples at display-height/stride resolution. Those
are scene-size costs; dirty uploads avoid copying their stable packed output.
There was no profile basis for editing vel-text.

Lane A findings, **read only; not changed**:

- `App::scene` calls `workspace.sync_shared()`; `changed()` calls it per input too.
  `workbench.rs::sync_shared` enumerates all owners and scans all tabs for each,
  allocating owner/peer vectors, including on unchanged scrolls. This grows with
  open-document/view history (O(tabs²)); an identity/revision/selection dirty path
  should avoid repeating it. It is not evidence of an O(file bytes) text compare:
  `Text::eq` already skips pointer-identical roots.
- Interactive `App::scene` allocates group/sheet visibility maps and clones groups
  each frame. Bound this against many tabs/groups in a native profile before any
  additional optimization. The current one-file headless view bench omits it.
- Legacy telemetry times only `input_latency` records. Frame intervals therefore
  use the logged-submit proxy, and poster/main alignment has Reporter-creation
  skew. For more precise native timing, lane A can emit `submit_main_ms` (or
  `native_scroll_frame`) and a shared absolute/monotonic clock anchor. The analyzer
  accepts explicit submit timestamps now and labels both boundaries honestly.
- Lane A's `$VELOCITY_LOG_DIR` change is consumed by the driver; older binaries
  need their compile-time `--legacy-output-dir`. No lane-A source was edited.

## Diagnostic run retained

`evidence/headless-initial.json` retains the first five-pair application result.
It **failed**: scroll p50 0.3084165 → 0.292312 ms (-5.22%), p95
0.38829795 → 0.52864615 ms (+36.14%), p99 0.43953958 → 0.89428949 ms
(+103.46%). Typing had large run-to-run variation too. It is not a passing
regression claim. Initial micro results are retained beside it; small-block
comparisons regressed the unchanged/partially changed copy models, motivating
the coarse/whole-buffer comparison fast paths in the final implementation.

Final micro, paired and required-check results follow.

## Final measurements and acceptance

**Performance acceptance: FAIL / not cleared.** The code, strict checks and native
gate are ready for review, but these measurements do not establish the required
≤5% regression bound. No native candidate result is claimed. Do not merge on the
strength of the microbenchmarks alone.

Final raw evidence is in `evidence/`: nine-pair micros, five-pair application
results and all their input JSONL, a three-pair same-binary control, strict-check
statuses and binary hashes. `/tmp/velocity-scroll-round14/velocity-candidate-measured`
is the final standalone release build. The standalone build command matches the
baseline (`cargo build --release -p velocity`); micro executables build separately.

| Micro (median of nine pairs) | Before | Candidate | Change / scope |
|---|---:|---:|---|
| Cache churn, 128 entries | 3.712 µs/op | 0.486 µs/op | -86.9% |
| Cache churn, **2048 entries**, actual 10 MB fixture | 51.737 µs/op | 0.763 µs/op | -98.5%; bounded content hashing included |
| Complete flag, **297,030 lines / 10,000,000 bytes** | 328.354 µs/query | 1.099 ns/query | O(lines) → O(1) on published snapshots |
| Upload CPU model, unchanged 2 MiB | 102.881 µs | 151.235 µs | **+47.0%**, bytes copied 2 MiB → 0 |
| Upload CPU model, indicator changes | 109.201 µs | 184.577 µs | **+69.0%**, bytes copied 2 MiB → 4 KiB |
| Upload CPU model, 512 KiB body changes | 130.065 µs | 190.603 µs | **+46.5%**, bytes copied 2 MiB → 512 KiB |
| Upload CPU model, all 2 MiB change | 209.175 µs | 204.440 µs | -2.3%, still copies 2 MiB |

The cache micro uses 8,192 distinct bounded lines from the actual fixture and
30,000 miss-driven operations per trial, with glyph shaping outside its boundary.
It isolates eviction churn rather than predicting the hit ratio of every native
viewport. The completeness case is a fully exact snapshot, the old scan's common
steady state after highlighting finishes. Legacy mutable iteration is a documented
one-scan recache exception, tested for equivalence.

Upload timings are **not a speedup claim**. The final protocol alternates the old
and new implementation on each identical frame, nine trials of 2,000 frames; scene
generation is outside both timers. It models the old fresh CPU staging copy and
the new resident comparison + persistent CPU copy. Comparisons read both arrays;
this can cost more CPU than a fresh memcpy even when copying fewer bytes. It omits
Metal buffer allocation, mapped-buffer costs, asynchronous reuse, GPU transfer and
submission. The baseline/candidate native gate must judge the net frame cost. The
CPU comparison overhead is a concrete remaining risk, not a claimed zero-cost skip.
Earlier block-trial results remain in `micro-render-coarse-trials.json` and the
initial evidence; they are not substituted for this final protocol.

| Existing headless bench (median of **five paired runs**) | Before | Candidate | Delta |
|---|---:|---:|---:|
| 10 MB exact-highlight/minimap scroll p50 | 0.302354 ms | 0.297354 ms | -1.65% |
| Same scroll p95 | 0.456937 ms | 0.452998 ms | -0.86% |
| Same scroll p99 | 0.833256 ms | 0.788102 ms | -5.42% |
| Highlighted typing p50 | 1.245417 ms | 1.222063 ms | -1.88% |
| Highlighted typing p95 | 2.294462 ms | 2.195900 ms | -4.30% |
| Highlighted typing p99 | 3.754905 ms | 4.711919 ms | **+25.49% FAIL** |
| Plain typing, one caret, p50 | 0.270917 ms | 0.281813 ms | +4.02% |
| Same, p95 | 0.500088 ms | 0.419700 ms | -16.07% |
| Plain typing, 1000 carets, p50 | 5.627917 ms | 5.914771 ms | **+5.10% FAIL** |
| Same, p95 | 15.565987 ms | 18.809523 ms | **+20.84% FAIL** |

The highlighted edit-only/scene-only p99s also failed (+48.20%/+29.58%);
`headless-paired.json` preserves every tracked percentile and all five trial values.
The highlighted view benchmark measures `View::scene`, and excludes native
`App::scene` chrome, font rasterization, uploads, drawable acquisition and scanout.
A negative delta here cannot validate those native costs.

The same-binary A/A control uses the **identical SHA-256 baseline executable** for
both labels. Its 1000-caret p99 was 127.776 → 152.700 ms (**+19.51% FAIL**), while
p95 was 70.411 → 71.847 ms (+2.04%). Those absolute times also differ markedly from
the five-pair campaign above. This demonstrates substantial host/run variability;
it does **not** excuse or turn the candidate's failed comparisons into a PASS.
No more repeated runs were used to search for a passing result. Reproduce the
control with:

```sh
python3 -I tools/scroll-gate/paired-headless.py \
  --base /tmp/velocity-scroll-round14/velocity-base \
  --candidate /tmp/velocity-scroll-round14/velocity-base \
  --pairs 3 --bench typing1 --bench typing1000 \
  --output /tmp/velocity-scroll-control-repeat
```

## Validation and reviewer handoff

All on the final Rust source:

- `cargo fmt --all --check`: PASS.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo test --workspace --release`: PASS, **529 tests / 105 suites**, no failures.
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps`: PASS.
- `python3 -I tools/scroll-gate/analyze.py --self-test`: PASS (seven tests).
- `tools/scroll-gate/run.zsh --self-test`: PASS (analyzer plus three driver checks,
  including stale-pid filtering and fallback termination after a stuck quit helper).
- `zsh -n tools/scroll-gate/run.zsh` and Python AST parsing: PASS.
- `tools/scroll-gate/run.zsh --build-poster`: PASS, no warnings, **compile only**.

No GUI, Metal device, native event posting, or packaging script was run. Native
App::scene/Renderer::render sampling, staging-belt Metal validation, and all six
real-event gate cells remain for the reviewer outside the sandbox. README.md
contains the exact build/run/report commands and permissions. The generated native
report enforces the specified absolute budgets and ≤5% paired regressions; this
candidate is not labeled accepted until those requirements actually pass.

## Reviewer decision (2026-11-16)

The dirty-range instance upload (`vel-render/src/upload.rs`, staging belt in `gpu.rs`) was
removed before merging: its own CPU model costs 47-69% more per frame in the common unchanged and
indicator-only cases, and no native measurement showed a net gain. The indexed LRU layout cache and
the O(1) published-snapshot completeness flag were kept. The typing p99 / 1,000-caret p95 numbers
above were re-measured by the reviewer on the merged candidate (see the merge commit message).
