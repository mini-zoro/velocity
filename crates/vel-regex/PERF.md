# Lane B, round 9: Vintage, TextMate and highlighting throughput

Measured November 15, 2026; Darwin 25.1.0 arm64, rustc 1.99.1 (58cf8f9c3).
Release builds, one timed task at a time. Other activity on the shared machine was not controlled.
All reported medians use five samples; P95 is nearest rank (the maximum of five).
No sample was removed. MB/s uses decimal megabytes. These measurements supersede the round 5 timing table; historical raw files remain available.

The 2.5 MB/s background target remains open for **rust, javascript, cpp**.
Correctness checks pass; performance closure is separate. The vi and TextMate compatibility limits are in their crate documentation.

## Production background parse + style + checkpoints

The example invokes the existing vel-highlight DocumentHighlighter and HighlightService. Each language receives one full-file warmup, then five fresh workers.
The timer includes worker creation, parsing, styling, checkpoints and completion polling (1 ms). It excludes fixture I/O, syntax loading, warmup, Text construction and worker teardown.
Stable transition memoization is enabled: 16,384 entries, 4 MiB source-text cap and clock replacement. Snapshot and operation metadata are bounded by entry count; the source-text cap is not a total-memory cap.

| Language | MB | Worker MB/s | Target ≥2.5 | Median / P95 seconds | Uncached parser MB/s |
| --- | ---: | ---: | --- | ---: | ---: |
| rust | 10.000 | 1.531 | open | 6.530920 / 6.823963 | 1.704 |
| javascript | 10.000 | 1.932 | open | 5.176507 / 6.302747 | 1.524 |
| python | 5.037 | 3.681 | pass | 1.368200 / 1.529441 | 1.923 |
| cpp | 5.117 | 1.736 | open | 2.946852 / 3.323644 | 2.027 |
| json | 20.118 | 11.507 | pass | 1.748281 / 2.144763 | 5.283 |
| markdown | 5.000 | 3.952 | pass | 1.265165 / 1.743844 | 3.310 |
| yaml | 5.006 | 11.160 | pass | 0.448535 / 0.516853 | 4.919 |
| html | 5.002 | 17.688 | pass | 0.282789 / 0.314053 | 8.238 |

## Ordered matcher, identical uncached parser

Both backends disable stable-line memoization and reuse LineOps storage. A complete warmup precedes five alternating Oniguruma/candidate runs.
This measures real context transitions, regex searches, captures, branch corrections and scope operations. It is distinct from the background worker above.
The before executable was built from the original round 9 checkout (cfd76b3, before these edits) in an isolated /tmp workspace. Both versions read the same fixtures and pinned package corpus.

| Language | Before MB/s | Final MB/s | Oniguruma MB/s | Final speedup | Final median / P95 seconds | Before→final median time | Oniguruma median time change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| rust | 1.750 | 1.704 | 1.289 | 1.322× | 5.868801 / 6.221913 | +2.7% | -13.4% |
| javascript | 1.639 | 1.524 | 1.254 | 1.215× | 6.560773 / 7.819945 | +7.5% | +18.2% |
| python | 1.758 | 1.923 | 0.954 | 2.016× | 2.619175 / 2.701506 | -8.6% | -12.8% |
| cpp | 1.821 | 2.027 | 1.942 | 1.044× | 2.525008 / 3.021103 | -10.2% | -12.2% |
| json | 5.358 | 5.283 | 2.824 | 1.870× | 3.808177 / 3.890217 | +1.4% | +3.4% |
| markdown | 3.082 | 3.310 | 1.878 | 1.763× | 1.510424 / 1.523670 | -6.9% | -9.0% |
| yaml | 3.424 | 4.919 | 2.894 | 1.700× | 1.017569 / 1.158175 | -30.4% | -38.3% |
| html | 7.008 | 8.238 | 4.936 | 1.669× | 0.607212 / 0.776976 | -14.9% | -19.0% |

The initial full run showed >5% wall-time regressions for C++ and HTML during large timing swings. That run is retained in round9-throughput-first.json.
The initial sequential rechecks below preceded the final hot-path cleanup. They retain five samples per executable and supplement the final tables and interleaved diagnostic below.

| Recheck | Before MB/s | After MB/s | Median time change | Oniguruma median time change |
| --- | ---: | ---: | ---: | ---: |
| cpp | 1.543 | 1.106 | +39.6% | +11.4% |
| html | 6.735 | 4.458 | +51.1% | +11.6% |

The final regression diagnostic links the original and current parsers into one process, warming each over a complete file and alternating 1,024-line chunks in both orders. Each state persists across chunks, with memoization disabled. Timers sum active chunks only; the five per-pass pairs are retained in round9-paired.json. This diagnostic does not replace contiguous wall throughput above.

| Interleaved final comparison | Before median s | After median s | Median paired time change |
| --- | ---: | ---: | ---: |
| cpp | 2.994522 | 3.016792 | +0.1% |
| html | 0.717897 | 0.739733 | +4.4% |
| python | 2.889895 | 2.792718 | -0.6% |
| javascript | 10.749528 | 10.938619 | -1.2% |

The diagnostic copies the original crates to /tmp and changes only package names to allow both versions to link. Source digests are recorded in its JSON. Run scripts/paired-round9.py --before /path/to/original/checkout to reproduce it. round9-paired-cpp-html.json retains the earlier two-language comparison. The final diagnostic also includes Python: its preliminary wall-time median was 6.5% slower, and the paired check confirmed the routing regression described below. round9-paired-javascript.json adds five JavaScript pairs after its final contiguous wall-time samples also moved more than 5%.

## Context profiles and general changes

Context profiling is a separate instrumented run over the first approximately 200 KB per language, five complete parser passes. Per-query Instant overhead is included here, not in the full-file tables.

| Context | Queries per pass | Candidate median / P95 ms | Oniguruma median ms | Speedup |
| --- | ---: | ---: | ---: | ---: |
| Markdown::__main | 1 | 0.003125 / 0.018792 | 0.001084 | 0.347× |
| Rust::type-path | 186 | 0.269041 / 0.303672 | 0.097501 | 0.362× |
| Rust::declaration-identifier | 1820 | 9.656003 / 9.947065 | 4.151561 | 0.430× |
| Rust::statements | 3740 | 6.060679 / 7.829619 | 13.021388 | 2.149× |
| Rust::impl-trait-types | 60 | 0.181413 / 0.215421 | 0.057207 | 0.315× |
| Rust::union-identifier | 3 | 0.004584 / 0.007583 | 0.001542 | 0.336× |
| C++::#anon_data-structures-struct-definition_0 | 5 | 0.007042 / 0.009791 | 0.002626 | 0.373× |

General rules: classify ASCII once per input line, avoid a duplicate native span search before extracting captures for exact candidates, and retain direct Oniguruma for sets below the existing four-pattern threshold. No syntax names enter these choices.
Native source compilation, assertion guards, empty-match policy and LF edge cases still gate the exact path. Captures are materialized only for the final winner.
A proposed direct-native route for early empty exits and bounded exact sets caused a 10.6% paired Python regression. Restoring the old capture path did not fix it; removing those routing heuristics did. The failed all-language run is retained as round9-before-routing-*, with the two five-pair isolation runs in round9-python-capture-path-experiment.json and round9-python-dfa-routing.json. The final tables use the corrected general routing.
The TextMate while continuation handler is outlined as cold code; ordinary YAML lines never scan the stack for it. Cache-disabled parsing avoids unused line hashing and locking. Production caching uses clock replacement and a larger bounded window. A two-hit admission experiment was removed after five Rust samples regressed; the raw experiment remains in round9-worker-admission-rust.json.
round9-worker-before-clock.json, round9-worker-clock-only.json and round9-worker-window-{rust,js}.json preserve the cache experiments. Shared-machine variation prevents treating these sequential observations as a precise isolated speedup estimate.
round9-pre-cold-* retains the discarded revision that published every exact DFA span to the native cache. Those extra writes were removed. Low-frequency context measurements (sometimes one query per pass) are especially sensitive to timer overhead. Rust helper-context ratios can remain below 1 even when the whole parser is faster: all-native parent contexts prepopulate more native search results. The profile reports that cost instead of hiding it behind whole-language averages.

The larger checkpoint window exposed a general weak-stack interner bug: once more than 8,192 stack buckets stayed live, every subsequent intern scanned the entire pool. Cleanup now waits for another 8,192 buckets after each sweep. Weak references still do not retain stack data, and a regression test checks both live-stack scan frequency and eventual dead-metadata cleanup.
The retained before-sweep full worker run measured Markdown at 0.558 MB/s; the isolated five-sample check after the fix measured 5.384 MB/s. These sequential observations are diagnostic evidence; the fresh all-language table above is the final result. All round9-before-sweep-* inputs and round9-worker-sweep-markdown.json remain available.

## Correctness and compatibility evidence

- Official corpus: 242 files, 193,704 lines, 8,831,216 bytes, 486 rewinds; zero divergences.
- Eight complete large fixtures: 8 files, 1,704,072 lines, 65,279,368 bytes, 7,624 rewinds; zero divergences.
- The diagnostic run compares every candidate query's pattern index, span and capture slots, then every LineOps/correction/rewind/warning against Oniguruma. No file sampling or skipped official inputs.
- The release differential separately checks memoized transitions against uncached Oniguruma. The steady-state allocation fixture remains at zero Rust allocations with and without memoization; it does not measure C allocations or whole-worker allocations.
- Monotonic locked baseline: scopes 125,877/0, symbols 688/0, reindent 30/5, zero skips. The five existing reindent failures remain open. The baseline file was not lowered or rewritten.
- 203 loaded syntaxes (the existing public Plain Text.tmLanguage now loads), 242 official test files. Eleven TextMate tests include XML/JSON semantics, repositories, dynamic end/while captures, include linking, scope expectations and cache invalidation. JSON, Diff and Log grammars are fetched from MIT-licensed VS Code 1.105.0 with its license at test time; the final local run fetched all three. Offline transport failures print a skip.
- Injections, capture retokenization and interpolated scope names return explicit diagnostics. The public CSS grammar's retokenized captures are not supported. Zip packages remain open.
- Vintage covers the requested command surface; its compatibility matrix and host integration contract are in ../vel-vintage/COVERAGE.md. Tests include 4,096 random command sequences, multiple cursors and Unicode graphemes.
- fmt, clippy --all-targets -D warnings, release tests and rustdoc -D warnings pass for vel-vintage, vel-syntax and vel-regex. There are no vel-scope source changes.
- 58 touched-crate release tests/doctests and 11 downstream vel-highlight tests/doctests pass. round9-checks.json records the checked source hashes, raw evidence hashes and verified public grammar paths.

## Reproduction and raw inputs

```sh
cargo fmt -p vel-vintage -p vel-syntax -p vel-regex --check
cargo clippy -p vel-vintage -p vel-syntax -p vel-regex --all-targets -- -D warnings
PROPTEST_CASES=4096 cargo test --release -p vel-vintage -p vel-syntax -p vel-regex
cargo test --release -p vel-highlight
cargo test --release -p vel-syntax --test tm_language public_mit -- --nocapture
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-vintage -p vel-syntax -p vel-regex --no-deps
cargo build --release -p vel-syntax --bin regex-bench --example highlight_throughput
# Set VEL_REGEX_FIXTURE_ROOT to a checkout containing all eight fixtures if needed.
target/release/regex-bench --diff-only --verify-matches --output crates/vel-regex/round9-differential.json
target/release/regex-bench --bench-only --output crates/vel-regex/round9-throughput.json
target/release/regex-bench --bench-only --quick --profile --output crates/vel-regex/round9-contexts-final.json
target/release/examples/highlight_throughput crates/vel-regex/round9-highlight.json
python3 crates/vel-regex/scripts/report-round9.py
```

The initial before rechecks use the original executable with --bench-only --language cpp/html. The initial after rechecks use the discarded span-publication executable with the same arguments. The source directories and fixture override are independent.
All raw samples live beside this report. The previous round's compile-latency, resident-memory and edit-latency measurements and older budget table remain in [PERF-round5.md](PERF-round5.md). Those measurements were not retaken and are not claimed as new results.

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| spikes/data/rust-10mb.rs | 10000000 | d5f3d0c59a12ba34446e540cfd7b5e1a3eab6c03176f3aff048281f12e031380 |
| spikes/data/javascript-10mb.js | 10000000 | 400c06f10bba70bb06b45041cf059fdf14c12b85c65f1471f4d7e46acbf8a79a |
| prototypes/sregex/data/python-5mb.py | 5036973 | afbb08566b90b6026457b81b082954064dfe8ab73ce6b28854aa02907b659721 |
| prototypes/sregex/data/cpp-5mb.cpp | 5117175 | c0eb8c7c021829f6aea6c7fcc9796fd9d79fa9783e4963eacabcbb08ba0f1251 |
| spikes/data/json-20mb.json | 20117640 | ddf09db9da7c68a1b8e072bafb8f74a2de77214e4353661ba81b80884bdfb623 |
| spikes/data/markdown-5mb.md | 5000000 | 62fde7c6ddbd39ebe98050bc446f3259566121e9e10fa68ebebbed0fb8058e6c |
| prototypes/sregex/data/yaml-5mb.yaml | 5005611 | da7202d0739a1dee46dbe7a57c04779ef5ed138655bbca876cb11979f1e7ed14 |
| prototypes/sregex/data/html-5mb.html | 5001969 | 75d5a85f7957eace008b6fdf28a2c820c161385d8d08073bd91aa12a1c2ec445 |
