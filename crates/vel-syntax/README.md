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
before checking assertions. Symbol/reference assertions and modified tests are
reported as skips. The regression test checks the full pinned corpus, per-syntax
counts, the denominator and the skip count. Baseline updates require improvement
without per-syntax regressions. `tests/last-run.json` preserves evidence.

Limitations: `.tmLanguage` begin/end/while conversion explicitly returns
`LoadingError::UnimplementedTmLanguage`; zip packages are a later extension.
Symbol indexing and reindent execution are not implemented. Two selector-tail
compatibility failures remain in the JavaScript corpus. Numeric selector scores
remain an approximation of public specificity rules.

The loader, linker and interpreter adapt MIT-licensed syntect source, including
PR #705 at `3eea6bf27014e260d00e99552d4b861410457dd8`. Original contributors include
Tristan Hume, Keith Hall, Google Inc, titoBouzout and contributors to the branch/fail
and inheritance work. See [LICENSE-syntect](LICENSE-syntect) and the
[upstream change](https://github.com/trishume/syntect/pull/705). The public corpus
is used in place under its existing license; no syntax/theme assets are copied.
Velocity's scope and regex crates are independent of syntect's runtime and dumps.
