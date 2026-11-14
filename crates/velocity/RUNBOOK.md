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
and bundle launches. Follow IME_TEST.md, then test resize, fullscreen, light/dark change,
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
Palette UI, snippet field navigation, syntax highlighting and wrap_lines remain deferred.
Use IME_TEST.md's editing smoke checklist before accepting native behavior.
