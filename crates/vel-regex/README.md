# vel-regex

`MultiDfa` is the production ordered-context backend; `OnigBackend` remains the
reference and exact fallback. Both implement `MatcherBackend` / `PatternMatcher`.
`SyntaxSet` uses MultiDfa by default. Patterns and captures remain independent of
the syntax interpreter.

```rust
use std::sync::Arc;
use vel_regex::{MultiDfa, MultiDfaConfig};
let backend = Arc::new(MultiDfa::new(MultiDfaConfig {
    memory_limit: 32 * 1024 * 1024,
    ..MultiDfaConfig::default()
}));
# let _ = backend;
```

Contexts with fewer than four rules use the original Onig matcher directly.
Context registration does no regex compilation. First search compiles an ASCII
candidate machine: forward/reverse hybrid DFAs, or Aho-Corasick for complete literal
sets. Identical ordered lists and empty-match policies share a set. One bounded pool
owns all machines and transition caches; LRU eviction releases them even when a parser
checkpoint still references the matcher. Default total budget: 32 MiB for the entire
backend (all syntaxes), with up to 1 MiB per-direction transition caches. `stats()` reports
actual API memory, evictions and first-use compile durations. Source/HIR, Onig,
parser/checkpoint memory and transient compiler allocations are separate.

The front end normalizes Onig extended-mode whitespace, dot-all flags, `\h`, POSIX
classes and anchors. Simple bounded boundary assertions use byte predicates; a single
positive lookahead can also be consumed by a candidate automaton without changing the
reported span. Capture-free assertion-only positive guards restore a zero-width span
directly; unhelpful epsilon candidates stay in Onig. Independent lookaheads remain independent. Unsupported internal
assertions can be removed only to form a conservative candidate superset, followed by
exact Onig verification. Guard rejection repairs the query in original rule order.
It never runs an anchored check of every pattern at every candidate byte.

The earliest start wins, then the lowest original rule index. Rule exclusions and
nonempty-match policies are retained. Captures are materialized after the final winner
is chosen: proven fixed capture layouts use inline spans, other winners use Onig. Original source
validity and capture-slot counts come from Onig, preserving dead/unmatched captures.
Extended-mode class whitespace and Rust-only class operators also have raw Onig probes.
Lower-priority fallback rules cannot beat a winner at the query start and are omitted.

Every non-ASCII haystack uses Onig, including `\b` / `\w`: Onig's Unicode word class
and full folding are reproduced directly, including the prototype's problematic
`²³¹¼½¾` characters and join controls. Search-start-sensitive `\G` / `\K`, backreferences,
recursive constructs and caller-supplied single-pattern backends retain their semantics
and resource limits in the fallback. Unicode DFA execution is a remaining optimization.

`MultiDfaConfig::verify_matches` asserts full spans, priority and captures against a
fresh Onig query. Optional `adaptive` selection compares 64 warm context queries and
can retain Onig; it is disabled by default because short initial samples can mispredict
later parser traffic. Measured limitations and all performance budgets are explicit in
[PERF.md](PERF.md). No disk cache is needed for the measured small lazy compile costs.

`Regex` is lazy, Arc-shared and Send + Sync. Broken patterns never match; resource-limit
errors count in `metrics()`. Onig defaults are 10,000,000 retries and an 8 MiB match stack.
Captures use inline storage for four slots. `SearchCache` must be reset before every new
line and retains allocated workspaces; `\G` / `\K` bypass next-match reuse. Runtime
capture substitutions use escaped literal text and a bounded strong cache.

The literal-prefix filter is adapted from MIT-licensed syntect PR #705 at
`3eea6bf27014e260d00e99552d4b861410457dd8`, by Tristan Hume, Keith Hall, Google Inc,
titoBouzout and other contributors; see [LICENSE-syntect](LICENSE-syntect).
The context front end builds on this repository's `prototypes/sregex` work. No
application-private binaries, code, syntaxes or themes are used.

```sh
cargo test --release -p vel-regex -p vel-syntax -p vel-highlight
python3 crates/vel-regex/scripts/measure.py
python3 crates/vel-regex/scripts/report.py
```

Every timing phase uses at least five runs; profiling is separate from throughput.
The differential uses all 242 official test files, all 202 loaded syntaxes and all eight
complete large fixtures. It compares LineOps, corrections, rewinds and diagnostic
matches/captures. The release integration test additionally compares memoized stable
parser transitions against the uncached Onig path.
