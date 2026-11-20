# Velocity vs. Sublime Text: public baselines

Measured 2026-11-20 on Apple M1 Pro, 16 GB, arm64 macOS 26.1 (25B78), with a locked,
optimized Rust 1.99 build from lane C (`efe8b5b` plus changes in this worktree).
No Sublime executable, application bundle or package was downloaded or run. No
window, renderer, drawable or GPU workload was started in this round.

The [PLAN](PLAN.md#13-公开的性能基线) asks for at most 1.2× ST4 on the **same
machine and file**, and for lower costs on huge files and long lines. Public data
cannot establish that target here. Where shown, ratios are **descriptive**
Velocity p50 / public value for approximately corresponding metrics, not
hardware-normalized. A dash means a ratio would be misleading. Latencies below
are **p50 / p95** unless otherwise indicated; memory in decimal MB.
Single-observation metrics use one sample per process. Event latency pools
the 200 measured events from each process; percentiles use linear
interpolation. LA is the range of one-minute values from `uptime` immediately
before launching the corresponding processes. Shared load was substantial;
the workspace gate overlapped parts of the final suite, so wall-time tails
should not be read as a quiet-machine result.

## Reviewer correction (2026-11-20, read this first)

From 11:28 the reviewer demoted the lane processes with `taskpolicy -b` to run GUI
benchmarks on the same machine; child processes inherited background QoS (efficiency
cores, throttled). **Every wall time in the table below that was measured by this
lane after 11:28 is inflated and must not be compared with Sublime.** Retired
instruction counts are unaffected. Re-measured by the reviewer at normal priority
(`/tmp/r14-review/c27`, round 27 binary; machine still busy, LA 95–103, so CPU time is
the better guide):

| Workload (`--headless-perf-highlight`) | Warm pass, best of 3 | Process CPU (2 passes + startup) | Instructions | Per pass, approx. |
|---|---:|---:|---:|---:|
| jQuery 2.1.4 (9,211 lines) | 371 ms | 0.65–0.79 s | 6.62B | ≈ 0.3 s CPU ≈ 30k lines/s |
| jQuery 3.7.1 (10,717 lines) | 866 ms | 0.79–0.81 s | 6.94B | ≈ 0.35 s CPU |
| SQLite 3.8.7.1 `sqlite3.c` (151,639 lines) | 1,919 ms | 2.02–2.08 s | 14.25B | ≈ 0.9 s CPU ≈ 160k lines/s |

So full JavaScript highlighting is roughly **3–4× ST3's published 98 ms for jQuery 2.1**
(not 77×); C highlighting is fast. The JavaScript grammar's hot contexts remain the main
highlighting gap. Quiet-machine wall times for the rows below (startup proxy, Find in Files,
large-file open, long-line typing) still need to be re-measured.

## Comparison

| Metric | Sublime public value (source, hardware) | Velocity (M1 Pro, macOS 26.1) | Ratio | Comparability | PLAN verdict |
|---|---|---|---:|---|---|
| jQuery 2.1.4, full highlight | 98 ms (ST3 dev, [syntect](https://docs.rs/crate/syntect/5.2.0#performance), 2012 MBP) | Warm 7,583 / 9,593 ms (1.2k lines/s p50); first 7,518 / 9,873 ms; LA 10.72–14.79 | 77.4× | Similar jQuery 2.1 workload; legacy build, different hardware and load | Behind historical reference; ST4 target unverified |
| jQuery 3.7.1, full highlight | No same-file value | Warm 4,924 / 9,129 ms (2.2k lines/s p50); first 6,702 / 8,798 ms; LA 10.33–14.92 | — | Not comparable to the 2.1 timing | Baseline only |
| Startup, main to first scene | ~400 ms fresh-install mean, [MonoVM](https://monovm.com/blog/best-text-editors/#performance-and-benchmarks) (i5-12400); 557.7 ms one first paint, [ST4 4200 user log](https://github.com/sublimehq/sublime_text/issues/6853) (M4) | Blank 1201.6 / 2255.3 ms; LA 9.98–16.04; 5k 1186.7 / 1562.6 ms; LA 10.14–16.04 | — | Proxy; no GUI or controlled cold caches | Native target unverified |
| Settled headless memory | ~80 MB RAM, [MonoVM](https://monovm.com/blog/best-text-editors/#performance-and-benchmarks) (i5-12400); counter and idle workload unclear | Blank footprint 27.5 / 27.6 MB, RSS 48.1 / 48.2 MB; 5k footprint 30.9 / 31.4 MB, RSS 52.0 / 52.5 MB | — | Proxy; no renderer or plugin host | Native target unverified |
| 1M lines, 29 MB | 259.6 MB Activity Monitor; usability pass in 30 s, [Peeknext](https://peeknext.com/benchmarks/) (M2 Pro) | First 704.7 / 857.9 ms; full 702.2 / 850.9 ms; settled footprint 74.7 / 74.8 MB, RSS 98.3 / 99.4 MB; LA 9.79–16.19 | 0.29× footprint | Similar size/language; different bytes, process and highlight policy | Lower memory proxy; equivalent usability unverified |
| 1M lines, 29 MB typing | 31.6 ms display p95, [Peeknext](https://peeknext.com/benchmarks/) (M2 Pro) | Input to scene 0.390 / 16.945 ms; LA 9.79–16.19 | — | Not comparable | Display latency unverified |
| 5M lines, 145 MB | 1.10 GB Activity Monitor; usability pass in 30 s, [Peeknext](https://peeknext.com/benchmarks/) (M2 Pro) | First 63.6 / 130.1 ms; full 5.24 / 8.14 s; settled footprint 202.3 / 202.3 MB, RSS 209.1 / 226.2 MB; LA 9.79–16.19 | 0.18× footprint | Similar size/language; different bytes, process and highlight policy | Lower memory proxy; equivalent usability unverified |
| 5M lines, 145 MB typing | 31.8 ms display p95, [Peeknext](https://peeknext.com/benchmarks/) (M2 Pro) | Input to scene 0.453 / 17.866 ms; LA 9.79–16.19 | — | Not comparable | Display latency unverified |
| `sqlite3.c` input to scene | 18.1 / 30.0 ms to pixel change, [Peeknext](https://peeknext.com/benchmarks/) (M2 Pro) | Typing 1.680 / 35.016 ms; first scene from open 232.5 / 286.8 ms; full 230.9 / 285.7 ms; LA 10.14–16.04 | — | Not comparable endpoints; official SQLite minus two spaces | Display latency unverified |
| `gcc.c` input to scene | 20.2 / 30.4 ms to pixel change, [Peeknext](https://peeknext.com/benchmarks/) (M2 Pro) | Typing 0.924 / 36.694 ms; first scene from open 785.2 / 1274.7 ms; full 679.2 / 1038.6 ms; LA 10.21–16.47 | — | Not comparable endpoints; exact GCC file, different autocomplete | Display latency unverified |
| 10k single line | [Qualitative only](https://forum.sublimetext.com/t/how-to-determine-if-sublime-is-slow-vs-plugins/25618/4); no ST4 timings | Typing 2.323 / 24.288 ms; scroll 1.134 / 19.478 ms; LA 9.57–16.26 | — | Not comparable | No ST4 comparison |
| 100k single line | [Qualitative only](https://forum.sublimetext.com/t/how-to-determine-if-sublime-is-slow-vs-plugins/25618/4); no ST4 timings | Typing 0.098 / 1.320 ms; scroll 0.039 / 0.205 ms; LA 9.57–16.06 | — | Not comparable | No ST4 comparison |
| 1M single line | [Qualitative only](https://forum.sublimetext.com/t/how-to-determine-if-sublime-is-slow-vs-plugins/25618/4); no ST4 timings | Typing 0.095 / 6.589 ms; scroll 0.041 / 0.169 ms; LA 9.57–16.06 | — | Not comparable | No ST4 comparison |
| 50 MB single line | [Qualitative only](https://forum.sublimetext.com/t/how-to-determine-if-sublime-is-slow-vs-plugins/25618/4); no ST4 timings | Typing 0.200 / 11.267 ms; scroll 0.045 / 0.201 ms; LA 9.68–16.06 | — | Not comparable | Local typing p95 missed 4 ms; scroll met 2 ms; ST4 unverified |
| 250 MB plain log open | ~3 s, [MonoVM](https://monovm.com/blog/best-text-editors/#performance-and-benchmarks) (i5-12400); unpublished bytes/end condition | First 160.5 / 284.5 ms; full 9.93 / 13.81 s; settled footprint 308.1 / 308.3 MB, RSS 291.7 / 328.7 MB; LA 10.03–17.17 | — | Proxy; synthetic log, different system and endpoint | Fast prefix; full-open parity unverified |
| Find in Files | No credible ST4 timing | 23.13 / 28.74 s; 35,857 searched, 162,864 result lines; LA 10.64–17.63 | — | Not comparable | Baseline only |
| Goto Anything, 100k prepared paths | No credible ST4 timing | `srcmainrs` 57.3 / 64.7 ms; `tokioruntime` 22.4 / 43.2 ms; LA 10.61–18.03 | — | Not comparable | Baseline only |
| `sqlite3.c`, full highlight | Not published | Warm 21.05 / 33.18 s (7.2k lines/s p50); first 29.53 / 34.50 s; LA 10.63–18.25 | — | Not comparable | Baseline only |
| 10 MB Rust, full highlight | Not published | Warm 74.77 / 110.67 s (4.0k lines/s p50); first 94.17 / 101.49 s; LA 10.87–15.22 | — | Not comparable | Baseline only |
| 10 MB JavaScript, full highlight | Not published | Warm 86.89 / 100.95 s (3.2k lines/s p50); first 73.31 / 135.82 s; LA 10.04–16.87 | — | Not comparable | Baseline only |
| Typing, input to present-return | No same-endpoint public value | Current not measured; [P0 Fifo prototype](../prototypes/macos-shell/RESULTS.md) 1.748 / 3.188 ms at 2400×1600 on Rust; one run, 500 events, LA 5.7–8 | — | Not comparable to pixel change or physical photons | Current native budget unverified |
| Near-4K GPU render | 3 ms full render at 4K ([Sublime HQ](https://www.sublimetext.com/blog/articles/hardware-accelerated-rendering), RX560 / 2080 Ti) | Current not measured; [P0 Fifo prototype](../prototypes/macos-shell/RESULTS.md) render pass 1.566 / 2.109 ms at 4096×2178; one run, 600 frames, LA 5.7–8 | — | Similar scale; different stage and hardware | Current native budget unverified |

**English.** Velocity's large-file mode uses substantially less memory in
this headless exercise, but it does less highlighting work. Full-file
JavaScript highlighting remains behind the historical ST3 reference. The
public data and headless proxies do not establish any same-machine ST4 parity:
native startup, display latency, GPU time and equivalent huge-file usability
are still open.

**中文。** Velocity 的大文件模式在本次无窗口测试中明显更省内存，但没有做与
Sublime 相同的全文件高亮。JavaScript 全量高亮仍落后于历史 ST3 参考值。
公开数据与本机的 CPU 代理不足以证明任何项目与 ST4 同机持平；原生启动、
按键到屏幕、GPU 时间以及等效的大文件可用性仍待验证。

## Source audit

- **Peeknext (formerly Koi):** [benchmarks, updated 2026-08-13](https://peeknext.com/benchmarks/).
  The competitor tested ST4 build 4200, Mac mini M2 Pro, 16 GB, macOS 15.7.5,
  60 Hz. Keypress.sh measures synthetic keypress to software-observed pixel
  change; 200 appearances/disappearances, 196 retained. This is **not physical
  key-to-photon**. Neither our CPU scene nor a present-return timestamp measures
  scanout or photons. Physical key-to-photon needs instrumented input and a
  display plus a high-speed camera/photodiode; reproducing Peeknext itself needs
  its display-based software protocol. GCC runs disabled Sublime autocomplete.
- Peeknext's large-file test uses generated C, 29 MB/1M lines and 145 MB/5M
  lines. ST4 passes those and fails the 10M-line usability check (30 s limit,
  syntax working throughout, navigation and edits). Open times within that
  limit are **not reported**. Settled memory was the Activity Monitor **Memory**
  column after editing and navigation, **not RSS**. The published numbers do
  not establish how ST4 stores or maps files.
- **Startup/memory/open:** [MonoVM, 2026-04-27](https://monovm.com/blog/best-text-editors/#performance-and-benchmarks)
  reports a three-run fresh-install startup mean, approximate RAM and a
  250 MB plain log open on i5-12400, 16 GB, NVMe, Ubuntu 24.04. No ST build,
  exact end condition, cache-control protocol or RAM counter is specified; the
  article also describes its memory workload as a 50 MB log elsewhere.
  Consider it a low-confidence cross-platform reference, not a verified ST4
  idle baseline. A single [ST4 build 4200 user console log, 2026-02-21](https://github.com/sublimehq/sublime_text/issues/6853)
  on an Apple M4/macOS 26.3 gives 544.944 ms internal startup / 557.650 ms
  first paint, indexing off, OpenGL on; cache/session state and repetitions
  unknown. No defensible cold 50–80 ms, idle 55–80 MB range or 1.4 s endpoint
  with enough provenance to use as a target was located.
- **Long lines:** [Sublime staff forum reply, 2017-01-18](https://forum.sublimetext.com/t/how-to-determine-if-sublime-is-slow-vs-plugins/25618/4)
  describes lag at tens of thousands of characters, especially with multiple
  cursors. It is qualitative, predates ST4 and gives no line distribution,
  machine or timings. There is no verified ST4 10k cutoff here.
- **Highlight:** [syntect 5.2 README](https://docs.rs/crate/syntect/5.2.0#performance)
  attributes 98 ms for highlighting jQuery 2.1 to an ST3 dev build, mid-2012
  15-inch Retina MacBook Pro; ~200 ms click to pixels is separately reported.
  The historical measurement date, jQuery patch release, iteration count and
  exact instrumentation are not given (README evidence is archival, not ST4).
  9.2k lines in 98 ms is about 94k lines/s. No equivalent public full-file ST4
  parse-and-color run was found.
- **Rendering:** [Sublime HQ, 2021-04-28](https://www.sublimetext.com/blog/articles/hardware-accelerated-rendering)
  reports full OpenGL render time, not a GPU pass percentile: 3 ms at 4K on
  RX560/Ubuntu 20.04 and 2080 Ti/Windows 10. The Intel HD 6000/macOS 11.1
  result is **18 ms at 4K**, **9 ms at 1440p**. 8K/3 ms is reported for the
  Windows 2080 Ti, not the RX560. Window scene details and repetitions are
  unspecified.
- No defensible ST4 timings were found for Find in Files or Goto Anything,
  nor comparable memory data with many tabs. A [Find in Files thread](https://forum.sublimetext.com/t/string-search-in-100-000-files-is-slow/62511)
  reports an ST3 result, then confirms ST4 became parallel in build 4130;
  it gives no ST4 timing. A [build 4180 memory report, 2024-10-18](https://forum.sublimetext.com/t/sublime-text-4180-memory-leak-and-high-usage/73899)
  measures Windows Task Manager commit on two JS files, not macOS resident
  memory or many tabs. Neither is used as a numeric comparator.

## Workloads and reproduction

`scripts/prepare-perf-corpus.py` pins the two CDN jQuery releases, the official
SQLite 3.8.7.1 amalgamation and the public GCC workload with SHA256. The generated
fixtures and their hashes are in
`spikes/data/sublime-comparison/manifest.json` (shared and git-ignored). The official
`sqlite3.c` differs from the [Peeknext copy](https://github.com/peeknext/keypress/blob/main/test_files/sqlite3.c)
by just two trailing spaces at the end of the opening comment; GCC is the
[same published file](https://github.com/peeknext/keypress/blob/main/test_files/gcc.c).
jQuery 2.1.4 (247,597 bytes, 9,210 newline records) is the closer workload;
3.7.1 (285,314 bytes, 10,716 newline records) is the requested 3.x fixture.
The 10 MB inputs are the [spike Rust/JS corpora](../spikes/RESULTS.md#输入数据).
Velocity counts an empty trailing line; e.g. the generated C files are exactly
1M/5M LF-terminated records and 29/145 MB, with different source from Peeknext.

- Highlighting times include a fresh `DocumentHighlighter`, all lines,
  exact scope transitions, styling and incremental checkpoints; excludes file
  I/O and asset loading. The second pass warms shared grammar/regex data but
  creates a new highlighter (no retained document transitions or styled lines).
- Headless startup is from the first timestamp in `main` through the first
  `App::scene` return, not dyld, AppKit window creation or display. A blank
  run and a 5,000-line bundled Rust run are separate. Fresh process with warmed
  asset/disk caches, not a cold GUI launch. Idle footprint is sampled after a
  150 ms polling/settling interval; it includes no GPU, plugin host or real
  user profile.
- Streamed open starts with a blank, warmed workspace and stops at the first
  nonempty scene, and separately at complete text installation; the poll sleeps
  0.5 ms between checks (scheduler latency can be longer). First scene can be
  a prefix, plain or partially highlighted. Settled memory is after visiting
  the tail/start/middle, typing, scrolling and a short settle. Large-file policy
  may restrict syntax/analysis to a viewport; this is not a whole-file parse
  and cannot pass Peeknext's all-lines highlighting criterion.
- Long-line fixtures are single ASCII lines with horizontal scroll, no wrap;
  larger sizes warm the long-line index. Typing is synthetic text insertion
  through the editor state, caret reveal and `App::scene`; scroll is a scene
  at each position. These are CPU numbers, not native event, submit or
  present-return. 20 warmups and 200 observations per process; no dropped
  observations in the measured range. The 10k fixture deliberately runs below
  the 16 KiB long-line threshold and uses ordinary unwrapped layout.
- Find in Files uses the production worker, case-sensitive literal `pub fn`,
  no context lines, and ordered collection without result UI. Roots were the
  temporary offline copy of this machine's cached crates, the spike's cached
  crates and the existing bundled syntax tree. Goto Anything
  ranks already prepared 100,000 synthetic paths to get the top 50 (no walk
  or preview). Paired retired instructions count **whole processes** including
  setup/teardown, not only the timed interval.

`scripts/bench-headless.py target/release/velocity --find-root DIR [...]`
keeps JSONL, uptime and macOS own-child counters, with five new processes per
case. For headless event series see the aggregation note beside the results.
Use an isolated home and
`RUSTUP_HOME=/Users/zoro/.rustup CARGO_HOME=/Users/zoro/.cargo` for measurement;
builds used an offline temporary copy of the existing Cargo registry to keep
the real Cargo home unchanged. No privileged access was used.

Local raw output is under `/tmp/velocity-lane-c-perf.6cvr9S/`: preliminary
records in `before/`, per-case final records and counters in `final-matrix/`,
the paired jQuery trials in `final-highlight-paired/`, and the interrupted
instruction run plus final instruction attempts in their own directories.
`/usr/bin/time -l` could not query `kern.clockrate` in this sandbox; both the
comparison runner and instruction gate use `proc_pid_rusage(RUSAGE_INFO_V4)`
after exit and before reaping instead.

For the later native tests, run [scripts/bench-gui.sh](../scripts/bench-gui.sh)
from an awake, unobscured macOS desktop. It deliberately creates windows,
isolates profile/logs, repeats every workload five times and reports actual
drawable size (scale/window clamping can prevent an exact 4K run). It reports
Metal present-handler scanout, injection to queue present-return, CPU and GPU
time, and both memory metrics with different endpoints. It was **not run** here.

## Changes and next measurement

Native `sample` could not attach to the headless child in the sandbox. The
existing `regex-bench --bench-only --profile --language javascript` was run
against the full 3.7.1 source in place of the 10 MB JS input. With transition
memoization disabled and the profiler's timers on, the largest contexts by
median summed matching time were `expression-begin` (1,623 ms),
`literal-variable` (1,197 ms), `prototype` (494 ms) and `statement` (444 ms).
The multi-pattern profile registered 37 contexts, 17 native fallbacks and zero
DFA evictions on that input. Those are attribution measurements, not the
unprofiled throughput numbers. The older full-corpus
[round-13 differential](../crates/vel-regex/round13-differential.json) did see
evictions.

The ordered matcher now reuses verified group-zero spans for captureless native
rules, avoiding a second search when another candidate overwrote native
registers. Explicit and optional groups still use the existing path.
`winner_preserves_whole_match_and_optional_groups` checks both paths with
UTF-8 byte offsets. DFA admission now charges actual bytes and does not evict
machines before a small or failed compile; a per-context transient build still
occurs outside the resident cap. `small_machines_do_not_evict_each_other_on_admission`
asserts two compilations, no eviction and the byte bound in a small pool.
The admission fix is not an explanation for jQuery's zero-eviction profile.

In five alternating pairs with the same benchmark harness on both builds,
jQuery 2.1.4 went from **6.875B to 6.816B retired instructions** (median paired
change **−0.86%**) and jQuery 3.7.1 from **7.296B to 7.105B** (**−1.70%**).
Warm p50/p95 wall time was 6,646/9,197 → 7,583/9,593 ms and 6,116/9,637 →
4,924/9,129 ms, respectively. There is no consistent elapsed-time speedup;
the paired work reduction is modest and the absolute gap remains.

In the preliminary 250 MB open trials, whole-process CPU was 2.68–2.96 s
while wall time was 3.40–17.04 s. That includes editing and scene work; it
does not establish bit-for-bit loader performance or a comparable three-second
GUI result. The preliminary watchdog also cut off two 10 MB Rust processes
and three 10 MB JS processes; the final series uses a longer watchdog and
does not pool those incomplete processes.

The final blank startup spent 1,124 ms median in setup versus 77 ms in
`App::scene`; the 5,000-line case spent 1,148 ms versus 39 ms. These are
separate stage medians, not additive estimates. Whole-process median work
was 0.95B and 1.04B instructions, respectively. The 250 MB case took 11.07B
instructions including the edit/scroll exercise; Find in Files took 17.95B
including ordered results, and Goto Anything took 0.94B including path setup.
For the 10k and 100k lines it was 3.53B and 1.37B respectively; the larger
case uses the indexed path, so this is not a single algorithm's scaling curve.

1. **Highlight throughput.** Run same-file pairs on a quiet machine, with
   counters for native rule attempts, failed lookaheads and winner verification.
   Start with conservative identifier/literal filters in `expression-begin`
   and `literal-variable`, without changing regex order, captures or branch
   semantics. Gate with the official grammar assertions, ordered backend
   differentials and both jQuery versions plus C, Rust and 10 MB JS.
2. **Startup and full load.** Split resource discovery, theme/settings
   hydration and grammar asset validation inside startup setup before moving
   work past the first scene. For the 250 MB log, separate file reads, text
   validation, digest, rope construction and final buffer install; retain the
   provisional read-only prefix and full-load correctness tests. Avoid scoring
   a fast prefix as a win over an unknown full-open endpoint.
   For long lines, profile the uncapped ordinary layout below 16 KiB and
   separately time index updates, scene construction and scheduling for 50 MB.
   Recheck with ASCII and Unicode, wrap on and off, before moving the policy
   threshold or claiming the typing budget.
3. **Native parity.** Run the GUI script with a quiet host and record the
   matching warm native floor. For same-file input-to-screen, use the same
   60 Hz display and the same synthetic pixel-change method as Peeknext;
   use an external sensor to measure physical latency. Obtain exact generated
   C bytes and align syntax and memory policy before making a huge-file claim.
   Publish the workload and counters even when only third-party ST4 results
   are available.

## Validation

The focused `vel-regex` release tests and strict workspace Clippy passed.
For the instruction gate against the original `efe8b5b` binary, nine complete
alternating pairs gave **+0.13%** for `smoke1` and **−2.42%** for
`carets1000` (median of paired ratios). Both are below the +1% limit.
On the default 10 MB `features` input, both binaries hit the internal
120-second language-readiness deadline in two attempted pairs. Those runs
are incomplete; there is no valid default-fixture instruction comparison.
On a diagnostic fixture (the first 30,000 Rust corpus lines; 1,037,801
bytes), the one matched pair took 223.93B → 213.69B instructions (**−4.57%**).
The second baseline run hit the 600-second watchdog, so this is not a
regression-gate pass. Repeat the default input with
`scripts/bench-headless.py target/release/velocity --instruction-gate
--before BASE_BINARY --only features` on a quiet host.

The full `/tmp/r14-review/gate.sh` run passed formatting and strict Clippy,
but exited at the workspace tests: **1,605 passed, 8 failed**
(`cargo test` exit 101). All 242 pinned syntax fixtures passed the assertion
gate, and the Onig/multi/memoized differential passed on 193,704 lines.
Rustdoc was not reached. In serial reruns on the same build:

- The two build-cancellation tests and the installed-archive grammar reload
  passed.
- The two sustained-build-output tests still fell short of the prescribed
  poll count. The zh-Hans draw-list audit still flagged temporary path
  components, and the loose user-grammar reload still hit its readiness
  deadline.
- The 4 MiB **wrapped** JSON test exceeded its 600 ms typing limit both in the
  suite (1.017 s) and in isolation (1.298 s). It is a different workload from
  the unwrapped long-line rows above. The gate failure does not establish a
  new regression against `efe8b5b`; these tests need baseline reruns under
  comparable load before attribution.

The full test log is `/tmp/r14-review/gate-velocity-lane-c.log`. Individual
reruns are in `targeted-*-rerun.log` alongside the timing data.
