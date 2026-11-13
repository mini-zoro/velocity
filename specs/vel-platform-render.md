# Spec: `vel-platform` (native AppKit) + `vel-render` (wgpu) + `velocity` app shell

Status: P1 / lane A (platform, rendering, UI). Locations: `crates/vel-platform/`, `crates/vel-render/`,
`crates/velocity/` (binary). macOS only for now: gate platform code with
`#[cfg(target_os = "macos")]` so `cargo check` on other targets still compiles empty stubs.

## Why (decided in P0, see docs/PLAN.md §3.6 and prototypes/macos-shell/RESULTS.md)

The winit + wgpu prototype renders a 4K screen of code in ~1.6 ms GPU and handles typing in < 2 ms,
but reaches the first frame only after ~390 ms (winit event loop 118 ms + window 136 ms + ~100 ms
waiting for the first redraw), while a native Swift AppKit + CAMetalLayer program reaches actual
scanout in ~150–175 ms on the same machine. winit on macOS also ignores IME replacement ranges.
So: **own AppKit layer (objc2) + keep wgpu**. Reuse what works from `prototypes/macos-shell/`
(atlas, glyph cache, shaders, font fallback with CoreText bitmap fallback, measurement tooling) —
moving that code into the product crates is expected; clean it up as you move it.

## `vel-platform` — AppKit integration (objc2 0.6 / objc2-app-kit 0.3 family)

- App bootstrap: `NSApplication` with our delegate (`define_class!`), activation policy regular,
  minimal main menu (App menu with About/Hide/Quit, Edit menu with Undo/Redo/Cut/Copy/Paste/Select
  All wired to editor commands). Bundle-less execution must work (dev), and an `.app` bundle
  layout must be producible by a script (`scripts/bundle.sh`) with Info.plist
  (`NSHighResolutionCapable`, bundle id `dev.velocity.Velocity`).
- Window: `NSWindow` + our `NSView` subclass that is layer-hosting with a `CAMetalLayer`
  (`wantsUpdateLayer`, `layerContentsRedrawPolicy` as appropriate), correct `contentsScale` on
  backing changes, live resize redraws without stretching, full-screen, tabbing disabled for now,
  dark/light appearance change notification.
- **Startup path**: everything needed for the first frame happens in
  `applicationDidFinishLaunching` (or earlier) and the **first frame is rendered and presented
  before/at `orderFront`**, without waiting for a display-link tick or a redraw event round trip.
  Measure with `MTLDrawable.addPresentedHandler` (true scanout) as well as present-return.
- Frame pacing: `CADisplayLink` from the view (macOS 14+) driving redraws **only when dirty**;
  no busy loop; ProMotion aware (request up to the display's max rate while scrolling/animating).
- Input:
  - Keys: `keyDown:`/`keyUp:`/`flagsChanged:` → a platform-neutral `KeyEvent` (physical key code,
    characters, characters-ignoring-modifiers, modifiers incl. left/right, repeat flag, timestamp).
    Route through `interpretKeyEvents:` so the input method sees every key first.
  - **`NSTextInputClient`, complete**: `insertText:replacementRange:`,
    `setMarkedText:selectedRange:replacementRange:`, `unmarkText`, `hasMarkedText`, `markedRange`,
    `selectedRange`, `attributedSubstringForProposedRange:actualRange:`,
    `validAttributesForMarkedText`, `firstRectForCharacterRange:actualRange:` (candidate window at
    the caret, in screen coordinates, correct on Retina and multi-display), `characterIndexForPoint:`,
    `doCommandBySelector:` (map standard selectors like `deleteBackward:`, `insertNewline:`,
    `moveLeft:` … to editor commands). Ranges are **UTF-16** (use `vel_text` conversions). Honor
    replacement ranges (Pinyin/Japanese reconversion, emoji picker, press-and-hold accent menu).
  - Mouse: down/drag/up with click count and modifiers; scroll wheel with precise deltas and
    momentum/phase; cursor shape (I-beam over text).
- Services: `NSPasteboard` text read/write. Open-file dialog and drag-and-drop: out of scope.
- API shape: a `Platform::run(app: impl AppHandler)` with callbacks (`on_key`, `on_text_input`
  (insert/marked/unmark), `on_mouse`, `on_scroll`, `on_resize`, `on_frame(&mut FrameContext)`,
  `on_appearance`, `on_quit_requested`) and a `WindowHandle` with `request_frame()`,
  `set_title`, `scale_factor`, `content_size`, plus the IME query side as a trait the app
  implements (`text_input_state()` returning selected/marked UTF-16 ranges and caret rect).

## `vel-render` — wgpu renderer

- wgpu 30 Metal backend created from our `CAMetalLayer` (`SurfaceTargetUnsafe::CoreAnimationLayer`).
  Fifo present mode only (Immediate/Mailbox showed problems in P0), `desired_maximum_frame_latency = 1`.
- Primitives batched by instancing: solid/rounded rects (selection, caret, current line, gutter),
  glyphs from an atlas (swash raster, CoreText bitmap fallback for glyphs swash can't render —
  P0 found PingFang needs it), underline/squiggle, simple shadow. One draw call per primitive
  kind per layer; a `Scene` API the UI layer fills each frame.
- Glyph atlas: grow on demand (multiple pages), LRU eviction, subpixel positioning buckets,
  color glyphs (emoji) in an RGBA page.
- **Pipeline cache** persisted to disk (wgpu `PipelineCache` if supported on Metal; otherwise
  measure and document) to cut cold-start shader compilation (P0 cold-ish device creation: 1.49 s).
- Text shaping: monospace fast path for ASCII runs (no shaping, advance = cell width), shaping via
  swash/harfrust for non-ASCII runs with font fallback (PingFang SC for CJK, Apple Color Emoji).
  Line layout cache keyed by line content hash + style.

## `velocity` — app binary (demo editor view for now)

- Opens a file given on the command line (default: a bundled sample) into a `vel_text::Text`
  (do **not** depend on `vel-buffer`; another agent is building it in parallel), shows gutter with
  line numbers, text with a trivial token coloring (reuse the prototype lexer), caret, selection,
  current-line highlight, smooth trackpad scrolling, typing / Backspace / Enter / arrows, IME
  preedit rendered inline with underline.
- Measurement modes ported from the prototype: `--bench-startup` (phases incl. presented
  scanout), `--bench-scroll`, `--bench-typing`, `--bench-memory` (report **phys_footprint** and
  resident size). Same JSONL format so `prototypes/macos-shell/scripts/measure.py` concepts carry
  over; put scripts in `crates/velocity/scripts/`.

## Budgets (M1 Pro, release, measured by the reviewer outside the sandbox)

| Metric | Target |
|---|---|
| Warm start → first frame **scanout** (presentedHandler), 1200×800 window, 5,000-line file | ≤ native floor + 30 ms (floor ≈ 150–175 ms at loadavg 6–8; re-measure floor in the same session) |
| Typing: input → present-return, Fifo | p50 ≤ 2 ms, p95 ≤ 4 ms |
| Scroll frame CPU (event → submit), 4K window | p95 ≤ 1.5 ms |
| GPU frame time, 4K full screen of text | p95 ≤ 3 ms |
| phys_footprint after first frame (5,000-line file) | ≤ 80 MB (report what it is; Sublime ~55–80 MB) |

## Constraints

- Your sandbox has **no window server and no Metal**. Keep logic testable headlessly (layout math,
  atlas packing, IME range/geometry state machine, UTF-16 mapping, scene batching, stats). Write
  `crates/velocity/RUNBOOK.md` with exact commands; the reviewer runs the GUI and returns JSONL.
- `unsafe` is unavoidable for objc2/Metal interop: keep it inside `vel-platform`/`vel-render`,
  every block with a `// SAFETY:` comment (workspace lint `undocumented_unsafe_blocks` is deny).
- Workspace rules: fmt, `clippy --workspace --all-targets -D warnings`, tests, rustdoc.
- Do not modify `crates/vel-text` or `crates/vel-buffer` (other lane). Only add the three crates
  above (+ root `Cargo.lock`). The root `Cargo.toml` already includes `crates/*`.

## Deliverables

Three crates with README design notes, `crates/velocity/RUNBOOK.md`, an updated `IME_TEST.md`
(copy from the prototype, adapted), and `crates/velocity/RESULTS.md` (Chinese) as a template to be
filled after the reviewer's run.
