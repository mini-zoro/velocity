#!/usr/bin/env python3
"""Build the lane-B handoff from retained release evidence."""
import json
import math
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def read(path):
    return json.loads((ROOT / path).read_text())


def stats(values):
    values = sorted(values)
    return {'samples': len(values), 'median': statistics.median(values),
            'p95': values[math.ceil(len(values) * .95) - 1], 'max': max(values)}


index = read('crates/vel-index/round6-timings.json')
complete = read('crates/vel-complete/round6-timings.json')
highlight = read('crates/vel-highlight/round6-timings.json')
paired = read('crates/vel-highlight/round6-comparison.json')
corpus = read('crates/vel-index/round6-corpus.json')
summary = {}
summary['edit_visible_ms'] = stats(v for row in highlight if row['case'] == 'worker_edit_10k_visible_100' for v in row['samples'])
summary['scope_run_bytes'] = 12
summary['definition_ms'] = stats(row['ms_per_query'] for row in index if row['case'] == 'definition_million')
summary['fuzzy_ms'] = {q: stats(row['ms'] for row in index if row['case'] == 'fuzzy_million' and row['query'] == q)
                       for q in sorted({row['query'] for row in index if row['case'] == 'fuzzy_million'})}
summary['rss_bytes'] = stats(row['rss_bytes'] for row in index if row['case'] == 'million_memory')
summary['peak_rss_bytes'] = stats(row['peak_rss_bytes'] for row in index if row['case'] == 'million_memory')
summary['reindex_ms'] = stats(row['ms'] for row in index if row['case'] == 'edited_rust_file')
summary['complete_ms'] = {q: stats(row['ms'] for row in complete if row['query'] == q) for q in sorted({row['query'] for row in complete})}
summary['corpus_seconds'] = stats(row['seconds'] for row in index if row['case'] == 'corpus_cold')
summary['restart_seconds'] = stats(row['seconds'] for row in index if row['case'] == 'corpus_restart')
summary['corpus_files_per_second'] = corpus['files'] / summary['corpus_seconds']['median']
summary['rust_background_mb_s'] = stats(row['mb_per_second'] for row in highlight if row['case'] == 'full_10mb_rust_background')
summary['paired_before_s'] = stats(row['before_complete_lf_s'] for row in paired)
summary['paired_after_s'] = stats(row['after_scopes_s'] for row in paired)
summary['paired_time_change_percent'] = (summary['paired_after_s']['median'] / summary['paired_before_s']['median'] - 1) * 100

(ROOT / 'crates/vel-index/round6-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
s = summary
max_fuzzy = max(row['max'] for row in s['fuzzy_ms'].values())
max_complete = max(row['max'] for row in s['complete_ms'].values())
report = f'''# Lane B round 6: handoff and evidence

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
| Stable edit to exact 100-line batch, 10k lines | P95 ≤0.2 ms | P95 {s['edit_visible_ms']['p95']:.5f} ms; median {s['edit_visible_ms']['median']:.5f}; 500 edits |
| Compact scope-run payload | ≤16 bytes | 12 bytes, repr(C), tested |
| Exact definition lookup, 1M symbols | ≤1 ms | median {s['definition_ms']['median']:.6f} ms/query; worst batch mean {s['definition_ms']['max']:.6f} |
| Fuzzy symbols, 1M distinct names/top 50 | ≤10 ms | worst observation {max_fuzzy:.3f} ms; 25 samples/query |
| Index service RSS, 1M definitions + word/bigram counters | ≤200 MB | max RSS {s['rss_bytes']['max']/1e6:.3f} MB; max measured ingest peak {s['peak_rss_bytes']['max']/1e6:.3f} MB |
| One edited Rust file, 8,188 bytes | ≤50 ms | median {s['reindex_ms']['median']:.3f} ms; P95/max {s['reindex_ms']['max']:.3f} ms, including debounce |
| Completion, 100k project words + trigrams/punctuation | ≤5 ms | worst observation {max_complete:.3f} ms; five samples/query |
| Cargo-registry Rust corpus | ≥10k files, report rate/time | 10,000 files / {corpus['bytes']:,} bytes; {s['corpus_files_per_second']:.2f} files/s; median {s['corpus_seconds']['median']:.3f} s |
| Restart of that corpus | Avoid reparsing unchanged files | all 10,000 restored; zero parsed; median {s['restart_seconds']['median']:.3f} s |
| Rust 10 MB actual background parse/style/scopes | ≥2.5 MB/s | {s['rust_background_mb_s']['median']:.3f} MB/s — still open |

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
'''
for query, value in s['fuzzy_ms'].items():
    report += f"| `{query or '(empty)'}` | {value['median']:.3f} | {value['p95']:.3f} | {value['max']:.3f} |\n"
report += '''
| Contextual 100k-word completion query | Median ms | P95/max ms |
| --- | ---: | ---: |
'''
for query, value in s['complete_ms'].items():
    report += f"| `{query or '(empty)'}` | {value['median']:.3f} | {value['max']:.3f} |\n"
report += f'''
## Correct LF input and tracked performance

The previous highlighter fed Text::line(), which excludes LF, despite the parser's complete-line
contract. The implementation now slices through the next line start. Scope runs/styles include
LF and merge correctly across line boundaries. Lane A must clip spans before indexing a
Text::line() body. Unicode/newline/branch/palette tests compare scopes and symbols with the
corrected syntax reference.

The paired core comparison compiles commit 968f120's styling engine with only that same LF fix,
and compares it to the new scope/style engine. Both share the warmed SyntaxSet, use the real
10 MB Rust fixture, and alternate order for five pairs. Median core time is
{s['paired_before_s']['median']:.6f} s before and {s['paired_after_s']['median']:.6f} s after
({s['paired_time_change_percent']:.2f}% time change). A single active-stack lookup supplies scope
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
'''
(ROOT / 'crates/vel-index/ROUND6.md').write_text(report)
print(json.dumps(summary, indent=2))
