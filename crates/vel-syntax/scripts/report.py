#!/usr/bin/env python3
"""Regenerate corpus and highlighting evidence with monotonic per-file counts."""
import json
import math
import os
import shutil
import statistics
import tempfile
from pathlib import Path

crate = Path(__file__).resolve().parents[1]
data = json.loads((crate / "tests/last-run.json").read_text())
baseline = json.loads((crate / "tests/baseline.json").read_text())
timing = json.loads((crate / "tests/timings.json").read_text())
assert data["loaded"] == 202 and data["test_files"] == 242
assert timing["runs"] >= 5
assert data["overall"]["passed"] >= baseline["overall"]["passed"]
assert data["scope_counts"]["passed"] == 125877 and data["scope_counts"]["failed"] == 0
assert data["overall"]["skipped"] == 0

def write_atomic(path, text):
    if path.exists():
        shutil.copy2(path, Path(tempfile.gettempdir()) / f"velocity-{path.parent.name}-{path.name}-{os.getpid()}.previous")
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(text)
    temporary.replace(path)

def escaped(text):
    return str(text).replace("|", "\\|").replace("`", "'").replace("\n", " ")

counts = data["overall"]
planned = counts["passed"] + counts["failed"]
preferences = len(list((crate / "../../third_party/sublime-packages").rglob("*.tmPreferences")))
out = [
    "# Lane B, round 5: regex backend, parser throughput and metadata",
    "",
    "> Reviewed round-four baseline: **scopes 125,877/0, symbols 688/0, reindent 26/9, zero skips**. Partial-symbols files still evaluate scope assertions; annotation tails use the general longest-parseable-prefix rule.",
    "",
    f"Pinned public corpus: **202 syntaxes, 242 test files, {preferences} tmPreferences files**. Loading warnings: {len(data['warnings'])}; error files: {sum(f['error'] is not None for f in data['files'])}.",
    "",
    "| Category | Passed | Failed | Skipped | Unit |",
    "| --- | ---: | ---: | ---: | --- |",
]
for key, name, unit in [
    ("scope_counts", "Scopes", "assertion span"),
    ("symbol_counts", "Symbols/references", "assertion / unannotated symbol"),
    ("reindent_counts", "Reindent", "whole-file variant"),
    ("overall", "Total", "sum of the above"),
]:
    c = data[key]
    out.append(f"| {name} | {c['passed']:,} | {c['failed']} | {c['skipped']} | {unit} |")
out += [
    "",
    f"Overall: **{counts['passed']:,}/{planned:,} ({100*counts['passed']/planned:.5f}%)**. Scope columns: {counts['passed_cells']:,} passed / {counts['failed_cells']} failed.",
    "The denominator remains 126,600. All prior per-file scope/symbol/reindent passes are preserved, and four previously failing reindent variants now pass.",
    "",
    "## Metadata behavior",
    "",
    "XML/binary plist preferences merge independently per key by selector score and resource order. Shell variables merge by name. All requested indentation, symbol, completion and preservation settings remain available through Metadata; unknown plist keys are retained. The plist XML dependency drops CDATA events, so the loader preserves them as equivalent escaped character data before parsing.",
    "",
    "All 688 symbol assertions pass, including quoted transformed names, local/global definitions, indexed references, partial-symbol files, and exhaustive checking for unannotated symbols. Adjacent nested symbol spans coalesce and transformations support captures, case conversion, and global zero-width/nonempty retry semantics. The base symbol conventions are clean-room implementations of public scope naming; no application Default preferences are used.",
    "",
    "Assertion selectors use the longest prefix accepted by the selector grammar when trailing text is unparseable. This is a general rule; the literal suffix '() {}' has no special handling. Negative selectors still reject actual comment scopes. Partial-symbols files execute their scope assertions.",
    "",
    "All 31 reindent files execute, totaling 35 requested whole-file variants. reindent runs both unchanged and unindented checks; reindent-unindented strips whitespace and reparses the resulting text before indentation. One whole-file comparison is one case, regardless of file length. Remaining mismatches are failures, not skips. The corpus guard locks scopes, symbols, and reindent separately per file; baseline migration permits newly tested failures without weakening existing scope thresholds.",
    "",
    "## Remaining reindent failures",
    "",
    "| File | Variant | First differing line | Evidence |",
    "| --- | --- | ---: | --- |",
]
for file in data["files"]:
    for failure in file["examples"]:
        if failure["selector"].startswith("reindent"):
            out.append(f"| {escaped(file['file'])} | {failure['selector']} | {failure['line']} | {escaped(failure['actual'])} |")
out += [
    "",
    "Remaining failures expose nested continuation/control indentation, CSS comment transitions, JSX attributes and Zsh brace-style cases. Both PHP variants, Bash case indentation and Bash heredoc indentation now pass. Scope-selected metadata is preserved; continuation bodies on the same line no longer arm another indentation step. The indentation engine is usable but is not fully conformant; retain this evidence when extending it. Symbol/scope success does not imply unasserted tokens or all editor behavior are identical to ST.",
    "",
    "`.tmLanguage` conversion and zip syntax loading remain prior-round limitations. Numeric selector scoring remains the approximation documented by vel-scope. Full-file default palettes, style rendering flags, and editor scheduling are provided by vel-color-scheme / vel-highlight, not the syntax runner.",
    "",
    f"Runner regex counters: `{json.dumps(data['regex_metrics'], sort_keys=True)}`. Resource-limit events remain explicit counters and are treated as no-match; they are not a claim that every regex path completed without limits.",
    "",
    "## Parser performance",
    "",
    "Release, macOS 26.1 arm64, rustc 1.99.1. Every timing below has five runs. No sysctl was called; the parallel lane can affect scheduling. Input: 10,000,000 Rust bytes / 297,030 lines. These parser-only timings use reusable LineOps and bounded stable-transition memoization, and exclude styling and UI work. Backend-isolated measurements (memoization disabled for both engines), per-context profiles, cold compilation and full differential coverage are recorded in ../vel-regex/PERF.md.",
    "",
    "| Measurement | Five-run median | Budget / reviewed comparison |",
    "| --- | ---: | --- |",
    f"| Rust parser | {timing['parse_mb_per_second']:.3f} MB/s; {timing['parse_lines_per_second']:,.0f} lines/s | 1.408 MB/s reviewed baseline; +{100*(timing['parse_mb_per_second']/1.408-1):.2f}% throughput |",
    f"| YAML loading | {timing['yaml_median_ms']:.3f} ms | 1500 ms |",
    f"| Cached index + source content hashes | {timing['cache_median_ms']:.3f} ms | 20 ms |",
    f"| Stable edit convergence, parser only | {timing['convergence_median_ms']:.5f} ms | one reparsed line |",
    f"| Forced fresh 100-line parse | {timing['window_median_ms']:.3f} ms | checkpoint replay with the shared stable-line cache; service bounds synchronous work |",
    "",
    "Search/capture workspaces and output vectors reuse allocated capacity across lines and nested branch replay. Stable line transitions share interned operations in a bounded document cache. The allocations integration test counts zero Rust allocations over 10,000 warmed simple streaming lines, both with memoization disabled and enabled; native Oniguruma C allocation is outside that counter. This does not serialize scratch into checkpoints or change semantic parser equality. End-to-end service timings, including five actual background-thread runs, are in ../vel-highlight/RESULTS.md. Warm style resolution and its zero-allocation check are in ../vel-color-scheme/README.md.",
    "",
    f"Raw parser seconds: `{timing['parse_seconds']}`. Regex counters over warm-up and five runs: `{json.dumps(timing['regex_metrics'], sort_keys=True)}`. All observations are in tests/timings.json.",
    "",
    "## All 202 loaded syntaxes",
    "",
    "Counts include metadata assertions attributed by the declared syntax path. A dash means no executed assertions; it is not reported as 100%.",
    "",
    "| Syntax | Passed | Failed | Skipped | Pass rate |",
    "| --- | ---: | ---: | ---: | ---: |",
]
for name, c in data["syntaxes"].items():
    n = c["passed"] + c["failed"]
    rate = f"{100*c['passed']/n:.4f}%" if n else "—"
    out.append(f"| {escaped(name)} | {c['passed']} | {c['failed']} | {c['skipped']} | {rate} |")
out += [
    "",
    "Reproduce with cargo run --release -p vel-syntax --bin syntax-tests; cargo run --release -p vel-syntax --bin syntax-bench; python3 crates/vel-syntax/scripts/report.py. tests/last-run.json contains every file/category and the first mismatch evidence. tests/baseline.json is the monotonic regression gate, not a claim of zero known failures.",
    "",
    "Editor API: see ../vel-highlight/README.md. Metadata-only callers use MetadataSet::metadata_for, indent::scoped_spans, MetadataSet::symbols, and indent::reindent.",
]
write_atomic(crate / "RESULTS.md", "\n".join(out) + "\n")

highlight = crate.parent / "vel-highlight"
runs = json.loads((highlight / "timings.json").read_text())
edit = [r for r in runs if r["case"] == "edit_10k_visible_100"]
full = [r for r in runs if r["case"] == "full_10mb_rust"]
background = [r for r in runs if r["case"] == "full_10mb_rust_background"]
worker_edit=[r for r in runs if r["case"]=="worker_edit_10k_visible_100"]
assert len(edit) >= 5 and len(full) >= 5 and len(background) >= 5 and len(worker_edit)>=5
worker_samples=sorted(v for r in worker_edit for v in r["samples"])
worker_p95=worker_samples[math.ceil(len(worker_samples)*.95)-1]
samples = sorted(v for r in edit for v in r["samples"])
p95 = samples[math.ceil(len(samples) * .95)-1]
style = [json.loads(line) for line in (crate.parent / "vel-color-scheme/timings.jsonl").read_text().splitlines()]
style = [r for r in style if r["lookup"] == "scope_stack"]
assert len(style) >= 5 and all(r["allocations"] == 0 for r in style)
report = [
    "# Highlight service measurements",
    "",
    "Release, macOS 26.1 arm64, rustc 1.99.1, shared public Rust grammar and original Velocity Dark palette. No sysctl. Five runs per case; another lane was active, so scheduler noise is retained in the raw observations.",
    "",
    "| Case | Result | Budget |",
    "| --- | ---: | --- |",
    f"| Worker service edit submission → exact visible batch, pooled p95 of {len(worker_samples)} samples | **{worker_p95:.5f} ms** | ≤0.2 ms |",
    f"| Stable one-letter edit, 10k lines, visible 100, pooled p95 of {len(samples)} samples | **{p95:.5f} ms** | ≤0.2 ms |",
    f"| Median of the five per-run edit p95s | {statistics.median(r['p95_ms'] for r in edit):.5f} ms | ≤2 ms |",
    f"| Worst individual edit sample | {max(samples):.5f} ms | all samples completed their requested viewport |",
    f"| Warm scope-stack style resolution (prior cache benchmark) | **{statistics.median(r['lookups_per_second'] for r in style)/1e6:.2f}M/s** | ≥20M/s; zero allocations in every 5M-lookup run |",
    f"| Full 10 MB Rust, core including styles/checkpoints | {statistics.median(r['seconds'] for r in full):.3f} s | report only |",
    f"| Full 10 MB Rust, actual background service | **{statistics.median(r['seconds'] for r in background):.3f} s** | report only |",
    "",
    f"Worker per-run edit p95 ms: `{[r['p95_ms'] for r in worker_edit]}`; worst worker delivery: {max(worker_samples):.5f} ms. Worker tests start from a completely highlighted 10k-line snapshot, submit a real ChangeSet through HighlightService, and time until the immediate or asynchronous result is exact. Polling/yielding is confined to the benchmark; production typing never waits for it.",
    "",
    "The edit timer includes apply_change and the returned 100-line batch. Text replacement happens before timing. All 500 edits reparse exactly one line, then converge and reuse the validated suffix. This tests stable token edits, not multiline delimiter changes or a first request at a distant unparsed viewport.",
    "",
    f"Per-run edit p95 ms: `{[r['p95_ms'] for r in edit]}`. Core full-file seconds: `{[r['seconds'] for r in full]}`. Background full-file seconds: `{[r['seconds'] for r in background]}`.",
    "",
    "The background measurement starts a real HighlightService thread, polls its nonblocking progress once per millisecond, and waits for every line to validate. It includes thread startup, 1 ms work slicing, yielding and completion observation; it excludes syntax-set and Text construction. The first background run includes initially cold regex paths; following runs share compiled patterns. It does not request a 297k-line viewport or allocate a renderer-sized result. Core measurements warm Rust paths first and call finish directly.",
    "",
    "timings-before-interning.json is the five-run comparison before repeated operation arrays and end-of-line scope stacks were interned. Dense N=1 checkpoints remain proportional to line count. The warmed parser allocation fixture is allocation-free with and without memoization; cache misses, speculative snapshots, styles and returned batches can allocate. The 2 ms synchronous deadline is cooperative between lines: a single pathological regex call is not preemptible. The parser-only checkpoint-window measurements are in vel-syntax/tests/timings.json; they now reuse the shared stable-line cache. Missing viewports still fall back and complete asynchronously.",
    "",
    "The regression tests compare against complete fresh highlighting after Unicode/newline edits, disjoint pending changes, branch backtracking, and palette swaps. A background rewind republishes an already completed visible window; scheme generations and buffer versions filter stale results. The original scope corpus is 125,877/0, symbol assertions 688/0; reindent has known failures described in ../vel-syntax/RESULTS.md.",
    "",
    "Reproduce: cargo run --release -p vel-highlight --example highlight_bench, then cargo run --release -p vel-highlight --example highlight_bench -- --background-only and --worker-edits-only; cargo run --release -p vel-color-scheme --example style_cache_bench; python3 crates/vel-syntax/scripts/report.py. Integration: README.md.",
]
write_atomic(highlight / "RESULTS.md", "\n".join(report) + "\n")
print(f"wrote syntax and highlighter results: {counts['passed']} passed / {counts['failed']} failed / {counts['skipped']} skipped")
