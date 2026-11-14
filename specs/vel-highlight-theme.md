# Spec: `vel-color-scheme` + `vel-highlight` + tmPreferences metadata

Status: P1 / lane B (core engine). Locations: `crates/vel-color-scheme/`, `crates/vel-highlight/`,
additions to `crates/vel-syntax/` (metadata) — plus a bundled default color scheme in
`crates/velocity/resources/Packages/Color Scheme - Velocity/` (lane B owns this directory).

Goal: everything the editor needs to show **correctly colored code that stays correct while
typing**, with Sublime-compatible color schemes.

## 1. `vel-color-scheme`

- Load `.sublime-color-scheme` (lenient JSON: comments, trailing commas — reuse `vel-config`'s
  parser if convenient, otherwise a local one) per https://www.sublimetext.com/docs/color_schemes.html :
  `variables`, `globals` (all documented global keys: background, foreground, caret, invisibles,
  line_highlight, selection, selection_border, inactive_selection, misspelling, fold_marker,
  accent, gutter, gutter_foreground, line_diff_*, brackets_*, tags_*, find_highlight,
  minimap_border, rulers, shadow, …), `rules` (`scope`, `foreground`, `background`,
  `foreground_adjust`, `selection_foreground`, `font_style` incl. `bold italic underline
  stippled_underline squiggly_underline glow`, `hashed_foreground` gradients).
- Color syntax: hex (#RGB/#RGBA/#RRGGBB/#RRGGBBAA), named CSS colors, `rgb()/rgba()/hsl()/hsla()/
  hwb()`, `var()`, and the **`color()` mod function** with `alpha()/a()`, `saturation()/s()`,
  `lightness()/l()`, `blend()`, `blenda()`, `min-contrast()` as documented.
- `.tmTheme` (plist) support with the same internal model.
- `.hidden-color-scheme`, color scheme override files (User/X.sublime-color-scheme merging), and
  auto dark/light pair settings (`color_scheme` may be `"auto"` with `dark_color_scheme` /
  `light_color_scheme`).
- **Style resolution**: `Highlighter::style_for(&ScopeStack) -> Style` using `vel-scope` selector
  scores (documented specificity rules: more atoms/deeper match wins, later rules win ties);
  cache by interned scope-stack id; must be allocation-free on the hit path.

## 2. Default color schemes (our own design)

Design and ship **"Velocity Dark"** and **"Velocity Light"** (original palettes and rule sets;
do not copy Mariana/Monokai/Breakers/Celeste/Sixteen or any third-party scheme). Cover the
scope names from https://www.sublimetext.com/docs/scope_naming.html so the 202 official syntaxes
look good. Include a short design note (palette, contrast ratios ≥ 4.5:1 for normal text,
≥ 3:1 for comments).

## 3. `vel-syntax` metadata (tmPreferences)

- Load `.tmPreferences` (plist) settings keyed by scope selector: `shellVariables` (TM_COMMENT_START,
  TM_COMMENT_END, TM_COMMENT_START_2…, TM_COMMENT_DISABLE_INDENT), indentation rules
  (`increaseIndentPattern`, `decreaseIndentPattern`, `bracketIndentNextLinePattern`,
  `disableIndentNextLinePattern`, `unIndentedLinePattern`, `indentParens`), `showInSymbolList`,
  `showInIndexedSymbolList`, `showInIndexedReferenceList`, `symbolTransformation`,
  `symbolIndexTransformation`, `cancelCompletion`, `preserveIndent`.
- API: `metadata_for(&ScopeStack) -> Metadata` (merged by selector score).
- Use it to (a) enable the official **symbol assertions** (`@@@@ definition`, `@@@@ reference`,
  `partial-symbols`) in the syntax-test runner, and (b) the **reindent tests**
  (`reindent`, `reindent-unchanged`, `reindent-unindented`) with an indentation engine that
  follows ST's documented rules. Report the new pass rates; skip only what remains unsupported.

## 4. `vel-highlight` — incremental highlighting service

- Owns a `vel_text::Text` snapshot + `Arc<SyntaxSet>` + color scheme; produces per-line style
  spans. Runs on a background thread; the UI thread requests "lines a..b of version V" and
  receives results tagged with the version (stale results discarded or mapped).
- Checkpoints: store the `ParseState` every N lines (and at every line in the visible window);
  on edit (a `vel_buffer` change set), invalidate from the first changed line and re-parse until
  the state converges with the stored one (stop early). Handle `Rewind` results from branch
  backtracking.
- **Visible-first**: parse up to the viewport synchronously if the gap is small (≤ 2 ms budget),
  otherwise show unstyled/approximate text and fill in asynchronously; continue the rest of the
  file in the background at low priority; never block typing.
- Syntax detection: by file name/extension (incl. `hidden_file_extensions`), then
  `first_line_match`, with user override.
- Budgets: re-highlight after a 1-char edit in the middle of a 10k-line file, visible window
  100 lines: ≤ 2 ms p95 (the v1 parser's forced 100-line parse was 2.49 ms — optimize the parser
  hot path: avoid per-line allocations, reuse search caches, intern scope ops); full-file
  background highlight of `spikes/data/rust-10mb.rs`-like 10 MB Rust: report time; style
  resolution ≥ 20M lookups/s on cache hits.
- Also fix the two remaining JavaScript selector-tail failures if feasible.

## Rules

- Create/modify only: `crates/vel-color-scheme/`, `crates/vel-highlight/`, `crates/vel-syntax/`,
  `crates/vel-scope/`, `crates/vel-regex/`, `crates/velocity/resources/Packages/Color Scheme - Velocity/`
  (+ root `Cargo.lock`). Lane A is changing `velocity`, `vel-platform`, `vel-render`, `vel-config`,
  `vel-commands`, `vel-fileio` in parallel — do not touch them. The editor integration of
  `vel-highlight` will be done by lane A next round: write a clear integration section in
  `crates/vel-highlight/README.md`.
- Keep `crates/vel-syntax/tests/baseline.json` monotonic (never lower pass counts; new skips need a
  reason in RESULTS.md).
- Workspace checks for touched crates (fmt, clippy -D warnings, release tests, rustdoc).
