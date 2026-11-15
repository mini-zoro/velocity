# vel-syntax

Sublime YAML syntax loading and line interpretation, with official corpus tests.
The locked assertion results for the original 202 syntaxes are in [RESULTS.md](RESULTS.md).
TextMate loading adds the existing Plain Text definition to the index (203 syntaxes).

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
from source definitions (YAML, XML or JSON). Scope names, rather than process-local IDs,
are serialized.

## Multiple roots and editor migration (round 10)

`SyntaxSet::load_cached_roots(roots: &[&Path], extra: &[(&str, &str)], cache_dir)`
loads ordered package trees and virtual definitions into one lazy binary index.
Each root contains package directories, e.g. `Rust/Rust.sublime-syntax`. Later
roots replace earlier definitions **by the entire package-relative path**, before
inheritance and includes link. The last extra at a path wins over every root.
Virtual paths may start with `Packages/`; `/` and `\\` are normalized. Distinct
paths remain distinct even when their display names or scopes match; later
resources win lookup ties, with `User/` resources last within each root. YAML,
XML tmLanguage and JSON tmLanguage all participate.

The digest frames root boundaries, resource paths, every source's contents
(including shadowed inputs), extra order and a schema version. Edits, additions,
removals and root/extra reordering invalidate it. Root locations are omitted:
moving identical trees reuses the cache, whose paths remain `Packages/...`.
`find_syntax_by_path` also accepts the winning sources' current physical paths;
these lookup aliases are rebuilt per load and never serialized.
Corrupt cache entries rebuild atomically from the already-read source bytes.
File and directory symlinks are followed; dangling links and grammar-named
directories are skipped, while loops and other traversal failures are errors.
Omit optional missing roots; an explicitly supplied missing root is an error.
`load_cached` is a one-root wrapper; `load_roots` and `load_directories` use the
same override selection without persistent caching. The lower-level builder
continues to support explicit physical paths.

Lane A: replace the **whole** `Assets::bundled` method in
`crates/velocity/src/language.rs` with the following. The original fallback text
is supplied in memory; the public corpus stays in place. The four telemetry slots
stay compatible, with staging/cleanup now zero. Keep `packages_root` and
`with_user_metadata`; remove `stage_syntaxes` and `stage_plain_text`, and replace
the staging-specific test with a direct-roots test. Keep `retire` for its other
analysis-service callers. No change in `crates/velocity` is included in this lane.

```rust,ignore
pub fn bundled() -> Result<Arc<Self>, String> {
    static ASSETS: OnceLock<Result<Arc<Assets>, String>> = OnceLock::new();
    ASSETS
        .get_or_init(|| {
            let root = packages_root();
            let metadata_root = root.clone();
            let metadata = std::thread::Builder::new()
                .name("velocity-metadata".into())
                .spawn(move || {
                    let phase = std::time::Instant::now();
                    let result = MetadataSet::load_directories(&[metadata_root])
                        .map_err(|e| e.to_string());
                    (result, phase.elapsed().as_secs_f64() * 1000.)
                })
                .map_err(|e| e.to_string())?;
            let cache = std::env::temp_dir().join("velocity-syntax-cache");
            let extra = [(
                "Packages/Default/Plain Text.sublime-syntax",
                "name: Plain Text\nfile_extensions: [txt]\nscope: text.plain\ncontexts: { main: [] }\n",
            )];
            let phase = std::time::Instant::now();
            let syntaxes = SyntaxSet::load_cached_roots(&[root.as_path()], &extra, &cache)
                .map_err(|e| e.to_string());
            let load_ms = phase.elapsed().as_secs_f64() * 1000.;
            let (metadata, metadata_ms) =
                metadata.join().map_err(|_| "metadata loader panicked")?;
            Ok(Arc::new(Self {
                syntaxes: Arc::new(syntaxes?.0),
                metadata: Arc::new(metadata?),
                startup_ms: [0., load_ms, 0., metadata_ms],
            }))
        })
        .clone()
}
```

For user syntax overrides, pass the existing `data.join("Packages")` directory
after the shipped root, filtering absent optional directories first. Definitions
inside `User/` remain addressable as `Packages/User/...`.
`MetadataSet::load_roots(roots, extra)` and the existing `load_directories` now
select preferences by the same full resource path, including linked files and
directories. Replacing a tmPreferences file removes keys from the shadowed file;
distinct files still merge settings by selector specificity, then resource order.
Metadata remains parsed directly; the persistent binary cache is for syntaxes.

Run `cargo run --release -p vel-syntax --example roots_probe` to measure content
hashing plus lazy index loading for the 203 corpus syntaxes and one virtual extra.

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

TextMate `.tmLanguage` XML plists and `.tmLanguage.json` are loaded by directory
and cached loaders. `SyntaxDefinition::load_tm_language(source)` accepts either
text form for in-memory grammars. Patterns/repositories (including local overrides),
`#name`, `$self`, document-relative `$base`, `source.x` and `source.x#name` includes,
match/name/captures, begin/end and their captures/contentName, numeric begin-to-end
backreferences, and applyEndPatternLast use the existing linked interpreter.
Begin/while rules check outer-to-inner on each subsequent line, apply whileCaptures,
and unwind nested contexts when continuation fails. Supply lines including LF.

Injections/injectionSelector, capture retokenization, and interpolated scope names
return `LoadingError::TmLanguage` diagnostics rather than being silently dropped.
Directory loading records these as warnings, consistently with malformed YAML.
Zip packages remain a later extension. The cache schema/content digest includes
both TextMate forms. The public fetch-at-test-time check loads the MIT-licensed
VS Code 1.105.0 JSON, Diff and Log grammars and verifies emitted scopes; transport
failures print an explicit offline skip. CSS in that version requires capture
retokenization and is currently rejected with the documented diagnostic.
The official corpus now includes the existing public Plain
Text.tmLanguage, bringing the loaded index to 203 syntaxes; assertion counts are
unchanged. `tests/tm_language.rs` has local semantic/cache fixtures and downloads
three MIT VS Code grammars plus their license at test time, explicitly skipping
when curl/network is unavailable.
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
document (16,384 entries, at most 4 MiB of source text), with shared operation arrays.
A clock replacement policy gives recently reused transitions a second pass before
eviction; first-use admissions stay cold. The entry cap bounds snapshot/operation
metadata separately from the source-text byte cap. The weak stack interner
spaces full cleanup passes by another 8,192 buckets after each sweep, even when
more than 8,192 checkpoint stacks remain live. Checkpoints share this cache but
semantic equality ignores it. Absolute replay counters are restored before
interpreting a cache miss. `with_line_cache_capacity(0)` disables this optimization for backend
comparisons; branch corrections and warnings are never memoized.

`tests/differential.rs` compares every official LineOps, correction and rewind with Oniguruma,
and separately checks memoized transitions. `regex-bench --diff-only --verify-matches` also
compares every regex match/capture and all eight complete large fixtures. Measurements and
limits are in [../vel-regex/PERF.md](../vel-regex/PERF.md).

`MetadataSet::fingerprint()` returns a stable digest of preference rule text, settings and
resource order for the project index cache. Runtime metadata/regex caches and process-local
scope IDs do not enter that digest. `vel-highlight::ScopeSnapshot` now supplies the editor's
corrected scopes and incremental symbol candidates from its existing parse; editor integration
should consume it instead of calling `indent::scoped_spans` a second time.
