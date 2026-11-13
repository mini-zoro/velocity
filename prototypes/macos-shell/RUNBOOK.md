# Reviewer runbook

Run these commands from a normal Terminal logged into the macOS desktop, outside the agent
sandbox. Keep benchmark windows visible. Every `--bench-*` mode completes and closes its own window;
no typing, scrolling or clicking is needed. Manual input testing is separate in [IME_TEST.md](IME_TEST.md).

## Build and headless validation

```sh
cd /Users/zoro/dev/velocity/prototypes/macos-shell
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
python3 -m unittest discover -s scripts -p 'test_*.py'
```

The repository pins Rust 1.99. Dependencies are exact-pinned in `Cargo.lock`; use `--locked` when
reproducing an existing lockfile. Only this prototype's target directory is used. Do not run a GUI
mode in the restricted sandbox: build/test results are the only local validation there.

## One command for the entire automatic suite

```sh
cd /Users/zoro/dev/velocity/prototypes/macos-shell
bash scripts/run-all.sh --metal always
```

This freshly recompiles/relinks only `vel-shell`, captures environment/version/power information,
runs startup once cold-ish plus ten warm times, runs a raw-Metal priming launch plus ten warm
clear-only comparisons, runs scrolling and typing at both sizes, sweeps the entire corpus for RSS,
and tries Mailbox/Immediate scroll and typing experiments. It prints JSONL results and saves each
run, stderr log, runner status and aggregate under `output/`. Unsupported experimental present modes
are recorded as `unsupported`; other failures make the suite exit nonzero. Expect several minutes.
Do not type into these windows; physical keys invalidate the suite's completeness validation.

The normal default `--metal auto` runs the raw comparison only when median instance/adapter/device
phase costs total at least 20 ms and at least 30% of warm main-to-first-present time. `--metal always`
collects the comparison even if that threshold is not met. The raw path is clear-only: report it as
a startup lower bound, without equating it to a full text screen.

## Startup in isolation

```sh
bash scripts/startup.sh
bash scripts/startup.sh --metal always
```

Each invocation creates a fresh batch. It runs `cargo clean --release -p vel-shell`, then
`cargo build --release --locked`; dependency artifacts and existing results are kept. The first
launch is labelled **cold-ish**, followed by ten warm launches. Building/linking itself warms OS
caches; there is no cache purge or claim of a true cold boot. `startup-summary-*.jsonl` contains
P50/P95/P99/max/mean for each cumulative main/process time and each phase cost. Cold-ish is reported
separately with N=1. All original samples are retained.

Single automatic launch, without rebuilding:

```sh
./target/release/vel-shell --bench-startup
./target/release/vel-shell --bench-metal
```

The milestone endpoint is `first_present_returned`. It does not time compositor completion or
physical display. `main_entered.process_ms` captures estimated loader/pre-main time, anchored using
the kernel process birth time. Review both process-relative and main-relative numbers against the
100 ms target. `measurement_ready` exposes tracing/output/watchdog setup overhead.

## Native AppKit startup floor

The reviewer supplied a separate Swift AppKit + CAMetalLayer clear-only program. Run it outside
the sandbox, with no concurrent compilation, and save its output separately from the JSONL suite:

```sh
cd /Users/zoro/dev/velocity/prototypes/macos-shell
bash native-floor/run.sh 11 | tee "output/native-floor-$(date -u +%Y%m%dT%H%M%SZ).log"
```

The script builds `native-floor/floor` with `swiftc -O`, prints loadavg, and launches it the requested
number of times (default six). It does not use winit or wgpu. Treat the first launch separately and
use the following ten for a new warm cohort. Its output is plain text, not the suite's JSONL schema;
the automatic suite does not invoke this script. Keep every per-launch line, display/scale setting,
power state, and the loadavg with any derived statistics.

Phase timestamps start at the first timed Swift statement, rather than process birth. The
`presented(scanout)` label runs after `MTLDrawable.addPresentedHandler`, then dispatches onto the
main queue. It observes display presentation rather than present-return, but includes callback
dispatch delay; the current code does not sample `drawable.presentedTime`. Do not equate its numbers
with Rust's `Queue::present` return or with an external photon measurement. The clear-only workload
also excludes the editor's text/atlas work. The reviewer's previously reported 150–175 ms range has
no saved per-launch log in the first suite batch, so its P50/P95 cannot be recomputed yet.

## Scroll, default and large

```sh
./target/release/vel-shell --bench-scroll
./target/release/vel-shell --bench-scroll --large
```

Default size is 1200×800 logical points. `--large` requests 3000×1800 logical points. Every run logs
actual physical/logical size, scale factor and monitor refresh. On a 2× display the large request
would be 6000×3600 physical pixels if the OS permits it. If constrained, move the test to a large
display and rerun; a clamped window is not a 4K/5K measurement. For a specific size:

```sh
./target/release/vel-shell --bench-scroll --width 2560 --height 1440
```

There must be 600 `scroll_frame` records followed by one `scroll_summary`. CPU cost includes surface
acquisition; GPU duration is timestamped render-pass execution. GPU support is reported explicitly
and unavailable GPU statistics remain null. Readback happens after sampling. The run then sweeps
the loaded file automatically and writes the final RSS before exiting. Short files count every
scroll-boundary clamp; use the default corpus for the intended workload. The suite rejects size changes, boundary-clamped
scrolling and missing enabled GPU samples instead of accepting those as comparable measurements.

For 120 Hz testing, run on the ProMotion display with the intended refresh setting, then repeat
both sizes. Record the reported monitor refresh and macOS display setting in the report. A nominal
refresh value alone cannot prove a sustained 120 Hz adaptive-refresh presentation rate. Keep power
mode, display, window visibility and other workload consistent between comparisons.

## Typing, default and large

```sh
./target/release/vel-shell --bench-typing
./target/release/vel-shell --bench-typing --large
./target/release/vel-shell --bench-typing --seed 42
```

A run injects 500 characters using seeded randomized 30–90 ms deadlines, then exits after every
insertion is associated with a submitted/presented frame. Review `typing_sample`, `typing_summary`,
`timer_lateness` and `external_input_events`. Late/coalesced events are preserved. These injections
begin inside the event-loop timer handler and use the same edit entry point as real key events.
They do not exercise the physical keyboard, HID stack or an OS IME.

## Whole-file memory

```sh
./target/release/vel-shell --bench-memory
./target/release/vel-shell --bench-memory --lines 0 --timeout 600
./target/release/vel-shell --bench-memory --large --lines 0 --timeout 600
```

Each captures RSS and physical footprint after the first frame, sweeps one viewport at a time
through every loaded line, waits for submitted work, captures both again, and exits. Default runs load 5,000 lines. Use
`--lines 0` for the entire `rust-10mb.rs`; the full sweep can take longer, hence the 600-second
watchdog. The final memory record must say `whole_loaded_buffer_visited: true`. Keep loaded byte/line
counts and `file_was_truncated` with any memory figures. Memory records retain `rss_bytes` from
`MACH_TASK_BASIC_INFO.resident_size` and add `phys_footprint_bytes` from
`TASK_VM_INFO` revision 1's `phys_footprint`, the accounting used for Activity Monitor's Memory
comparison. Each field has its own error and method; the two kernel reads are consecutive rather
than an atomic snapshot. RSS includes resident shared/file-backed pages, while footprint has
different charges, including compressed private memory. Do not infer one from the other or from
the `getrusage` high-water mark. Neither is a complete ownership breakdown of GPU/WindowServer memory.

The first reviewed batch (`20261113T150046.972369Z-74020`) predates the footprint field. Its results
remain valid RSS measurements; footprint cannot be filled in retrospectively. Rebuild, then repeat
the commands above or the full suite to collect both metrics. New failed footprint readings emit
null plus `phys_footprint_error`; the helper rejects these. Historical records without the added
field remain readable. Use footprint for comparisons with Activity Monitor/public editor figures,
and report units explicitly: MiB = 1,048,576 bytes, MB = 1,000,000 bytes.

## Present-mode experiments

```sh
./target/release/vel-shell --bench-scroll --present mailbox
./target/release/vel-shell --bench-scroll --present immediate
./target/release/vel-shell --bench-typing --present mailbox
./target/release/vel-shell --bench-typing --present immediate
```

A mode absent from the Metal surface's capability list emits an error and exits nonzero. Preserve
that output as an unsupported experiment. There is no automatic Fifo fallback. Fifo is the reference
for normal frame pacing; Immediate can tear and its results must be labelled.

The first batch's Mailbox runs are unsupported. Immediate runs have 28–31 s first-frame delays and
large total surface-retry counts, so keep them as diagnostic experiments. The ~280 MB final RSS
belongs to the **default-size Immediate scroll** run; 4K Fifo ends at ~89 MB. Counters currently span
startup and sampling, and do not identify retry reasons. A rerun should keep the first-frame startup
records alongside timing/memory summaries instead of judging Immediate only by its fast scroll median.

## Unicode/IME and real-key latency (manual)

```sh
./target/release/vel-shell --log-input fixtures/ime-and-unicode.rs
```

Follow [IME_TEST.md](IME_TEST.md), type normally, scroll and resize, then close the window.
`input_event` records contain key/IME contents and receipt timestamps; `input_latency` records link
changes to the next frame's submission and `Queue::present` return. The logger's synchronous file/
stdout work is part of these instrumented real-key timings. The frame counter can associate several
changes with one frame. Candidate coordinates are logical client coordinates, updated after edits,
preedit changes, scrolling, resizing and DPI changes. Cross-display tests should include 1× and 2×.

The automatic suite leaves the real OS IME decision to this human checklist. The headless tests
cover only our state transitions, geometry, shaping and bitmap generation.

## Files to return and failure handling

A successful automatic run ends with exactly one `kind: "complete", complete: true`. A native panic,
watchdog expiry, user-closed window, unavailable Metal adapter, failed GPU readback or incomplete
sample count is a failed run. Do not aggregate partial files. Scripts write `*-runner-*.jsonl`
records with status, command, wall duration and result path. An existing explicit output name is
rejected to preserve prior data; default names are unique.

Return all relevant `output/*.jsonl`, the matching `*.stderr.log`, and completed [IME_TEST.md](IME_TEST.md)
observations. Include display resolution/refresh, scale, power mode, macOS version and machine. The
suite captures part of this automatically. If a run fails, return its partial JSONL and stderr too;
its final error should identify initialization, surface, GPU query, font or watchdog stage. Rust
panics need their stderr/backtrace, which can be collected on a rerun with:

```sh
RUST_BACKTRACE=1 ./target/release/vel-shell --bench-scroll
```

The Chinese [RESULTS.md](RESULTS.md) now contains the verified first batch and recommendations.
Append new batches with their input, actual resolution, present mode and timing boundary. Still
pending are physical-footprint hardware measurements, native-floor per-launch logs, manual IME/
visual comparison, 4K Immediate and 120 Hz. Keep the original first-batch artifacts unchanged.
