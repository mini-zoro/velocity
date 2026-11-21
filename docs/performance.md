# Velocity vs. Sublime Text: public baselines

Measured 2026-11-20 on Apple M1 Pro, 16 GB, arm64 macOS 26.1 (25B78), with
locked, optimized Rust 1.99 builds. Round 30 compares against `6cf9be9` in
its own section below. The source comparison table uses the round 29
candidate and a same-build `4dcbc9c` baseline for highlighting; other
rows are historical round 27 observations (`efe8b5b` plus the round 27 changes).
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

## Reviewer correction and round 29 re-measurement

From 11:28 the reviewer demoted the lane processes with `taskpolicy -b` to run GUI
benchmarks on the same machine; child processes inherited background QoS (efficiency
cores, throttled). **Round 27 wall times measured by this lane after 11:28 are
inflated and must not be compared with Sublime.** Retired
instruction counts are unaffected. Re-measured by the reviewer at normal priority
(`/tmp/r14-review/c27`, round 27 binary; machine still busy, LA 95–103, so CPU time is
the better guide):

| Workload (`--headless-perf-highlight`) | Warm pass, best of 3 | Process CPU (2 passes + startup) | Instructions | Per pass, approx. |
|---|---:|---:|---:|---:|
| jQuery 2.1.4 (9,211 lines) | 371 ms | 0.65–0.79 s | 6.62B | ≈ 0.3 s CPU ≈ 30k lines/s |
| jQuery 3.7.1 (10,717 lines) | 866 ms | 0.79–0.81 s | 6.94B | ≈ 0.35 s CPU |
| SQLite 3.8.7.1 `sqlite3.c` (151,639 lines) | 1,919 ms | 2.02–2.08 s | 14.25B | ≈ 0.9 s CPU ≈ 160k lines/s |

Round 27's jQuery 2.1 per-pass CPU estimate was roughly **3–4× ST3's
published 98 ms** (not 77×). This was a rough historical comparison, not a
same-machine ST4 result. Quiet-machine wall times for the remaining historical
rows (startup proxy, Find in Files, large-file open, long-line typing) still
need to be re-measured.

Round 29 alternated five fresh processes per side for each workload. Both
binaries used `cargo build --release --locked -j 6 -p velocity` and
`MACOSX_DEPLOYMENT_TARGET=14.0`; neither was package-built. Runs used disposable
homes, zsh, and temp/log roots. Cargo builds used an offline snapshot of the
registry rather than writing to the real Cargo home. Warm-up populated the
same disposable grammar cache; each measured invocation runs a first and a
warm full pass with fresh document state. Darwin own-child retired instructions
and `wait4` CPU user time include **both passes, startup, and teardown**.
The wall-throughput column is the median second pass, not CPU throughput;
one-minute load averages ranged from 15.25 to 108.02. Percent changes are
medians of paired ratios.

| Full highlight | Lines | Instructions, before → after | Paired change | CPU user s, before → after | Warm k lines/s, before → after |
|---|---:|---:|---:|---:|---:|
| jQuery 2.1.4 | 9,211 | 6.691B → 5.143B | −23.09% | 0.845 → 0.616 | 19.0 → 25.9 |
| jQuery 3.7.1 | 10,717 | 6.994B → 5.369B | −23.53% | 0.812 → 0.658 | 23.9 → 30.2 |
| SQLite `sqlite3.c` | 151,639 | 14.363B → 14.247B | −0.86% | 1.974 → 2.028 | 126.3 → 146.8 |
| `rust-10mb.rs` | 297,030 | 61.028B → 60.324B | −1.29% | 7.375 → 7.961 | 80.7 → 65.0 |
| `javascript-10mb.js` | 281,484 | 64.527B → 49.265B | −23.54% | 8.216 → 6.269 | 58.3 → 81.5 |

Both jQuery inputs use about **1.30× fewer instructions**, short of the 2×
target. The non-JavaScript instruction budget held. User CPU rose for SQLite
and the Rust median, and Rust warm wall throughput was lower. The median of
paired CPU ratios was +2.71% for SQLite and +0.21% for Rust; these runs do not
establish a non-JavaScript latency win or rule out a small CPU regression.

## Comparison

| Metric | Sublime public value (source, hardware) | Velocity (M1 Pro, macOS 26.1) | Ratio | Comparability | PLAN verdict |
|---|---|---|---:|---|---|
| jQuery 2.1.4, full highlight | 98 ms (ST3 dev, [syntect](https://docs.rs/crate/syntect/5.2.0#performance), 2012 MBP) | 5.143B instructions and 0.616 s user CPU per process; warm 25.9k lines/s | — | Similar jQuery 2.1 workload; legacy build, different hardware and load | Work reduced; ST4 target unverified |
| jQuery 3.7.1, full highlight | No same-file value | 5.369B instructions and 0.658 s user CPU per process; warm 30.2k lines/s | — | Not comparable to the 2.1 timing | Local before/after only |
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
| `sqlite3.c`, full highlight | Not published | 14.247B instructions and 2.028 s user CPU per process; warm 146.8k lines/s | — | Not comparable | Local before/after only |
| 10 MB Rust, full highlight | Not published | 60.324B instructions and 7.961 s user CPU per process; warm 65.0k lines/s | — | Not comparable | Local before/after only |
| 10 MB JavaScript, full highlight | Not published | 49.265B instructions and 6.269 s user CPU per process; warm 81.5k lines/s | — | Not comparable | Local before/after only |
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

## Native GUI runs

The supplied `/tmp/guibench/FINDINGS.md` records native measurements of
`e163180` on the M1 Pro at 1200×800. In warm runs, Velocity's first
presented callback arrived at about 257 ms but was skipped
(`presentedTime == 0`); the first positive `presentedTime` was 363–410 ms,
about 390 ms median. The native floor's 237 ms median is a callback, not
a verified scanout. In Velocity's traced run, the window was ordered at
251 ms, the first present returned at 256 ms, and foreground activation
was about 19 ms later.

Round 34 still pre-renders offscreen, then orders the window, activates
the app for a foreground launch, and acquires/presents the first drawable
in the same callback. A background launch remains ordered back without
activation. The new `activated_before_first_present` milestone and the
ordering test verify the call sequence, not compositor acceptance. On the
next native run, compare skipped drawable count and first **positive**
`presentedTime` against this trace, and make the floor wait for the same
endpoint before comparing the two. No native window was opened in this round.

The prior one-caret Fifo typing run recorded 0.99/1.35 ms p50/p95
injection to present-return, and 178 MB physical footprint / 229 MB RSS
after typing. These are native endpoints; neither the public Sublime idle
range nor the windowless font probe is like-for-like.

## Round 29 attribution

`sample` could not attach to the headless child in the sandbox. An independent
build with `--features highlight-profile` adds JSONL events for per-context
queries, per-regex native searches and capture searches, candidate scans,
cache hits, resolved branch fails, and the existing highlighter phase timers.
Use the ordinary build for counters and throughput; the diagnostic timers
and tables perturb execution.

Warm jQuery 2.1.4 before the changes: `expression-begin` was 51.6 ms /
11,268 queries, `literal-variable` 49.0 ms / 8,059, `statement` 17.4 ms /
3,764, and `prototype` 15.3 ms / 91,126. Of 197,748 context searches, there were
132,909 multi-candidate calls (49,517 scans and 83,392 reused nominations),
14,689 multi-backend native-only calls, and 50,150 direct small-context calls.
Of the multi-candidate calls, 125,393 had a hybrid fallback list, and 411 had
a rejected nominee. The actual work was 424,048 native span searches and
1,764 native capture searches. The largest
native rules included the identifier/function-assignment lookahead (3,843
calls, 12.8 ms), the reserved-word lookahead (3,826, 11.7 ms),
`(?=\S)` (27,788, 6.1 ms), and the identifier-call lookahead (4,569, 5.5 ms).
jQuery 2.1.4 took 7,861 branch snapshots and performed 51 resolved fails;
jQuery 3.7.1 took 8,229 and performed 49. Neither rewound a previous line.

| Experiment | jQuery 2.1.4 | jQuery 3.7.1 | Decision |
|---|---:|---:|---|
| Broader context-result memoization and nested required literals | 6.657B → 5.317B | 7.038B → 5.556B | SQLite +2.6%; did not keep broad admission |
| Selective memoization, direct nonspace check, and no synthetic branch regex | 6.674B → 5.250B | 7.010B → 5.478B | SQLite +1.59%; needed another pass |
| Exact-start native fallback, narrower admission, less filter setup | 6.704B → 5.198B | 7.044B → 5.420B | SQLite −0.13%; keep exact-start and filter setup |
| Release, without the unused complete-context cache | 6.691B → 5.143B | 6.994B → 5.369B | SQLite −0.86%; Rust and JS corpora pass |

These are cumulative builds, not independent attribution of individual fixes.
The candidate profile recorded no complete-context memo hits on either jQuery
file after narrowing admission, so that additional cache was dropped.
The exact-start probe is only used when a confirmed candidate is already at the
cursor, or a nominated rule is being checked at its nominated start. It retains
the full line for lookaround and never caches an anchored miss as an unbounded
miss. Literal prefilters relax only necessary positive assertions and leave real
regex Unicode escapes alone; no bundled grammar bytes changed.

**Ranked inputs to round 30.** These budgets are elapsed time in one instrumented
warm jQuery 2.1.4 candidate pass (314 ms across all phases). They locate work
that can be reduced; they do not predict instruction savings.

1. **Render reuse and scope allocation (95 ms, 30%).** There were 3,224
   render hits out of 9,211 calls; the 5,987 misses built 21,710 style spans and
   the interner stored 13,533 distinct active stacks. Try sharing operation
   identity between the parse cache and render cache, and make common
   push/pop transitions cheaper without losing the hidden clear/restore stack.
2. **Rejected nominees and native lookaheads (53 ms in actual native span
   searches, 17%).** Native calls fell from 424,048 to 72,810; hybrid calls
   with a fallback list fell from 125,393 to 37,167. Wider projection raised
   rejected nominees from 411 to 7,931. `literal-variable` still accounts for
   32 ms of context time and `expression-begin` about 14 ms. Measure continuing
   the candidate search after a rejected guard instead of doing a full native
   retry each time. Preserve same-start priority and verify all captures.
3. **Parser state and branch snapshots (about 82 ms outside timed context
   searches, 26%, including profiling overhead).** Only 51 of 7,861 branch
   snapshots fail; there are no cross-line rewinds on either jQuery version.
   Test sharing captured trigger text and allocating replay prefixes only
   when needed. The 82 ms is an upper bound for all non-context parse work,
   not a measured branch-only cost.
4. **Line input and checkpoints (15 ms, under 5%).** Only 445 checkpoint
   scope copies were needed. Measure sharing operation storage across the
   parser and renderer before attempting sparser incremental checkpoints.

**Temporary artifacts.** Each temporary index now holds a private,
coordinator-owned directory, with process-exit cleanup for detached workers.
The stale sweep runs from the first `TemporaryIndex::new` on a background
thread behind a process-wide `Once`, not from `main()`. It removes only
regular directories with exact legacy index names, our effective UID, and a
positive PID that returns `ESRCH`; it never follows symlinks. The subprocess
tests cover cancel, exit with a running index, and
stale/live/non-directory/symlink discrimination. Matrix reports go to stderr
by default, and benchmark fixtures use RAII with opt-in workbench replay.
Shared grammar caches and durable save recovery are deliberately not swept.

**Round 29 validation.** The 242-file official assertions and 193,704-line
Onig/multi/memoized differential passed with prefilter verification enabled.
The assertion evidence and serialized operations, corrections, rewinds and
warnings produced identical BLAKE3 digests to `4dcbc9c`. The final same-build
instruction gate passed: `features` −0.30% (227.511B → 226.866B),
`carets1000` −0.15% (4.076B → 4.067B), and `smoke1` +0.17%
(304.666M → 305.010M).

`/tmp/r14-review/gate.sh` passed formatting and strict workspace Clippy.
Workspace release tests: 1,624 passed and one failed. The known
`zh_hans_render_scenes_show_no_unexpected_ascii_copy` audit reported ASCII
components of the sandbox fixture paths. Rustdoc was not reached by the gate.
The temp-index tests and default headless workbench cleanup passed; no index,
matrix or workbench fixture directories remained in the test roots.

Round 29 raw JSONL, own-child counters, and logs: `/tmp/velocity-c29.oe1zlL/`
(`release-highlight`, `release-instruction`, `gate-release-workspace.log`,
`syntax-*-digest.log`, and `profile-*.jsonl`).

## Round 30 highlighting

The parser, checkpoints, and render cache now share immutable operation
sequences. The render cache holds the operation allocation for the life of its
identity key. Only lines that leave the stack's saved clear history unchanged
can hit that cache. UTF-8 offsets and gradients still use the exact renderer
where required. Active stack IDs reuse a single owned scope slice; single
push/pop transitions avoid allocating a group key. Symbol selection evaluates
only selectors with symbol settings, retaining per-setting specificity and
resource order and leaving the general preference resolver alone.

On a rejected DFA nominee, the matcher probes the native rules still eligible
at that exact start. The projected first-byte filter can only exclude rules;
native search supplies the complete winner, including captures. Earlier
native-only rules retain priority. If nothing eligible matches, the DFA
continues one byte later on ASCII input, with a bounded native fallback for
repeated rejection. No bundled syntax bytes changed.

### Step attribution

Two alternating fresh-process pairs per step (retired instructions in
billions, before → after; median paired change). The steps were cumulative
except that the branch experiment was removed. Small swings in the two-pair
smoke measurements are not a final regression gate; neither smoke benchmark
parses or colors a document in its timed typing loop.

| Workload | Share operations and render keys | Scope transitions and symbol selection | Resume rejected nominees | Lazy branch prefixes and shared trigger text (discarded) |
|---|---:|---:|---:|---:|
| jQuery 2.1.4 | 4.904 → 4.843 (−1.24%) | 4.833 → 4.469 (−7.53%) | 4.411 → 4.132 (−6.32%) | 4.130 → 4.099 (−0.74%) |
| jQuery 3.7.1 | 5.087 → 5.054 (−0.64%) | 5.020 → 4.599 (−8.39%) | 4.596 → 4.265 (−7.20%) | 4.267 → 4.228 (−0.92%) |
| `sqlite3.c` | 12.392 → 12.371 (−0.18%) | 12.335 → 12.188 (−1.19%) | 12.131 → 11.688 (−3.65%) | 11.702 → 11.716 (+0.12%) |
| `rust-10mb.rs` | 55.646 → 55.639 (−0.01%) | 55.676 → 55.096 (−1.04%) | 54.950 → 35.966 (−34.55%) | 35.989 → 36.091 (+0.28%) |
| `javascript-10mb.js` | 46.021 → 45.991 (−0.06%) | 46.095 → 45.818 (−0.60%) | 45.707 → 41.817 (−8.51%) | 41.969 → 40.323 (−3.92%) |
| `--headless-view-features` | 215.891 → 219.645 (+1.78%) | 216.161 → 200.738 (−7.15%) | 207.547 → 197.593 (−4.80%) | 197.445 → 193.301 (−2.11%) |
| `--headless-smoke` | 0.320 → 0.307 (−3.77%) | 0.320 → 0.309 (−3.16%) | 0.306 → 0.320 (+4.46%) | 0.319 → 0.305 (−4.38%) |
| `--headless-smoke --carets 1000` | 4.139 → 4.254 (+2.76%) | 4.219 → 4.043 (−4.06%) | 4.092 → 4.118 (+0.63%) | 3.942 → 4.087 (+3.92%) |

The branch experiment fell below the 3% stopping threshold on both jQuery
inputs. No replay or semantic equality changes from that experiment remain.
The 3.35B target was not met.

In a separate instrumented warm jQuery 2.1.4 pass, the distinct active stack
count stayed at 13,533. The interner retains one owned scope slice per exact
stack instead of two, but built style spans rose from 21,710 to 22,370
(3,224 → 3,214 render hits). The span-count goal is still open. Native span
searches fell from 72,810 to 66,296 and native capture searches from 1,268
to 1,142. `literal-variable` and `expression-begin` took 10.1 and 9.6 ms
in the final profile; render still took 56.3 ms of the 188.4 ms timed
phases. These profile timings are diagnostic, not the paired measurement.

### Final comparison with `6cf9be9`

Five alternating pairs per workload. Both builds used Rust 1.99.1,
`cargo build --release --locked -j 6` and `MACOSX_DEPLOYMENT_TARGET=14.0`,
with the same bundled grammar bytes. Cargo used a disposable offline registry
snapshot. Each pair shared a disposable home, grammar cache, log directory,
and a temp root populated with 381 ordinary directories. The load averages
at process launch ranged from 13.74 to 65.35. Counters cover the whole
process, including both passes on the five highlight fixtures. Counts are
side medians; percentages are medians of paired ratios.

| Workload | Retired instructions, before → after | Median paired change |
|---|---:|---:|
| jQuery 2.1.4 | 4.885B → 4.165B | −14.64% |
| jQuery 3.7.1 | 5.115B → 4.333B | −15.34% |
| `sqlite3.c` | 12.346B → 11.766B | −4.95% |
| `rust-10mb.rs` | 55.642B → 36.042B | −35.18% |
| `javascript-10mb.js` | 46.051B → 41.880B | −9.06% |
| `--headless-view-features` | 203.118B → 190.884B | −10.38% |
| `--headless-smoke` | 306.775M → 305.702M | −0.67% |
| `--headless-smoke --carets 1000` | 4.088B → 4.063B | −0.50% |

All eight pass the +1% instruction cap. The 4.165B result is about 1.61×
fewer instructions than the historical 6.69B, not 2×. In the additional
19-pair typing series, instructions changed by +0.26% (one caret) and
−0.19% (1,000 carets). Median paired edit-and-scene p50 changes were
+0.56% and +0.24%; p95 changes were −7.38% and +0.09%, respectively.
The shorter series' paired p50 changes went in the other direction. These
busy-host data do not establish a repeatable typing slowdown.

The final Rust 10 MB runs used 5.99 → 3.97 s user CPU and 18.06B → 12.13B
cycles; warm throughput medians were 91.9k → 137.1k lines/s. Nearly all
of the instruction gain arrived in the rejected-nominee continuation step.
The full 10,000,000-byte Rust fixture passed the native reference comparison
with `--verify-matches`: 297,030 lines, 4,236,666 operations, zero divergences.
Separately, three same-build `4dcbc9c` → `6cf9be9` pairs at load 47–99
did not reproduce the round 29 wall regression: 7.34 → 6.67 s user CPU
and 52.1k → 68.2k warm lines/s. This does not identify the cause of the
prior slowdown. Two quiet `4dcbc9c` → `6cf9be9` pairs, at one-minute
load 3.23–3.86 before launch, also did not reproduce it: 5.52 → 5.08 s
user CPU, 109.2k → 117.8k warm lines/s, and 60.957B → 55.529B retired
instructions. There is no evidence from these quiet pairs that round 29's
prefilter introduced a Rust throughput regression.

### Validation and remaining work

The 242-file official assertion gate and the 193,704-line reference,
multi-matcher and shared-memoized differential passed with prefilter
verification enabled. Both digests exactly match the same-build baseline:

- Assertion evidence: `6c861d72d9008d749f6a4ab5cc47ee2649e3907a8f554cd30eae6474aba7a83a`
- Operations, corrections, rewinds and warnings: `1a53ce33b5bd89ff5b0faa616255e4f5712046273118ec01ff9e6941fec913af`

The workspace gate fallback omitted packaging. Formatting, strict workspace
Clippy, the release build, and rustdoc with warnings denied passed. Unit and
integration tests: **1,624 passed, 1 failed**. The
`shipped_catalogs_pass_draw_list_text_audit` failure flagged only the ASCII
components of the disposable path in the zh-Hans scenes. The first doctest
run overlapped the diagnostic profile build and hit missing build artifacts;
all 13 doctests passed in a serial rerun.

Ranked next work:

1. **Scope and span storage.** Share style-span arrays across equivalent
   visual output without coalescing query-visible scope paths. Measure both
   retained arrays and built spans, not just cache hits.
2. **Native lookaheads.** Attribute the remaining native work in
   `literal-variable` and `expression-begin` to specific guards. Try
   tighter necessary-condition checks; keep the native captures as the
   reference and measure Rust and C as well as both jQuery versions.
3. **Parser state and checkpoints.** Measure state hashing and checkpoint
   retention separately. Lazy replay prefixes and captured-text sharing as
   tested here are below the 3% jQuery cutoff.

Round 30 raw records are in `/tmp/velocity-c30.eYn9Fz/`: the four
`stage-*` directories, `final-pairs/`, `final-typing/`, `rust-exact.json`,
`rust-quiet-final/`, the corpus digest logs, and the workspace gate logs.

## Round 34 performance

The renderer shares arrays keyed by contiguous byte ends and exact styles,
not by scope names. Scope runs, selector and symbol queries, and saved
clear/restore state retain their original identities. For a palette made
entirely of single-scope alternatives, the theme resolver caches each
scope's contribution per style field. It always uses the general selector
resolver for logical/path selectors, unusually long selectors or a stack
deep enough for specificity to saturate.

The native identifier guards can only reject an exact-start probe. They
recognize the expanded assignment, call and reserved-word rules; Unicode,
identifier escapes and uncertain matches still use Oniguruma. There are no
grammar changes. Parser fingerprints reuse the interned stack's content
digest, and speculative replay history is hashed only if a caller needs
equality or a hash. Independently, a dense line checkpoint can use its
exact interned end path by itself when no saved clear is outstanding.

MobileAsset collections are opened for positioned reads. Only the requested
face's table directory and table bytes are retained, with offsets rebased for
Swash. Layout-only advance probes omit outline tables. The original file is
never mapped; a short or changed read fails safely and an already loaded face
owns its bytes. Shaping, hinting, outlines and the CoreText fallback path
are unchanged. Existing sealed fonts are still mapped; custom replaceable
fonts still use the existing snapshot path.

### Paired work

Five alternating fresh-process pairs, with `b740212` as the control.
Both sides used `cargo build --release --locked -j 6 -p velocity
-p vel-render --bin velocity --example font_footprint`,
`MACOSX_DEPLOYMENT_TARGET=14.0` and the bundled languages at the same pin.
The archived control has the same diagnostic software renderer selection
and font probe as the candidate. Builds used a disposable offline Cargo
registry; runs used a fixture HOME, zsh, temp directory and log directory.
Counts are side medians; changes are medians of the paired ratios. Counts
include both full highlight passes, process startup and teardown.

| Full highlight | Before | After | Change |
|---|---:|---:|---:|
| jQuery 2.1.4 | 4.185B | 3.254B | −22.28% |
| jQuery 3.7.1 | 4.360B | 3.384B | −22.45% |
| SQLite `sqlite3.c` | 11.799B | 10.859B | −7.99% |
| `rust-10mb.rs` | 36.077B | 34.027B | −5.70% |
| `javascript-10mb.js` | 41.947B | 35.425B | −15.47% |

jQuery 2.1.4 meets the 3.35B target. In exploratory cumulative five-pair
stages on jQuery 2.1.4, palette/span reuse reduced instructions by 17.64%,
parser hashing by another 4.18%, and the exact-start guards by another
1.18%. Those percentages are paired estimates from separate runs, not
additive. The separate checkpoint-retention stage saved 0.62% on jQuery
2.1.4 (3.190B → 3.174B) and 0.69% on jQuery 3.7.1. Independent warm
profile counts:

| Counter | Before | After |
|---|---:|---:|
| Built style spans | 22,370 | 17,500 |
| Distinct retained style arrays | 5,997 | 3,568 |
| Exact interned scope paths | 13,533 | 13,533 |
| Additional per-line scope snapshots | 9,211 | 0 |
| New owned scope checkpoint copies | 445 | 0 |
| Native span searches | 66,296 | 61,850 |
| Native capture searches | 1,142 | 1,113 |

Complete render hits stayed at 3,214; another 2,429 lines reused only the
style array. The call lookahead fell from 4,668 to 296 native searches
and the reserved-word lookahead from 2,468 to 839. Some assignment probes
that previously failed the general literal filter now reach native
verification; assignment searches rose from 518 to 2,073. Neither
instrumented profile is a throughput measurement.

### Windowless font footprint

Five fresh processes per side, alternating. The probe measures Darwin
`TASK_VM_INFO.phys_footprint` after shaping and rasterizing the sample's
unique glyphs (no atlas, whole editor, GPU or window). Reproduce with
`target/release/examples/font_footprint ascii`, `… cjk`, and
`… cjk "$ASSET" PingFangSC-Regular`, where ASSET is the installed
MobileAsset `PingFang.ttc`. The sandbox's default Chinese cascade resolves
to the sealed `PingFangUI.ttc`, so it is not a test of the writable asset.

| Font case | Physical footprint before → after | RSS before → after |
|---|---:|---:|
| ASCII, default Menlo | 1.66 → 1.62 MB | 7.78 → 7.82 MB |
| Chinese, sealed system cascade | 5.74 → 5.62 MB | 20.84 → 20.82 MB |
| Chinese, explicit MobileAsset face | 80.32 → 15.30 MB | 86.38 → 21.41 MB |

The explicit face retains 13,227,080 bytes instead of the collection's
78,222,888 bytes; neither version maps it. Native app footprint after
typing has not been remeasured. Eight software text snapshots of
`r6-cjk-zh` and `r5_sidebar` (zh-Hans, dark/light, 1×/2×) were
byte-identical. Metal has no adapter in the sandbox. The asset-specific
test also compares the selected face's shaping and outline tables to the
original collection and exercises a truncated read separately.

### Validation and next work

The official 242-file assertion gate and the 193,704-line Oniguruma,
multi-matcher and memoized differential both matched the baseline with
prefilter verification enabled. Assertion evidence digest:
`6c861d72d9008d749f6a4ab5cc47ee2649e3907a8f554cd30eae6474aba7a83a`.
Operations, corrections, rewinds and warnings digest:
`1a53ce33b5bd89ff5b0faa616255e4f5712046273118ec01ff9e6941fec913af`.

The explicit 10 MB Rust `features` gate gave 192.192B → 196.350B
instructions (side medians), with a **−0.91% median paired change**. The
feature workload has asynchronous work; the medians are not an estimate
of the paired change. The final serial 41-pair typing series gave +0.39%
for one caret (305.354M → 306.452M) and +0.65% for 1,000 carets
(4.086B → 4.113B). One-minute host load was 33.45–41.35, despite no
overlapping builds from this lane. The 1,000-caret result was +0.60% in
five matched pairs and +1.19% in a separate 19-pair run while builds were
active. An exploratory series with different selected Cargo targets also
failed at +2.75%. A 19-pair identical-binary control was −0.45%. The
final paired gates meet the +1% cap; the shorter series do not establish
a repeatable slowdown.

Formatting and strict workspace Clippy passed. The workspace gate
reported **1,691 passed, 1 failed**: the known zh-Hans draw-list path
audit flagged ASCII parts of the disposable directory name. Rustdoc with
warnings denied, the four bundle fixture tests, and the CLI forwarding
acceptance test passed separately. No app was packaged.

Raw records: `/tmp/velocity-c34.OVRveV/`, including
`highlight-confirmed/`, `checkpoint-pairs/`, `features-confirmed/`,
`typing-final-41/`, `profile-{before,after}.jsonl`, `memory-confirmed/`,
`scene-{control,confirm}/`, both syntax logs, `gate.out`,
`workspace-tests.log` and `forwarding.log`.

Ranked remaining work:

1. **Native startup and memory verification.** Compare first positive
   `presentedTime` and skipped draws to the original trace and native
   floor. Re-run native typing memory with ASCII, a single CJK face, and
   mixed scripts/weights to measure the app-wide working set.
2. **Highlighting.** Keep the 3.35B cap and exact-output gates. Examine
   the assignment probes and the remaining native identifier/empty-rule
   costs before considering more aggressive changes to parser replay.
3. **Multi-face collection storage.** Measure the working set with
   multiple regional faces and weights before adding sharing or eviction
   for their duplicate tables.

## Round 27 changes and next measurement

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

## Round 27 validation

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

The round 27 gate wrote `/tmp/r14-review/gate-velocity-lane-c.log`. Individual
reruns are in `targeted-*-rerun.log` alongside the timing data.
