# Spec: production multi-pattern regex backend ("sregex") + parser hot path (P2 throughput)

Status: P2 / lane B (core engine). Locations: `crates/vel-regex/`, `crates/vel-syntax/`,
`crates/vel-highlight/` (only if needed for measurement).

## Why

Correctness is done (scopes 125,877/0, symbols 688/0 on the official corpus), but full-file
background highlighting of a 10 MB Rust file takes 7.44 s (~1.34 MB/s) with the Oniguruma
backend. Sublime Text 3 already highlighted ~2.5 MB/s (~94k lines/s, incl. styling) on 2017
hardware, and ST4 claims faster matching. Throughput matters for opening large files, the
minimap, indexing and find-in-files result highlighting. Target: **≥ 2.5 MB/s parse+style on the
Rust corpus, ≥ 2× the current Oniguruma backend across the 8 canonical languages**, with
identical results.

Prior art: `prototypes/sregex/` (README, RESULTS.md) — multi-pattern hybrid DFA per context,
Onig→regex-syntax front end, boundary-lookahead guards, Onig fallback for the rest; it was
1.36× faster in aggregate (geomean 1.92×) and byte-identical except a documented Unicode `\b`
difference, but lost badly on Rust `statements` (0.46×) and C++ `global`, and eager compilation
cost up to 2.96 s / 370 MB for C++. Read its "conclusions and risks" section first.

## Requirements

1. `vel-regex` gets a `MultiDfa` backend implementing the existing backend trait, selected per
   context automatically; Oniguruma stays as the fallback and as the reference.
   - Onig syntax front end: port and harden the prototype's translator (`\h`, POSIX classes,
     `(?x)`, `\A \z \Z \G`, Onig's `\b`/`\w` Unicode semantics — fix the `²³¹¼½¾` word-boundary
     difference by defining word characters exactly like Oniguruma's default syntax).
   - **Lazy** compilation per context (only contexts actually entered), shared compiled sets for
     identical ordered pattern lists, bounded memory per syntax (configurable; default ≤ 32 MB
     across all compiled DFAs; evict LRU), optional on-disk cache of serialized DFAs keyed by
     pattern-set hash if it pays off.
   - Make lookahead cheap: compile common bounded lookaheads into the automaton (e.g. trailing
     `(?=\s|$)`, `(?!\w)`), use a priority-ordered candidate set instead of per-pattern anchored
     repair, literal prefilters (memchr/aho-corasick) where a context's patterns all start with
     literals; profile Rust `statements` and C++ `global` specifically and fix why they lose.
   - Captures computed only for the winning pattern.
2. Parser hot path in `vel-syntax`: no per-line heap allocation in the steady state (reuse op
   vectors, search caches, scope buffers), interned scope ops, cheaper context-stack updates.
3. **Differential testing is mandatory**: a test mode that runs both backends on every line of the
   whole official corpus (all 242 test files + their syntaxes) and on `spikes/data`-style large
   files, asserting identical `LineOps` (and identical rewinds). Any divergence is a bug unless
   it's a documented Onig quirk the backend intentionally reproduces.
4. Keep the corpus baseline monotonic (scopes 125,877/0, symbols 688/0, reindent ≥ 26).
5. Also fix as many of the 9 remaining reindent failures as reasonable (C++/JS continuation
   indentation, JSX, PHP, shell `case`/heredoc), using `.tmPreferences` rules faithfully — no
   per-test special cases. (The reviewer replaced a per-test `strip_suffix("() {}")` hack with a
   general rule last round; general rules only.)

## Budgets (M1 Pro, release, ≥ 5 runs, report median/P95)

| Metric | Target |
|---|---|
| Rust 10 MB parse+style, full file | ≥ 2.5 MB/s (today 1.34 MB/s) |
| 8 canonical languages, parse-only, vs Oniguruma backend | ≥ 2× each (report per language) |
| Worst single context vs Oniguruma | ≥ 1.0× (no context slower than the fallback) |
| First-use compile latency of a context (p95) | ≤ 2 ms (lazy) |
| Memory of compiled DFAs after highlighting all 8 corpora | ≤ 32 MB |
| Edit → visible re-highlight p95 (existing service benchmark) | stay ≤ 0.2 ms |

## Rules

- Modify only `crates/vel-regex/`, `crates/vel-syntax/`, `crates/vel-scope/`, `crates/vel-highlight/`
  (+ root `Cargo.lock`). Lane A works on UI crates and `velocity` in parallel.
- fmt, clippy -D warnings, release tests, rustdoc -D warnings for touched crates. Report in
  `crates/vel-regex/PERF.md` (Chinese).
