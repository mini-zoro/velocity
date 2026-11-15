#!/usr/bin/env python3
"""Render the round-10 report from retained, paired measurement evidence."""
import json
from pathlib import Path

CRATE = Path(__file__).resolve().parents[1]


def estimate(value, unit=""):
    lo, hi = value["ci95"]
    return f'{value["median"]:.3f}{unit} [{lo:.3f}, {hi:.3f}]'


def main():
    paired = json.loads((CRATE / "round10-paired.json").read_text())
    loading = json.loads((CRATE / "round10-loading-paired.json").read_text())
    profile = json.loads((CRATE / "round10-profile-final.json").read_text())
    diff = json.loads((CRATE / "round10-differential.json").read_text())
    rows = []
    for row in paired["timings"]:
        rows.append(f'| {row["language"]} | {estimate(row["before_mb_per_second"])} | {estimate(row["after_mb_per_second"])} | {estimate(row["paired_speedup"], "×")} |')
    open_targets = [r["language"] for r in paired["timings"] if r["language"] in ("rust", "javascript", "cpp") and r["after_mb_per_second"]["median"] < 2.5]
    phases = []
    for row in profile:
        p = row["profile"]
        regex_seconds = sum(c["nanos"][0] for c in p["regex_contexts"].values()) / 1e9
        phases.append(f'| {row["language"]} | {p["input_seconds"]:.3f} | {p["parse_seconds"]:.3f} ({regex_seconds:.3f} regex) | {p["render_seconds"]:.3f} | {p["checkpoint_seconds"]:.3f} | {p["rust_allocations"]:,} / {p["rust_allocated_bytes"] / 1e6:.1f} MB |')
    totals = {key: {field: sum(r[field] for r in diff[key]) for field in ("bytes", "lines", "rewinds", "divergences")} for key in ("official", "large_diff")}
    assert len(diff["official"]) == 242 and len(diff["large_diff"]) == 8
    assert all(t["divergences"] == 0 for t in totals.values())
    warm = loading["warm_ms_including_content_hashes"]
    cold = loading["cold_build_ms"]
    warm_samples = sorted(value for pair in loading["samples"] for value in pair["after"]["warm_ms_including_content_hashes"])
    warm_p95 = warm_samples[(95 * len(warm_samples) + 99) // 100 - 1]
    text = f'''# Lane B, round 10: syntax roots and paired worker throughput

Measured November 15, 2026, Darwin 25.1.0 arm64, rustc 1.99.1 (58cf8f9c3).
Baseline: unmodified main `fcab5c3` (includes the `aad07c6` symlink fix).
No commits are made by this lane. Historical round-9 evidence is in [PERF-round9.md](PERF-round9.md).

## Production worker: alternating full-file A/B

Every row has {paired['pairs']} pairs, alternating A/B then B/A. Separate persistent
executables load the same public corpus and fixture, each warming one complete file
before sampling. Exactly one measured worker runs at a time; no builds, tests or
other measurements from this lane run concurrently with the final pairs. External
activity on the shared machine is uncontrolled. No final samples are removed.

The timer includes worker creation, parsing, regex searches, branch corrections,
scope/style/run generation, dense checkpoints and completion polling at 1 ms.
It excludes fixture I/O, Text/engine construction, syntax loading, warmup and
teardown. A 200 ms pause after dropping each detached service is outside timing.
Default production caches and the 32 MiB shared DFA pool remain enabled.

Values are median decimal MB/s with 95% percentile bootstrap intervals in brackets.
The speedup is the median of the paired A-seconds/B-seconds ratios. Bootstrap
resampling preserves whole pairs: 20,000 draws, fixed seed 1031. These descriptive
intervals do not model correlated external machine load. Binary/source/input
digests, run order and every raw sample are in `round10-paired.json`.

| Language | Main MB/s [95% CI] | Candidate MB/s [95% CI] | Paired speedup [95% CI] |
| --- | ---: | ---: | ---: |
{chr(10).join(rows)}

The ≥2.5 MB/s median target remains open for: **{', '.join(open_targets) or 'none'}**.
The confidence intervals above are reported separately from that median threshold.

## Multi-root loading

`SyntaxSet::load_cached_roots(&[&Path], &[(&str, &str)], cache_dir)` selects whole
resources by package-relative path before linking. Later roots win, User loads
last within each root, and virtual definitions override roots. YAML and both
TextMate forms participate. `load_cached` is a thin wrapper. Content hashes include
ordered boundaries, paths and every input, including shadowed sources and extras.
Cache hits retain lazy contexts, checksum validation and atomic rebuilds. Stable
virtual paths permit relocation; current physical-path lookup aliases are rebuilt
per call and excluded from serialization. File/directory symlinks load; dangling
links are ignored. Explicit missing roots/loops/errors are reported.

`MetadataSet::load_roots` and `load_directories` use the same file replacement
semantics; removed keys from a shadowed file cannot leak through. Distinct files
still merge settings by specificity and order. Metadata has no persistent cache.

The exact editor replacement is in [vel-syntax/README.md](../vel-syntax/README.md#multiple-roots-and-editor-migration-round-10).
This lane leaves `crates/velocity` untouched. The injected original YAML Plain Text
fallback produces an index of 204 resources: 203 public definitions plus one extra.

The loading comparison uses five alternating complete process pairs over identical
204-definition inputs. Main reads a prepared file-symlink tree; the candidate reads
the public tree plus a virtual fallback. Staging itself is excluded. Each process
times one cold binary-cache miss, then 20 content-hashed cache hits. The warm
estimate resamples the five per-process medians, preserving the process as a unit.

| Loading (ms) | Main [95% CI] | Candidate [95% CI] |
| --- | ---: | ---: |
| Cold build | {estimate(cold['before'])} | {estimate(cold['after'])} |
| Warm content hashes + lazy index | {estimate(warm['before'])} | {estimate(warm['after'])} |

The median is within the 20 ms budget; the pooled candidate P95 is {warm_p95:.3f} ms,
with {sum(t > 20 for t in warm_samples)} of {len(warm_samples)} calls above 20 ms
(maximum {max(warm_samples):.3f} ms). The tail therefore remains above the budget.
Cold-build median time is {(cold['after']['median'] / cold['before']['median'] - 1) * 100:+.1f}%
relative to main; the intervals overlap.

Raw loading samples are in `round10-loading-paired.json`; the first contended
diagnostic remains in `../vel-syntax/tests/round10-roots-first.json`.

## General hot-path changes and profiles

Literal filters now consider required concat members, suffixes and positive
forward assertions through conservative regular supersets. They never infer a
required literal from one optional/alternative branch or a lookbehind, and case
folding/unsupported dialect constructs retain native matching. Successful matches
and all captures still come from the existing ordered matcher. No syntax names or
fixture text enter routing decisions.

Rendered shapes share spans and compact scope runs using the full input stack
(including saved clears), exact operations and byte length. Hashed token colors
bypass sharing; palette/metadata changes clear it. All UTF-8 boundaries still
validate. The cache caps 8,192 entries and 4 MiB of retained operation/span/run
arrays; stack/entry metadata is bounded separately by entry count. Same-byte
push/pop/clear groups reuse IDs without interning invisible intermediate stacks,
up to 65,536 transitions. Groups containing Restore use the actual saved history.

The opt-in `profile` build measures input, parser (regex plus context transitions
and memoization), scope/render work, checkpoints and Rust allocations. Its timers
and allocation atomics are absent from throughput builds. Regex per-query timers
and allocation instrumentation add overhead, so the following seconds are
diagnostics rather than throughput comparisons; C allocations are not counted.
The initial profile (`round10-profile-before.json`) identified Rust's impl-for
lookahead, JavaScript variable/expression contexts and C++ block context as the
largest regex costs. Final per-context detail is retained in `round10-profile-final.json`.

| Language | Input s | Parser s (regex subset) | Render s | Checkpoint s | Rust allocations / cumulative bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
{chr(10).join(phases)}

An anchored-native-search experiment was removed: paired JavaScript regressed
because point-only negative results could not reuse a suffix-wide absence result.
A separate search-range shortcut also failed an existing capture/span test and
was discarded. Neither is present in the final source. Intermediate `round10-*`
files retain experiments; `round10-paired.json` is the final production evidence.
The before-anchored experiment later overlapped verification and is diagnostic only.

## Correctness and checks

- Official differential: {len(diff['official'])} files, {totals['official']['lines']:,} lines,
  {totals['official']['bytes']:,} bytes, {totals['official']['rewinds']:,} rewinds, zero divergences.
- Complete large fixtures: {len(diff['large_diff'])} files, {totals['large_diff']['lines']:,} lines,
  {totals['large_diff']['bytes']:,} bytes, {totals['large_diff']['rewinds']:,} rewinds, zero divergences.
- The release differential enables `vel-regex/verify-prefilters`, checking every
  literal rejection against native matching, and `--verify-matches`, checking
  winning pattern/span/capture slots. LineOps, corrections, rewinds and warnings
  compare against the reference backend. The workspace differential also checks
  memoized transitions. Resource-limit failures remain recoverable no-matches.
- The locked corpus baseline is unchanged: scopes 125,877/0, symbols 688/0,
  reindent 30/5, zero skips. Existing reindent compatibility failures remain open.
- Root tests cover overrides before linking, renamed inherited parents and file
  includes, duplicate-name ordering, virtual-only loading, extras/order/root edits,
  addition/removal, corruption, relocation, physical aliases and linked packages.
  Rendering tests cover UTF-8, clear/restore history and token-hashed colors;
  downstream tests cover rewinds, edits, palette changes and editor integration.
- Gate results and evidence digests are in `round10-checks.json`.

## Reproduction

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p vel-syntax -p vel-regex -p vel-highlight --all-targets --all-features -- -D warnings
cargo test --workspace --release
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
cargo build --release -p vel-syntax --example worker_probe --example roots_probe
# Build worker_probe against main in a separate checkout with the same example.
python3 crates/vel-regex/scripts/paired-round10.py \\
  --before /path/to/main/target/release/examples/worker_probe \\
  --data /path/to/all/eight/fixtures --pairs 10 \\
  --languages cpp rust javascript python json markdown yaml html \\
  --output crates/vel-regex/round10-paired.json
# roots_probe on main uses load_cached(&packages, &cache) at both load sites.
python3 crates/vel-regex/scripts/paired-loading-round10.py \\
  --before /path/to/main/target/release/examples/roots_probe \\
  --output crates/vel-regex/round10-loading-paired.json
cargo build --release -p vel-syntax --bin regex-bench \\
  --features vel-regex/verify-prefilters --target-dir /tmp/velocity-oracle
VEL_REGEX_FIXTURE_ROOT=/path/to/all/eight/fixtures \\
  /tmp/velocity-oracle/release/regex-bench --diff-only --verify-matches \\
  --output crates/vel-regex/round10-differential.json
cargo build --release -p vel-syntax --example worker_probe --features profile \\
  --target-dir /tmp/velocity-profile
# Pass corpus root, fixture path, syntax display name; send run then quit on stdin.
```
'''
    output = CRATE / "PERF.md"
    temporary = output.with_suffix(".md.tmp")
    temporary.write_text(text)
    temporary.replace(output)


if __name__ == "__main__":
    main()
