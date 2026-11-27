# Round 13: HTML export, commands and remaining performance targets

Measured 2026-11-16, Darwin 25.1.0 arm64, rustc 1.99.1. Baseline is unmodified
`85e6cc14cb8d805ba0d592b004794281c833084d`. All work is uncommitted.

`vel-export::export_range(&Text, Range<usize>, &[StyledLine], &ColorScheme, &Options)`
and `copy_as_html(&Text, &Selection, ...)` return `Result<HtmlExport, Error>` with
standalone `html` and source-preserving `plain_text` flavours. `write_print_file`
writes/closes a persistent unique `.html` file and returns its path. The caller
opens the default browser and owns cleanup; lane A supplies clipboard/menu/browser
integration. The API validates ranges and highlight geometry, keeps Unicode, quotes
HTML/CSS, expands tabs by grapheme display columns, and supports colors, alpha,
font family/point size, decorations and optional original line numbers.
See [integration and limits](../vel-export/README.md). Golden tests and the benchmark
read the actual print file after its writer drops; paired exports require identical
HTML digests. The temporary file is not deleted before a browser can consume it.

The [coverage audit](../vel-commands/COVERAGE.md) contains **195 names: 61 implemented,
118 partially implemented, 13 missing and 3 out of scope**. It scans literal public
examples/changelog names plus the entire standard registry and spec additions.
Forwarding to a host alone counts as partial. Export cores are ready, while
`copy_as_html`/`print` command integration remains missing and is counted honestly.
A test checks table counts and registration claims. No Default package or editor
binary was inspected. All new adapters are in `vel-commands/src/text_ops.rs`;
`lib.rs` adds one module declaration and one registration call.

New buffer operations cover wrap/paragraph expansion, reverse/stable-unique/seeded
shuffle on lines and selected strings, selection sorting, snake/camel/Pascal/kebab/
UPPER_SNAKE conversion, mapped per-view marks and bookmarks, append, complement/
endpoint selections and ROT13. Whitespace trimming gains the Sublime-spelled alias.
Existing title/swap case, split-lines and chain receive edge/dispatch coverage.
Tests check Unicode, disjoint/reversed selections, empty/CR/LF/EOF cases, undo/redo,
seeded permutation properties, primary marks, edit mapping, per-view isolation,
rulers/tabs, invalid arguments, interception, macros and chain recursion. Exact
undocumented choices are `ST-BEHAVIOR: unverified` in
[BEHAVIOR.md](../vel-buffer/BEHAVIOR.md).

All performance comparisons alternate complete A/B and B/A pairs. No builds, tests
or other measurements from this lane run during a measured pair. Shared-machine
load is uncontrolled: C++ wall times varied greatly. No samples were removed.
95% percentile bootstrap intervals use 20,000 draws, seed 1313, and preserve whole
pairs/process clusters. These descriptive intervals do not model external load
correlation. Throughput medians use per-run decimal MB/s; paired speedups use
A-seconds/B-seconds. Loading/export intervals retain all 20 observations in each
of five process pairs as one cluster.

The C++ target is met by the **final median**, 2.558 MB/s,
with a wide interval; its lower CI remains below 2.5 MB/s. The initial full-language
sweep and two additional complete C++ sets are all retained. The final C++ row below
uses the last 10-pair set; the other rows use the original 10-pair sweep. Earlier
C++ samples did not consistently reach the absolute target; do not treat the last
median as a machine-independent guarantee.

| Worker language | A MB/s [95% CI] | B MB/s [95% CI] | Paired speedup [95% CI] |
| --- | ---: | ---: | ---: |
| cpp | 2.003 [1.489, 2.222] | 2.558 [1.787, 3.681] | 1.441 [1.121, 1.674] |
| rust | 2.578 [1.699, 2.822] | 2.580 [1.678, 3.190] | 1.057 [0.909, 1.246] |
| javascript | 2.845 [2.261, 3.074] | 2.963 [2.393, 3.125] | 1.034 [0.988, 1.204] |
| python | 7.443 [7.179, 7.541] | 7.487 [7.384, 7.548] | 1.021 [0.955, 1.074] |
| json | 19.139 [14.722, 20.995] | 19.564 [17.498, 21.485] | 1.046 [0.977, 1.469] |
| markdown | 6.429 [5.840, 6.606] | 6.494 [5.850, 6.560] | 0.997 [0.962, 1.097] |
| yaml | 13.763 [13.506, 14.204] | 13.858 [13.619, 14.012] | 0.998 [0.979, 1.016] |
| html | 22.583 [21.889, 23.392] | 22.334 [21.859, 22.722] | 0.977 [0.954, 0.995] |

Worker timing includes service creation, full background parsing, scope/style/run
construction, checkpoints and completion polling. It excludes fixture I/O, engine
construction, one full warmup and asynchronous teardown. Production caches remain
enabled. Inputs/binaries/source fingerprints and every sample are in
full sweep, first C++ follow-up
and final C++ follow-up.

The matcher now permits 1 MiB per-direction transition caches/NFA allowances,
retaining the existing **32 MiB shared resident-machine cap**. Larger combined rule
sets can use candidate machines instead of falling back to individual native
searches. This is a general budget change, with no language/grammar/fixture routing.
Source/HIR, native regex memory, parser/checkpoints and transient compilation remain
outside that machine cap. Canonical scope interning also avoids split/join allocation
on an exact interned-name hit.

Both loading binaries read identical 204-definition roots plus the same virtual
Plain Text fallback. Four bounded readers fetch source bytes concurrently; hashing,
overrides and errors retain sorted input order. Every invocation still reads and
hashes all content, including shadowed sources; no timestamp-only validation or
in-memory syntax-index shortcut is introduced. A new 64-resource test exercises
parallel override order and equal-length content changes on winners and shadowed
files; existing symlink/relocation/corruption/ordering tests still pass.

| Loading statistic | A ms [95% CI] | B ms [95% CI] |
| --- | ---: | ---: |
| Cold binary-cache miss | 1054.113 [1020.137, 1122.365] | 978.156 [926.301, 1002.986] |
| Warm median | 16.337 [13.276, 19.846] | 11.553 [11.361, 13.157] |
| Warm p95 | 29.679 [18.979, 91.454] | 16.610 [12.059, 17.963] |

Warm p95 meets the 20 ms target, including its reported upper CI. Cold here means
an absent binary cache, not an empty OS filesystem cache. Raw evidence:
loading pairs.

The export fixture is 400,000 bytes, 10,000 LF-terminated highlighted lines and
30,000 supplied spans (two resolved styles), with tabs, CJK, emoji, HTML punctuation,
three font decorations and line numbers. Parsing/highlight preparation is outside
timing. Both HTML and plain-text construction are timed. This is the stated fixture,
not a guarantee for arbitrary line lengths or style counts. A is the initial new
exporter; B reuses its display-column scratch allocation. There is no pre-round-13
export implementation in the baseline commit.

| HTML export statistic | A ms [95% CI] | B ms [95% CI] |
| --- | ---: | ---: |
| Median | 26.772 [26.705, 26.885] | 24.700 [24.640, 25.331] |
| p95 | 28.240 [27.330, 28.470] | 25.886 [25.640, 27.385] |

The tested export is within 50 ms. Raw evidence: export pairs.
The rejected line-copy validation experiment is preserved in
experimental pairs; that code is absent from B.

The real vel-buffer perf worker runs over the unchanged 100 MB fixture. Baseline
buffer/text sources were archived from the baseline commit into a separate build;
Cargo's compiler-artifact messages select the exact `vel-buffer` bench executable.
Source loading/setup/assertions are outside its original timers. There are 10 pairs
per case, plus 40 additional pairs for find, 50 MB undo and 100k-caret typing after
noisy initial measurements. The combined table includes **all** samples from both
passes. For the requested paired comparison, the largest median paired slowdown is
**4.20%** (`find_10000`), below 5%. The interval for that microbenchmark is broad;
this is a point-estimate gate, not a statistical upper-bound guarantee. The unpaired
find median is +7.32%; paired and marginal medians differ under this load, and that
uncertainty is retained explicitly instead of claiming an unconditional no-regression
result. Undo metadata remains identical.

| Buffer scenario | Pairs | Baseline [95% CI] | Candidate [95% CI] | Paired latency change [95% CI] |
| --- | ---: | ---: | ---: | ---: |
| type_10000 | 10 | 0.961375 [0.917042, 0.998271] | 0.917167 [0.896854, 1.007749] | -0.54% [-9.19%, +5.91%] |
| type_100000 | 50 | 9.480854 [9.339625, 9.670146] | 9.479062 [9.364479, 9.604916] | -0.25% [-1.63%, +1.05%] |
| find_10000 | 50 | 0.029895 [0.028166, 0.033791] | 0.032083 [0.030000, 0.035208] | +4.20% [-19.40%, +16.08%] |
| undo_10000 | 10 | 1.118729 [1.105667, 1.123166] | 1.116479 [1.105333, 1.133312] | +0.19% [-1.27%, +1.77%] |
| delete_50mb | 10 | 0.694792 [0.690708, 0.706875] | 0.694583 [0.677375, 0.705791] | -0.60% [-2.98%, +2.79%] |
| undo_50mb | 50 | 0.001562 [0.001458, 0.001834] | 0.001583 [0.001417, 0.001770] | +1.21% [-12.50%, +13.55%] |
| map_100000 | 10 | 3.910395 [3.860958, 3.986542] | 3.909812 [3.867666, 3.962500] | +0.27% [-3.01%, +2.37%] |
| undo_metadata | 10 | 48.048900 [48.048900, 48.048900] | 48.048900 [48.048900, 48.048900] | +0.00% [+0.00%, +0.00%] |

The `undo_metadata` row uses bytes/caret, not milliseconds. Positive changes are
slowdowns; intervals invert the paired speedup distribution. Raw evidence:
first pass,
follow-up,
combined statistics.

Validation passed: fmt, workspace/all-target clippy with `-D warnings`, syntax/regex
all-feature clippy, `cargo test --workspace --release`, rustdoc with `-D warnings`,
and 1,024-case buffer model/operation tests. The first complete workspace attempt
hit the existing TLS test's non-atomic readiness-file race; the targeted retry and
second complete workspace run passed. No files in vel-packages were changed.
Check records retain that failed attempt as well as successes.

Native verified differential: **242 official files / 193,704 lines / 8,831,216 bytes /
486 rewinds, zero divergences**; **8 large fixtures / 1,704,072 lines / 65,279,368 bytes /
7,624 rewinds, zero divergences**. The release oracle enables `verify-prefilters`
and `--verify-matches` to compare rejection decisions and winner spans/capture slots,
as well as operations, corrections, rewinds and warnings. Workspace tests also
exercise memoized transitions. Raw differential.

Reproduction (prepare baseline binaries from 85e6cc1 before changing sources):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
cargo build --release -p vel-syntax --example worker_probe --example roots_probe
cargo build --release -p vel-export --example export_bench
cargo bench -p vel-buffer --bench perf --no-run --message-format=json
python3 crates/vel-regex/scripts/paired-round13.py worker \
  --before /saved/worker_probe --after target/release/examples/worker_probe \
  --baseline-commit 85e6cc14cb8d805ba0d592b004794281c833084d \
  --data /path/to/velocity-with-fixtures --pairs 10 --languages cpp \
  --output /tmp/cpp.json
# Use kind loading with roots_probe, export with export_bench, or buffer with
# the Cargo-reported perf executables and --data /path/to/text-100mb.txt.
# Buffer --cases selects follow-up scenarios without discarding the first pass.
cargo build --release -p vel-syntax --bin regex-bench --features vel-regex/verify-prefilters
VEL_REGEX_FIXTURE_ROOT=/path/to/velocity-with-fixtures \
  target/release/regex-bench --diff-only --verify-matches --output /tmp/differential.json
```
