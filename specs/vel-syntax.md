# Spec: `vel-scope` + `vel-regex` + `vel-syntax` — Sublime syntax engine v1 (correctness first)

Status: P1 / lane B (core engine). Locations: `crates/vel-scope/`, `crates/vel-regex/`,
`crates/vel-syntax/`.

Goal of this round: **load all 202 official syntaxes and pass ≥ 90% of the official syntax-test
assertions**, with an architecture that lets the multi-pattern DFA matcher from
`prototypes/sregex/` replace the regex backend later (P2) without touching the parser.
Throughput is secondary this round but must be measured.

Test corpus: `third_party/sublime-packages/` (git submodule of sublimehq/Packages pinned to
`3f8b2c5`, 202 `.sublime-syntax`, 242 `syntax_test_*` files; permissive license — keep notices).
Baseline to beat (spikes/RESULTS.md §2): stable syntect 5.3 loads 78/202 and passes e.g.
Rust 99.5%, C++ 61.5%, Markdown 13.7%, JavaScript/HTML not loadable.

Allowed references (clean-room, see CONTRIBUTING.md): the official docs
https://www.sublimetext.com/docs/syntax.html , https://www.sublimetext.com/docs/scope_naming.html ,
https://www.sublimetext.com/docs/selectors.html , the syntax files and tests themselves, and
open-source implementations such as syntect (MIT; incl. the semantics described in its PR #705).
Adapting syntect code is allowed with attribution in the crate README.

## `vel-scope`

- Atom interning (global, thread-safe, cheap `Copy` ids). `Scope` = sequence of atoms of
  **any length** (syntect's 8-atom limit broke ActionScript). `ScopeStack` with
  push/pop/clear(n)/restore semantics needed by `clear_scopes`.
- `Selector` parsing and matching with the full documented syntax: descendant (space), `-`
  exclusion, `|`/`,` alternation, `&` intersection, parentheses, and the **`>` child operator**
  (added in ST build 4213). Matching returns a score so color schemes can use it later
  (implement the documented specificity rules as far as the docs specify; mark guesses with
  `// ST-BEHAVIOR: unverified`).
- Very hot path: matching a selector against a stack must not allocate.

## `vel-regex`

Oniguruma-compatible regex facade used by the parser.

```rust
pub struct Regex;                 // compiled once, shared (Arc), Send + Sync
pub struct PatternSet;            // the ordered match patterns of one context
impl PatternSet {
    /// Earliest match starting at or after `start` within `line`; ties → lowest pattern index.
    /// Captures are computed only for the winner. Honors that patterns may be anchored with
    /// `\G` relative to `start` and may look behind `start` (search the whole line, begin at start).
    pub fn find_earliest(&self, line: &str, start: usize, cache: &mut SearchCache) -> Option<Match>;
}
```

- Backend this round: the `onig` crate (same flavor as Sublime's fallback engine) behind a trait
  so a `MultiDfa` backend can be added later. Use a per-line search cache like syntect's (cache
  each pattern's next match position on the current line).
- Dynamic patterns: patterns containing back-references to captures of the push match
  (`\1` in `escape`/pop patterns) are compiled per instantiation with the captured text
  regex-escaped; cache instantiations.
- Oniguruma retry/stack limits: a pattern that hits the limit counts as "no match" and is
  counted in metrics, never panics.
- Compile lazily on first use; keep compile errors as values (a broken pattern must not take down
  the whole syntax — report it and treat as never-matching).

## `vel-syntax`

### Loading

- `.sublime-syntax` YAML, `version: 1` and `version: 2` with their documented semantic
  differences; `name`, `scope`, `file_extensions`, `hidden_file_extensions`, `first_line_match`,
  `hidden`, `variables` (recursive `{{var}}` expansion), `contexts`, `extends` (single and
  multiple parents with the same base; package paths like
  `Packages/JavaScript/JavaScript.sublime-syntax`), `meta_prepend` / `meta_append` on contexts in
  inheriting syntaxes, `prototype`, `meta_include_prototype`, `meta_scope`, `meta_content_scope`,
  `clear_scopes` (`true` or N).
- Patterns: `match`, `scope`, `captures`, `push`, `pop` (`true` or N, alone or together with
  push/set/embed/branch as allowed by version 2), `set`, `embed` + `escape` + `embed_scope` +
  `escape_captures`, `branch_point` + `branch`, `fail`, `include` (local context, `scope:x.y`,
  `scope:x.y#ctx`, package-path forms), anonymous contexts, `with_prototype`.
- A `SyntaxSet` built from a set of package directories (and later `.sublime-package` zips) with
  lookup by scope, by name, by file extension, by `first_line_match`. Cross-syntax references are
  resolved **lazily**: an `embed`ded syntax is only resolved/compiled when first entered
  (ST4 behavior; makes Markdown etc. cheap).
- `.tmLanguage` (plist XML) loading: convert to the same internal representation (begin/end/while
  rules); lower priority — do it if time permits, otherwise leave a clear `unimplemented` error.
- Binary cache of the parsed (not regex-compiled) definitions keyed by file content hashes
  (e.g. postcard/bincode into a cache dir), so loading metadata for all syntaxes at app start is
  fast. Budget: building the `SyntaxSet` index for all 202 syntaxes from cache ≤ 20 ms; from YAML
  ≤ 1.5 s total (one-time).

### Parsing

- Line-oriented `ParseState` (context stack + branch-point bookkeeping), `parse_line(&mut self,
  line: &str) -> LineOps` where `LineOps` is a compact list of `(byte_offset, ScopeOp)`
  (push/pop/clear/restore). Lines are passed **with** their trailing `\n` (ST semantics — patterns
  may match `\n`).
- `branch_point` / `branch` / `fail`: on `fail`, rewind to the branch position (possibly on an
  earlier line) and try the next alternative. Keep the state snapshots needed to rewind across
  lines, with a documented limit on how many lines a branch may span (ST has one; pick a sensible
  value, mark `ST-BEHAVIOR: unverified`), and return a `Rewind { from_line }` signal so the caller
  can re-emit ops for already-returned lines.
- Infinite-loop guards: zero-length matches that don't change the stack advance by one char;
  push/pop cycles at the same position are detected.
- `ParseState: Clone + Eq + Hash` cheaply (shared, hash-consed context stacks) so that incremental
  re-highlighting can stop as soon as the state after a line equals the cached one.

### Syntax test runner

- Implement the official syntax test format: header line
  `<comment> SYNTAX TEST [modifiers] "Packages/…/X.sublime-syntax"` (also the compact `T:` form
  if present), assertion lines with `^` (columns of the previous non-assertion line) and `<-`
  (column of the comment start), selectors on assertions, and `@` symbol assertions (report as
  *skipped* if symbol indexing isn't implemented). Reindent modifiers: skipped and reported.
- A binary `syntax-tests` (`cargo run --release -p vel-syntax --bin syntax-tests -- [filter]`) and a
  `cargo test` integration test that runs the whole corpus and fails if the pass rate drops below
  the committed baseline in `crates/vel-syntax/tests/baseline.json` (per syntax: assertions
  passed/failed/skipped). Update the baseline only when the rate improves.
- Report (Chinese) `crates/vel-syntax/RESULTS.md`: per-syntax table (202 loaded?, assertions
  pass/fail/skip, top failure causes), overall pass rate, comparison with the syntect baseline.

## Budgets

| Item | Target this round |
|---|---|
| Official syntaxes loaded without error | 202 / 202 |
| Official syntax-test assertions passed (all 242 files) | ≥ 90 % overall; Rust/JSON/YAML/Python ≥ 98 %; JS/C++/Markdown/HTML ≥ 90 % |
| Panics on the corpus | 0 |
| Parse throughput (parse-only, official Rust syntax, `spikes/data/rust-10mb.rs` style input; generate if missing) | ≥ syntect-onig (1.02 MB/s, ~30k lines/s) |
| Re-parse a 100-line visible window after a 1-char edit (incremental, state convergence) | ≤ 2 ms |
| SyntaxSet index for 202 syntaxes from binary cache | ≤ 20 ms |

## Rules

- No `unsafe` outside FFI necessity (the `onig` crate is FFI already; you shouldn't need any).
- Workspace checks (fmt, clippy -D warnings, tests, rustdoc) must pass.
- Don't modify `crates/vel-text`, `crates/vel-buffer` (stable) or the lane-A crates.
