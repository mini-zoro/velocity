# Spec: huge files and very long lines (engine side)

Status: P3 / lane B. Goal: beat Sublime Text where it is weakest. Public reports (docs/PLAN.md §1.3):
ST loads everything into RAM, becomes unusable above ~5–10M lines, and lags with 10k+ character
lines. Velocity's `vel-text` already loads 1 GB in ~260 ms; the remaining work is everything around it.

## 1. Progressive loading (`vel-fileio` is lane A's; add an engine crate `vel-largefile` that it can call)

- Stream a file into `vel-text` in chunks on a background thread, publishing snapshots of the
  already-loaded prefix every ~16 MB (or 100 ms) so the editor can show and scroll the beginning of
  a multi-GB file immediately; report progress; cancellable; correct UTF-8/encoding handling across
  chunk boundaries (reuse `vel-fileio`'s decoders through a small trait if needed, or stream UTF-8
  and fall back to a full decode for other encodings — document).
- Budget: first screenful available ≤ 100 ms for a 4 GB file; full load of 1 GB ≤ 1 s total with
  progress events; memory ≤ 1.15× file size for UTF-8.

## 2. Feature policy for huge documents

- A `LargeDocumentPolicy` that, given byte size / line count / longest line, decides which features
  run: full background highlighting vs viewport-only highlighting (parse from the nearest checkpoint,
  give up after a time budget and show plain text), indexing off, word-wrap off by default above a
  threshold, minimap sampling, spell check off, diff against disk off above a threshold. Thresholds
  are settings (document names and defaults). Pure, testable.
- Viewport-only highlighting support in `vel-highlight`: request lines a..b of a huge document
  without parsing the prefix (start from a "best effort" state at a line boundary and mark spans as
  approximate until a background pass catches up — design it so correctness is restored eventually).

## 3. Very long lines

- Typing and cursor movement in a single 50 MB line must stay interactive: provide in `vel-text` (or a
  new helper) O(log n) access to grapheme boundaries, columns and display-width prefix sums inside a
  line (chunked index), so layout can lay out only the visible horizontal window of a line.
- Highlighting long lines: cap per-line parse work (ST-like behavior: give up on lines longer than a
  setting, e.g. 16k chars, and render them unstyled or with a simple fallback) without breaking the
  parse state for following lines.
- Budgets: insert a character in the middle of a 50 MB single-line file ≤ 2 ms engine time; move
  caret by word / to line end ≤ 1 ms; compute the visible horizontal window (200 columns) ≤ 0.5 ms.

## 4. Binary files

- Detect binary content; provide a hex-view model (offset, 16 bytes per row, ASCII column) backed by
  `memmap2` without loading the whole file, read-only for now (Sublime opens binaries as hex).

## 5. Optional (if time remains)

- C++ background highlighting is 2.40 MB/s vs the 2.5 target; warm syntax-cache load p95 is 21.2 ms vs
  20 ms. Paired A/B only.

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-b (branch `lane-b`). Modify `vel-text`, `vel-highlight`,
  `vel-syntax`, `vel-regex`, `vel-scope` and create `vel-largefile` (+ root `Cargo.lock`); additive changes
  only to `vel-text`/`vel-buffer` APIs, re-run their perf benches (no regression > 5%). Do not commit.
- Lane A integrates next round (it is finishing the theme/minihtml UI now) — write README integration
  sections. Run `cargo test --workspace --release` before finishing.
