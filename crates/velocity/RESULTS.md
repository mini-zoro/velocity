<!-- ROUND11-RESULTS-BEGIN -->
# Round 11: typing gate, Vintage and user TextMate grammars

The final binary meets the spec's upper-confidence-bound gate. Each comparison uses the same
round-nine `/tmp/velocity-round10-baseline/velocity`, ten alternating AB/BA process pairs and
500 timed keymap/transaction/CPU scenes per process, with review UI enabled and zero synthetic
index words. The mandatory gate passed before Vintage/grammar feature work began.

| State vs round 9 | Median process p50 | Median paired p50 change | Paired 95% CI | Ratio of medians / paired 95% CI |
| --- | --- | --- | --- | --- |
| Merge state before fix | 0.528583 → 0.548781 ms | +6.89% | +3.90%…+9.32% | +3.82% / +1.18%…+9.23% |
| Mandatory gate before feature work | 0.473251 → 0.410500 ms | -13.11% | -14.10%…-12.06% | -13.26% / -13.83%…-12.06% |
| Finished round-11 binary | 0.474135 → 0.405979 ms | -13.83% | -15.28%…-13.26% | -14.37% / -15.37%…-13.21% |

The bootstrap resamples process pairs 20,000 times; individual key samples are not treated as
independent processes. The runner gates both p50 upper bounds at +5%. No lane-A compilation,
tests or other benchmark ran during a series. All 40,000 typing samples (including the retained pre-capacity-guard validation series), definition samples,
process quantiles, binary/source hashes and diagnostic phases are in
[timings-round11.json](timings-round11.json). Raw files and frozen executables:
`/tmp/velocity-round11/{before,gate,final,final-capacity}`. The finished binary is `final-capacity`;
the earlier `final` series is retained as additional evidence. The regression reproduction used the clean merge
state (`3f4f4cb`, application code `7c84f61`). No samples or timing outliers were dropped.

The optimization retains widget styles for unchanged full paths/settings while continuing
animation sampling and theme invalidation; it also hoists scene-invariant line settings.
Existing scene goldens pass unchanged. A deterministic test covers recycled resolver-address
collisions. Separate component diagnostics and the exact reproduction commands are in
[ROUND11.md](ROUND11.md) and [RUNBOOK.md](RUNBOOK.md).

Vintage is off by default and enabled with `vintage_enabled`; the three public Vintage options
are supported. The shared native/headless route handles vi edits, pass-through keys and registry
requests, multiple carets, grouped vi undo/dot/macros, block/visual carets, mode/prefix status,
input-panel Ex/search and asynchronous reindent completion. Insert-mode IME preedit stays
transient; command mode bypasses document IME input. New input supersedes unfinished reindent.
User XML/JSON TextMate and YAML grammars load from ordered roots, plus existing archive resources,
and reload detection, syntax picker/palette and open views without replacing unsaved text.

Final CPU theme p95 budgets: popup 0.208 ms / ≤2 ms,
200 annotations 0.774 ms / ≤2 ms,
theme switch 0.997 ms / ≤50 ms.

Verification: workspace fmt, strict all-targets clippy, `cargo test --workspace --release`
(**442 passed, zero failed/ignored**), strict workspace rustdoc, release build, shell/Python
syntax checks and the GUI script's preparation-only path all pass. Fresh/read-only/corrupt
profiles, corrupted/unavailable grammar caches, live settings/grammar changes and IME are
covered headlessly. All changes stay in lane-A crates plus Cargo.lock; no commit was made.

Native visual/IME/clipboard/dialog/Metal behavior remains for human review. No native result is
inferred from CPU evidence. Outside the sandbox, run:

```sh
zsh crates/velocity/scripts/review-round11-gui.zsh
```

Follow [MANUAL_TEST.md](MANUAL_TEST.md)'s Round 11 checklist; engine compatibility limits are in
ROUND11.md and vel-vintage/COVERAGE.md. Earlier reports follow.
<!-- ROUND11-RESULTS-END -->

<!-- ROUND10-RESULTS-BEGIN -->
# Round 10 results: themed widgets and minihtml

Implemented without a commit in `velocity`, `vel-ui`, `vel-render`, `vel-platform`, `vel-config`
and `vel-session`, plus root Cargo.lock. The theme/minihtml engines, original bundled theme art,
lane-B crates and syntax staging implementation remain unchanged.

Tabs, sidebar/Goto rows, quick panels, search controls, menus, tooltips, status controls, fold
icons, minimaps and scrollbars now use the original dark/light themes. User fragments, file
icons, UI scaling, density variants, nine-slice layers, fonts and finite transitions reach the
scene painter. The output panel follows live window theme/settings/scheme changes. Native
text uses the existing font stack with matching metrics; native appearance/title-bar behavior
is implemented but awaits the human check below.

Definition hover and completion/quick-panel details render minihtml. The review demo exercises
popup flags, viewport clipping/scrolling, command/browser links and titles; inline/below/block
phantoms; right-aligned annotations; build errors escaped into HTML annotations; and ordinary
read-only HTML sheet tabs that retain their content through Hot Exit. Images decode on bounded
workers and share the atlas. See [ROUND10.md](ROUND10.md) for behavior and practical limits.

## Isolated CPU evidence

Ten alternating process pairs per workload, with both release binaries copied and hashed.
Typing uses the same round-nine ten-file Git/prose/completion fixture with review UI enabled
and 500 real keymap edits/scenes per process. Scrolling measures 300 four-view Rust scenes per
process. No compilation, tests or other lane-A benchmark ran alongside these processes.
Five further current-binary processes measure 100 popup opens, 100 annotation frames and
50 theme switches each. All 10,000 typing samples, 6,000 scrolling samples, budget samples,
process results, environment/source hashes and confidence intervals are retained in
[timings-round10.json](timings-round10.json). Raw evidence: `/tmp/velocity-r10-final-20261115T121806Z`.

| Probe | Budget | Measured p50 / p95 |
| --- | --- | --- |
| Typing delta versus round 9 | ≤ +5% | +4.89% / +3.65% |
| Four-view scrolling delta versus round 9 | ≤ +5% | +3.22% / +1.14% |
| 2,048-byte popup to first CPU scene | ≤ 2 ms | 0.173 / 0.214 ms |
| 200 visible annotations, CPU frame | ≤ 2 ms | 0.720 / 0.739 ms |
| Dark/light switch to next CPU scene | ≤ 50 ms | 0.811 / 0.957 ms |

These are medians of per-process quantiles. Typing changed from 0.492 / 0.573
to 0.516 / 0.594 ms; scrolling changed from 0.667 / 0.803 to
0.689 / 0.812 ms. Paired bootstrap 95% intervals for typing are
+1.84% to +6.44% at p50 and -4.03% to +7.06% at p95; scrolling intervals are
-0.80% to +6.79% and -1.41% to +37.81%. The point estimates meet the 5% gate;
the intervals cross it, so the results do not establish a 5% upper confidence bound.
Individual maxima were 0.403 ms for popups,
1.502 ms for annotations and
6.627 ms for switching. Native shaping, uploads,
GPU execution and scanout are excluded from all these CPU measurements.

The runner initially stopped after the first successful theme metrics record because that mode
has no separate completion-marker record. Its success check now requires the final sample counts
and a successful process exit. Resume verified the same frozen binary/source hashes, retained
all completed A/B samples and ran the five budget processes; the original theme attempt remains
in the raw directory. No timing failure or outlier was discarded.

## Verification and reviewer handoff

- [x] `cargo fmt --all --check`.
- [x] `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] `cargo test --workspace`: 395 tests passed.
- [x] Final rerun of all six touched crates: 158 tests passed, including 108 Velocity tests.
- [x] `RUSTDOCFLAGS='-D warnings' cargo doc` for all six touched crates, with `--no-deps`.
- [x] Dark/light structural goldens; lifecycle/link/scroll/clip, multiline baseline, phantom tracking,
  annotation row invariance, image/density/scale, deep ancestry, resolver equivalence, output-panel
  updates and HTML-sheet restore coverage. Existing fresh/read-only/corrupt-profile coverage passes.
- [x] Reviewer script syntax checks; frozen-binary CPU A/B and all three feature budgets.
- [x] Lane-A-only file scope; no GUI/Metal execution; no commit.
- [ ] Human native fonts/Retina/OS appearance/title bar/hover/press/fade/IME/link checks,
  Metal validation and native CPU/GPU/scanout measurements from [MANUAL_TEST.md](MANUAL_TEST.md#round-10-theme-and-minihtml-acceptance).

The initial parallel workspace run hit the existing socket test's deadline; both its sequential
rerun and the subsequent full default workspace rerun passed. Detailed gate logs accompany
the raw measurements. The final touched-crate rerun includes the additional output-panel test.

Run on an awake, unlocked display outside the sandbox:

```sh
zsh crates/velocity/scripts/review-round10-gui.zsh /tmp/velocity-round10-baseline/velocity
```

The script freezes both executables, records hashes, runs automatic theme review at ordinary/4K
sizes, uses Metal validation, performs native typing/scrolling A/B, then opens a disposable
manual profile. Supply another saved round-nine binary if that temporary baseline is unavailable.

Earlier sections below are historical evidence.
<!-- ROUND10-RESULTS-END -->

<!-- ROUND9-RESULTS-BEGIN -->
# Round 9 results: complete history, Git, incremental diff and spelling

Implemented in `velocity`, `vel-session`, `vel-commands`, `vel-config` and root Cargo.lock;
left uncommitted on `lane-a`. Lane-B crates are consumed read-only. Earlier sections below
are historical reports; the human has since confirmed Chinese IME and is testing Hot Exit.

Hot Exit now captures `Buffer::export_history` and uses checked `restore_history`, retaining
text and soft undo/redo, saved identity, grouping/continuation, selections, regions, per-view
settings and original edit ownership. Every clone is remapped before activation; hidden clean
files remain lazy. Changed disk files or invalid native imports discard history safely while
retaining unsaved text. Old profiles upgrade the history they actually stored. Randomized
tests compare 128 two-view round trips with 200 subsequent actions each; application tests
cover typing continuation and repeated lazy-clone/owner transfers.

Users can inspect file/folder Git badges and tooltips, dimmed ignored entries, virtual missing
paths, branch/count status and optional configured merge-tool actions. Status/blob reads use
read-only gix. Diff markers, wrapped inline original rows with scalar shading, modification
navigation and undoable per-hunk reverts use saved/index/HEAD references. BOM/legacy encodings
and CRLF normalize through the editor's text model. Spelling shares exact visible scopes with
the existing highlighter, has independent clone requests, and offers navigation, suggestions,
add/ignore persistence and package dictionary choices. Publications are buffer/version/config
checked; undo snapshot gaps reset consumers instead of inventing intermediate roots.

## Final isolated measurements

One copied release executable, SHA-256 `5b969a8c3359d7ed534f7696e00272c1cb39f040ad84b82600a417b7085b325b`;
ten alternating process pairs, 500 real keymap edits/CPU scenes and 100 definition jumps per
process. A prepared popup, sidebar and status bar are present in both modes. Each edit's
immediate scene suppresses stale annotation ranges; exact worker preparation is outside the
input timer. Worker jobs flush after scene construction. No other lane-A benchmark or
compilation ran during the series. All 10,000 typing samples, scheduling outliers, raw runs,
source hashes and process medians are retained in [timings-round9.json](timings-round9.json).

| Probe | Budget | Final measurement |
| --- | --- | --- |
| Typing p50, features off → on | ≤5% increase | 0.563459 → 0.569646 ms, **+1.10%** |
| Typing p95, off → on | Native absolute target ≤4 ms, pending GUI | 2.399813 → 2.190195 ms CPU |
| Full-history save: 50 dirty files / 100 views / 100 text steps each | ≤30 ms background | p50 5.402, p95 6.606, **max 9.773 ms** |
| First-window restore dependencies, 1 loaded / 99 lazy sheets | ≤300 ms CPU proxy; native scanout pending | median **100.580**, max **150.363 ms** |

Typing p50's independent-process bootstrap ratio interval is
**[0.97294, 1.05895]** (10,000 resamples, seed 9).
The point estimate meets the gate; its upper bound reaches +5.89%, so this sample does not
establish strict 5% equivalence. The p95 ratio interval is
[0.74736, 1.19427]. Do not infer scanout/HID
latency from these CPU measurements.

Five history processes provide 50 worker saves. Save includes native delta packing, validation,
JSON, atomic write and file/directory fsync; capture samples are separate. Restore includes
session read/parse, resources and selected native history import. The existing content-validated
syntax cache is retained; contexts remain lazy. Resource setup uses file symlinks with a copy
fallback, concurrent metadata loading and worker cleanup; tests ensure source files are not
modified. Restored editors share immutable keymap tables with independent pending chords.

Development diagnostics remain in `timings-round9-initial.json`, `-handoff.json`,
`-badge-diagnostic.json` and `-components.json`. Earlier full-series results are in
`timings-round9-before-keymap.json`: typing +0.73%, restore max 352.95 ms. Restore/staging/resource
pilots are in `timings-round9-restore-pilot.json`, `-staging-pilot.json` and
`-resource-diagnostics.json`. These include failed budget observations. The hard-link experiment
was reverted; only the final copied binary contributes to the table above.

## Verification and native handoff

All **124 tests** pass (velocity 97, session 13, config 9, commands 5). Scoped fmt, clippy
all-targets `-D warnings`, release tests, rustdoc `-D warnings`, release build and diff whitespace
checks passed. Exact commands and frozen-source verification are in timings-round9.json;
logs are in [the validation directory](output/round9-review-final-verified/validation).
Both Python entry points and the native runner pass syntax checks. The disposable fixture
preparation was exercised without opening the app. One earlier unchanged IPC socket test
returned ENOTCONN; the final complete suite passes.

**No GUI or Metal execution occurred in the sandbox.** On an awake, unlocked, visible display,
run `zsh crates/velocity/scripts/review-round9-gui.zsh`. It executes five native alternating
on/off pairs, 4K typing, full-history startup, Metal validation and the interactive fixture.
The exact expanded commands and timing boundaries are at the top of [RUNBOOK.md](RUNBOOK.md).
Native typing ≤2/4 ms, ≤5% regression, restore ≤300 ms and Metal/notification behavior remain
reviewer checks. [MANUAL_TEST.md](MANUAL_TEST.md) records Chinese IME as human-confirmed, puts
complete Hot Exit history first, and adds pending Git/diff/spelling/encoding/clone checks.
<!-- ROUND9-RESULTS-END -->

<!-- ROUND8-RESULTS-BEGIN -->
# Round 8 results: one parse, project navigation, completion and snippets

Implemented and package-verified; **uncommitted**. Changes are restricted to `velocity`,
`vel-commands` and shared root Cargo.lock. Concurrent lane-B work was left untouched.
The previous round-7 section below is an archive; its scroll exception was accepted in 5b128ba.
This round closes that exception with an isolated comparison.

The editor now consumes `ScopeSnapshot` and `HighlightService::symbols`; the lane-A ScopeCache,
its parser, operation arrays and checkpoints are gone. Scope-derived style/comment/tag/fold
facts are cached by the interned stack ID instead of rebuilding strings/stacks for each token.
Styles/scopes are clipped to line bodies
before rendering/analysis. A dedicated publisher builds unsaved index analysis from the complete
matching scopes and symbols, with no syntax parse. Navigation runs on a separate worker. IME
preview text and clone placeholders never enter published analysis. Cache/session creation does
not reload resources; actual resource reloads preserve folders added at runtime and unsaved text.

Users can search project symbols (Cmd+Shift+R), jump to definitions (F12/Cmd+Alt+Down) and
references (Shift+F12), preview multiple locations, navigate 128 saved jumps, click hover
locations and inspect live indexing progress/logs in Help → Indexing Status. Completion is
asynchronous, shows kinds/annotations/plain details, flips above the caret and adapts to short
splits. Arrows, Enter/Tab policies, cycling, mouse commit, delay/selectors/triggers and field
contexts are wired. Stale commits query the new prefix; moving the caret/tab or composing cancels
pending results. Package and inline snippets support Tab/Shift+Tab, live mirrors, field outlines,
Unicode/multiple carets, completion/pairs inside fields, Escape and ordinary undo/Hot Exit text.

## Scroll exception and final regression check

One child process at a time, the same actual 10 MB Rust fixture and 3840×2160 logical CPU view;
ten independent processes per binary, 600 measured scenes each. Order is counterbalanced A/B,
then B/A. No other benchmark ran in lane A. Process medians avoid treating correlated frames as
independent runs; intervals bootstrap those medians (10,000 resamples, fixed seed 8). All raw
samples and scheduling outliers remain in the run directories.

| Comparison | old / new process p50 (ms) | change | old / new 95% CI (ms) |
| --- | --- | --- | --- |
| Archived round 6 → accepted round 7 | 0.435 / 0.440 | +1.21% | [0.42775, 0.4441355] / [0.431521, 0.44490625] |
| Archived round 6 → final round 8 | 0.445 / 0.428 | -3.85% | [0.43063525, 0.4798125] / [0.4256355, 0.45561474999999996] |

Verdict: the round-7 +8.2% regression did not reproduce in the isolated session. Final round-8
scroll also has no >5% p50 regression with non-overlapping confidence. Binary and fixture hashes
are in [timings-round8.json](timings-round8.json); the original round-6/7 records are unchanged.

## Parser CPU and memory

Five alternating process pairs use the identical `parse_bench.rs` in accepted 5b128ba and the
final editor. Only CLI/main measurement hooks are added to the isolated reference archive.
Each opens ten real Rust files, 1,000 lines each, **527,800 UTF-8 bytes total**, with warmed
syntax assets. The timer waits for complete highlighter and editor analysis. CPU is total
process user+system time over all threads; footprint is kernel TASK_VM_INFO phys_footprint.
Figures below are medians of five processes. RSS is retained separately in JSON.

| Probe | round 7 | one parser | change |
| --- | ---: | ---: | ---: |
| Process CPU (ms) | 2685.410 | 809.664 | -69.85% |
| Wall time (ms) | 416.087 | 153.530 | -63.10% |
| phys_footprint (MB) | 102.926 | 56.903 | -46.023 MB / -44.71% |

The ten-file project with live index/completion popup uses **81.463 MB**,
-21.463 MB versus round 7 and
24.560 MB above the one-parser probe.
Adding the synthetic 100k-word vocabulary uses **96.520 MB**,
-6.406 MB versus round 7.
The real-project memory target is no more than +20 MB over round 7; these headless kernel
footprints meet it. Native fonts/Metal footprint remains a reviewer measurement.

## Budgets and verification

| Workload | Budget | measured CPU | samples |
| --- | --- | --- | ---: |
| Typing with popup + 100k extra project words, p50 / p95 | ≤2 / 4 ms | 0.441 / 0.498 ms | 2500 |
| Already-open cross-file definition, p50 / p95 / max | ≤20 ms | 0.437 / 0.522 / 1.470 ms | 500 |
| Ten-file index + popup phys_footprint delta vs round 7 | target ≤+20 MB | -21.463 MB | 5 processes |

Typing starts with a current popup before each real keymap/command transaction and paints the
popup in the timed scene. Worker preparation is outside the timer. Definition time includes a
navigation request, worker delivery, opening an already-open target, reveal and CPU scene. Index
publication is drained before footprint and jump measurements. Synthetic vocabulary preparation
is outside query/typing timing. These are CPU proxies; no AppKit, native font or Metal subsystem
was initialized for measurements. The automatic GUI mode records actual positive drawable
scanout separately from submit and CPU scene time.

Strict fmt, clippy all-targets with -D warnings, release tests (92 passing across the two
packages), rustdoc with -D warnings and release build passed. Logs are retained under
[output/round8-release-intelligence/validation](output/round8-release-intelligence/validation), with commands and source hashes in JSON.
Coverage includes stale completion commits/caret changes, delay/trigger/Tab policies, short
popup layouts, field/mirror/IME/undo behavior, clone cancellation, package reloads, live status,
unsaved indexing, navigation previews/history and fresh/read-only/corrupt profiles. Existing
Hot Exit and Chinese IME regressions still pass. Native behavior/IME geometry/Hot Exit acceptance
remain in [MANUAL_TEST.md](MANUAL_TEST.md); exact automatic and interactive commands are at the
top of [RUNBOOK.md](RUNBOOK.md). No GUI/Metal run is claimed from this sandbox.

Reproduction helpers: `scripts/build-parser-reference.py`, `scripts/measure-intelligence.py`
and `scripts/compare-scroll.py`. Earlier complete experimental series remain under output;
only the final copied binary contributes the round-8 table above.
<!-- ROUND8-RESULTS-END -->

# Round 7 results: workbench, sessions, projects and builds

Implemented and headless-verified; **uncommitted**. Users can arrange draggable pane grids,
clone a shared document with independent selections/scroll and shared undo, multi-select tabs
for side-by-side display, move/close selected sheets, and open independent native windows.
The View menu adds full screen and distraction-free settings. Project Open/Save As/Edit/Close,
Recent/Quick Switch and per-project workspaces are wired. CLI project/stdin/new/add/wait requests
forward over one private local socket; wait completes when the requested sheets close.

Sessions retain dirty/untitled text, normal command-level undo/redo, layouts, folds, selections,
syntax, query/replacement history, sidebar and recent paths. Hidden clean tabs restore lazily.
Hot Exit policies differ for explicit close, crash recovery and quit. A debounced worker writes
atomically with file/directory fsync; storage failures remain recoverable and do not claim a
successful dirty Hot Exit. New tests start with absent profiles and cover read-only storage,
corrupt JSON, wrong schemas, and a file occupying a required directory. Corrupt bytes are kept
aside where writable, otherwise preserved in place with a diagnostic. Optional corrupt user
settings/keymaps/metadata do not make startup depend on successful profile repair.

Build systems support project/resource definitions, platform/variant merging and documented
variables/substitutions. Cmd+B / Cmd+Shift+B / Cancel run an asynchronous process group with
bounded stdout/stderr delivery and retained output; cancellation and task teardown reap the
process tree. Output syntax/wrap, status/elapsed time, F4/Shift+F4, double-click navigation and
inline text diagnostics are integrated. The original [example project](fixtures/workbench.sublime-project)
has failure/success/cancel fixtures without writing build artifacts.

This lane changes velocity, vel-ui, vel-config, vel-commands, vel-platform, the new vel-build
and vel-session, plus Cargo.lock. Lane B sources and owned color schemes were consumed read-only.
No GUI/AppKit window, Metal device, display wake or unlock was started in this session.

**All 132 Rust tests pass:** application 72, UI 5, config 8, commands 5, platform 6,
build 6, session 7, fileio 7, search 8 and render 8. Scoped fmt, clippy `-D warnings`, rustdoc
`-D warnings`, release build, measurement-parser self-test and bundle shell syntax pass.
Exact runnable validation/native commands are at the top of [RUNBOOK.md](RUNBOOK.md);
[MANUAL_TEST.md](MANUAL_TEST.md#round-7--workbenchsessionbuild-acceptance-native-review-pending)
contains the pending human checklist. No native acceptance result is inferred from unit tests.

## Five sequential CPU runs

Same copied release binary for workbench and preceding-round regression suites:
`5c6262d9eeacd88dc856bec9fb328ebea4c5b3cec881a40acfcd44eac5da1e91`.
Host: `Darwin-25.1.0-arm64-arm-64bit-Mach-O`, `rustc 1.99.1 (58cf8f9c3 2026-10-13)`.
[Machine-readable results](timings-round7.json) retain source hashes, all runs, quantiles,
boundaries, comparison data and references to preceding series. Raw workbench
[summary](output/round7-cpu-20261114T073311.813812Z/summary.json), [first run](output/round7-cpu-20261114T073311.813812Z/run-1.jsonl)
and all other runs retain every sample/outlier. Measurements omit the debug layer.

| Workload | Budget | Measured p50 / p95 / max (ms) | Samples |
| --- | --- | --- | --- |
| 50-file durable worker save | ≤20 ms | 0.361 / 0.775 / 1.665 | 50 |
| 50-file first-window CPU restore | ≤300 ms native first window | 176.070 / 179.153 / 179.892 | 5 |
| Four-group typing, command + CPU scenes | p50 ≤2 / p95 ≤4 ms | 1.698 / 1.925 / 8.284 | 1,500 |
| Four-group scroll CPU scenes | p95 ≤2 ms | 1.533 / 1.670 / 8.909 | 1,500 |

All ten per-run saves start from one snapshot of 50 files: ten hidden 11,000,000-byte files,
five small dirty documents with undo, and 35 other clean files. The first save creates an
absent profile/Sessions directory. Worker timing includes validation/encoding, 64 KiB buffered
JSON, temporary-file IO, POSIX file fsync, rename and directory fsync; fixture/snapshot creation
and queue delay are excluded. The files are generated outside the timer. This is not a claim
about a session containing 50 large dirty histories.

Every restore installs one selected document and leaves **49 lazy**, including all ten large
files. First CPU scene readiness ranges 159.757–180.756 ms.
The restore figure includes session read/parse, resources/assets and selected-file installation.
It excludes native window creation, fonts, GPU and scanout; **the complete ≤300 ms native target
remains unmeasured**. Use `--bench-startup --restore-session` in the runbook to measure it.

Four groups occupy 3840×2160 logical points, four 1920×1080 viewports into one highlighted
10k-line Rust buffer. Each has its own layout/selection/scroll and a different visible interval.
Exact viewport styles and backend readiness are prepared before timing. Typing includes keymap,
command/journal/shared-buffer transfers and four CPU scenes; the immediate post-key scene can
use current-text fallback. Scroll times four exact scenes after scroll/highlight preparation.
Asynchronous catch-up, native input, fonts, uploads, GPU and presentation are excluded. Native
input-to-present and minimap GPU budgets remain pending reviewer hardware runs. Outliers above
the p95 limits are retained in the table and raw data.

## Round-6 workload comparison

The original [round-6 record](timings-round6.json) is unchanged. The final binary repeats its
single-view workload five times: 3,000 scroll, 2,500 typing, 500 rewrap and 100 fold/unfold
samples. The script retains its historical `round6-cpu` output prefix; this directory contains
the **round-7 binary**: [summary](output/round6-cpu-20261114T073612.389128Z/summary.json).

| CPU workload | Round 6 p50 / p95 (ms) | Round 7 p50 / p95 (ms) | Change p50 / p95 |
| --- | --- | --- | --- |
| scroll | 0.438896 / 0.737346 | 0.475000 / 0.618422 | +8.23% / -16.13% |
| typing | 1.903354 / 3.341221 | 1.852667 / 2.137978 | -2.66% / -36.01% |
| rewrap | 0.006271 / 0.008583 | 0.006042 / 0.006834 | -3.64% / -20.38% |
| fold | 9.091916 / 10.602915 | 8.787063 / 9.733515 | -3.35% / -8.20% |
| unfold | 0.904917 / 1.180403 | 0.859854 / 1.004718 | -4.98% / -14.88% |

**Performance review remains required before merging:** scroll p50 exceeds the 5% regression rule in CONTRIBUTING.md. The absolute typing/scroll budgets pass in these CPU workloads. The regression is recorded and has not been waived.

Earlier complete series are retained in timings-round7.json, including the pre-final native
bookkeeping builds and a rerun of the archived round-6 binary. One archived-control run briefly
overlapped a headless smoke check, so that series is diagnostic evidence rather than an isolated
control. Concurrent lane-B work and host scheduling were not controlled; no causal attribution
or universal performance improvement is claimed. Existing 1/1,000-caret headless smoke modes
also completed; mixed Unicode/newline/wrapping smoke timings are not native latency evidence.

## History API handoff and native limits

No vel-buffer sources changed. The public-API journal restores normal text undo/redo, save
points and before/after selections, with worker-side UTF-8 deltas and bounded history. It cannot
faithfully serialize private soft-selection history, change-map history or original edit-owner
ViewIds; some commands' internal transaction grouping can differ. Live clone undo continues to
use the native shared Buffer history. This is an explicit Hot Exit fidelity gap.

Requested API: `Buffer::export_history() -> owned, versioned HistorySnapshot` and checked
`Buffer::restore_history(text, snapshot, view_id_map)`. Preserve groups/steps, forward/inverse
edits, owners, selections, cursor, saved identity, limits and per-view soft history. Export should
hold O(1) owned snapshots for worker serialization. Payload/validation details are in
[vel-session/README.md](../vel-session/README.md#public-buffer-history-adapter-and-requested-api).

Native windows/focus/close/quit, IME in lower/right panes, drag/tab connectors, full screen,
distraction-free appearance, CLI wait/forwarding, streamed process output, error badges and
crash/Hot Exit recovery still require the reviewer checklist. Rust compilation and headless
coverage establish their wiring and state behavior, not their visible/native acceptance.

---

# Round 6 results: syntax, language editing and core views

Implemented and headless-verified, **uncommitted**. Users now see syntax colors, real
bold/italic faces and underline decorations, original live dark/light schemes, detected syntax
in the status bar and Set Syntax palette choices. Cmd+/ and Cmd+Alt+/ toggle language comments;
Enter/reindent use metadata; Cmd+R previews transformed file symbols. Scope-aware pairing and
bracket/tag matching ignore strings/comments. Unicode word wrapping, visual vertical movement,
tracked folds, token minimap scrolling and whitespace/guides/rulers/caret/view settings are wired
through the editor. Project-wide symbol indexing remains deferred by the spec.

This lane edited velocity, vel-ui, vel-render, vel-platform, vel-config, vel-commands and the
shared Cargo.lock. Lane B's concurrent sources and owned color-scheme resources were consumed
read-only. One HighlightService is owned per buffer and shared by splits; an editor dispatcher
keeps its cooperative catch-up off the UI thread. Version/scheme checks reject stale styles.
Scope prefixes become available while a separate worker finishes symbols/folds/tags/minimap;
selector predicates require current scope coverage at every caret. No AppKit window, Metal
device, display wake or unlock was started in this session.

**Validation: 100 Rust tests pass** across eight lane-A crates: commands 5, config 8, fileio 7,
platform 6, render 8, search 8, UI 3, application 55. New cases cover metadata commands and stale
reindent, real scopes/quote contexts, symbols/preview cancellation, syntax/settings/appearance,
style/font/minimap propagation, bracket/tag independence, Unicode wrapping and vertical movement,
fold tracking/undo, minimap row relocation/dragging and wrapped IME preview/commit geometry.
Targeted fmt, clippy --all-targets with -D warnings and rustdoc with -D warnings pass for all
six edited crates. Release build, telemetry parser/completeness self-test, bundle shell syntax
and diff whitespace checks pass. GPU/Metal validation and the native checklist remain pending.

Five sequential release CPU runs began **2026-11-14 05:21:28 UTC**, Darwin 25.1.0 arm64,
rustc 1.99.1. Every run used one copied binary (SHA-256 and source fingerprints are in
[timings-round6.json](timings-round6.json)); lane B remained active. Quantiles below pool every
sample using linear interpolation. No outlier was removed.

| CPU workload | Samples | p50, ms | p95, ms | Max, ms | Spec target / comparison |
| --- | ---: | ---: | ---: | ---: | --- |
| Actual 10,000,000-byte Rust first exact screen after default CPU scene | 5 | 13.317 | 16.562 | 16.645 | <=150 ms; native scanout pending |
| Exact highlighted viewport + minimap, 3840×2160 logical scene | 3,000 | 0.439 | 0.737 | 10.422 | p95 <=2 ms; CPU scene only |
| 1 caret, highlighted 10k-line Rust, keymap → edit → CPU scene | 2,500 | 1.903 | 3.341 | 86.102 | p50 <=2 / p95 <=4 ms; present-return pending |
| 1-character rewrap, 100k lines, exactly one row rewrapped | 500 | 0.00627 | 0.00858 | 0.02255 | <=1 ms |
| Fold all, 100k lines, real command/regions/projection | 100 | 9.092 | 10.603 | 16.390 | <=20 ms |
| Unfold all, same document | 100 | 0.905 | 1.180 | 4.339 | <=20 ms |
| Minimap GPU at 4K | — | — | — | — | <=1 ms; reviewer measurement pending |

The pooled CPU proxies meet the numerical targets. **Individual typing p95s were 2.349,
6.040, 4.0006, 2.233 and 2.198 ms**: runs 2 and 3 exceeded 4 ms, so repeatability is not an
unqualified pass. All five medians were below 2 ms. The largest typing sample was 86.1025 ms;
its scene stage was 85.9241 ms. Edit-stage p95s ranged 0.069–0.209 ms, and the bulk of measured
latency was scene construction. Concurrent work was present, but the causes of the tail samples
were not isolated. The raw evidence preserves them for review.

Scroll waits for exact version-matched viewport styles outside each timer, then times a CPU
scene including the minimap; it does not include asynchronous highlight delivery. The logical
3840×2160 viewport exceeds the logical area of a conventional 4K Retina window and makes no
physical display claim. Typing waits for exact highlighting before each key; the ensuing scene
may use current-text fallback until the worker supplies new-version spans. Its timer includes
keymap dispatch, transaction and CPU scene, and excludes native event receipt, shaping/raster,
uploads, GPU and presentation. Rewrap excludes the preceding buffer transaction. Fold/unfold
includes commands, named regions and layout projection, with painting outside the timer.

Durable aggregate: [timings-round6.json](timings-round6.json). Raw JSONL, environment, stdout
and the measured binary are retained in
[the run directory](output/round6-cpu-20261114T052128.851998Z/summary.json).
[measure-views.py](scripts/measure-views.py) reproduces five runs without initializing GUI/GPU.
Earlier timing experiments remain in output; the table above describes the final binary.

Reviewer GUI commands and exact native boundaries are at the top of [RUNBOOK.md](RUNBOOK.md).
Open now correlates the exact highlighted drawable with positive presentedTime; Scroll has an
isolated minimap timestamp pass. These native paths have only been compiled. Record actual
physical size/scale/refresh, repeat five times and fill [MANUAL_TEST.md](MANUAL_TEST.md).
No native typing, scanout, GPU budget or visual-quality result is inferred from CPU checks.

Current limits: undo/redo and generic buffer operations whose transaction descriptors are not
exposed rebuild the syntax service and wrap snapshot; normal insert/snippet/paste/delete and
language edits preserve exact ChangeSets. The one-character benchmark covers the incremental
path. Full symbol/fold/tag aggregation runs on a worker after scope convergence. Existing
metadata reindent compatibility gaps are documented in [vel-syntax/RESULTS.md](../vel-syntax/RESULTS.md).
Paragraph bidi and project symbol indexing are outside this round.

Optional lane-B API requests, with no functional blocker:

- vel-highlight: enqueue-only requests for multiple viewport intervals and versioned scope
  snapshots, including branch corrections, would remove our dispatcher/duplicate scope worker.
- vel-syntax: cached multiple roots or additional in-memory definitions, retaining lazy
  contexts/stable resource paths, would remove the temporary source tree that adds our
  original Plain Text fallback to the public syntax corpus.

Historical round-5 and earlier evidence follows.

---

# Round 5 results: widgets, Goto, palette, find/replace, file search and side bar

Status: implemented and headless-verified; GUI/Metal measurements remain pending. No GUI,
Metal device creation, display wake or unlock was attempted. Nothing was committed. New work
is confined to vel-ui, vel-search, velocity, vel-platform, vel-config, vel-commands and Cargo.lock;
the existing vel-render/vel-fileio seams were also tested. Lane B's files/dependencies were not used.

Users can navigate files/one-based positions with transient previews and a simple split, fuzzy
word rows, a resource/menu-driven command palette with binding hints/session recency, buffer-backed
IME/undo inputs, find/replace/incremental find, streaming Find Results with jumps, lazy project
folders and Open Files, scrolling/reorderable/closable tabs and a full plain-text status bar.
Replace in Files prepares a read-only plan on a worker, checks it is still current, asks native
confirmation, rejects stale/dirty targets and retains byte-for-byte backups before atomic saves.
Symbol navigation remains a provider hook pending the explicitly deferred syntax integration.

Validation: **78 Rust tests pass** across eight crates (commands 5, config 7, fileio 7,
platform 6, render 7, search 8, UI 3, application 35), including ranking goldens, stale-query
cancellation, Goto grammar, rope seams/zero-width matches/groups/preserve-case, unsaved open-file
search, result formatting/backups, exclusions, focus routing, IME display cells, selection-scoped
shrinking replacements, transient-source/viewport restoration and an automatic CPU panel scenario.
Targeted cargo fmt, clippy --all-targets with -D warnings and rustdoc with -D warnings pass for
all touched crates and the render/fileio regression seams. Python telemetry completeness self-tests,
full-matrix dry-run, shell syntax and git diff whitespace checks also pass. The original 92-line
IME checklist is preserved verbatim at the beginning of MANUAL_TEST.md.

Release CPU evidence, sequential processes on Darwin 25.1.0 arm64, 2026-11-14 03:16:21 UTC.
Observed load averages: 6.73 / 8.40 / 7.54; parallel lane may be active. No GUI/GPU in these results.
Search quantiles use nearest rank; the existing headless app smoke uses its existing interpolation.

| Workload | Samples | p50 | p95 | Max | Budget |
| --- | ---: | ---: | ---: | ---: | ---: |
| 100k prepared paths, synchronous top 100, prefix/reset/backspace | 180 | 2.148 ms | 3.453 ms | 3.687 ms | <=10 ms/key |
| Same corpus, worker queue + scoring + receipt | 90 | 2.260 ms | 3.638 ms | 3.767 ms | <=10 ms/key |
| 100,000,000-byte literal highlight, 381,679 matches | 5 | 14.382 ms | 22.501 ms | 22.501 ms | <=200 ms |
| 60-line viewport, same rope | 100 | 0.0089 ms | 0.0140 ms | 0.0872 ms | <=5 ms |
| 1k carets, keymap -> buffer -> CPU scene | 50 | 2.306 ms | 4.477 ms | 5.616 ms | CPU smoke; no GUI claim |
| 1 caret, first CPU smoke process | 50 | 0.3294 ms | 0.6454 ms | 1.041 ms | CPU smoke; no GUI claim |
| 1 caret, median of five subsequent process quantiles | 5 x 50 | 0.1739 ms | 0.3059 ms | see raw runs | Warm descriptive repeat |

Keymap: 1,000 bindings / 1,000,000 lookups, mean **0.1921 us**, compared with round 4's
0.1905 us (+0.84%, below the 5% regression limit). The first short 1-caret smoke p50 was 6.5%
above round 4's single observation, so it was investigated with five additional sequential
processes; every repeat was faster than that observation. All original/repeated logs are retained,
not discarded. The 1,000-caret p95 is also below round 4's observation. These short smoke runs
under concurrent machine load are not a native input-latency benchmark.

P0 PLAN §3.3 recorded naive 100k rescoring at **6–73 ms**. This round's generated corpus differs,
so this is not a controlled same-corpus speedup claim. All 90 worker observations on the documented
round-5 corpus are below 10 ms. Background index construction includes path normalization; it is
outside query scoring. Prefix narrowing retains all matches, top-K sorting touches only retained
hits, and stale generations cannot publish. Opening an overlay can additionally prepare a source
snapshot/recency data; per-key updates reuse it. The automatic GUI panel workload includes that
first-open work and UI polling/scene/GPU submission, which the CPU matcher figures exclude.

Raw JSONL and environment: [summary](output/round5-cpu-20261114T031621.130072Z/summary.jsonl).
App logs: [1 caret](output/round5-cpu-20261114T031621.130072Z/headless-1.jsonl),
[1,000 carets](output/round5-cpu-20261114T031621.130072Z/headless-1000.jsonl).
The same directory retains all five 1-caret repeats and stderr. Search text/index construction
is outside timing; timed search operates on ChunkCursor, without a full-buffer copy.

GUI review uses automatic `--bench-panels` (exactly 100 scripted actions / 100k candidates,
default and requested 4K), plus the existing automatic startup/floor/scroll/typing/memory matrix.
measure.py validates the exact panel sample IDs/count and rejects incomplete or contaminated runs.
All modes preserve the asleep/locked-display fail-fast path and independent watchdog. The new
mode has only been compiled and its controller/validator exercised headlessly. No native timing
or appearance result is inferred from those checks. Exact commands are at the top of RUNBOOK.md;
the human acceptance checklist is MANUAL_TEST.md.

Documented limits: no live symbols/highlighting until the next lane integration; Rust regex syntax
excludes lookaround/backreferences/recursion; disk grep is line-oriented; Where's comma separator
cannot be embedded in a path/glob; disk replacements require saving dirty target tabs and are
atomic per file; session/workspace persistence and the P3 theme engine remain deferred. Native
IME geometry, clipping, font quality, split/tab pointer behavior and appearance still need GUI QA.

Historical round-4 and earlier evidence follows.

---

# Round 4 results: settings, commands and real editing

Status: headless checks complete; native GUI/Metal measurements remain pending. No GUI,
Metal device, display wake or screen-unlock operation was attempted in the sandbox.
No changes to vel-buffer/vel-text APIs were needed. Defaults were authored from scratch.

Delivered: vel-config, vel-commands, vel-fileio and native integration. Files open/save in
tabs with encoding/BOM/line-ending retention, multi-caret edits/history, native file dialogs,
contextual keymaps and pairing, save prompts, settings reload and external-change handling.
Plain text is used until the syntax lane is integrated.

Headless validation: 61 Rust tests across the six lane crates (commands 5, config 6, fileio 7,
platform 6, render 7, application 30), including Unicode properties and the skipped-first-
drawable state machine. cargo fmt, clippy with -D warnings, rustdoc with -D warnings,
Python parser/dry-run checks, shell syntax and Swift helper compilation pass. Native
notifications did not arrive in the initial sandbox watcher test; the watcher now also
checks watched-file metadata, and atomic-replacement coverage passes with that fallback.

Release CPU evidence (one process per workload, not a GUI budget result):

| Workload | Observed |
| --- | --- |
| 1,000 indexed key bindings, 1,000,000 events | mean 0.1905 µs/lookup |
| 100,000,000-byte generated UTF-8 load | worker queue 0.0668 ms; complete 157.437 ms |
| Existing spikes/data/text-100mb.txt through vel-fileio | worker queue 0.0393 ms; complete 166.238 ms |
| keymap → command → buffer → CPU scene, 1 caret, 50 samples | p50 0.3094 ms; p95 0.6330 ms |
| same path, 1,000 carets, 50 samples | p50 2.2718 ms; p95 4.9673 ms |

Raw summary: [round4-cpu-20261113T182158526934Z.jsonl](output/round4-cpu-20261113T182158526934Z.jsonl).
Application logs: [1 caret](output/headless-1794594120757759-58865.jsonl),
[1,000 carets](output/headless-1794594120787205-58866.jsonl).
These samples use the GUI injector's Unicode/newline/ASCII mixture, including a two-scalar
combining sequence. CPU timing excludes GPU/native input. It differs from the previous
round's four-character smoke workload, so it establishes a new baseline rather than a
like-for-like regression comparison. No GUI latency or startup budget is claimed.

Startup fix: retained offscreen texture, cheap repeated copy/present, explicit skipped
callbacks, first callback receipt and first_real_scanout. Milestones flush as they occur.
CoreGraphics session/display checks run before launch; hosting-window occlusion is checked
while waiting with a one-second orderFront grace. measure.py also preflights before each
launch. The new Swift floor has the same retry/diagnostic behavior. Hardware must confirm
both successful scanout and approximately two-second failure on blocked displays.

Reviewer commands and the 100 MB asynchronous-open measurement are in RUNBOOK.md. Complete
MANUAL_TEST.md's new editing smoke checklist, including encodings, tabs, dirty close prompts,
multicursor undo and external reload. Record actual native key equivalents, shifted keys,
keypad/modifier taps, candidate windows and the requested display configurations.

Known scope limits: Rust regex context syntax (no lookaround/backreferences); simple
plain-text scope selectors; no palette UI/snippet field navigation/wrap_lines. Text macro
state is per view. Native appearance, input geometry and keyboard layouts require GUI QA.

The earlier round's evidence/template follows for historical reference; its demo limitations
and smoke workload no longer describe the current implementation.

---

# Lane A/P1：原生 AppKit / wgpu 评审记录

状态：**硬件待测**。沙箱没有 window server 和 Metal，未运行 GUI，不填写虚构的
启动、打字、滚动、GPU 或 GUI 内存成绩。完整复现命令见 RUNBOOK.md。

本轮交付 vel-platform、vel-render、velocity；仅更新这三个 crate 与 Cargo.lock。
采用 objc2 AppKit + layer-hosting NSView + CAMetalLayer，渲染保留 wgpu 30.0.2 Metal。
首屏先在隐藏阶段完成文字布局/字形上传/离屏 GPU 提交，orderFront 后在同一个
applicationDidFinishLaunching 回调内直接 acquire/copy/present；不等待重绘事件或显示链 tick。
MTLDrawable presented handler 记录真实 presentedTime，并单列回调到达时间。

## 沙箱可验证项

- [x] cargo fmt --all
- [x] cargo clippy -p vel-platform -p vel-render -p velocity --all-targets -- -D warnings
- [x] cargo test -p vel-platform -p vel-render -p velocity
- [x] RUSTDOCFLAGS='-D warnings' cargo doc -p vel-platform -p vel-render -p velocity --no-deps
- [x] Python 测量解析/完整性自测、shell 语法检查
- [x] Swift 原生下限 helper 编译（不启动）
- [x] --headless-smoke：50 次 Unicode 编辑 + 可视区 scene CPU 时间（仅 smoke，无 GUI/GPU）

本轮最终 headless 验证共 **36 项 Rust 单元测试通过**（platform 5、render 7、app 24），
fmt / clippy / rustdoc 均通过。未遇到并行 vel-text/vel-buffer 工作导致的依赖构建失败。

CPU smoke 原始文件：[headless-1794586442049930-19843.jsonl](output/headless-1794586442049930-19843.jsonl)。
release、5000 行、50 次 Unicode 编辑 + 可视区 scene：p50 **0.2125 ms**、p95 **0.3746 ms**，
mean 0.2221 ms、max 0.4486 ms。仅一个 smoke 进程，包含冷的首批 CPU 布局；未建立跨版本回归基线。
`gui_started=false`、`gpu_started=false`；不可与 GUI 输入/滚动/GPU 延迟预算混用。
首屏 GUI footprint 及所有硬件表格仍待填。

## 硬件环境与输入

| 项目 | 实测 |
| --- | --- |
| 时间、macOS、机型、内存 | 待填 |
| 电源、load average、并发任务 | 待填 |
| 显示器、逻辑/物理分辨率、scale、refresh | 待填 |
| 输入法、键盘布局 | 待填 |
| release 构建 / 冷暖条件 / run 目录 | 待填 |
| first-frame surface 重试次数 | 待填 |

## 同一会话下限与启动

| 指标（ms） | N | p50 | p95 | 说明 |
| --- | --- | --- | --- | --- |
| Swift floor main → presentedTime | 待填 | 待填 | 待填 | 新 helper 是真实 scanout，旧 P0 是回调到达 |
| Velocity warm main → scanout | 待填 | 待填 | 待填 | 1200×800 points，5000 行 |
| Velocity warm process → scanout | 待填 | 待填 | 待填 | 包含内核进程出生到 main |
| warm main → present-return | 待填 | 待填 | 待填 | 不等同 scanout |
| scanout → callback 延迟 | 待填 | 待填 | 待填 | 诊断回调迟到 |
| Velocity cold-ish main → scanout | 待填 | 待填 | 待填 | 新构建首次，不清系统/驱动缓存 |

目标：warm ≤ 同会话 floor + 30 ms。差值：待填。原始 JSONL：待填。
阶段：didFinishLaunching、window/layer、主字体、adapter/device、shader、pipeline、scene、
first_offscreen_submitted、orderFront、first_submit、present-return、scanout。逐项填入汇总。

wgpu 30.0.2 的 PipelineCache 当前只实现 Vulkan；Metal adapter 支持位与
实际 device/shader/pipeline 创建时间仍由环境记录确认。未支持时不会写假 binary cache。

## 输入、滚动与内存

| 工作负载 / 指标 | N | p50 | p95 | 目标 / 结果 |
| --- | --- | --- | --- | --- |
| typing 1200×800 receipt → present-return | 500 | 待填 | 待填 | ≤2 / ≤4 ms |
| typing 实际 4K receipt → present-return | 500 | 待填 | 待填 | ≤2 / ≤4 ms |
| 4K scroll CPU event → submit | 600 | 待填 | 待填 | p95 ≤1.5 ms |
| 4K GPU render pass | 600 / 不支持 | 待填 | 待填 | p95 ≤3 ms |
| after_first_frame phys_footprint | 待填 | 待填 | 待填 | ≤80 MB |
| after_first_frame resident_size | 待填 | 待填 | 待填 | 单列 RSS |
| after_full_sweep footprint / RSS | 待填 | 待填 | 待填 | 检查有界缓存 |

实际分辨率、timestamp 支持、surface 重试、外部输入污染、watchdog/错误记录：待填。
不将 GPU pass 时间当端到端延迟；不将 RSS 当 Activity Monitor 内存。

## 原生行为人工验收

MANUAL_TEST.md 完成情况：待填。重点记录 UTF-16 replacementRange、日文再转换、表情选择器、
长按重音、组合选择区、候选窗在 Retina/多显示器的位置，以及 focus/resize/scroll。
字体：Menlo、PingFang、Apple Color Emoji；CoreText fallback 次数与颜色/基线观感：待填。
菜单/剪贴板/撤销、全屏、深浅色切换、idle CPU / 120 Hz：待填。

当前限制：单选区 demo；无保存/项目/完整语法/段落 bidi/自动换行。fallback/shaping 是
代码编辑器网格方案，复杂脚本与 ZWJ/旗帜仍须视觉核对。首屏离屏纹理会短暂增加
GPU 内存；Metal 驱动、AppKit 生命周期和 callback 行为只能在真机验证。

结论与后续修复：待评审数据后填写。
