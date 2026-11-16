# Spec: scroll phase 2 (renderer and highlight per-frame costs) and a real-event native scroll gate

Status: P4 / lane B, round 14. Locations: `crates/vel-render/`, `crates/vel-highlight/`, `crates/vel-text/` (only if a
profile points there), and a new top-level `tools/scroll-gate/` (+ root `Cargo.lock`).

## Background (reviewer measurements, 2026-11-16)

Real trackpad-style events (pixel `CGEvent` scroll, phases set, 144 Hz input, 10 s) on a 10 MB Rust file, 3440x1440
display at 144 Hz, measured with `--log-input` JSONL plus `sample` on the main thread:

| build | frames (ideal 1440) | receipt→submit p95 | frame interval p99 | gaps > 20 ms |
|---|---|---|---|---|
| before phase 1 | 1072 / 1102 | 7.2 ms | 22–25 ms | 13–14 |
| after phase 1 (main) | 1395 / 1269 | 3.2 ms | 14.6–16.9 ms | 7–8 |

Phase 1 (already on main): the menu bar was rebuilt on every frame in which a background poll reported a change
(about a third of main-thread time), every scroll event rendered synchronously and blocked on `nextDrawable`, and
IME geometry was invalidated twice per event. After phase 1 the main thread is ~73 % idle; the largest remaining
item is `Renderer::prepare` → `Queue::write_buffer` (`_platform_memmove`, ~3 % of wall time, all instances
re-uploaded every frame through a fresh staging allocation), followed by size-dependent work below.

## 1. Per-frame costs that grow with file size or history

- `LineLayoutCache::insert` (`vel-render/src/cache.rs`) evicts with an O(n) `min_by_key` scan over 2048 entries on
  every miss. Make lookup, insert and eviction O(1) (intrusive LRU / generation ring), same capacity and hit rate.
- `ScopeSnapshot::complete` (`vel-highlight/src/scopes.rs`) is `lines.iter().all(..)`, O(lines), called per frame.
  Maintain the count of inexact lines incrementally so it is O(1); prove equivalence with a property test.
- Instance upload (`vel-render/src/gpu.rs` `prepare`): avoid a fresh staging allocation and a full copy every frame
  (persistent staging / `write_buffer_with` / a staging belt, and skip unchanged ranges — e.g. minimap instances when
  only the viewport indicator moved). Keep `desired_maximum_frame_latency = 1` and Fifo.
- Profile `Renderer::render` and `App::scene` with a large file for anything else O(document) per frame and fix it
  inside your crates. If the cause is in lane-A crates (`velocity`, `vel-platform`), write it up for the reviewer
  instead of editing.

## 2. Native scroll gate (`tools/scroll-gate/`)

The reviewer runs this outside the sandbox (you have no GUI/Metal); everything except the event poster must be
testable headless.
- `scrollhid.swift`: given a pid, activates the app, aborts with exit 2 unless that pid owns the frontmost window
  under the target point, then posts pixel scroll `CGEvent`s with begin/changed/end phases (and optional momentum
  phases) at a fixed rate via `.cghidEventTap`. Build with `swiftc` from the driver.
- `run.zsh`: launches a given binary with a fresh `--data-dir` and `--log-input`, waits a settle time, runs the poster,
  samples the main thread with `sample <pid> 4` while scrolling, quits the app. Telemetry contract (lane A
  implements it this round): the JSONL goes to `$VELOCITY_LOG_DIR` (the script sets it to a temp dir); for older
  binaries fall back to a `--legacy-output-dir` argument (they wrote to a compile-time source path).
- `analyze.py` (`python3 -I`, standard library only): frames, receipt→submit percentiles, frame-interval
  percentiles, gaps > 20 ms / > 50 ms, the display refresh rate (passed in or read from the JSONL), and main-thread
  busy % from the `sample` call tree (non-idle samples of the first thread; idle = blocked in `mach_msg` under the
  run loop). `analyze.py --self-test` checks it against fixture JSONL and a fixture sample file.
- Paired mode: alternate base/candidate binaries N times (default 3 each) and report per-metric medians and deltas.
- Workloads: 10 MB Rust (`spikes/data/rust-10mb.rs`), a 1 MB normal source file, a 200-line file; finger-only and
  finger + momentum.
- Gate (144 Hz display, 144 Hz input, 10 s): frames ≥ 95 % of ideal, frame interval p99 ≤ 2 refresh intervals,
  zero gaps > 50 ms, receipt→submit p95 ≤ 4 ms, main-thread busy ≤ 35 %. Document how to run it in
  `tools/scroll-gate/README.md`, including the Accessibility/Input Monitoring permission the poster needs.

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-b (branch `lane-b`). Do not commit. Only the locations above. Lane A
  edits `velocity`, `vel-platform`, `vel-config` and packaging in parallel.
- Paired A/B for every performance claim (existing headless scroll/typing benches; ≤ 5 % regression anywhere), plus
  micro-benchmarks for the cache and the completeness flag at 10 MB. fmt, clippy `-D warnings`,
  `cargo test --workspace --release`, rustdoc `-D warnings`.
