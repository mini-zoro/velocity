# Real-event native scroll gate

Candidate performance acceptance is **not cleared**. See [RESULTS.md](RESULTS.md)
for passing correctness checks, measured regressions and the same-binary control.

Run from the repository root on an unlocked macOS desktop. The driver builds a Swift
pixel-scroll poster, launches **interactive** Velocity with a fresh data directory for
every trial, lets it settle, posts HID events, collects `sample <pid> 4` during scrolling,
requests normal application termination, and analyzes the saved evidence. It never uses
`--bench-scroll`, timer-injected actions, or the packaging script.

The default matrix is 10 MB Rust (`spikes/data/rust-10mb.rs`), its first approximately
1 MB of normal source, and a generated 200-line Rust file, each with finger-only and
finger + momentum gestures. There are three runs per binary per matrix cell: adjacent
base/candidate pairs in AB, BA, AB order (36 launches total). All paths are resolved
before launch. Results survive failed runs; an incomplete manifest cannot pass.

## Reviewer commands

The local pre-change binary is saved at `/tmp/velocity-scroll-round14/velocity-base`
(source HEAD `4b65f50e2df98be1ffeb3cdd699e5713ed46e046`). To use another baseline,
substitute its absolute binary path and its compile-time legacy output directory.
No commit or packaging step is required.

```sh
cd /Users/zoro/dev/velocity-lane-b
# Optional explicit build; the run command also builds/rebuilds it automatically.
tools/scroll-gate/run.zsh --build-poster
cargo build --release -p velocity

# One command builds the poster if needed, runs all pairs, and writes both reports.
# Select a NEW output directory; an existing one is deliberately rejected.
tools/scroll-gate/run.zsh \
  --base /tmp/velocity-scroll-round14/velocity-base \
  --candidate "$PWD/target/release/velocity" \
  --legacy-output-dir "$PWD/crates/velocity/output" \
  --refresh-hz 144 --input-hz 144 --duration 10 --runs 3 \
  --width 3200 --height 1200 --output /tmp/velocity-scroll-native-r14

cat /tmp/velocity-scroll-native-r14/report.md
# Recompute from preserved evidence without launching any application:
python3 -I tools/scroll-gate/analyze.py \
  --manifest /tmp/velocity-scroll-native-r14/manifest.json \
  --report /tmp/velocity-scroll-native-r14/report.json
```

Use window dimensions that fit the tested display, in logical points; record the actual
display mode and scale when reviewing. The defaults are 1600×1000 points, 144 Hz display,
144 Hz input, 10 seconds, five seconds of settling **after** startup telemetry appears.
Pass the actual display refresh with `--refresh-hz`; it is an operator-supplied value,
not a measured HID or scanout rate. Increase `--settle` to let background highlighting
finish on slower hardware. `--workload rust-10mb --gesture finger --runs 1` selects a
short diagnostic run. It does not substitute for the full gate matrix.

## Permissions and event ownership

In System Settings → Privacy & Security, grant **Accessibility** to the built poster
(`tools/scroll-gate/.build/scrollhid`) and the invoking Terminal/iTerm/Codex application.
Enable **Input Monitoring** for them if macOS requires it for synthetic HID events.
Reauthorize a rebuilt poster if macOS invalidates its permission. `sample` must also
be permitted to inspect the launched process. Do not run this in the sandbox.

The poster activates the supplied pid and locates a normal on-screen window belonging
to it. Its default target is 45% across, halfway down that window, away from the tabs
and minimap. `--x X --y Y` overrides this in global Quartz screen coordinates; pass both.
Before **every** event, it checks the frontmost application and the topmost visible
window under the point. A wrong owner, missing permissions, or invalid target exits 2.
Do not move another window over the target while measuring.

Movement events use `.pixel` units, `isContinuous=1`, and `.cghidEventTap`, with finger
phases began/changed/ended (1/2/4) and momentum begin/continue/end (1/2/3). Each second
is a gesture, with alternating directions so the 200-line file keeps scrolling;
momentum occupies the final 35% of a cycle and decays. Movement slots use an absolute
monotonic deadline at the requested rate; zero-delta phase boundaries are additional
events. Poster logs retain delivered movement count, elapsed time, missed slots and
maximum lateness. More than 1% late slots, a >50 ms deadline miss, or an incorrect
duration/count invalidates the evidence (exit 2).

## Telemetry and measurement boundaries

Each launch receives its own `$VELOCITY_LOG_DIR`. The driver selects the one JSONL
whose embedded pid matches that launch. For old binaries that ignore the variable,
`--legacy-output-dir PATH` is the compile-time `crates/velocity/output` directory;
`--base-legacy-output-dir` and `--candidate-legacy-output-dir` override it separately.
The matching file is copied, never moved/deleted. A fresh `--data-dir` isolates sessions,
settings and IPC. Each trial retains command/environment, stdout/stderr, telemetry
origin, `events.jsonl`, `poster.jsonl`, and `main.sample.txt`.

The analyzer needs only Python's standard library and runs under `python3 -I`:

- Frames: successful input-correlated submissions inside the poster's 10-second
  window; startup/settling and termination are excluded. Scripted benchmark telemetry,
  app errors, non-monotonic timestamps and missing samples invalidate the run.
- Receipt→submit p50/p95/p99: `receipt_to_submit_ms`, without rounding before checks.
- Frame interval p50/p95/p99: explicit `submit_main_ms` when available. Existing builds
  only emit `input_latency.logged_main_ms`, so those intervals are explicitly labeled
  **legacy submit-log proxies**, including logging/present-return scheduling jitter.
  These are not actual scanout measurements. An optional `native_scroll_frame` record
  with the same latency and explicit submit fields takes precedence over legacy records.
- The poster window aligns with the UNIX-microsecond prefix of Reporter `run_id`.
  Legacy reporters create this just after main entry, leaving a small launch alignment
  offset (about 3 ms locally); reports disclose that boundary. Leading/trailing quiet
  intervals enter percentile and gap calculations so a frozen tail cannot disappear.
- Gaps: counts strictly greater than 20 and 50 ms, including those quiet edges.
- Main-thread busy: `(first_thread_samples - run_loop_mach_msg_samples) / first_thread_samples`.
  Only disjoint `mach_msg` receive subtrees directly under the run-loop receive path
  count as idle. Nested mach wrappers count once. Other threads and the flat symbol
  summary are ignored; drawable, GPU and semaphore waits remain busy. The sample
  covers four seconds within scrolling, not the entire ten-second interval.
- Display refresh can instead come from a JSONL `display_refresh_hz`/`refresh_hz` field
  when analyzing a single capture without `--refresh-hz`.

## Thresholds and paired report

At 144 Hz display/input and 10 seconds, ideal = 1,440 frames:

| Metric | Absolute candidate limit, **every** trial |
|---|---:|
| Input frames | ≥1,368 (95% of ideal) |
| Frame interval p99 | ≤13.888889 ms (two refresh intervals) |
| Gaps >50 ms | 0 |
| Receipt→submit p95 | ≤4 ms |
| Main-thread busy | ≤35% |

Other input/display rates use `duration × min(input_hz, display_refresh_hz)` ideal
frames and `2000 / display_refresh_hz` ms for the interval limit. Gaps >20 ms are
reported without an additional absolute ceiling. Every metric is reported separately
per workload/gesture; the six workloads are never pooled. Medians across paired runs
produce base/candidate values and percentage deltas. Each latency/interval percentile,
gap count and busy percentage permits at most a 5% increase; frames permit at most a
5% decrease. A zero baseline allows only zero for a lower-is-better metric; a new
nonzero value fails, with `delta_pct: null` rather than infinity in JSON.

Exit status is 0 for PASS, 1 for a measured gate/regression failure, and 2 for missing,
invalid, refused, or incomplete evidence. Inspect per-run checks in `report.json` as
well as medians in `report.md`. A headless PASS validates the analyzer, not native
scroll performance or Metal correctness.

## Headless checks and microbenchmarks

```sh
python3 -I tools/scroll-gate/analyze.py --self-test
tools/scroll-gate/run.zsh --self-test
cargo run --release -p vel-render --example scroll_costs
cargo run --release -p vel-highlight --example completeness_bench
python3 -I tools/scroll-gate/paired-headless.py \
  --base /tmp/velocity-scroll-round14/velocity-base \
  --candidate "$PWD/target/release/velocity" --pairs 5 \
  --output /tmp/velocity-scroll-headless-repeat
```

Fixture checks cover the first-thread idle calculation (including a busy drawable
mach wait and a misleading second-thread/flat summary), percentiles, exact limits,
zero baselines, the 1,440-frame window, frozen tails, incomplete pairs, and rejected
poster counts. Driver checks cover alternating trial order, workload generation and
pid-based legacy discovery. Swift is compiled but never launched by headless checks.
Cleanup checks also cover falling back to process termination when the quit helper stalls.

The renderer microbenchmark keeps the old eviction scan as its baseline and alternates
nine pairs. Upload results are **CPU allocation/copy models**, not measurements of
Metal staging or submit latency. The completeness microbenchmark uses the actual
10,000,000-byte fixture's line count. `RESULTS.md` records measured evidence and the
remaining reviewer checks.
