# Spec: `vel-buffer` — buffers, selections, regions, undo, multi-cursor edit operations

Status: P1 / lane B (core engine). Location: `crates/vel-buffer/` (depends on `vel-text`).

`vel-buffer` is the editing model under every view: what Sublime Text's plugin API calls
`Buffer`, `View.sel()`, `Selection`, `Region`, `add_regions`, `change_id`/`transform_region_from`,
undo/redo/soft undo, and the text-level behavior of the core editing commands. It has **no UI, no
layout and no syntax knowledge**. Everything that needs pixels (vertical movement, page up/down,
wrapping) gets coordinates through a small trait the layout layer implements later.

Design constraint (docs/PLAN.md §5.3, §8): names and semantics follow Sublime Text's public API
documentation (https://www.sublimetext.com/docs/api_reference.html) so that a Python plugin host
can be added later as a thin adapter. Clean-room rules in `CONTRIBUTING.md` apply: documentation
and black-box behavior only.

Where Sublime's exact behavior is not documented, choose the behavior described here, mark it in
code with `// ST-BEHAVIOR: unverified` and list it in `crates/vel-buffer/BEHAVIOR.md`, so that
golden tests recorded against the real editor can confirm or correct it later.

## 1. Model

```rust
pub struct BufferId(u64);
pub struct ViewId(u64);

/// Shared document: text, undo history, change tracking. Several views may show one buffer.
pub struct Buffer { /* Text, history, change log, views' region sets */ }

/// Per-view editing state: selection, named region sets, (later) folds and viewport.
pub struct ViewState { /* Selection, RegionSets, xpos per region */ }

/// Sublime-style region: `a` = anchor, `b` = caret; empty when a == b. Offsets are byte offsets
/// into the buffer's `vel_text::Text` (the API adapter converts to/from code points later).
pub struct Region { pub a: usize, pub b: usize, pub xpos: Option<f64> }

/// Ordered, non-overlapping set of regions; the selection of a view. Never empty after a
/// normalizing operation (a view always has at least one caret).
pub struct Selection { /* Vec<Region>, sorted by begin() */ }
```

- `Region` methods mirror ST: `begin, end, size, empty, contains(point|region), cover, intersection,
  intersects`, plus `to_range()`.
- `Selection::add` merges overlapping regions and also **touching non-empty** regions; two empty
  carets at the same point collapse into one. Merged region direction: keep the direction of the
  region that was added last (`// ST-BEHAVIOR: unverified`).
- `Selection` operations: `add, add_all, subtract, clear, contains, iter, len, primary()` (the last
  region added interactively is primary; keep an index), `regions_on_line`.

## 2. Change tracking and position mapping

- Every committed transaction produces a `ChangeId` (monotonic `u64` per buffer) and a compact
  `ChangeSet` (sorted, non-overlapping `(old_range, new_len)` list in pre-edit coordinates).
- `Buffer::change_id()`, `Buffer::transform_point_from(point, change_id, bias)` and
  `transform_region_from(region, change_id)` map positions across all later changes (keep the log
  bounded; transforming from an ID older than the retained log returns `None`).
- Bias rules for a point exactly at an edit boundary:
  - Selections: carets at the start of an **insertion** move after the inserted text (this is how
    typing works with multiple cursors). A caret inside a **deleted** range moves to the deletion
    start.
  - Named region sets (`add_regions`): a region's `begin` sticks **right** and its `end` sticks
    **left**, i.e. text inserted exactly at either boundary is *not* included in the region; an
    empty region behaves like a caret (`// ST-BEHAVIOR: unverified`). Regions that collapse to empty
    are kept (ST keeps empty regions) unless the set was created with a `drop_empty` flag.

## 3. Transactions, undo, redo, soft undo

```rust
impl Buffer {
    /// All edits inside one transaction are expressed in pre-transaction coordinates, sorted,
    /// non-overlapping, and applied with `vel_text::Text::apply_edits` in one pass.
    pub fn transact(&mut self, view: ViewId, kind: EditKind, edits: &[Edit], new_selection: Selection) -> ChangeId;
    pub fn undo(&mut self, view: ViewId) -> bool;
    pub fn redo(&mut self, view: ViewId) -> bool;
    pub fn soft_undo(&mut self, view: ViewId) -> bool;
    pub fn soft_redo(&mut self, view: ViewId) -> bool;
}
```

- Undo entries store the inverse edits (deleted text is kept as `vel_text::Text` slices/snapshots,
  not copied strings, so undoing a 100 MB deletion is cheap) plus the selection **before** and
  **after**.
- **Grouping** (matches how ST feels when typing): consecutive `EditKind::InsertChars` transactions
  from the same view merge into one undo step while the caret positions continue exactly where the
  previous insertion ended and the inserted text has the same character class run (a group breaks
  at a word→whitespace→word transition only when the previous group already contains a word; i.e.
  undo removes roughly one word at a time). Any selection change not caused by the edit itself, any
  other `EditKind`, or an explicit `Buffer::break_undo_group()` ends the group.
  `// ST-BEHAVIOR: unverified` — keep the policy in one function so it is easy to change.
- **Soft undo** undoes the last selection change *or* edit, whichever is more recent (selection-only
  changes are recorded in a bounded per-view history, ≥ 256 entries).
- After redo history exists, a new transaction discards it.
- The buffer tracks `is_dirty()` against a `mark_saved()` point and survives undo/redo across it
  (undo back to the saved state ⇒ not dirty).
- Memory: history is bounded by entry count and by total retained bytes (configurable; defaults
  10,000 entries / 256 MiB), dropping the oldest entries.

## 4. Text-level editing operations (pure functions over Buffer + ViewState)

Implement as functions in `vel_buffer::ops` that compute the edits and the new selection and then
call `transact`. These are the text semantics behind ST commands; the command layer (later
`vel-commands`) will map command names/arguments onto them.

| Operation | ST command it backs | Notes |
|---|---|---|
| `insert(text)` | `insert` | Replaces every selection with `text`; carets end after it. Multi-line paste with N lines into N selections distributes one line per selection when `text` has exactly N lines (ST behavior for `paste` — implement as a separate `paste(text)` op) |
| `insert_snippet` | — | out of scope |
| `delete_left(by)` / `delete_right(by)` | `left_delete` / `right_delete` | `by = Char` deletes one **grapheme cluster**; with `translate_tabs_to_spaces` + `tab_size`, `delete_left` in leading whitespace deletes back to the previous tab stop (setting passed in) |
| `delete_word(forward)` | `delete_word` | uses word classes below |
| `move_by(unit, forward, extend)` | `move` | units: `Characters` (graphemes), `Words`, `WordEnds`, `Subwords`, `SubwordEnds`, `Stops` (configurable), `Lines`/`Pages` via the `VerticalLayout` trait; non-extending horizontal move with a non-empty region collapses it to the respective edge without moving further |
| `move_to(target, extend)` | `move_to` | `Bol`, `Eol`, `HardBol` (first non-whitespace vs column 0 toggle), `HardEol`, `Bof`, `Eof`, `Brackets` (out of scope until syntax) |
| `expand_selection(to)` | `expand_selection` | `Word`, `Line`, `Indentation`, `Brackets`(skip); `Scope` later |
| `find_under_expand()` | `find_under_expand` (Cmd+D) | empty caret ⇒ select the word under it (whole-word + case-sensitive matching enabled for subsequent finds); otherwise add the next occurrence after the last selection, wrapping around, skipping already-selected occurrences |
| `find_all_under()` | `find_all_under` | all occurrences |
| `single_selection()` | `single_selection` | keep primary |
| `split_selection_into_lines()` | `split_selection_into_lines` | |
| `select_lines(forward)` | `select_lines` (column add) | needs `VerticalLayout` |
| `join_lines`, `swap_line_up/down`, `duplicate_line`, `indent`/`unindent`, `toggle_comment`(skip), `upper_case`/`lower_case`/`title_case`/`swap_case`, `transpose`, `sort_lines`, `reverse_lines`, `trim_trailing_whitespace` | same names | multi-selection aware; operate on all lines touched by any selection, each line once |

Word classes: a character is `Word` unless it is whitespace (`Whitespace`) or appears in the
`word_separators` setting (default
``./\()"'-:,.;<>~!@#$%^&*|+=[]{}`~?``, documented in ST's default settings) — then it is
`Punctuation`. Word movement stops at class boundaries (newline is its own stop). Subword movement
additionally stops at lowercase→uppercase transitions and at `_` boundaries inside identifiers.

`VerticalLayout` trait (implemented later by the layout crate; provide a monospace test
implementation with `tab_size` and East-Asian-wide = 2 columns):

```rust
pub trait VerticalLayout {
    /// x position (opaque units) of a byte offset, and the offset closest to x on another line.
    fn x_of(&self, text: &vel_text::Text, offset: usize) -> f64;
    fn offset_at(&self, text: &vel_text::Text, line: usize, x: f64) -> usize;
    fn lines_per_page(&self) -> usize;
}
```

Vertical moves keep `Region::xpos` (goal column) across consecutive vertical moves and reset it on
horizontal moves/edits.

## 5. Performance budget (M1 Pro, release)

| Scenario | Target |
|---|---|
| Type one character with 10,000 carets (one per line) on a 100 MB buffer: compute edits + `transact` + selection update + undo-group merge | ≤ 3 ms |
| Same with 100,000 carets | ≤ 25 ms |
| `find_under_expand` adding the 10,000th occurrence in a 100 MB buffer | ≤ 2 ms per call (amortized: incremental search from the last selection, not a full rescan) |
| `undo` of a 10,000-caret typing group | ≤ 5 ms |
| Delete 50 MB (select all + delete) and undo it | ≤ 5 ms each (no text copy) |
| Map 100,000 named regions through one 10,000-edit transaction | ≤ 10 ms |
| Memory of an undo entry for a 10,000-caret single-char insert | ≤ 64 bytes per caret |

Bench in `crates/vel-buffer/benches/perf.rs` (`harness = false`) using `spikes/data/text-100mb.txt`;
report median/P95 of ≥ 5 runs in `crates/vel-buffer/PERF.md` (Chinese).

## 6. Correctness

- Property tests (proptest) against a naive model (a `String` + `Vec<(usize, usize)>` selection,
  edits applied sequentially) for: transactions, undo/redo round trips (undo⁽ⁿ⁾ then redo⁽ⁿ⁾ returns
  the exact text *and* selections), soft undo, position mapping through random change sequences,
  selection normalization invariants, region-set bias rules.
- Unit tests for every operation in §4 with multi-cursor cases, Unicode (CJK, emoji ZWJ sequences,
  combining marks), empty buffer, buffer without trailing newline, CR inside lines.
- `BEHAVIOR.md` lists every `ST-BEHAVIOR: unverified` decision.
- No `unsafe`. clippy/fmt/doc clean with workspace lints.

## 7. Also in this round: `vel-text` 1 GB load

`vel-text` misses one budget: `from_reader` 1 GB ASCII (654 ms in your run, 735 ms in the reviewer's
run outside the sandbox at loadavg ≈ 6; crop 259 ms; CPU-bound). Make it ≤ 300 ms, e.g. by building
leaves in parallel (read big blocks, find UTF-8-safe split points, build leaf runs on a scoped
thread pool sized to available parallelism, then assemble the tree), and/or by deferring the
per-leaf LF index until first use. Keep peak RSS ≤ 1.15× and all other budgets. Add a
`from_reader` variant without threads if you change the default, and re-run the full vel-text
bench. Also move intermediate exploration data out of the published crate: keep only
`perf-final.jsonl` and `perf-summary.json`; put `perf-data/` under a gitignored path.
