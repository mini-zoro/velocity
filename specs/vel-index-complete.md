# Spec: scope output for the editor, project index, completions, snippets

Status: P2/P3 / lane B (core engine). Locations: `crates/vel-highlight/` (scope output),
new `crates/vel-index/`, new `crates/vel-complete/`, new `crates/vel-snippet/`.

References (public docs only): https://www.sublimetext.com/docs/indexing.html ,
completions.html, the API reference (CompletionItem, CompletionList, CompletionFormat, Kind,
SymbolRegion, SymbolLocation, `View.symbol_regions()`, `Window.symbol_locations()`), the snippet
format documented by Sublime (and the TextMate snippet grammar it derives from).

## 1. Scopes from `vel-highlight` (one parse for both colors and editing)

Lane A currently runs a *second* parser to get scopes for selector contexts, comment toggling,
bracket matching and symbols, because `HighlightService` only emits styles. Extend the service so
the same parse publishes, per line and version: compact scope runs (interned scope-stack ids +
byte ranges) and a `scope_at(line, byte) -> ScopeStackId` / `extract_scope` /
`match_selector(point, selector) -> score` query API; plus the per-file **symbol list** (from
`showInSymbolList` + `symbolTransformation`) maintained incrementally. Budget: no measurable
regression in edit→visible latency (keep p95 ≤ 0.2 ms) and memory ≤ 16 bytes per token run.
Write a migration note for lane A in `crates/vel-highlight/README.md`.

## 2. `vel-index` — project symbol index

- Background indexing of all project files (`ignore` walker + `index_exclude_patterns`,
  `index_exclude_gitignore`, binary/size limits), using `vel-syntax` + metadata to extract
  **definitions** (`showInIndexedSymbolList`, `symbolIndexTransformation`) and **references**
  (`showInIndexedReferenceList`), with kinds (function, type, namespace, variable, …) and
  locations (file, row, col, byte range).
- Worker pool sized by `index_workers` (0 = auto), low priority (macOS QoS utility/background),
  cancellable; incremental re-index on file change (`notify`) and on unsaved buffer edits
  (debounced); persistence in an on-disk store (embedded DB such as `redb`, or a custom
  append-only format) under the user cache dir keyed by project, so restart doesn't re-index.
- Queries: `lookup_definition(name) -> Vec<SymbolLocation>`, `lookup_references(name)`,
  `fuzzy_symbols(query, k)` for Goto Symbol in Project, and the **word/n-gram statistics** needed
  by context-aware completion (words in the project, word-sequence counts, trailing punctuation).
- Status reporting (progress, log lines) for an "Indexing Status" window.
- Budgets: index `~/.cargo/registry/src` style corpus (≥ 10k files) — report files/s and total
  time; query `lookup_definition` ≤ 1 ms; fuzzy symbols over 1M symbols ≤ 10 ms; RSS of the index
  service ≤ 200 MB for 1M symbols; re-index of one edited file ≤ 50 ms.

## 3. `vel-complete` — auto-complete engine

- Sources: words in the current buffer, words in other open buffers, the project index (when
  `auto_complete_use_index`), `.sublime-completions` files (with `trigger`, `contents`,
  `annotation`, `kind`, `details`, scope selector), snippets (`.sublime-snippet` with
  `tabTrigger` + `scope`), and the ST4 **context-aware** suggestions from word-sequence statistics.
- Ranking like ST: fuzzy match quality, prefix/word-start matches, proximity to the caret,
  frequency, recency (`auto_complete_use_history`), exact tab-trigger matches first, honoring
  `auto_complete_preserve_order` and `auto_complete_include_snippets(_when_typing)`.
- Triggers: `auto_complete`, `auto_complete_selector`, `auto_complete_triggers`,
  `auto_complete_delay`, `auto_complete_size_limit`, `auto_complete_commit_on_tab`,
  `auto_complete_with_fields`, `auto_complete_cycle`, `auto_complete_when_likely`.
- API for lane A: `query(view_snapshot, carets, scope_provider) -> CompletionList` (items with
  `trigger`, `annotation`, `completion`, `completion_format` (text/snippet), `kind`, `details`),
  async and cancellable. Budget: ≤ 5 ms per keystroke for a 100k-word project vocabulary.

## 4. `vel-snippet` — snippet engine

- Parse `.sublime-snippet` (XML: `content` CDATA, `tabTrigger`, `scope`, `description`) and the
  inline snippet syntax used by `insert_snippet {contents}` and completions: `$0..$n`,
  `${n:default}` with nesting, mirrors, `${n/regex/format/options}` transformations (Boost-style
  format strings incl. conditionals `(?1:…:…)` and case changes `\u \U \l \L \E`), environment
  variables (`$SELECTION`, `$TM_SELECTED_TEXT`, `$TM_FILENAME`, `$TM_FILEPATH`, `$TM_LINE_INDEX`,
  `$TM_LINE_NUMBER`, `$TM_CURRENT_LINE`, `$TM_CURRENT_WORD`, `$TM_SOFT_TABS`, `$TM_TAB_SIZE`,
  `$TM_COMMENT_*` from metadata), escapes.
- Field model as pure data: given a buffer and carets, produce edits + the field regions in
  order; `next_field` / `prev_field` / mirror updates as edits; indentation of inserted lines
  follows the caret line's indentation (and `translate_tabs_to_spaces`). Lane A wires the UI
  (has_next_field contexts, Tab/Shift+Tab).
- Tests: a table of snippet strings × inputs → expected text, fields and mirror behavior.

## Rules

- Modify only lane-B crates: `vel-highlight`, `vel-index` (new), `vel-complete` (new), `vel-snippet`
  (new), and `vel-syntax`/`vel-scope`/`vel-regex` if needed (+ root `Cargo.lock`). Lane A is working on
  `velocity` and UI crates in parallel and will integrate these APIs next round — provide clear
  README integration sections and examples.
- Keep the syntax-test baseline monotonic. fmt, clippy -D warnings, release tests, rustdoc -D warnings.
