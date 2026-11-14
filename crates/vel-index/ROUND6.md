# Lane B round 6: handoff and evidence

Implemented scope publication/query APIs, the project index, completion queries/workers and the
snippet field engine. Changes are uncommitted and restricted to the permitted lane-B crates and
Cargo.lock. Lane A's parallel files were left to lane A. API examples and lifecycle contracts:
[highlight/migration](../vel-highlight/README.md#round-six-editing-scopes-and-symbols-remove-the-second-parser),
[index](README.md), [completion](../vel-complete/README.md), [snippets](../vel-snippet/README.md).

## Budget results

Release, Darwin arm64 / macOS 26.1, rustc 1.99.1. MB means decimal MB. P95 uses nearest rank.
Reported timings use Instant and OS RSS FFI, without time/sysctl wrappers. The host is shared
with lane A; all samples, including scheduling outliers, remain in the JSON. No CPU/load value
was invented. No performance timing overlaps another lane-B benchmark or test run.

| Metric | Target | Measured |
| --- | --- | --- |
| Stable edit to exact 100-line batch, 10k lines | P95 ≤0.2 ms | P95 0.07704 ms; median 0.04819; 500 edits |
| Compact scope-run payload | ≤16 bytes | 12 bytes, repr(C), tested |
| Exact definition lookup, 1M symbols | ≤1 ms | median 0.000629 ms/query; worst batch mean 0.003864 |
| Fuzzy symbols, 1M distinct names/top 50 | ≤10 ms | worst observation 3.545 ms; 25 samples/query |
| Index service RSS, 1M definitions + word/bigram counters | ≤200 MB | max RSS 175.145 MB; max measured ingest peak 175.145 MB |
| One edited Rust file, 8,188 bytes | ≤50 ms | median 25.780 ms; P95/max 30.067 ms, including debounce |
| Completion, 100k project words + trigrams/punctuation | ≤5 ms | worst observation 2.092 ms; five samples/query |
| Cargo-registry Rust corpus | ≥10k files, report rate/time | 10,000 files / 139,497,024 bytes; 121.13 files/s; median 82.559 s |
| Restart of that corpus | Avoid reparsing unchanged files | all 10,000 restored; zero parsed; median 12.041 s |
| Rust 10 MB actual background parse/style/scopes | ≥2.5 MB/s | 2.001 MB/s — still open |

Scope payload is the compact token record, excluding line/checkpoint structures, Arc headers,
stack/style interning tables and symbol strings. The syntax/highlight service still uses dense
checkpoints. Project RSS includes the loaded cached SyntaxSet, four idle indexing workers,
1M distinct definition names, location postings, 1M word frequencies and 1M bigrams across 1,000
files. Five fresh processes each execute five query rounds. Definition figures are means of
1,000-call batches; the table reports their distribution, rather than claiming individual-call
P95. RSS is sampled after ingestion; the sampled ingest peak includes fixture construction.
Fuzzy and completion figures include result construction, sorting and cancellation checks.
Completion preparation/resource loading is outside the query timer; the current view is small.
The edited-file benchmark includes normal 15 ms debounce and does not bypass it with flush.

## Query distribution

| Million-symbol query | Median ms | P95 ms | Max ms |
| --- | ---: | ---: | ---: |
| `(empty)` | 0.010 | 0.019 | 0.020 |
| `render` | 2.118 | 2.509 | 2.522 |
| `rw99` | 2.295 | 2.545 | 2.580 |
| `widget42` | 3.327 | 3.542 | 3.545 |
| `zzzz` | 0.377 | 0.462 | 0.977 |

| Contextual 100k-word completion query | Median ms | P95/max ms |
| --- | ---: | ---: |
| `(empty)` | 1.639 | 1.694 |
| `candidate42` | 2.008 | 2.092 |
| `wc99` | 1.299 | 1.412 |
| `word` | 1.659 | 1.726 |
| `zzzz` | 0.654 | 0.680 |

## Correct LF input and tracked performance

The previous highlighter fed Text::line(), which excludes LF, despite the parser's complete-line
contract. The implementation now slices through the next line start. Scope runs/styles include
LF and merge correctly across line boundaries. Lane A must clip spans before indexing a
Text::line() body. Unicode/newline/branch/palette tests compare scopes and symbols with the
corrected syntax reference.

The paired core comparison compiles commit 968f120's styling engine with only that same LF fix,
and compares it to the new scope/style engine. Both share the warmed SyntaxSet, use the real
10 MB Rust fixture, and alternate order for five pairs. Median core time is
4.700917 s before and 4.603446 s after
(-2.07% time change). A single active-stack lookup supplies scope
IDs, cached styles and symbol membership. This avoids a scope-output regression over equivalent
input; it does not close the separate 2.5 MB/s background target. The old Markdown __main and
Rust type-path regex-context targets remain as reported in vel-regex/PERF.md; no matcher/backend
changes were made in this round.

## Corpus, cache and validation

The corpus consists of the first 10,000 lexicographically sorted local Cargo registry .rs files
up to 2 MiB, hardlinked into a temporary tree. It is a Rust-only parser-heavy registry corpus,
not a claim to have indexed every registry resource. Five cold scans use separate cache dirs,
four utility-QoS workers, public package preferences, and include walking, file I/O, extraction,
word/sequence statistics, journal writes and flush. Syntax/preference loading is outside timing.
Every run produced 1,672,269 definition/reference locations, with zero skipped/failed files.
Five restarts restore all files without a syntax parse. The on-disk format uses checksummed
append-only records with torn-tail recovery and syntax/metadata/options invalidation.

Validation passed for the touched crates: package-scoped fmt, clippy all-targets with -D warnings,
release tests and README doctests, and rustdoc with -D warnings. The full official syntax baseline
and differential were rerun: scopes 125,877/0, symbols 688/0, reindent 30/5, zero skips, zero
LineOps/rewind/memoization divergences. Baseline files were not edited. Remaining reindent failures
are inherited; this round changes metadata fingerprinting, not parser/regex semantics.
Tests cover incremental scopes and symbols, branch repairs, stale queries/overlays, field/mirror
and transformation tables, independent carets, visual indentation, scalar-vs-prefix fuzzy
scores, storage-chunk boundaries, a replacement-index model, ignore updates, notify or its
500 ms metadata fallback, project-local cache exclusion, restart and truncated journal recovery.

The API stays cooperative around individual syntax/regex calls. Full-file symbol assembly runs
on the dispatcher; it joins incrementally maintained candidates instead of parsing again.
`analyze_scopes` + `update_buffer_analysis` reuse that output for unsaved project indexing too;
a counting matcher backend test verifies zero context builds on this path.
Retired interned names/sequence keys and journal history await cache rebuild for reclamation.
Snippet edits rewrite each instance atomically; numeric capture replacements and documented
conditionals/case changes are implemented, while named replacement aliases are not exposed.
These limits are documented in the crate READMEs.

Reproduction after building the three examples:

```sh
cargo build --release -p vel-index -p vel-complete -p vel-highlight --examples
python3 crates/vel-index/scripts/measure.py --corpus /path/to/10000/rust/files
python3 crates/vel-highlight/scripts/compare-round6.py
python3 crates/vel-index/scripts/report.py
```

The comparison helper creates its reference workspace only in /private/tmp, reads the accepted
baseline with git show, and never edits repository history. Raw evidence: round6-timings.json,
round6-corpus.json, ../vel-complete/round6-timings.json,
../vel-highlight/round6-timings.json, ../vel-highlight/round6-before.json and
../vel-highlight/round6-comparison.json. round6-summary.json is generated from these files.
