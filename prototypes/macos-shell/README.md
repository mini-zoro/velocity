# macOS shell prototype

Standalone `vel-shell` research binary for [the lane A spec](../../specs/macos-shell.md).
It measures a winit window and a Metal-only wgpu text renderer before those components become
product crates. Original source uses the repository's **MIT OR Apache-2.0** licensing.

Start with [RUNBOOK.md](RUNBOOK.md). [RESULTS.md](RESULTS.md) contains the verified first hardware
batch and recommendations; footprint reruns, manual IME/visual testing and 120 Hz remain pending.
The sandbox cannot create a usable window or Metal device. No GUI benchmark is part of `cargo test`.

```sh
cd /Users/zoro/dev/velocity/prototypes/macos-shell
cargo build --release
./target/release/vel-shell
```

## Architecture

The binary uses winit's `ApplicationHandler` on the main thread. `ControlFlow::Wait` is the idle
state. Edits, scrolling, marked text, expose, resize, and scale changes request a redraw. Synthetic
typing uses `WaitUntil` deadlines. Retryable surface acquisition failures retry after 16 ms;
occlusion pauses redraw requests until an unocclusion event. There is no idle animation or
blinking caret. Automatic modes have an independent watchdog, including during initialization.

| Module | Responsibility |
| --- | --- |
| `cli.rs` | Argument validation, mutually exclusive automatic modes |
| `editor.rs` | `Vec<String>`, grapheme movement/deletion, selection, transient IME state, bounded line-token cache |
| `lexer.rs` | Single-line keyword/string/comment/number/identifier colors |
| `layout.rs` | Logical-point cells, tab stops, gutter, selection, preedit underline/range/cursor, clipping and candidate anchor |
| `font.rs` | Menlo ASCII mapping; cached swash shaping for non-ASCII graphemes; lazy CoreText-resolved PingFang/emoji faces |
| `atlas.rs` | Pure padded shelf allocator and quarter-pixel phase calculation |
| `render.rs`, `quads.wgsl` | RGBA glyph atlas, instance buffer, two draws, deferred GPU timestamp readback |
| `native.rs` | Kernel process birth/RSS/physical footprint, font resolution, CoreText bitmap raster fallback |
| `report.rs`, `stats.rs` | Monotonic milestones, serialized JSONL sink, watchdog, quantiles, deterministic jitter/sweep planning |
| `app.rs` | Native event handling, common edit entry point, benchmark state transitions |
| `metal.rs` | Separate clear-only `CAMetalLayer` startup comparison |
| `cpu_tests.rs` | Cross-module regressions and device-free WGSL validation |
| `scripts/measure.py` | Fresh-build launch matrix, completeness checks, startup aggregation |

The local `[workspace]`, `Cargo.lock`, and `.cargo/config.toml` keep this prototype independent of
the product workspace. AppKit/QuartzCore features are limited to the APIs used here so unrelated
CloudKit/CoreData/CoreImage frameworks are not added by their default bindings. Its target directory is `prototypes/macos-shell/target/`. The helper scripts
also set `CARGO_TARGET_DIR` explicitly. It has no dependency on `crates/vel-text`.

### File and editor model

The default input is the **first 5,000 lines** of `../../spikes/data/rust-10mb.rs`, resolved relative
to the manifest. Missing default data generates Rust-like lines in memory; an explicitly supplied
missing file is an error. `--lines 0` reads the entire UTF-8 file. JSON records the source, loaded
bytes/lines and whether the limit truncated it. A terminal newline creates an empty last editor
line when the entire file is loaded. CRLF is normalized.

Printable input, Enter, Backspace, arrows, Home/End, PageUp/PageDown, trackpad pixel scrolling,
Shift+arrows, mouse placement and drag selection are supported. Edits stay in memory. Mouse and
arrow edits are deferred to the IME while a preedit is active. Composition does not modify the
buffer; commit replaces the selection once; empty preedit/disable clears marked text.

### Text and GPU path

Geometry uses 13-point Menlo cells and 20-point line height in logical points. Physical raster size
is `13 × scale_factor`. Font glyphs come from the system, with stable swash font cache identities.
System font files are mapped read-only instead of copying entire CJK/emoji collections into heap
memory. Only visited line tokens are cached, with a 1,024-line FIFO bound.

The common ASCII path performs direct charmap lookup. Non-ASCII extended graphemes are shaped
with swash and assigned whole-cell widths via `unicode-width`; missing glyphs select PingFang SC
or Apple Color Emoji discovered through CoreText. This is a code-editor fast path: it does not
provide paragraph bidi or shaping across adjacent graphemes.

A CPU raster regression on this system found that CoreText resolves PingFang to a UI face for
which swash produces no image. If swash cannot rasterize a glyph, the original **retained CTFont**
is sized and drawn into a CPU bitmap, then uploaded into the same atlas. Keeping the resolved
font avoids macOS silently substituting a different font when a private PostScript name is
requested again. Bitmap placement/orientation and CJK/emoji pixel production have headless tests.
This fallback is recorded in `fonts.coretext_fallback_raster_count` and warnings. It is not a
complete swash-versus-CoreText visual/performance comparison.

The cache key is `(face, glyph ID, physical ppem at 1/64 pixel, quarter-pixel x phase)`. Baselines
snap to physical pixels. The atlas has transparent one-pixel padding, starts at 2048² RGBA8, and
can grow to 4096². A growth rebuilds every instance's UVs before drawing. A scale change clears the
atlas. Oversized workloads fail explicitly instead of drawing stale UVs. Rectangles sample a
reserved white texel; per-instance clipping keeps scrolled text/selection out of the gutter.
There is **one rectangle draw plus one glyph draw**, independent of glyph count.

Fifo and `desired_maximum_frame_latency = 1` are the defaults. Experimental Mailbox/Immediate
requests are checked against surface capabilities; unsupported requests emit an error, with no
silent Fifo substitution. Format, supported modes, actual size, scale and monitor refresh are
included in JSON. Shader parsing and validation run through Naga without creating a GPU device.

### Measurement contracts

All measurements emit one JSON object per line to stdout and the same records to a unique
`output/*.jsonl`. Explicit `--output` paths must be new `.jsonl` files directly in that directory.
Progress/errors go to stderr. A successful automatic run ends with `kind: "complete"` and
`complete: true`; incomplete files must not be treated as successes. Scripts check sample counts,
RSS/footprint availability, sweep coverage, and physical-keyboard contamination. Legacy results
without the additive footprint field remain readable.

- Startup records `main_entered`, `measurement_ready`, `event_loop_ready`, `window_created`,
  `wgpu_instance`, `surface_created`, `adapter`, `device_queue`, `surface_configured`,
  `renderer_ready`, `file_loaded`, `font_loaded`, `atlas_warmed`, `first_frame_encoded`, and
  `first_present_returned`. Each has `main_ms`, `process_ms`, and `delta_ms` from the previous
  milestone. Pipeline creation is included in `renderer_ready`; atlas warm means CPU rasterization
  and queued texture writes, with upload submitted in the first frame.
- The kernel provides process birth in wall time through `proc_pidinfo(PROC_PIDTBSDINFO)`.
  A single wall snapshot at main entry anchors that birth to monotonic `Instant` deltas. Missing
  birth data produces null process-relative values and an error, without a fabricated zero.
  Small wall/monotonic sampling error remains; subsequent wall-clock changes do not change phases.
- CPU frame cost is **elapsed wall time** from `RedrawRequested` handler entry to queue submission,
  including scrolling, layout, raster cache misses, uploads and surface acquisition stalls.
  Acquisition and encode/submit are also reported separately. Present-return intervals describe
  application cadence, without proving display refresh/scanout. It is not thread CPU accounting.
- Scroll excludes the first frame and measures exactly 600 subsequent frames, one 20-point line
  per successful frame. Boundary-clamped samples are counted for short files. GPU time brackets
  the render pass (clear, rectangles, glyphs), using `TIMESTAMP_QUERY` when supported. It excludes
  uploads, resolve/copy/readback and presentation. All query readback waits until the 600 submissions
  have finished; there is no per-frame GPU fence/readback wait.
- Typing deadlines use seeded SplitMix64 jitter in inclusive 30–90 ms intervals. Each timer event
  enters the same `input()` edit handler as real keys. Late deadlines and coalesced insertions
  remain samples; timer lateness is reported separately. This measures the application path,
  without an OS/HID injection claim. Physical keyboard/IME events with `--log-input` timestamp
  receipt before logging and attach all pending changes to the next successfully submitted frame.
- **`present()` returning does not mean pixels were displayed.** wgpu 30 exposes it as
  `Queue::present`. No result here measures WindowServer composition, scanout or photons. External
  pixel/high-speed-camera testing is still needed for the end-to-end latency goal.
- RSS uses `task_info(MACH_TASK_BASIC_INFO).resident_size` after the first frame's GPU work has
  completed and after a viewport-by-viewport sweep. Scroll performs the sweep after its 600 samples;
  `--bench-memory` performs it directly. Default sweeps cover the loaded 5,000-line buffer;
  `--lines 0` covers the actual whole input file. RSS includes buffer, fonts, caches and native
  frameworks; driver allocations/shared GPU memory are not fully characterized by it. Atlas and
  instance-buffer allocation sizes are logged separately. Each memory record also includes
  `phys_footprint_bytes` from `task_info(TASK_VM_INFO, REV1).phys_footprint`, with an independent
  method/error. This is the charge to compare with Activity Monitor's Memory figures; it differs
  from RSS and can include compressed private memory. Historical RSS-only batches require a rerun
  for footprint. The native prefix ABI and short/error replies are covered by headless tests.
- Quantiles linearly interpolate sorted samples at `(N−1) × percentile`, including even-count
  medians. Empty/invalid samples produce null statistics, not zero timing. Startup's ten warm runs
  are a small sample; its P95 is descriptive, not a confidence bound.

CPU/typing samples are buffered until the measured sequence ends, then serialized. First-frame
metadata and RSS collection occur after the first-present timestamp. `--log-input` adds synchronous
I/O inside real input processing, so record whether it was enabled. The startup path includes
instrumentation setup and reports that phase explicitly.

## Limits for review

The reviewer exercised Metal surfaces, presentation and timestamps outside the sandbox at 60 Hz;
see RESULTS.md for the first batch. The GUI still cannot run inside this sandbox. Retina appearance,
real AppKit IME behavior, footprint hardware data and 120 Hz need further review. A requested
3000×1800-point window can be constrained by the current display; treat its **actual physical size**
as the test resolution. ProMotion may adapt refresh dynamically.

The lexer has no multiline parsing state. The grapheme slow path is not full international text
layout; check ZWJ emoji, flags, combining marks and Kana conversion visually. winit's macOS backend
currently ignores `insertText:replacementRange:` ranges and conditions commit delivery on marked
text, so emoji-picker insertion/reconversion are specific review risks. Native system font assets
can vary across macOS releases. A full CoreText rasterization comparison and AppKit
`NSTextInputClient` replacement are decisions for the measured report.
