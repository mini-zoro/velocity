#!/usr/bin/env python3
"""Collect sequential CPU view measurements without starting AppKit or Metal."""
import argparse
import datetime
import hashlib
import json
import platform
from pathlib import Path
import shutil
import subprocess


def statistics(values):
    """Use the same interpolated quantiles as the Rust reporter."""
    ordered = sorted(values)

    def quantile(p):
        rank = (len(ordered) - 1) * p
        lower = int(rank)
        upper = min(lower + 1, len(ordered) - 1)
        return ordered[lower] + (ordered[upper] - ordered[lower]) * (rank - lower)

    return {"samples": len(values), "p50": quantile(.5), "p95": quantile(.95),
            "p99": quantile(.99), "max": ordered[-1]}


def main():
    """Keep raw samples, binary identity and aggregate results in a new directory."""
    repo = Path(__file__).resolve().parents[3]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--binary", type=Path, default=repo / "target/release/velocity")
    parser.add_argument("--output-dir", type=Path)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    if not args.binary.is_file():
        parser.error("build velocity --release first, or supply --binary")
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output = args.output_dir or repo / "crates/velocity/output" / f"round6-cpu-{stamp}"
    output.mkdir(parents=True, exist_ok=False)
    output = output.resolve()
    binary = output / "velocity"
    shutil.copy2(args.binary, binary)
    environment = {
        "captured_utc": stamp, "platform": platform.platform(), "machine": platform.machine(),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip(),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "source_hashes_at_run_time": {
            name: hashlib.sha256((repo / name).read_bytes()).hexdigest()
            for name in ["crates/vel-regex/src/lib.rs", "crates/vel-syntax/src/state.rs",
                         "crates/vel-highlight/src/lib.rs", "Cargo.lock"]
        },
        "note": "One copied release binary; sequential runs; retain all outliers. Lane B is active.",
        "gui_started": False, "gpu_started": False,
    }
    (output / "environment.json").write_text(json.dumps(environment, indent=2) + "\n")
    records = []
    runs = []
    print(output, flush=True)
    for run in range(1, args.runs + 1):
        path = output / f"run-{run}.jsonl"
        with path.with_suffix(".stdout.log").open("w") as log:
            result = subprocess.run([str(binary), "--headless-view-features", "--output", str(path)],
                                    cwd=repo, stdout=log, stderr=subprocess.STDOUT, timeout=240)
        if result.returncode:
            raise SystemExit(f"run {run} failed ({result.returncode}); see {path.with_suffix('.stdout.log')}")
        rows = [json.loads(line) for line in path.read_text().splitlines()]
        records.extend(rows)
        observed = {row["kind"]: {key: value for key, value in row.items()
                    if key.endswith("stats_ms") or key == "after_default_scene_ms"}
                    for row in rows if "stats_ms" in row or "fold_stats_ms" in row
                    or row["kind"] == "first_highlighted_screen_cpu"}
        runs.append(observed)
        print(f"run {run}: {json.dumps(observed)}", flush=True)
    pooled = {}
    for kind, key in [("highlight_minimap_scroll_cpu", "samples_ms"),
                      ("highlighted_typing_cpu", "samples_ms"), ("rewrap_100k_cpu", "samples_ms"),
                      ("fold_unfold_100k_cpu", "fold_samples_ms"),
                      ("fold_unfold_100k_cpu", "unfold_samples_ms")]:
        pooled[f"{kind}:{key}"] = statistics([value for row in records if row["kind"] == kind
                                               for value in row[key]])
    summary = {"environment": environment, "unit": "ms", "runs": runs, "pooled": pooled,
               "first_screen_ms": [row["after_default_scene_ms"] for row in records
                                   if row["kind"] == "first_highlighted_screen_cpu"],
               "boundaries": "CPU proxies only; native present, scanout and minimap GPU remain pending."}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print("summary: " + str(output / "summary.json"), flush=True)


if __name__ == "__main__":
    main()
