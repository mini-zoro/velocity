#!/usr/bin/env python3
"""Unattended runner/aggregation. Standard library only; stdout is JSONL."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "output"
BINARY = ROOT / "target/release/vel-shell"


def stats(values):
    """Interpolated quantiles, identical to src/stats.rs; no missing-value zeros."""
    values = sorted(values)
    if not values or any(not math.isfinite(x) or x < 0 for x in values):
        return None

    def q(p):
        rank = (len(values) - 1) * p
        lo, hi = math.floor(rank), math.ceil(rank)
        return values[lo] + (values[hi] - values[lo]) * (rank - lo)

    return dict(count=len(values), p50=q(.5), p95=q(.95), p99=q(.99),
                max=max(values), mean=statistics.mean(values))


def aggregate(records, kind, cohort):
    runs = [r for r in records if r.get("kind") == kind]
    if not runs:
        raise ValueError(f"no {kind} records for {cohort}")
    phases = [m["phase"] for m in runs[0]["milestones"]]
    if any([m["phase"] for m in r["milestones"]] != phases for r in runs):
        raise ValueError("phase lists differ across runs; refusing a partial aggregate")
    results = []
    for index, phase in enumerate(phases):
        row = dict(schema=1, kind="startup_aggregate", renderer=kind,
                   cohort=cohort, phase=phase, runs=len(runs), unit="ms")
        for source, key in [("main_ms", "from_main"),
                            ("process_ms", "from_process"),
                            ("delta_ms", "phase_cost")]:
            values = [r["milestones"][index][source] for r in runs]
            row[key] = stats(values) if all(v is not None for v in values) else None
        results.append(row)
    return results


def write_records(path, records):
    with path.open("x", encoding="utf-8") as f:
        for record in records:
            line = json.dumps(record, ensure_ascii=False, allow_nan=False)
            f.write(line + "\n")
            print(line, flush=True)


def parse_records(text):
    return [json.loads(line) for line in text.splitlines() if line.strip()]


def validate_run(records, mode):
    errors = [r for r in records if r.get("kind") == "error"]
    if errors:
        raise ValueError("; ".join(r.get("message", "unknown error") for r in errors))
    if not any(r.get("kind") == "complete" and r.get("complete") for r in records):
        raise ValueError("missing complete=true marker")
    if mode == "scroll":
        samples = [r for r in records if r.get("kind") == "scroll_frame"]
        if len(samples) != 600 or [r["sample"]["index"] for r in samples] != list(range(600)):
            raise ValueError("scroll must contain exactly 600 ordered samples")
        summaries = [r for r in records if r.get("kind") == "scroll_summary"]
        if len(summaries) != 1 or summaries[0]["frames"] != 600:
            raise ValueError("missing/malformed scroll summary")
        if len({tuple(r["sample"]["physical_size"]) for r in samples}) != 1:
            raise ValueError("window size changed during scroll sampling")
        if any(r["sample"]["actual_scroll_points"] != 20 for r in samples):
            raise ValueError("short input hit a scroll boundary; use the default corpus")
        gpu_enabled = any(r.get("kind") == "environment" and
                          r["renderer"].get("timestamp_query_enabled") for r in records)
        if gpu_enabled and any(r.get("gpu_render_pass_ms") is None for r in samples):
            raise ValueError("enabled GPU queries have missing samples")
    elif mode == "typing":
        samples = [r for r in records if r.get("kind") == "typing_sample"]
        if len(samples) != 500 or len({r["sample"]["id"] for r in samples}) != 500:
            raise ValueError("typing must contain exactly 500 unique insertions")
        if not any(r.get("kind") == "typing_summary" and r["insertions"] == 500 for r in records):
            raise ValueError("missing typing summary")
    if mode in ("scroll", "memory"):
        memories = [r for r in records if r.get("kind") == "memory"
                    and r["phase"] == "after_whole_file_scroll"]
        if len(memories) != 1 or not memories[0]["details"]["whole_loaded_buffer_visited"]:
            raise ValueError("memory sweep did not visit the entire loaded buffer")
    for record in records:
        if record.get("kind") == "memory" and record.get("rss_bytes") is None:
            raise ValueError("RSS unavailable: " + str(record.get("rss_error")))
        if (record.get("kind") == "memory" and "phys_footprint_bytes" in record
                and record["phys_footprint_bytes"] is None):
            raise ValueError("phys_footprint unavailable: " + str(record.get("phys_footprint_error")))
        if record.get("external_input_events", 0):
            raise ValueError("benchmark contaminated by physical keyboard input")


class Runner:
    def __init__(self, args):
        self.args = args
        self.batch = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ") + f"-{os.getpid()}"
        self.failures = []
        OUTPUT.mkdir(exist_ok=True)
        self.environment = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / "target"))

    def build(self):
        # Recompile/relink only this package; keep downloaded dependencies and their builds.
        print("Fresh release build (only vel-shell artifacts cleaned)", file=sys.stderr, flush=True)
        subprocess.run(["cargo", "clean", "--release", "-p", "vel-shell"],
                       cwd=ROOT, env=self.environment, stdout=sys.stderr, check=True)
        subprocess.run(["cargo", "build", "--release", "--locked"],
                       cwd=ROOT, env=self.environment, stdout=sys.stderr, check=True)
        metadata = dict(schema=1, kind="run_environment", batch=self.batch,
                        platform=platform.platform(), machine=platform.machine(),
                        lockfile="Cargo.lock", coldish="first launch after relink; caches NOT purged")
        for label, command in [("rustc", ["rustc", "--version"]),
                               ("macos", ["sw_vers"]),
                               ("power", ["pmset", "-g", "batt"]),
                               ("cpu", ["sysctl", "-n", "machdep.cpu.brand_string"])]:
            try:
                result = subprocess.run(command, capture_output=True, text=True,
                                        timeout=10, check=False, cwd=ROOT)
                metadata[label] = result.stdout.strip() or result.stderr.strip()
            except (OSError, subprocess.TimeoutExpired) as error:
                metadata[label] = str(error)
        write_records(OUTPUT / f"environment-{self.batch}.jsonl", [metadata])

    def run(self, mode, label, extra=(), optional=False, timeout=None):
        path = OUTPUT / f"{label}-{self.batch}.jsonl"
        timeout = timeout or self.args.timeout
        command = [str(BINARY), f"--bench-{mode}", "--timeout", str(timeout),
                   "--output", str(path), "--present", self.args.present,
                   "--lines", str(self.args.lines), "--seed", str(self.args.seed)]
        if self.args.large:
            command.append("--large")
        command += list(extra)
        if self.args.file:
            command += ["--", str(Path(self.args.file).resolve())]
        print(f"Running {label}: {' '.join(command)}", file=sys.stderr, flush=True)
        started = time.monotonic()
        status = "ok"
        error = None
        records = []
        try:
            result = subprocess.run(command, cwd=ROOT, env=self.environment,
                                    capture_output=True, text=True, timeout=timeout + 15)
            (OUTPUT / f"{label}-{self.batch}.stderr.log").write_text(result.stderr, encoding="utf-8")
            if result.stderr:
                print(result.stderr, end="", file=sys.stderr, flush=True)
            records = parse_records(result.stdout)
            for record in records:
                print(json.dumps(record, ensure_ascii=False, allow_nan=False), flush=True)
            if result.returncode:
                # Unsupported experimental present modes are data, not a substituted Fifo run.
                errors = [r.get("message", "") for r in records if r.get("kind") == "error"]
                if optional and any("unsupported present mode" in e for e in errors):
                    status = "unsupported"
                else:
                    raise ValueError(f"exit {result.returncode}: " + "; ".join(errors))
            else:
                validate_run(records, mode)
            if path.exists() and parse_records(path.read_text(encoding="utf-8")) != records:
                raise ValueError("stdout and on-disk JSONL differ")
        except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
            status, error = "failed", str(exc)
            self.failures.append(dict(label=label, error=error))
            if not optional:
                print(f"FAILED {label}: {error}", file=sys.stderr, flush=True)
        write_records(OUTPUT / f"{label}-runner-{self.batch}.jsonl", [dict(
            schema=1, kind="runner_result", label=label, status=status, error=error,
            wall_seconds=time.monotonic() - started, result_file=str(path),
            command=command)])
        if status == "failed" and not optional:
            raise RuntimeError(f"{label} failed; see {path} and its stderr log")
        return records if status == "ok" else []

    def startup(self):
        self.build()
        cold = self.run("startup", "startup-coldish")
        warm = []
        for index in range(10):
            warm += self.run("startup", f"startup-warm-{index + 1:02}")
        summary = aggregate(cold, "startup", "cold-ish-n=1") + aggregate(warm, "startup", "warm-n=10")
        write_records(OUTPUT / f"startup-summary-{self.batch}.jsonl", summary)
        warm_rows = {r["phase"]: r for r in summary if r["cohort"] == "warm-n=10"}
        creation = sum(warm_rows[p]["phase_cost"]["p50"]
                       for p in ("wgpu_instance", "adapter", "device_queue"))
        total = warm_rows["first_present_returned"]["from_main"]["p50"]
        dominant = creation >= 20 and creation / max(total, .001) >= .30
        compare = self.args.metal == "always" or (self.args.metal == "auto" and dominant)
        write_records(OUTPUT / f"metal-decision-{self.batch}.jsonl", [dict(
            schema=1, kind="metal_comparison_decision", wgpu_creation_median_ms=creation,
            first_present_median_main_ms=total, threshold_ms=20, threshold_share=.30,
            dominant=dominant, run_comparison=compare, policy=self.args.metal)])
        if compare:
            # Raw Metal is clear-only and gets its own unaggregated priming launch + ten warm runs.
            self.run("metal", "metal-reference-prime")
            metal = []
            for index in range(10):
                metal += self.run("metal", f"metal-warm-{index + 1:02}")
            write_records(OUTPUT / f"metal-summary-{self.batch}.jsonl",
                          aggregate(metal, "metal_startup", "warm-n=10-clear-only"))

    def suite(self):
        self.startup()
        self.run("scroll", "scroll-default")
        self.run("scroll", "scroll-large", ["--large"])
        self.run("typing", "typing-default")
        self.run("typing", "typing-large", ["--large"])
        self.run("memory", "memory-entire-file", ["--lines", "0"], timeout=max(self.args.timeout, 600))
        for mode in ("mailbox", "immediate"):
            for bench in ("scroll", "typing"):
                self.run(bench, f"{bench}-{mode}", ["--present", mode], optional=True)
        write_records(OUTPUT / f"suite-{self.batch}.jsonl", [dict(
            schema=1, kind="suite_complete", complete=not self.failures,
            failures=self.failures, batch=self.batch)])
        if self.failures:
            raise RuntimeError("one or more experiments failed; inspect suite JSONL")


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("action", choices=["startup", "suite"])
    p.add_argument("--present", choices=["fifo", "mailbox", "immediate"], default="fifo")
    p.add_argument("--metal", choices=["auto", "always", "never"], default="auto",
                   help="auto compares raw Metal if creation >=20ms and >=30%% of warm main->present")
    p.add_argument("--large", action="store_true")
    p.add_argument("--lines", type=int, default=5000)
    p.add_argument("--seed", type=int, default=0x76656c6f63697479)
    p.add_argument("--timeout", type=int, default=180)
    p.add_argument("--file")
    return p


def main():
    args = parser().parse_args()
    if args.lines < 0 or args.timeout <= 0 or not 0 <= args.seed <= 2**64 - 1:
        raise SystemExit("invalid lines, timeout, or u64 seed")
    if args.action == "suite" and (args.large or args.present != "fifo"):
        raise SystemExit("suite owns its size/present-mode matrix; use startup for overrides")
    runner = Runner(args)
    try:
        getattr(runner, args.action)()
    except (RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
