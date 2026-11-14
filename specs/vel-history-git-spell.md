# Spec: undo-history export/import, Git + incremental diff engine, spell-check engine

Status: P3 / lane B (core engine). Locations: `crates/vel-buffer/` (history API), new `crates/vel-git/`,
new `crates/vel-diff/`, new `crates/vel-spell/`.

References (public docs only): incremental_diff.html, git_integration.html, spell_checking.html,
settings.html (`mini_diff`, `git_diff_target`, `show_git_status`, `spell_check`, `dictionary`,
`spelling_selector`, `ignored_words`), the API reference (`View.set_reference_document`,
`View.reset_reference_document`, `View.diff`... as documented).

## 1. `vel-buffer`: versioned history export/import (requested by lane A for Hot Exit)

- `Buffer::export_history() -> HistorySnapshot` (versioned, serde-serializable, compact: text
  deltas referencing the current text, not full copies; includes groups, soft-undo selection
  events, saved point, change ids as relative values, view-owner ids) and
  `Buffer::restore_history(text, snapshot, view_id_map) -> Result<…>` that validates the snapshot
  against the text (hash/length checks) and fails cleanly on mismatch.
- Round-trip property tests: random edit/undo/redo/soft-undo sequences → export → restore into a
  fresh buffer → every subsequent undo/redo/soft-undo produces identical text and selections.
- Budget: export of a 10,000-step history ≤ 5 ms; serialized size ≤ 2× the raw edit bytes.

## 2. `vel-diff` — incremental diff engine

- A line diff (Myers or histogram; `imara-diff` is acceptable) between the buffer and a reference
  text (saved file contents, or Git HEAD/index — see below), maintained **incrementally**: on each
  `vel-buffer` change set, re-diff only the affected region (expand to stable anchors), not the
  whole file. Output: hunks (added/modified/deleted with old/new line ranges) for gutter markers,
  `next_modification`/`prev_modification` targets, inline diff for a hunk (word/char-level within
  modified lines), and the edits needed to **revert a hunk** (`revert_hunk`) or all.
- Budgets: re-diff after a 1-char edit in a 100k-line file ≤ 1 ms p95; initial diff of a 100k-line
  file vs reference ≤ 50 ms (background).

## 3. `vel-git` — repository status

- Using `gix` (pure Rust): discover repos for project folders, branch name / detached HEAD,
  ahead/behind if cheap, per-file status for the side bar badges (untracked, modified, missing,
  staged addition/modification/deletion, unmerged, ignored — ST's documented badge set), folder
  status aggregation (most significant wins as documented), blob contents at HEAD and at the index
  for `git_diff_target` (`"index"` / `"head"`), watching `.git` to refresh. Respect large repos:
  status off the main thread, incremental refresh, ≤ 1 s for a 50k-file repo cold, ≤ 100 ms warm
  for a single-file change (report numbers on a large local repo, e.g. a checkout of
  `third_party/sublime-packages` and a bigger one generated or cloned for the test).
- No write operations (commit/stage) — Sublime Merge territory.

## 4. `vel-spell` — spell checking

- Hunspell-compatible dictionaries via `spellbook` (pure Rust). Ship **a permissively licensed
  English dictionary** only (e.g. SCOWL/en_US Hunspell under its BSD-like license — verify and
  include the license file); users can add others to `Packages/*/` like ST.
- `check_words(text, ranges)`, suggestions, `ignored_words`, user dictionary `add_word`; only
  ranges matching `spelling_selector` are checked (use `vel-highlight` scope runs); tokenization
  that skips URLs, camelCase/snake_case parts as ST does (document choices with
  `ST-BEHAVIOR: unverified`), upper-case-after-start-of-word fix noted in the ST4 changelog.
- Incremental: re-check only edited/visible lines; budget ≤ 1 ms for a 100-line viewport.

## Rules

- Modify only `vel-buffer`, `vel-diff`, `vel-git`, `vel-spell` (+ `vel-highlight` read-only use) and
  root `Cargo.lock`. `vel-buffer` is used by every lane-A crate: changes must be additive and keep all
  existing tests/benchmarks (re-run `crates/vel-buffer/benches/perf.rs`; no regression > 5%).
- Lane A works in parallel on `velocity`/UI crates and will integrate these next round: README
  integration sections with examples.
- fmt, clippy -D warnings, release tests, rustdoc -D warnings for touched crates.
