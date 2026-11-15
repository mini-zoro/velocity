#!/usr/bin/env python3
"""Render round 9 evidence without dropping any of the five timing samples."""
import hashlib
import json
import os
import statistics
from pathlib import Path

CRATE = Path(__file__).resolve().parents[1]
ROOT = CRATE.parents[1]
DATA = Path(os.environ.get("VEL_REGEX_FIXTURE_ROOT", ROOT))


def read(name):
    return json.loads((CRATE / name).read_text())


def median(samples):
    assert len(samples) >= 5
    return statistics.median(samples)


def p95(samples):
    assert len(samples) == 5
    return max(samples)


def mbps(row, key):
    return row["bytes"] / 1e6 / median(row[key])


def main():
    before = read("round9-before.json")
    parser = read("round9-throughput.json")
    worker = read("round9-highlight.json")
    profile = read("round9-contexts-final.json")
    diff = read("round9-differential.json")
    baseline = json.loads((ROOT / "crates/vel-syntax/tests/baseline.json").read_text())
    assert not parser["quick"] and not parser["instrumented"]
    assert profile["quick"] and profile["instrumented"]
    assert len(parser["timings"]) == len(worker["timings"]) == 8
    assert len(diff["official"]) == 242 and len(diff["large_diff"]) == 8
    assert all(r["divergences"] == 0 for r in diff["official"] + diff["large_diff"])
    by_language = {r["language"]: r for r in parser["timings"]}
    old = {r["language"]: r for r in before["timings"]}
    misses = [r["language"] for r in worker["timings"] if mbps(r, "seconds") < 2.5]
    rows = [
        "# Lane B, round 9: Vintage, TextMate and highlighting throughput",
        "",
        "Measured November 15, 2026; Darwin 25.1.0 arm64, rustc 1.99.1 (58cf8f9c3).",
        "Release builds, one timed task at a time. Other activity on the shared machine was not controlled.",
        "All reported medians use five samples; P95 is nearest rank (the maximum of five).",
        "No sample was removed. MB/s uses decimal megabytes. These measurements supersede the round 5 timing table; historical raw files remain available.",
        "",
        (f"The 2.5 MB/s background target remains open for **{', '.join(misses)}**." if misses else "All eight measured background workers meet the 2.5 MB/s target."),
        "Correctness checks pass; performance closure is separate. The vi and TextMate compatibility limits are in their crate documentation.",
        "",
        "## Production background parse + style + checkpoints",
        "",
        "The example invokes the existing vel-highlight DocumentHighlighter and HighlightService. Each language receives one full-file warmup, then five fresh workers.",
        "The timer includes worker creation, parsing, styling, checkpoints and completion polling (1 ms). It excludes fixture I/O, syntax loading, warmup, Text construction and worker teardown.",
        "Stable transition memoization is enabled: 16,384 entries, 4 MiB source-text cap and clock replacement. Snapshot and operation metadata are bounded by entry count; the source-text cap is not a total-memory cap.",
        "",
        "| Language | MB | Worker MB/s | Target ≥2.5 | Median / P95 seconds | Uncached parser MB/s |",
        "| --- | ---: | ---: | --- | ---: | ---: |",
    ]
    for r in worker["timings"]:
        n = r["language"]
        speed = mbps(r, "seconds")
        rows.append(f"| {n} | {r['bytes']/1e6:.3f} | {speed:.3f} | {'pass' if speed >= 2.5 else 'open'} | {median(r['seconds']):.6f} / {p95(r['seconds']):.6f} | {mbps(by_language[n], 'multi_seconds'):.3f} |")
    rows += [
        "",
        "## Ordered matcher, identical uncached parser",
        "",
        "Both backends disable stable-line memoization and reuse LineOps storage. A complete warmup precedes five alternating Oniguruma/candidate runs.",
        "This measures real context transitions, regex searches, captures, branch corrections and scope operations. It is distinct from the background worker above.",
        "The before executable was built from the original round 9 checkout (cfd76b3, before these edits) in an isolated /tmp workspace. Both versions read the same fixtures and pinned package corpus.",
        "",
        "| Language | Before MB/s | Final MB/s | Oniguruma MB/s | Final speedup | Final median / P95 seconds | Before→final median time | Oniguruma median time change |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for r in parser["timings"]:
        n = r["language"]
        change = (median(r["multi_seconds"]) / median(old[n]["multi_seconds"]) - 1) * 100
        control = (median(r["onig_seconds"]) / median(old[n]["onig_seconds"]) - 1) * 100
        rows.append(f"| {n} | {mbps(old[n], 'multi_seconds'):.3f} | {mbps(r, 'multi_seconds'):.3f} | {mbps(r, 'onig_seconds'):.3f} | {median(r['onig_seconds'])/median(r['multi_seconds']):.3f}× | {median(r['multi_seconds']):.6f} / {p95(r['multi_seconds']):.6f} | {change:+.1f}% | {control:+.1f}% |")
    rows += [
        "",
        "The initial full run showed >5% wall-time regressions for C++ and HTML during large timing swings. That run is retained in round9-throughput-first.json.",
        "The initial sequential rechecks below preceded the final hot-path cleanup. They retain five samples per executable and supplement the final tables and interleaved diagnostic below.",
        "",
        "| Recheck | Before MB/s | After MB/s | Median time change | Oniguruma median time change |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for n in ("cpp", "html"):
        a = read(f"round9-pre-cold-recheck-before-{n}.json")["timings"][0]
        b = read(f"round9-pre-cold-recheck-after-{n}.json")["timings"][0]
        change = (median(b["multi_seconds"]) / median(a["multi_seconds"]) - 1) * 100
        control = (median(b["onig_seconds"]) / median(a["onig_seconds"]) - 1) * 100
        rows.append(f"| {n} | {mbps(a, 'multi_seconds'):.3f} | {mbps(b, 'multi_seconds'):.3f} | {change:+.1f}% | {control:+.1f}% |")
    rows += [
        "",
        "The final regression diagnostic links the original and current parsers into one process, warming each over a complete file and alternating 1,024-line chunks in both orders. Each state persists across chunks, with memoization disabled. Timers sum active chunks only; the five per-pass pairs are retained in round9-paired.json. This diagnostic does not replace contiguous wall throughput above.",
        "",
        "| Interleaved final comparison | Before median s | After median s | Median paired time change |",
        "| --- | ---: | ---: | ---: |",
    ]
    paired_rows = read("round9-paired.json")["timings"]
    if (CRATE / "round9-paired-javascript.json").exists():
        paired_rows += read("round9-paired-javascript.json")["timings"]
    for r in paired_rows:
        ratios = [b/a for a, b in zip(r["before_seconds"], r["after_seconds"], strict=True)]
        rows.append(f"| {r['language']} | {median(r['before_seconds']):.6f} | {median(r['after_seconds']):.6f} | {(median(ratios)-1)*100:+.1f}% |")
    rows += [
        "",
        "The diagnostic copies the original crates to /tmp and changes only package names to allow both versions to link. Source digests are recorded in its JSON. Run scripts/paired-round9.py --before /path/to/original/checkout to reproduce it. round9-paired-cpp-html.json retains the earlier two-language comparison. The final diagnostic also includes Python: its preliminary wall-time median was 6.5% slower, and the paired check confirmed the routing regression described below. round9-paired-javascript.json adds five JavaScript pairs after its final contiguous wall-time samples also moved more than 5%.",
        "",
        "## Context profiles and general changes",
        "",
        "Context profiling is a separate instrumented run over the first approximately 200 KB per language, five complete parser passes. Per-query Instant overhead is included here, not in the full-file tables.",
        "",
        "| Context | Queries per pass | Candidate median / P95 ms | Oniguruma median ms | Speedup |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    contexts = profile["contexts"]
    comparable = []
    for name, a in contexts["onig"].items():
        b = contexts["multi"].get(name)
        if b and a["queries"] == b["queries"] and min(a["queries"]) and min(a["nanos"]) and min(b["nanos"]):
            comparable.append((median(a["nanos"]) / median(b["nanos"]), name, a, b))
    comparable.sort()
    chosen = ["Markdown::__main", "Rust::type-path", "Rust::declaration-identifier", "Rust::statements"]
    chosen += [r[1] for r in comparable[:5] if r[1] not in chosen]
    for name in chosen:
        a, b = contexts["onig"][name], contexts["multi"][name]
        rows.append(f"| {name} | {a['queries'][0]} | {median(b['nanos'])/1e6:.6f} / {p95(b['nanos'])/1e6:.6f} | {median(a['nanos'])/1e6:.6f} | {median(a['nanos'])/median(b['nanos']):.3f}× |")
    rows += [
        "",
        "General rules: classify ASCII once per input line, avoid a duplicate native span search before extracting captures for exact candidates, and retain direct Oniguruma for sets below the existing four-pattern threshold. No syntax names enter these choices.",
        "Native source compilation, assertion guards, empty-match policy and LF edge cases still gate the exact path. Captures are materialized only for the final winner.",
        "A proposed direct-native route for early empty exits and bounded exact sets caused a 10.6% paired Python regression. Restoring the old capture path did not fix it; removing those routing heuristics did. The failed all-language run is retained as round9-before-routing-*, with the two five-pair isolation runs in round9-python-capture-path-experiment.json and round9-python-dfa-routing.json. The final tables use the corrected general routing.",
        "The TextMate while continuation handler is outlined as cold code; ordinary YAML lines never scan the stack for it. Cache-disabled parsing avoids unused line hashing and locking. Production caching uses clock replacement and a larger bounded window. A two-hit admission experiment was removed after five Rust samples regressed; the raw experiment remains in round9-worker-admission-rust.json.",
        "round9-worker-before-clock.json, round9-worker-clock-only.json and round9-worker-window-{rust,js}.json preserve the cache experiments. Shared-machine variation prevents treating these sequential observations as a precise isolated speedup estimate.",
        "round9-pre-cold-* retains the discarded revision that published every exact DFA span to the native cache. Those extra writes were removed. Low-frequency context measurements (sometimes one query per pass) are especially sensitive to timer overhead. Rust helper-context ratios can remain below 1 even when the whole parser is faster: all-native parent contexts prepopulate more native search results. The profile reports that cost instead of hiding it behind whole-language averages.",
        "",
    ]
    regression = next(r for r in read("round9-before-sweep-highlight.json")["timings"] if r["language"] == "markdown")
    fixed = read("round9-worker-sweep-markdown.json")["timings"][0]
    rows += [
        "The larger checkpoint window exposed a general weak-stack interner bug: once more than 8,192 stack buckets stayed live, every subsequent intern scanned the entire pool. Cleanup now waits for another 8,192 buckets after each sweep. Weak references still do not retain stack data, and a regression test checks both live-stack scan frequency and eventual dead-metadata cleanup.",
        f"The retained before-sweep full worker run measured Markdown at {mbps(regression, 'seconds'):.3f} MB/s; the isolated five-sample check after the fix measured {mbps(fixed, 'seconds'):.3f} MB/s. These sequential observations are diagnostic evidence; the fresh all-language table above is the final result. All round9-before-sweep-* inputs and round9-worker-sweep-markdown.json remain available.",
        "",
        "## Correctness and compatibility evidence",
        "",
    ]
    for label, key in (("Official corpus", "official"), ("Eight complete large fixtures", "large_diff")):
        rs = diff[key]
        rows.append(f"- {label}: {len(rs)} files, {sum(r['lines'] for r in rs):,} lines, {sum(r['bytes'] for r in rs):,} bytes, {sum(r['rewinds'] for r in rs):,} rewinds; zero divergences.")
    rows += [
        "- The diagnostic run compares every candidate query's pattern index, span and capture slots, then every LineOps/correction/rewind/warning against Oniguruma. No file sampling or skipped official inputs.",
        "- The release differential separately checks memoized transitions against uncached Oniguruma. The steady-state allocation fixture remains at zero Rust allocations with and without memoization; it does not measure C allocations or whole-worker allocations.",
        f"- Monotonic locked baseline: scopes {baseline['scope_counts']['passed']:,}/0, symbols {baseline['symbol_counts']['passed']}/0, reindent {baseline['reindent_counts']['passed']}/{baseline['reindent_counts']['failed']}, zero skips. The five existing reindent failures remain open. The baseline file was not lowered or rewritten.",
        "- 203 loaded syntaxes (the existing public Plain Text.tmLanguage now loads), 242 official test files. Eleven TextMate tests include XML/JSON semantics, repositories, dynamic end/while captures, include linking, scope expectations and cache invalidation. JSON, Diff and Log grammars are fetched from MIT-licensed VS Code 1.105.0 with its license at test time; the final local run fetched all three. Offline transport failures print a skip.",
        "- Injections, capture retokenization and interpolated scope names return explicit diagnostics. The public CSS grammar's retokenized captures are not supported. Zip packages remain open.",
        "- Vintage covers the requested command surface; its compatibility matrix and host integration contract are in ../vel-vintage/COVERAGE.md. Tests include 4,096 random command sequences, multiple cursors and Unicode graphemes.",
        "- fmt, clippy --all-targets -D warnings, release tests and rustdoc -D warnings pass for vel-vintage, vel-syntax and vel-regex. There are no vel-scope source changes.",
        "- 58 touched-crate release tests/doctests and 11 downstream vel-highlight tests/doctests pass. round9-checks.json records the checked source hashes, raw evidence hashes and verified public grammar paths.",
        "",
        "## Reproduction and raw inputs",
        "",
        "```sh",
        "cargo fmt -p vel-vintage -p vel-syntax -p vel-regex --check",
        "cargo clippy -p vel-vintage -p vel-syntax -p vel-regex --all-targets -- -D warnings",
        "PROPTEST_CASES=4096 cargo test --release -p vel-vintage -p vel-syntax -p vel-regex",
        "cargo test --release -p vel-highlight",
        "cargo test --release -p vel-syntax --test tm_language public_mit -- --nocapture",
        "RUSTDOCFLAGS='-D warnings' cargo doc -p vel-vintage -p vel-syntax -p vel-regex --no-deps",
        "cargo build --release -p vel-syntax --bin regex-bench --example highlight_throughput",
        "# Set VEL_REGEX_FIXTURE_ROOT to a checkout containing all eight fixtures if needed.",
        "target/release/regex-bench --diff-only --verify-matches --output crates/vel-regex/round9-differential.json",
        "target/release/regex-bench --bench-only --output crates/vel-regex/round9-throughput.json",
        "target/release/regex-bench --bench-only --quick --profile --output crates/vel-regex/round9-contexts-final.json",
        "target/release/examples/highlight_throughput crates/vel-regex/round9-highlight.json",
        "python3 crates/vel-regex/scripts/report-round9.py",
        "```",
        "",
        "The initial before rechecks use the original executable with --bench-only --language cpp/html. The initial after rechecks use the discarded span-publication executable with the same arguments. The source directories and fixture override are independent.",
        "All raw samples live beside this report. The previous round's compile-latency, resident-memory and edit-latency measurements and older budget table remain in [PERF-round5.md](PERF-round5.md). Those measurements were not retaken and are not claimed as new results.",
        "",
        "| Fixture | Bytes | SHA-256 |",
        "| --- | ---: | --- |",
    ]
    for r in diff["large_diff"]:
        data = (DATA / r["file"]).read_bytes()
        assert len(data) == r["bytes"]
        rows.append(f"| {r['file']} | {len(data)} | {hashlib.sha256(data).hexdigest()} |")
    tmp = CRATE / "PERF.md.tmp"
    tmp.write_text("\n".join(rows) + "\n")
    tmp.replace(CRATE / "PERF.md")


if __name__ == "__main__":
    main()
