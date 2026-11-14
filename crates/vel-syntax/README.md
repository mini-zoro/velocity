# vel-syntax

Sublime YAML syntax loading and line interpretation, with official corpus tests.
The current measurements and all 202 per-syntax results are in [RESULTS.md](RESULTS.md).

```rust
use std::sync::Arc;
use vel_syntax::{ParseState, ScopeStack, SyntaxSet};

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let set = Arc::new(SyntaxSet::load_directories(&["third_party/sublime-packages"])?);
let syntax = set.find_syntax_by_extension("rs").ok_or("Rust is missing")?;
let mut state = ParseState::new(set.clone(), syntax);
let mut scopes = ScopeStack::new();
let output = state.parse_line("fn main() {}\n")?;
for (_, operation) in &output.ops {
    scopes.apply(operation)?;
}
# Ok(())
# }
```

Pass complete UTF-8 lines **including their trailing newline**. Operation offsets
are bytes, while syntax-test columns are Unicode character columns. Keep the
scope stack beside parser checkpoints. `ParseState: Clone + Eq + Hash + Send + Sync`
shares an immutable syntax set, uses copy-on-write interpreter snapshots, and
hash-conses context stacks. Equality permits convergence across changed line
counts after speculative branches have resolved.

`LineOps.rewind` and `LineOps.corrections` use **zero-based** line numbers. Apply
replacement operation lists in order (the last correction for a line wins), then
rebuild scope stacks from `rewind.from_line`. Already returned lines may change
for up to 128 preceding lines, the documented branch rewind limit. Branch buffers
are pruned to their live replay history. Zero-width stack cycles have explicit
guards and recover by advancing a UTF-8 character.

Loading covers inheritance, recursive variables, context append/prepend, v1/v2
scope semantics, prototypes, combined pop/actions, prioritized embed escapes and
cross-line branch/fail replay. Embedded syntax targets resolve when entered;
`SyntaxReference::contexts_are_loaded` exposes materialization for diagnostics.
`find_unlinked_contexts` is an explicit diagnostic traversal and materializes
contexts. For in-memory syntaxes, use `SyntaxSetBuilder::add_with_path` so package
references and inheritance can resolve by filename even when display names differ.

`SyntaxSet::load_cached(root, cache_dir)` keys the parsed index by definition
content hashes and a schema version. Cache files include a payload checksum and
are atomically replaced. Context bodies are compressed independently and decoded
only when entered; index metadata remains directly available. No compiled regex is serialized; corruption rebuilds
from YAML. Scope names, rather than process-local IDs, are serialized.

```sh
cargo run --release -p vel-syntax --bin syntax-tests
cargo run --release -p vel-syntax --bin syntax-tests -- Python
cargo run --release -p vel-syntax --bin syntax-tests -- --update-baseline
cargo run --release -p vel-syntax --bin syntax-bench
python3 crates/vel-syntax/scripts/report.py
```

The runner evaluates assertion **spans**: all asserted columns must match for a
case to pass. It parses annotation lines, recognizes pure assertions and compact
headers, handles past-EOL character columns, and applies branch corrections
before checking assertions. Symbol/reference assertions, partial-symbol exhaustiveness, and all reindent modifiers now execute.
Counts for scopes, symbols, and whole-file indentation variants are reported separately; the
current corpus has no skips. The regression test checks the full pinned corpus, per-syntax
counts, the denominator and the skip count. Baseline updates require improvement
without per-syntax regressions. `tests/last-run.json` preserves evidence.

Limitations: `.tmLanguage` begin/end/while conversion explicitly returns
`LoadingError::UnimplementedTmLanguage`; zip packages are a later extension.
All 688 symbol assertions pass. Unparseable annotation tails use the general longest
parseable selector prefix; `() {}` has no special handling and negative selectors remain strict.
Reindentation remains partially compatible; the exact failing variants and first mismatches
are recorded in RESULTS.md. Numeric selector scores
remain an approximation of public specificity rules.

The loader, linker and interpreter adapt MIT-licensed syntect source, including
PR #705 at `3eea6bf27014e260d00e99552d4b861410457dd8`. Original contributors include
Tristan Hume, Keith Hall, Google Inc, titoBouzout and contributors to the branch/fail
and inheritance work. See [LICENSE-syntect](LICENSE-syntect) and the
[upstream change](https://github.com/trishume/syntect/pull/705). The public corpus
is used in place under its existing license; no syntax/theme assets are copied.
Velocity's scope and regex crates are independent of syntect's runtime and dumps.

## Scope metadata and reindentation

`MetadataSet::load_directories(&[packages, user_preferences])` loads XML/binary tmPreferences
in deterministic path order, with later roots overriding equal scores. `metadata_for(&stack)`
merges each setting independently, and shellVariables merge by variable name. `Metadata::string`
and `boolean` expose indentation patterns, completion/preserve flags, and symbol visibility;
unknown settings remain in the plist-valued map. XML CDATA is preserved explicitly because the
plist dependency currently drops those events. No Sublime Default package preferences are used;
the small base symbol conventions are our own implementation of the public scope naming guide.

`indent::scoped_spans(&set, syntax, text)` parses a document and applies branch corrections.
Feed those byte ranges to `metadata.symbols(text, &spans)` for local/global definitions and
indexed references with transformed names. SymbolTransformation supports sed-style substitution
programs, comments, capture references, global flags, and case conversion. Empty global matches
retry a nonempty match at the same position, including TOML's reluctant whitespace rule.

`indent::reindent(text, &spans, &metadata, IndentOptions { tab_size: 4, spaces: true })` returns
reindented text. It uses scope-selected Oniguruma patterns, excludes comment/string brackets,
tracks continuation and brace contexts, preserves configured text, and respects tab stops.
It is not yet fully ST-compatible for the remaining complex cases in RESULTS.md. The runner
counts each requested whole-file variant once and reparses stripped text for
reindent-unindented; it never infers success from the input's existing whitespace.

`evaluate_with_metadata` supports independently supplied fixtures. `syntest::run` loads package
metadata once. Baseline migration preserves every previously passing scope case, then locks
scope/symbol/reindent counts independently per file. Newly evaluated indentation failures are
recorded as failures rather than hidden as skips or used to lower the scope baseline.

For an editor-owned incremental service, use vel-highlight rather than rebuilding checkpoints
and branch rewind handling in the UI. Its README documents transaction, undo/redo, viewport,
scheme-generation, and stale-result contracts.

## Ordered regex backend and streaming hot path

The default context factory is `vel_regex::MultiDfa`: lazy, shared ordered sets with a
32 MiB pool budget and Oniguruma fallback. Install `OnigBackend::default()` through
`with_matcher_backend` for the reference path. The backend keeps original rule indices,
empty-match policies, exclusions, captures and search-start semantics. Unicode input stays
in Oniguruma so its word boundaries (including ²³¹¼½¾) are reproduced exactly.

`ParseState::parse_line_into(line, &mut LineOps)` retains output capacity. Small captures and
capture operations use inline storage. Stable, non-speculative transitions are memoized per
document (4096 entries, at most 4 MiB of source text), with shared operation arrays. Checkpoints
share this cache but semantic equality ignores it. Absolute replay counters are restored before
interpreting a cache miss. `with_line_cache_capacity(0)` disables this optimization for backend
comparisons; branch corrections and warnings are never memoized.

`tests/differential.rs` compares every official LineOps, correction and rewind with Oniguruma,
and separately checks memoized transitions. `regex-bench --diff-only --verify-matches` also
compares every regex match/capture and all eight complete large fixtures. Measurements and
limits are in [../vel-regex/PERF.md](../vel-regex/PERF.md).
