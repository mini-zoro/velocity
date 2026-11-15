# Spec: huge files, very long lines and binaries in the editor

Status: P3 / lane A. Locations: `crates/vel-fileio/`, `crates/velocity/`, `crates/vel-ui/`, `crates/vel-render/`,
other lane-A crates. Consumes (read-only) `vel-largefile` (see its README: "Lane A: opening a huge file",
"Feature settings", "Lane A: long horizontal lines", "Lane A: binaries").

Goal: Velocity must stay responsive where Sublime Text does not (docs/PLAN.md §1.3): multi-GB files, millions
of lines, and single lines of tens of MB.

## 1. Opening huge files

- `vel-fileio` opens files above a threshold through `vel_largefile::load_utf8`/`stream_utf8`: the tab shows
  the already-loaded prefix immediately (first screen ≤ 100 ms after the window shows content for a 4 GB file),
  a progress indicator in the tab and status bar, scrolling within the loaded prefix while loading, and
  cancellation when the tab closes. Edits are disabled until the load completes (read-only banner), or allowed
  on the loaded prefix — choose and document; never lose data on save (saving before the load finishes must be
  impossible or must write the whole file).
- Non-UTF-8 encodings: follow the README's fallback (full decode on a worker) with the same progress UI.

## 2. Feature policy

- Apply `LargeDocumentPolicy` per document: viewport-only highlighting via `ViewportHighlighter` (approximate
  spans repaired in the background), indexing / spell check / diff / minimap / word wrap disabled or degraded
  per the policy and its settings; a status-bar hint shows what is degraded and a command re-enables a feature
  for this view.

## 3. Very long lines

- Layout only the visible horizontal window of long lines using `LongLineIndex` (grapheme, word and display
  columns); caret movement, selection, hit testing, IME rectangles and horizontal scrolling must work in a
  50 MB single line; update the index incrementally after edits (`update_after_edit`). Highlighting gives up
  on lines above the configured length (rendered plain) without breaking following lines.
- Budgets: typing in the middle of a 50 MB line: input → scene p95 ≤ 4 ms CPU; horizontal scroll frame ≤ 2 ms.

## 4. Binary files

- Detect binary files and open them in a read-only hex view tab backed by `HexView` (offset, 16 bytes per row,
  ASCII column), with Goto offset (`Ctrl+G`-style input), copy hex/text, and a 4 GB file scrolling smoothly.

## Tests and budgets

- Headless tests: prefix publication and cancellation through the editor path, policy application, long-line
  caret/selection/IME geometry, hex rows and copy, save-safety while loading.
- Keep typing and scroll budgets for normal files within 5% (paired A/B against main as in round 11).

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-a (branch `lane-a`). Do not commit. Modify only lane-A crates
  (+ root `Cargo.lock`); lane B works on release engineering in another worktree.
- `cargo test --workspace --release`, fmt, clippy -D warnings, rustdoc -D warnings. Automatic GUI modes and a
  MANUAL_TEST.md section (open a multi-GB log, a minified 50 MB JS line, a binary).
