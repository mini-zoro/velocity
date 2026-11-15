# Round 8: editor intelligence and parser removal

Only `velocity`, `vel-commands`, and root Cargo.lock belong to this lane's change. Run:

```sh
cargo fmt -p velocity -p vel-commands --check
cargo clippy -p velocity -p vel-commands --all-targets -- -D warnings
cargo test --release -p velocity -p vel-commands
RUSTDOCFLAGS='-D warnings' cargo doc -p velocity -p vel-commands --no-deps
cargo build --release -p velocity
```

CPU probes (no AppKit, CoreText or Metal initialization):

```sh
VELOCITY_R8_CPU=$(mktemp -d /tmp/velocity-r8-cpu.XXXXXX)
./target/release/velocity --headless-parse-memory --output "$VELOCITY_R8_CPU/parse.jsonl" --timeout 300
./target/release/velocity --headless-intelligence --output "$VELOCITY_R8_CPU/intelligence.jsonl" --timeout 300
./target/release/velocity --headless-intelligence --intel-words 0 --output "$VELOCITY_R8_CPU/ten-file-memory.jsonl" --timeout 300
```

The shared parser probe opens ten real Rust files, 1,000 lines each, with warmed assets and
waits for complete highlighter/editor analysis. It reports process user+system CPU and kernel
TASK_VM_INFO phys_footprint; no index or completion query is included. For the comparison,
`parse_bench.rs` is also compiled unchanged into a temporary archive of accepted commit
5b128ba, with only its CLI/main measurement hook added. The archive needs the existing public
`third_party/sublime-packages` tree. To build and measure the reference automatically:

```sh
VELOCITY_R8_REF=$(mktemp -d /tmp/velocity-r8-reference-parent.XXXXXX)
python3 crates/velocity/scripts/build-parser-reference.py --output-dir "$VELOCITY_R8_REF/reference"
python3 crates/velocity/scripts/measure-intelligence.py --old "$VELOCITY_R8_REF/reference/target/release/velocity" --output-dir "$VELOCITY_R8_REF/measurements"
```

This runs five sequential processes per workload. Source trees, binaries, hashes and raw runs are retained;
repository history is never changed. The intelligence probe adds real project services and a
prepared synthetic 100,000-word source, including word sequences, then times each keymap command
and CPU scene with its popup. Worker preparation occurs before each key. Definition time includes
a navigation request, worker delivery, opening an already-open target, reveal and CPU scene.
It uses an actual Rust definition in file0 and a reference in file9. This is a CPU workload;
it does not establish a physical display or HID latency budget.

Isolated scroll comparison (one child process at a time, alternating order, ten runs each):

```sh
python3 crates/velocity/scripts/compare-scroll.py --old crates/velocity/output/round6-cpu-20261114T052128.851998Z/velocity --new crates/velocity/output/round7-cpu-20261114T073311.813812Z/velocity --output-dir "$VELOCITY_R8_CPU/scroll-r7"
```

Keep the archived binary hashes in timings-round8.json; confidence intervals bootstrap the
independent process medians rather than treating 6,000 frames as independent processes.
Run no other lane-A benchmark during any timing series.

Reviewer GUI commands, outside the sandbox with a visible, unlocked display:

```sh
cargo build --release -p velocity
VELOCITY_R8_GUI=$(mktemp -d /tmp/velocity-r8-gui.XXXXXX)
for run in 1 2 3 4 5; do
  ./target/release/velocity --bench-intelligence --width 1200 --height 800 --data-dir "$VELOCITY_R8_GUI/profile-$run" --output "$VELOCITY_R8_GUI/intelligence-$run.jsonl" --timeout 300
done
./target/release/velocity --bench-intelligence --intel-words 0 --width 1200 --height 800 --data-dir "$VELOCITY_R8_GUI/memory-profile" --output "$VELOCITY_R8_GUI/ten-file-memory.jsonl" --timeout 300
./target/release/velocity --bench-intelligence --width 2048 --height 1089 --data-dir "$VELOCITY_R8_GUI/4k-profile" --output "$VELOCITY_R8_GUI/intelligence-4k.jsonl" --timeout 300
./target/release/velocity --bench-scroll --lines 0 --width 2048 --height 1089 spikes/data/rust-10mb.rs --data-dir "$VELOCITY_R8_GUI/scroll-profile" --output "$VELOCITY_R8_GUI/scroll-4k.jsonl" --timeout 300
./target/release/velocity -n --project crates/velocity/fixtures/intelligence.sublime-project crates/velocity/fixtures/intel-project/main.rs:4:12 --data-dir "$VELOCITY_R8_GUI/manual-profile" --log-input
```

The native intelligence mode exits after 500 successfully presented typing samples and 100
already-open definition jumps. It records actual physical size/scale/refresh, CPU command+scene,
injection→submit, injection→positive presentedTime and phys_footprint with index+completions.
Typing targets are p50/p95 ≤2/4 ms; already-open definition jumps ≤20 ms. Scanout is reported
separately. Startup, glyph uploads and worker preparation are outside typing samples. Close or
watchdog failures write incomplete/error records. `--intel-words 0` uses the real ten-file
vocabulary for the memory budget; the default
100k-word synthetic stress workload reports its larger footprint separately.
The 10-file memory target is ≤+20 MB versus
the accepted round-7 fixture; CPU phys_footprint is available in the headless comparison, while
native font/GPU footprint must be evaluated from these reviewer runs. Do not substitute RSS.

Use Cmd+Shift+R, F12/Cmd+Alt+Down, Shift+F12, Ctrl+- / Ctrl+Shift+-, hover and Help → Indexing
Status. `index_files`, `index_workers`, `index_exclude_patterns`, `index_exclude_gitignore`,
`index_skip_unknown_extensions` and `show_definitions` are settings. Completion settings follow
vel-complete's README, with delay in milliseconds and size limit in bytes. A dedicated
navigation worker remains independent of project word-statistic publication.
The publisher calls `analyze_scopes` on complete matching snapshots; benchmark setup waits
for a publication barrier before measuring footprint or definition jumps. Resource/package
reload replaces the prepared engines; queued results from the previous engine cannot commit.
The palette exposes `Snippet: Rust function`; typing `fn` then Tab expands it. Fields use
Tab/Shift+Tab, Esc and `clear_fields`; popup visibility and field contexts are available to User
keymaps. Details are rendered as plain text; minihtml remains deferred.


## Round 7 archive: layouts, sessions, projects and builds

Run from `/Users/zoro/dev/velocity`. Do not commit. This round changes velocity, vel-ui,
vel-config, vel-commands, vel-platform and the new vel-build/vel-session, plus Cargo.lock.
Other lanes' working-tree changes and Color Scheme - Velocity are left untouched.

```sh
cd /Users/zoro/dev/velocity
cargo fmt -p velocity -p vel-ui -p vel-config -p vel-commands -p vel-platform -p vel-build -p vel-session --check
cargo clippy -p velocity -p vel-ui -p vel-config -p vel-commands -p vel-platform -p vel-build -p vel-session --all-targets -- -D warnings
cargo test -p velocity -p vel-ui -p vel-config -p vel-commands -p vel-platform -p vel-build -p vel-session -p vel-fileio -p vel-search -p vel-render
RUSTDOCFLAGS='-D warnings' cargo doc -p velocity -p vel-ui -p vel-config -p vel-commands -p vel-platform -p vel-build -p vel-session --no-deps
cargo build --release -p velocity
python3 crates/velocity/scripts/measure.py --self-test
python3 crates/velocity/scripts/measure-workbench.py --runs 5
```

The last command is CPU-only and uses **3840×2160 logical points**, four 1920×1080 viewports,
a 10k-line Rust document, one real shared Buffer and one highlight/analysis service. Each
viewport has its own selection/scroll/layout cache and shows a different line interval. Every
typing/scroll sample waits for exact viewport styles and backend readiness **outside** the
timer. Typing times keymap/command, journal, buffer ownership and four CPU scenes; the immediate
post-key scene may use current-text fallback. Scroll times four exact CPU scenes after the
scroll state and asynchronous highlighting are prepared. Fonts, native input, uploads, GPU,
presentation and waiting for workers are excluded. Raw samples, all outliers, one copied binary,
source hashes, per-run results and pooled quantiles are retained in the printed output directory.
No native rendering or scanout is inferred from these results.

Each run also creates **50 files, including ten hidden 11,000,000-byte files and five small dirty
files with undo**. Fixture IO is outside timing. Save includes background validation/encoding,
buffered JSON, temporary-file write, POSIX file fsync, rename and directory fsync. The initial
save starts with a non-existent profile/Sessions directory. Restore times session IO/parse,
resource/assets setup and selected document installation in a new Workspace; CPU first-scene
readiness is separate. It reports the number of loaded/lazy files. This is a CPU proxy for the
300 ms first-window target; the native mode below is required to assess the complete target.
The save budget is 20 ms, restore 300 ms, four-group typing p50/p95 2/4 ms, scroll p95 2 ms.
All measurements and boundaries are in RESULTS.md and timings-round7.json.

Regression comparison with the preceding single-view workload (CPU only):

```sh
python3 crates/velocity/scripts/measure-views.py --runs 5
./target/release/velocity --headless-smoke --carets 1
./target/release/velocity --headless-smoke --carets 1000
```

## Native reviewer measurements — outside the sandbox

Use an awake, unlocked visible display. First produce an isolated, initially absent profile and
50-file session without any GUI. The directory names below are new on every run.

```sh
cd /Users/zoro/dev/velocity
cargo build --release -p velocity
VELOCITY_R7=$(mktemp -d /tmp/velocity-r7-review.XXXXXX)
python3 crates/velocity/scripts/measure-workbench.py --runs 1 --output-dir "$VELOCITY_R7/cpu"
VELOCITY_R7_PROFILE=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["runs"][0]["data_dir"])' "$VELOCITY_R7/cpu/summary.json")
./target/release/velocity --bench-startup --restore-session --data-dir "$VELOCITY_R7_PROFILE" --output "$VELOCITY_R7/restore.jsonl" --timeout 300
./target/release/velocity --bench-typing --groups 4 --carets 1 --width 2048 --height 1089 --lines 0 spikes/data/rust-10mb.rs --data-dir "$VELOCITY_R7/typing-profile" --output "$VELOCITY_R7/typing-4groups.jsonl" --timeout 300
./target/release/velocity --bench-typing --groups 4 --carets 1000 --width 2048 --height 1089 --lines 0 spikes/data/rust-10mb.rs --data-dir "$VELOCITY_R7/typing1000-profile" --output "$VELOCITY_R7/typing1000-4groups.jsonl" --timeout 300
./target/release/velocity --bench-scroll --groups 4 --width 2048 --height 1089 --lines 0 spikes/data/rust-10mb.rs --data-dir "$VELOCITY_R7/scroll-profile" --output "$VELOCITY_R7/scroll-4groups.jsonl" --timeout 300
MTL_DEBUG_LAYER=1 ./target/release/velocity --bench-startup --restore-session --data-dir "$VELOCITY_R7_PROFILE" --output "$VELOCITY_R7/metal-validation.jsonl" --timeout 300
```

Repeat with new output names at least five times. At scale 2, 2048×1089 logical is 4096×2178
physical; record **actual_physical_size**, scale and refresh rate because window constraints
can reduce it. Native startup correlates a positive presentedTime and includes AppKit/fonts/GPU
work. The first-window budget is 300 ms. Native typing uses receipt/injection → present-return
(p50/p95 2/4 ms), not OS/HID delivery. Native scroll CPU p95 is 2 ms. Minimap GPU p95 is 1 ms;
per-frame multi-pane pass samples should be checked in the native output. Missing timestamps,
blocked displays, retries, watchdog errors and Metal validation output are not passes. No GUI,
Metal, display wake or unlock was attempted in the agent session.

## Human feature review

```sh
./target/release/velocity --data-dir "$VELOCITY_R7/fresh-profile" --project crates/velocity/fixtures/workbench.sublime-project crates/velocity/fixtures/workbench-error.txt
./target/release/velocity -n --data-dir "$VELOCITY_R7/fresh-profile"
./target/release/velocity -a --data-dir "$VELOCITY_R7/fresh-profile" crates/velocity/fixtures
./target/release/velocity -w --data-dir "$VELOCITY_R7/fresh-profile" crates/velocity/fixtures/workbench-error.txt
printf 'unsaved stdin 你好\n' | ./target/release/velocity --data-dir "$VELOCITY_R7/fresh-profile" -
```

The first interactive CLI detaches the native application and forwards its request to the local
socket. Subsequent invocations reuse that process. `-w` holds the client until the requested
sheets close (or their window/application closes). It never polls the disk as a substitute for
closing. `-n` creates another native window; `-a` adds folders/opens files in an existing window.
The socket is private to the current user and profile and lives in the OS temporary directory,
so an absent or read-only profile does not block forwarding. Explicitly missing CLI files are
reported without terminating the editor. Automatic measurement modes bypass forwarding.

Choose **Workbench Example** in Tools → Build System, then Cmd+B. A diagnostic should stream
before the one-second exit, F4/Shift+F4 and output double-click should navigate to line 2, and a
text badge should appear there. Cmd+Shift+B exposes Success and Cancel Process Tree variants.
Run the latter and cancel; the UI must remain responsive and all child processes should end.
The fixture does not write build artifacts. Default public Cargo/Python/Make build resources and
User `.sublime-build` files are also available; unsupported custom targets report an error.

Use View → Layout presets, drag both grid dividers, focus/move groups (Ctrl+1…4 / Ctrl+Shift+1…4),
Clone File, New/Close Pane, Cmd/Shift-click tabs and sidebar entries, and Goto Cmd+Enter. Selected
tabs divide a group horizontally; group splits remain independent. Cmd+J is a Tab Selection chord
prefix; the menu/palette also expose these commands. Check separate clone selections/scroll,
shared undo after closing an owner view, batch movement/close, and IME geometry in lower/right panes.

Project Open/Save As/Edit/Close and Ctrl+Cmd+P should preserve folder rules and unknown project
keys, and restore a per-project workspace. Save As into a previously absent directory. Ensure
closing/switching projects under disabled/only-on-quit policy offers Save/Cancel/Discard before
replacing dirty documents. A workspace autosave must not cause repeated settings reloads.

Quit/relaunch with dirty, untitled and clean tabs, undo/redo, folds, syntax overrides, query and
replacement text, sidebar expansion/scroll, multiple windows and layouts. Try `hot_exit` values
`always`, `only_on_quit` and `disabled`, plus `remember_open_files: false`. Change an edited file
externally while the app is quit: retained text should stay dirty and its restored undo stack
should be empty. Check native full screen and distraction-free entry/exit, including a custom
`Packages/User/Distraction Free.sublime-settings` and sidebar visibility remembered per mode.

With the application quit, use an isolated profile to test corrupt/read-only storage:

```sh
mkdir -p "$VELOCITY_R7/corrupt/Sessions"
printf '{truncated' > "$VELOCITY_R7/corrupt/Sessions/Session.sublime-session"
./target/release/velocity --data-dir "$VELOCITY_R7/corrupt"
mkdir -p "$VELOCITY_R7/readonly"
chmod 555 "$VELOCITY_R7/readonly"
./target/release/velocity --data-dir "$VELOCITY_R7/readonly"
```

Corruption starts empty and keeps the corrupt file aside. Read-only startup remains usable;
clean windows may close, while dirty Hot Exit failures retain the live buffer and explain the
failed write. Restore write permission afterwards with `chmod 755 "$VELOCITY_R7/readonly"`.
Never run these destructive fixtures against a real profile. MANUAL_TEST.md has the fill-in rows.

## Buffer serialization handoff

No vel-buffer source was changed. The current public-API journal restores normal command-level
text undo/redo, save points and selections, but cannot serialize private soft-history events,
change maps or edit-owner ViewIds exactly. Generic internal transaction grouping may differ.
The exact requested export/import payload and ownership-remapping API are in
[vel-session/README.md](../vel-session/README.md#public-buffer-history-adapter-and-requested-api).
This limitation is explicit; full native history fidelity is pending that API.

---

# Round 6: syntax, language editing and view features

Run from `/Users/zoro/dev/velocity`. Keep lane B's concurrent files unchanged and do not
commit. Round-6 code touches velocity, vel-ui, vel-render, vel-platform, vel-config and
vel-commands, plus the shared root lockfile. Resources under Color Scheme - Velocity are
consumed without editing them. Older round sections below are historical.

```sh
cd /Users/zoro/dev/velocity
cargo fmt -p velocity -p vel-ui -p vel-render -p vel-platform -p vel-config -p vel-commands --check
cargo clippy -p velocity -p vel-ui -p vel-render -p vel-platform -p vel-config -p vel-commands --all-targets -- -D warnings
cargo test -p velocity -p vel-ui -p vel-render -p vel-platform -p vel-config -p vel-commands -p vel-fileio -p vel-search
RUSTDOCFLAGS='-D warnings' cargo doc -p velocity -p vel-ui -p vel-render -p vel-platform -p vel-config -p vel-commands --no-deps
cargo build --release -p velocity
VELOCITY_ROUND6_CPU=$(mktemp -d /tmp/velocity-round6-cpu.XXXXXX)
./target/release/velocity --headless-view-features --output "$VELOCITY_ROUND6_CPU/features.jsonl"
python3 crates/velocity/scripts/measure-views.py --runs 5
```

The CPU mode uses the actual 10,000,000-byte Rust fixture for first-screen highlighting
and 600 scroll scenes at **3840×2160 logical points**. Each timed scroll scene has
exact viewport styles; asynchronous delivery occurs before its timer. This is a deliberately larger CPU
viewport than a 1920×1080 logical Retina window; no physical display is inferred. Typing
uses 500 real keymap/command transactions in a fully highlighted 10k-line Rust document,
waiting for an exact styled viewport and backend catch-up before each timed key. The timer includes edit submission and
CPU scene construction; rendering may consume current-text fallback while a batch arrives.
It excludes native input receipt, font shaping/rasterization, uploads, GPU and presentation.
The rewrap timer starts after a one-character transaction in 100k lines and checks that
exactly one line rewrapped. Fold/unfold measures real commands, named regions and projection
rebuild over 99,999 nonempty lines plus the final empty line; painting is outside that timer.
All samples and boundaries are emitted as JSONL, including typing edit/scene stage statistics.
The Python wrapper copies one release binary, runs it sequentially, retains raw samples and
records binary/source hashes and interpolated pooled quantiles. It never overwrites results.
Use at least five sequential runs, retain outliers and record concurrent lane activity.

Final round-6 results: [RESULTS.md](RESULTS.md) and [timings-round6.json](timings-round6.json). All 100 lane-A tests and required strict
checks pass. Five CPU runs retain all outliers; two individual typing p95s exceed
4 ms despite the pooled p95 meeting the target. Native results remain pending.

## Reviewer GUI commands — run outside the sandbox

Use an awake, unlocked, visible display. These commands exit automatically. No GUI/Metal
measurement or display wake was attempted in the agent session.

```sh
cd /Users/zoro/dev/velocity
cargo build --release -p velocity
VELOCITY_ROUND6_GUI=$(mktemp -d /tmp/velocity-round6-gui.XXXXXX)
mkdir -p "$VELOCITY_ROUND6_GUI/data/Packages/User"
./target/release/velocity --bench-open spikes/data/rust-10mb.rs --data-dir "$VELOCITY_ROUND6_GUI/data" --output "$VELOCITY_ROUND6_GUI/open.jsonl" --timeout 300
./target/release/velocity --bench-scroll --lines 0 --width 2048 --height 1089 spikes/data/rust-10mb.rs --data-dir "$VELOCITY_ROUND6_GUI/data" --output "$VELOCITY_ROUND6_GUI/scroll-4k.jsonl" --timeout 300
./target/release/velocity --bench-typing --carets 1 --lines 0 --width 2048 --height 1089 spikes/data/rust-10mb.rs --data-dir "$VELOCITY_ROUND6_GUI/data" --output "$VELOCITY_ROUND6_GUI/typing-4k.jsonl" --timeout 300
MTL_DEBUG_LAYER=1 ./target/release/velocity --bench-open spikes/data/rust-10mb.rs --data-dir "$VELOCITY_ROUND6_GUI/data" --output "$VELOCITY_ROUND6_GUI/metal-validation.jsonl" --timeout 300
./target/release/velocity crates/velocity/fixtures/ime-and-unicode.rs --data-dir "$VELOCITY_ROUND6_GUI/data" --log-input
```

At scale 2, 2048×1089 logical is 4096×2178 physical. Window constraints can reduce it;
**record startup.actual_physical_size**, scale and refresh rate. Use corresponding logical
sizes on a 1× display. Performance runs above omit the debug layer; retain the validation
run separately. Repeat the open/scroll/typing commands with fresh output names for five runs.
Fill MANUAL_TEST.md for native fonts, IME, focus, appearance and pointer behavior.

Open waits for positive native scanout of an exact, current-version highlighted viewport.
`first_highlighted_screen.after_content_scanout_ms` compares that drawable with first content
scanout (≤150 ms). Drawable sequence correlation also handles skipped/zero-time callbacks;
present-return is never relabeled as scanout. Scroll and typing wait for document highlighting
and analysis before collecting samples. Scroll reports `minimap_gpu_summary.render_pass_ms`
for the isolated minimap pass (p95 ≤1 ms), plus full-frame GPU and CPU metrics. Native scroll
CPU p95 must be ≤2 ms. Typing's `inject_to_present_return` must have p50 ≤2 / p95 ≤4 ms.
Its scripted timer boundary is app receipt, not OS/HID input; the full boundary is in JSONL.
Missing timestamps, external input, occlusion/display failures and watchdog exits are not passes.

## Integration and current limits

Each buffer owns one HighlightService; split views reuse its results. A dispatcher moves the
service's cooperative 2ms request and indivisible regex calls off the UI thread. Exact
transaction descriptors feed inserts/snippets/paste/delete and language edits; undo/redo and
unobserved generic buffer operations rebuild from the current snapshot. Scope parsing uses a
separate incremental worker with convergence and branch corrections. It publishes current scope prefixes while parsing and refreshes corrected prefixes before
assembling symbols/folds; stale versions/schemes cannot populate the view.
Selector bindings require coverage of every caret; pending scopes fail both positive and
negative selector predicates. Large result retirement
runs off-thread. The original plain-text YAML fallback is composed into a caller-owned cached
source tree; optional bundles copy public syntaxes/metadata with their license notices.

Wrap geometry uses graphemes, configurable tabs, word boundaries and a prefix-count tree.
Same-line edits rewrap only edited lines. Physical row splicing may rebuild counts, but does
not rewrap the unchanged suffix. Unwrapped folds project tracked named regions without laying
out hidden text; highlight requests cover only visible intervals. A bounded, collision-checked
cell-layout cache reuses unchanged Unicode geometry. The minimap retains sampled token spans,
updates edited samples/row mappings incrementally and refreshes exact colors on worker delivery.
Full symbol/fold/tag aggregation runs on workers after scope convergence. Generic commands
and undo/redo use the documented snapshot-rebuild fallback when the shared buffer does not
expose transaction descriptors; those paths are outside the one-character rewrap measurement. The inherited
vel-syntax reindent compatibility gaps remain documented in its RESULTS.md. No project symbol
index or paragraph bidi is claimed by this round.

Lane-B API requests (optional cleanup; no functional blocker):

- vel-highlight: expose an enqueue-only viewport request, plus versioned scope snapshots at
  positions/lines. This would remove the extra dispatcher and duplicate scope interpreter
  while preserving nonblocking UI access, multiple visible intervals, corrections and
  scheme/version filtering.
- vel-syntax: allow multiple roots/additional in-memory definitions in load_cached while
  retaining lazy contexts and stable resource paths. This removes the temporary composed
  source tree used to include our Plain Text fallback.

# Round 5: panels, navigation, project tree and search

All commands start in `/Users/zoro/dev/velocity`. Do not commit this lane's work. The new
`vel-ui` and `vel-search` crates are workspace members through the existing `crates/*` glob.
No lane-B crate/API is required; the symbol provider is deliberately an integration hook.
The following checks target all touched crates plus the existing render/file-IO seams, so
parallel syntax/highlight work is not formatted or edited by these commands.

```sh
cd /Users/zoro/dev/velocity
cargo fmt -p vel-ui -p vel-search -p velocity -p vel-platform -p vel-config -p vel-commands -p vel-render -p vel-fileio --check
cargo clippy -p vel-ui -p vel-search -p velocity -p vel-platform -p vel-config -p vel-commands -p vel-render -p vel-fileio --all-targets -- -D warnings
cargo test -p vel-ui -p vel-search -p velocity -p vel-platform -p vel-config -p vel-commands -p vel-render -p vel-fileio
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-ui -p vel-search -p velocity -p vel-platform -p vel-config -p vel-commands -p vel-render -p vel-fileio --no-deps
python3 crates/velocity/scripts/measure.py --self-test
sh -n crates/velocity/scripts/bundle.sh
```

Automatic CPU measurements (no native initialization):

```sh
cargo run --release -p vel-search --example search_bench
cargo run --release -p vel-config --example keymap_bench
cargo run --release -p velocity -- --headless-smoke --carets 1
cargo run --release -p velocity -- --headless-smoke --carets 1000
```

The search example prints JSONL and exits. Its generated 100k path set, query sequence,
match density, sample counts and exact timing boundaries are documented in vel-search/README.md.
Budgets: fuzzy worker queue+score <=10 ms per query; literal highlights over exactly 100 MB
<=200 ms; 60-line viewport <=5 ms. Construction/index preparation is outside timing.
RESULTS.md records the measured quantiles and the earlier P0 6–73 ms comparison. UI polling,
scene construction, GPU time and actual display presentation are separate boundaries.

On an awake/unlocked logged-in Mac **outside the sandbox**, this new native mode runs a fixed
100-action sequence over 100k palette candidates: open palette, type three narrowing queries,
navigate, cancel, open find, type, open replace, hide and toggle the side bar. It repeats ten
times, emits exactly 100 `panel_sample` records and one `panel_summary`, and exits without any
human input or file changes. All native automatic modes retain display preflight, watchdog,
external-input contamination checks and a positive presentedTime startup boundary.

```sh
cargo build --release -p velocity
./target/release/velocity --bench-panels
./target/release/velocity --bench-panels --width 2048 --height 1089
python3 crates/velocity/scripts/measure.py --validate crates/velocity/output/panels-REPLACE-WITH-RUN.jsonl
```

The app prints its unique JSONL path; pass that actual path to `--validate`. It never overwrites
an old result. Panel timings are controller action -> worker polling -> scene -> queue submit /
present-return. They are neither HID delivery nor per-action scanout. The initial palette-open
sample includes candidate preparation; later keys reuse it. The 100k path matcher CPU benchmark
is the direct per-keystroke budget check. Check visual UI behavior with MANUAL_TEST.md separately.
No GUI mode, display wake/unlock or Metal initialization was attempted in the sandbox.

The existing automatic startup/floor/scroll/typing/memory matrix remains required. It now also
includes panel runs at default/4K sizes. Build and run it exactly as below; every process exits
by itself and blocked displays still fail fast with the observed reason.

```sh
mkdir -p crates/velocity/output
swiftc -O -module-cache-path /tmp/velocity-swift-module-cache \
  crates/velocity/scripts/native-floor.swift -o crates/velocity/output/native-floor
python3 crates/velocity/scripts/measure.py --floor crates/velocity/output/native-floor --runs 10
```

Human feature review (not a timing mode):

```sh
./target/release/velocity --log-input --project crates/velocity/fixtures/panels.sublime-project \
  crates/velocity/fixtures/ime-and-unicode.rs
```

| Keys | Added action |
| --- | --- |
| Cmd+P / Cmd+Shift+P | Goto Anything / command palette |
| Cmd+R / Ctrl+G | Current-file symbol hook / Goto line |
| Cmd/Ctrl+Enter in Goto | Pin target in a simple side-by-side split |
| Cmd+F / Cmd+Alt+F / Cmd+I | Find / Replace / incremental find |
| Cmd+Shift+F | Find in Files (Tab cycles Find, Replace, Where) |
| Cmd+G / Cmd+Shift+G | Find next / previous |
| Alt+Enter | Select all matches |
| Cmd+E / Cmd+Shift+E | Slurp find / replacement string |
| Cmd+Alt+G / Cmd+Alt+Shift+G | Find under / previous under |
| Ctrl+Shift+H / Ctrl+Alt+Enter | Replace next / all |
| Cmd+K, Cmd+B / Ctrl+0 | Toggle / focus side bar |
| Cmd+Alt+R / Cmd+Alt+C / Cmd+Alt+W | Toggle regex / case / whole word |
| Esc / Up/Down / PageUp/PageDown | Restore/cancel overlay or close panel / navigate rows |

The find panel also has clickable regex, case, word, wrap, selection and preserve-case flags,
plus Find All and Find Next/Replace All buttons. Enter in the Files panel starts streaming
search; the palette's Find: Replace in Files prepares a replacement plan after that search
finishes. Return to the Files panel to edit Replace/Where, rerun when query/scope flags change,
then invoke Replace in Files. Save dirty target documents first. Confirmation lists match/file
counts; each written original is kept as `NAME.velocity-backup-TIMESTAMP`. A file changed since
preview is rejected; batch writes are atomic per file, with backups retained on partial failure.
Where supports comma-separated folders, files, `<project>`, `<open files>`, include globs and
`-exclude` globs. Unsaved buffers are searched in `<open files>`; regex disk search is line-oriented.

The side bar loads directories only when expanded, with shared ignore/exclusion rules. Single
click previews; double click/Enter pins. Open Files and folders support keyboard navigation.
`open_dir` / `add_folder` accept `{"dir":"/path"}` or show a native directory picker through the
palette. `.sublime-project` paths resolve relative to that project. Added roots are session-only;
`.sublime-workspace` persistence is deferred. Tabs scroll with a wheel over the tab bar, close
with their button and reorder by dragging. Status reports position, selections, encoding, EOL,
indentation and Plain Text. Syntax highlighting and actual @symbol results await the next round.

The original IME checklist was renamed to MANUAL_TEST.md with all IME cases preserved.
The preceding run commands and new checklist are authoritative for round 5. Earlier round
notes and the existing hardware methodology follow for reference.

---

# Lane A/P1 reviewer runbook

Run the GUI commands on the logged-in Mac outside the sandbox. The sandbox has no
window server or Metal; only headless checks and helper compilation were run there.
macOS 14+ is required. Commands start in the repository root:

```sh
cd /Users/zoro/dev/velocity
cargo fmt --all
cargo clippy -p vel-platform -p vel-render -p velocity --all-targets -- -D warnings
cargo test -p vel-platform -p vel-render -p velocity
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-platform -p vel-render -p velocity --no-deps
python3 crates/velocity/scripts/measure.py --self-test
sh -n crates/velocity/scripts/bundle.sh
```

`cargo test` never runs an NSApplication, creates a window or requests a Metal device.
CoreText font/bitmap tests and kernel RSS/footprint observations are headless. The
parallel vel-text/vel-buffer lane may be changing; the commands above target this
lane's crates. Leave the other lane's files intact if its dependency build is broken.

## CPU-only smoke check

```sh
cargo run --release -p velocity -- --headless-smoke
```

It uses 5,000 bundled lines, applies 50 mixed Unicode edits through the common text
handler, builds visible scenes, checks Undo/Redo, records quantiles and kernel memory,
then exits. Output is a unique file under crates/velocity/output. These are CPU smoke
numbers, not GUI input/GPU/startup results. `--help` also avoids native initialization.

## Automatic hardware matrix and comparable native floor

Stop unrelated builds if possible. Record macOS/hardware, AC/battery, active displays,
scale, refresh, load averages and input layout in RESULTS.md. Keep the benchmark window
visible and do not type or move the mouse over it. Disabling auto-lock during the run
may be useful; an occluded/minimized window can cause retries or watchdog failure.

```sh
mkdir -p crates/velocity/output
swiftc -O -module-cache-path /tmp/velocity-swift-module-cache \
  crates/velocity/scripts/native-floor.swift \
  -o crates/velocity/output/native-floor
python3 crates/velocity/scripts/measure.py \
  --floor crates/velocity/output/native-floor --runs 10
```

The script builds release velocity, generates a deterministic ASCII 5,000-line workload,
then runs one cold-ish startup, 10 warm startups interleaved with 10 native-floor launches,
600 scroll frames after 60 warmup frames (default and requested 4K sizes), 500 jittered
insertions with 1 and 1,000 carets (default and requested 4K), and a full memory sweep. Each process exits on its
own. It captures stderr, validates every run and writes startup-summary.jsonl in a new
UTC-stamped output directory. Existing files are never overwritten by the app.

The new floor helper is adapted from prototypes/macos-shell/native-floor/floor.swift,
with the same clear-only workload and window size, but now records drawable.presentedTime
and callback receipt separately. Both it and velocity measure actual presentation.
The original P0 floor can also be passed with `--floor`, but its console timing is handler
receipt, which is labeled in captured JSONL. Neither floor does text/atlas/pipeline work.

Useful variations:

```sh
python3 crates/velocity/scripts/measure.py --only-startup --runs 10 \
  --floor crates/velocity/output/native-floor
python3 crates/velocity/scripts/measure.py --dry-run --only-startup --runs 1
python3 crates/velocity/scripts/measure.py --skip-build --input path/to/large.rs
./target/release/velocity --bench-memory --lines 0 path/to/large.rs --timeout 300
./target/release/velocity --bench-scroll --width 2048 --height 1089
./target/release/velocity --bench-typing --width 2048 --height 1089
```

`--width 2048 --height 1089` is 4096×2178 pixels at scale 2. Window constraints and scale
can change the actual resolution; **use startup.actual_physical_size** as the workload.
Use the appropriate logical size on a 1× 4K display. The mode does not toggle fullscreen.
For interactive fullscreen use the native green window control. ProMotion requests up
to NSScreen.maximumFramesPerSecond; test 120 Hz on the internal display separately.

`--output path/to/new.jsonl` selects another result path. `--present fifo` is accepted;
other present modes error before GUI creation. Default watchdog is 180 seconds, including
initialization. A timeout, missing scanout or external input invalidates a run.

## What to inspect

- Startup phases must show first_scene_ready → first_offscreen_submitted →
  first_frame_prepared_before_order_front → ordered_front → first_submit →
  first_present_returned. The complete text frame is GPU-submitted before orderFront;
  surface acquisition/copy/present run immediately after orderFront in the same
  applicationDidFinishLaunching callback. No redraw event/display-link tick is awaited.
  Count surface_retries. A first_frame_reprepared_after_native_resize phase means
  orderFront changed size/scale; the first texture was rebuilt immediately in the
  launch callback, with no redraw round trip. The startup record is emitted only after positive presentedTime.
- scanout_main_ms / scanout_process_ms measure actual drawable presentation. The latter
  includes kernel process birth→main when permitted. presented_callback_main_ms and
  scanout_to_callback_ms expose delivery lag. Milestones are sorted by actual timestamp;
  callback receipt can occur after later launch phases. first_present_returned is a
  different boundary and may precede physical presentation.
- environment.renderer records Fifo/latency=1, device_creation_ms, shader_module_ms,
  pipeline_creation_ms and pipeline_cache_supported/status. Pinned wgpu 30 Metal does
  not implement PipelineCache. Compare cold-ish/warm timings; do not describe driver
  caching as an app-persisted Metal binary cache. Cold-ish means first launch after build,
  with system/driver/font caches left intact; it is not a cold boot measurement.
- Scroll CPU is callback receipt→queue submit including layout, glyph lookup, uploads and
  acquisition. GPU is clear + primitive render pass, with deferred readback after 600
  frames. Timestamp unavailability yields null plus an explicit status, not a zero.
- Typing is one-shot NSTimer→keymap lookup→command→vel-buffer→submit /
  present-return. It measures the app path, not HID delivery or display scanout. Seed and
  30..90 ms jitter are recorded. The injector mixes ASCII, Chinese, emoji, combining marks
  and newlines; confirm that exactly 500 unique sample indices exist.
- memory/after_first_frame waits for GPU completion after first scanout; compare
  phys_footprint_bytes to Activity Monitor and the 80 MB target. rss_bytes is resident
  size, never relabeled as footprint. Errors/methods are independent. after_full_sweep
  visits every viewport while keeping caches bounded. Allocation metadata is not total
  process memory. Record both fields even when they exceed budget.

Budgets on the same M1 Pro session: warm scanout ≤ native floor + 30 ms; typing p50 ≤2 ms,
p95 ≤4 ms; 4K scroll CPU p95 ≤1.5 ms; GPU pass p95 ≤3 ms; first-frame footprint ≤80 MB.
Fill RESULTS.md with actual numbers, sample counts, run paths and failure/retry notes.
Small startup sample p95 is descriptive, not a confidence bound.

Validate saved application JSONL without opening a window:

```sh
python3 crates/velocity/scripts/measure.py --validate path/to/startup.jsonl path/to/scroll.jsonl
```

## Native input, appearance and bundle review

```sh
./target/release/velocity --log-input crates/velocity/fixtures/ime-and-unicode.rs
sh crates/velocity/scripts/bundle.sh
open -n crates/velocity/dist/Velocity.app --args \
  --log-input /Users/zoro/dev/velocity/crates/velocity/fixtures/ime-and-unicode.rs
```

The script produces dev.velocity.Velocity with NSHighResolutionCapable and macOS 14
minimum, preserving any prior bundle as a timestamped backup. It neither signs nor
installs the app. Bundled/default source is compiled into the executable; it does not
rely on the working directory. Check About/Hide/Quit and every Edit action in terminal
and bundle launches. Follow MANUAL_TEST.md, then test resize, fullscreen, light/dark change,
window movement between Retina/non-Retina displays, occlusion/unocclusion and idle CPU.
Check Metal validation output and physical candidate-window placement.

Return the entire stamped output directory (JSONL and stderr), completed IME checklist,
and OS/display/load observations when resuming the session. Hardware budgets and native
behavior remain unverified until those results are available.

## Round 4: settings, commands, tabs and real file IO

The previous single-selection demo is replaced by vel-buffer. All editing commands, IME
commits and benchmark insertions enter the same command layer. Highlighting is plain text;
`view::Highlighter` and `vel_config::keymap::ScopeProvider` are the later syntax seams.

Run the complete lane checks (all are headless):

```sh
cargo fmt --all
cargo clippy -p vel-config -p vel-commands -p vel-fileio -p vel-platform -p vel-render -p velocity --all-targets -- -D warnings
cargo test -p vel-config -p vel-commands -p vel-fileio -p vel-platform -p vel-render -p velocity
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-config -p vel-commands -p vel-fileio -p vel-platform -p vel-render -p velocity --no-deps
cargo run --release -p vel-config --example keymap_bench
cargo run --release -p vel-fileio --example load_bench
cargo run --release -p velocity -- --headless-smoke --carets 1
cargo run --release -p velocity -- --headless-smoke --carets 1000
```

The headless workload now uses the same eight-way Unicode/newline/ASCII mixture as GUI typing.
Its CPU scene timings cannot be compared directly with the older demo's four-character smoke
samples. No GUI budget is established by these CPU measurements.

The reviewer's asleep/locked display failure is fixed in both startup paths. Each milestone
is a flushed JSONL record as it happens. The prepared Velocity texture survives each present
until a positive `presentedTime` is observed. A skipped first drawable is expected and retried.
`first_real_scanout` is the Metal host-time value; `scanout_main_ms` is its elapsed main-time
counterpart. `first_callback_main_ms`, the first positive drawable's callback time, and
`skipped_drawables` are separate. Native-floor emits JSONL too, including skipped callbacks.

Before launching, measure.py and the app check `CGDisplayIsAsleep` and the console lock/session.
While waiting, the app/helper also check hosting-window occlusion. Initial occlusion gets a
one-second grace for orderFront; asleep/locked states fail immediately. Errors name the blocker
instead of waiting for the long watchdog. A fallback watchdog still bounds unrelated native hangs.

On the logged-in, awake Mac, keep the window visible and run:

```sh
cargo build --release -p velocity
swiftc -O -module-cache-path /tmp/velocity-swift-module-cache \
  crates/velocity/scripts/native-floor.swift -o crates/velocity/output/native-floor
python3 crates/velocity/scripts/measure.py --self-test
python3 crates/velocity/scripts/measure.py --floor crates/velocity/output/native-floor --runs 10
./target/release/velocity --bench-typing --carets 1
./target/release/velocity --bench-typing --carets 1000
```

The full matrix includes 1/1,000 carets at default/4K logical sizes, plus startup, scrolling,
native floor and memory. Typing samples include keymap lookup → command → vel-buffer → submit/
present-return. They remain app-receipt timings, not HID-to-scanout measurements.

Measure the existing 100 MB fixture through the asynchronous editor-open path:

```sh
./target/release/velocity --bench-open spikes/data/text-100mb.txt
```

Or generate a separate ASCII fixture:

```sh
python3 - <<'PY'
from pathlib import Path
path = Path('crates/velocity/output/open-100mb.txt')
if not path.exists():
    block = b'plain text loading workload\n' * 2400
    with path.open('xb') as stream:
        left = 100_000_000
        while left:
            part = block[:left]
            stream.write(part)
            left -= len(part)
PY
./target/release/velocity --bench-open crates/velocity/output/open-100mb.txt
```

This mode exits after both actual first scanout and completed loading. Compare startup
`scanout_main_ms` to 300 ms and `file_open/fully_loaded.elapsed_ms` to 1,000 ms. The first
prepared frame contains a progress tab. CPU worker timing is also recorded; `fully_loaded`
is receipt on the UI poll, so it includes up to 200 ms polling delay. Validate with
`python3 crates/velocity/scripts/measure.py --validate path/to/open.jsonl`.

Interactive examples:

```sh
./target/release/velocity crates/velocity/fixtures/sample.rs crates/velocity/fixtures/ime-and-unicode.rs:8:3
printf 'hello\n你好\n' | ./target/release/velocity -
./target/release/velocity --data-dir /path/to/Velocity-data --project /path/to/project.sublime-project
```

Default data directory: `~/Library/Application Support/Velocity`, or `VELOCITY_DATA_DIR`.
Loose overrides go in `Packages/User/Preferences.sublime-settings` and
`Packages/User/Default.sublime-keymap`; archives go in `Installed Packages`. Settings and
keymaps reload after changes (including newly created User overrides). `Plain Text.sublime-settings`
is the syntax layer for this round; project `settings` follow it. Per-view settings stay last.
Regex context syntax is Rust regex; unsupported patterns produce an explicit resource error.
Default resources in resources/Packages/Default were written for Velocity from scratch.

Primary shortcuts:

| Keys | Action |
| --- | --- |
| Cmd+N / O / S / Shift+S / W | New / Open / Save / Save As / Close tab |
| Cmd+Alt+S | Save all |
| Ctrl+Tab / Ctrl+Shift+Tab; Cmd+1…9 | Next / previous; choose tab |
| Cmd+Z / Cmd+Shift+Z; Cmd+U / Cmd+Shift+U | Undo / redo; soft undo / redo |
| Cmd+C / X / V / A | Clipboard / select all |
| Cmd+D; Cmd+K, Cmd+D; Cmd+Ctrl+G | Add next match; skip; select all matches |
| Cmd+click; Ctrl+Alt+Up/Down; Cmd+Shift+L | Add caret; add adjacent caret; split selection into lines |
| Esc | Collapse to one selection; cancel active IME composition |
| Cmd+L; Cmd+J; Cmd+Shift+D | Expand line; join lines; duplicate lines |
| Cmd+[ / ]; Cmd+Ctrl+Up/Down | Unindent / indent; swap lines |
| Tab / Shift+Tab; Enter | Tab or configured spaces / unindent; auto-indent newline |
| Cmd+K, Cmd+U/L/B | Upper / lower case / toggle gutter |
| Cmd+= / Cmd+- / Cmd+0 | Increase / decrease / reset font size |

Arrow/Shift+arrow, Option+arrow/deletion, Cmd+arrows, Home/End and PageUp/PageDown are
implemented. Minimal bracket/quote pairing and closing-character skip use contextual bindings.
New/close window commands use a separate process/window; recorded text macros are per view.
This round-4 section predates the round-5 palette/panels; snippet field navigation, syntax highlighting and wrap_lines remain deferred.
Use MANUAL_TEST.md's editing smoke checklist before accepting native behavior.
