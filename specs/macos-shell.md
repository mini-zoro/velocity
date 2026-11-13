# Spec: macOS shell prototype (P0 / lane A)

Location: `prototypes/macos-shell/` (standalone cargo workspace with its own `[workspace]` table;
**not** a product crate). Purpose: answer the three open P0 questions with numbers before we write
`vel-platform` / `vel-render` / `vel-layout` for real.

1. **Startup** — can a winit + wgpu (Metal) window show a full screen of code in ≤ 100 ms (warm)?
   Where does the time go?
2. **Frame cost and input latency** — is a glyph-atlas + instanced-quad renderer comfortably inside
   the frame budget at Retina resolution (and at 120 Hz ProMotion), and what is the
   key-event → frame-presented latency?
3. **IME** — does winit's IME support on macOS give a correct Chinese (Pinyin) input experience:
   preedit rendering, candidate window positioned at the caret, commit, cancellation?

Throwaway code is fine, but measurements must be honest and reproducible.

## What to build

A single binary `vel-shell` (`cargo run --release -- [flags] [file]`):

- Window via **winit** (latest stable release; if you choose a pre-release, justify it) and
  rendering via **wgpu** (Metal backend). Retina-aware (`scale_factor`), resizable, dark
  background.
- Load a text file (default: first 5,000 lines of `spikes/data/rust-10mb.rs`; fall back to a
  generated file if missing) into a simple `Vec<String>` buffer (no rope needed here).
- **Text rendering, monospace fast path**: font = Menlo (or SF Mono if available) from
  `/System/Library/Fonts`, ~13pt × scale factor. Rasterize glyphs on demand into a **glyph atlas
  texture** (try `swash`; optionally compare with CoreText rasterization via `objc2-core-text`
  for visual fidelity and report), cache by (glyph id, font, subpixel x-offset bucket). Draw all
  visible glyphs with **one instanced draw call** (plus one for rects). Per-token colors from a
  trivial lexer (keywords / strings / comments / numbers / identifiers) — not a syntax engine.
- **CJK fallback**: glyphs missing from the primary font come from PingFang SC (or the system
  fallback found via CoreText); include Chinese text and emoji in a test file.
- Caret (blinking off), selection rectangles, current-line highlight, a gutter with line numbers.
- Editing: printable input, Backspace, Enter, arrow keys, Cmd+A/Cmd+C/Cmd+V not required.
- Scrolling: trackpad pixel-precise scrolling and PageUp/PageDown.
- **IME**: `set_ime_allowed(true)`; handle `Ime::Enabled/Preedit/Commit/Disabled`; render the
  preedit string inline at the caret with an underline and the preedit cursor; call
  `set_ime_cursor_area` with the caret rectangle every time the caret moves so the candidate
  window follows the caret. Log every IME and keyboard event with timestamps when `--log-input`.
- Frame pacing: request redraws only when something changes (no busy loop), present mode Fifo with
  `desired_maximum_frame_latency = 1`; try Mailbox/Immediate as an experiment and report.

## Measurements (automated modes; each prints one JSON line per result to stdout)

- `--bench-startup`: timestamps (monotonic, relative to process start obtained from
  `proc_pidinfo`/`sysctl KERN_PROC` start time, and also relative to `main()` entry) for:
  main entered, event loop ready, window created, wgpu instance, adapter, device+queue, surface
  configured, font loaded, atlas warmed for the first screen, first frame encoded, first
  `present()` returned; then exit. A helper script `scripts/startup.sh` runs it 1× after a fresh
  build ("cold-ish") and 10× warm and prints the median/P95 of each phase.
  If wgpu instance/adapter/device creation is a large share, also measure a minimal raw-Metal
  path (`objc2-metal` + `CAMetalLayer`: create device, layer, clear + present) to compare.
- `--bench-scroll`: scroll by one line per frame for 600 frames; record CPU time per frame
  (event→encode→submit) and **GPU time per frame via timestamp queries** if
  `Features::TIMESTAMP_QUERY` is available on Metal (report if not). Report P50/P95/P99/max.
  Run at the default window size and at a large window (e.g. 3000×1800 logical points or
  fullscreen) to approximate 4K/5K.
- `--bench-typing`: a timer injects 500 synthetic character insertions (through the same code
  path as real key input, starting from the event handler) at randomized 30–90 ms intervals;
  measure inject → frame submitted, and inject → `present()` returned. Additionally, with
  `--log-input`, real key presses log event-receive → present latencies so a human can type and
  get numbers.
- Memory: RSS after first frame and after scrolling through the whole file (`task_info`).

## Constraints

- Work only in `prototypes/macos-shell/` (+ read `spikes/data/`). No git commits.
- **Your sandbox has no access to the window server or Metal** (`MTLCreateSystemDefaultDevice()`
  returns nil inside it). You cannot run the GUI yourself. Therefore:
  - keep all non-GPU logic (lexer, layout math, atlas packing, IME state machine, latency stats,
    JSON output) in pure functions with unit tests you *can* run;
  - make sure `cargo build --release` and `cargo test` pass;
  - make every measurement mode fully automatic (no human interaction), exit by itself, and
    write results to `prototypes/macos-shell/output/*.jsonl`;
  - write `RUNBOOK.md` with the exact commands to run the measurements;
  - a reviewer will run them outside the sandbox and send you the output and any panics/logs
    for the next iteration.
- Latest stable crates; record exact versions in `RESULTS.md`.

## Deliverables

- Source + `README.md` (architecture of the prototype) + `RUNBOOK.md`.
- `IME_TEST.md`: a manual checklist for a human tester (Pinyin: type `nihao` → candidate window at
  caret → choose `你好` → committed; Shift to toggle English mid-composition; Backspace inside
  preedit; Esc cancels preedit; candidate window follows the caret after scrolling/resizing;
  Japanese Kana with marked-text conversion; emoji picker Ctrl+Cmd+Space; dead keys Option+E then
  E → `é`; full-width punctuation), with an expected-result column.
- `RESULTS.md` (Chinese): filled in after the reviewer's run — tables per measurement, and a
  recommendation: keep winit or write our own AppKit layer (`NSTextInputClient`)? wgpu or raw
  Metal? swash or CoreText rasterization? Any blockers found.
