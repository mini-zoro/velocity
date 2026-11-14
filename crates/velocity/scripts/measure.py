#!/usr/bin/env python3
"""Unattended native benchmark matrix and strict JSONL validation (stdlib only)."""
from __future__ import annotations
import argparse
import ctypes
import ctypes.util
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
DEFAULT_OUTPUT = ROOT / "crates/velocity/output"


def display_preflight():
    """Read CoreGraphics session/display state, without creating GUI/Metal objects."""
    if sys.platform != "darwin":
        raise RuntimeError("GUI measurements require macOS")
    cg = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
    cf = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
    cg.CGMainDisplayID.restype = ctypes.c_uint32
    cg.CGDisplayIsAsleep.argtypes = [ctypes.c_uint32]
    cg.CGDisplayIsAsleep.restype = ctypes.c_bool
    cg.CGSessionCopyCurrentDictionary.restype = ctypes.c_void_p
    cf.CFStringCreateWithCString.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_uint32]
    cf.CFStringCreateWithCString.restype = ctypes.c_void_p
    cf.CFDictionaryGetValue.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    cf.CFDictionaryGetValue.restype = ctypes.c_void_p
    cf.CFBooleanGetValue.argtypes = [ctypes.c_void_p]
    cf.CFBooleanGetValue.restype = ctypes.c_bool
    cf.CFRelease.argtypes = [ctypes.c_void_p]
    session = cg.CGSessionCopyCurrentDictionary()
    key = cf.CFStringCreateWithCString(None, b"CGSSessionScreenIsLocked", 0x08000100)
    try:
        locked = cf.CFDictionaryGetValue(session, key) if session else None
        reasons = []
        if cg.CGDisplayIsAsleep(cg.CGMainDisplayID()):
            reasons.append("display asleep")
        if locked and cf.CFBooleanGetValue(locked):
            reasons.append("screen locked")
        if not session:
            reasons.append("console session unavailable")
        if reasons:
            raise RuntimeError("display preflight failed: " + ", ".join(reasons))
    finally:
        cf.CFRelease(key)
        if session:
            cf.CFRelease(session)


def summarize(values):
    if not values or any(not math.isfinite(v) or v < 0 for v in values):
        return None
    values = sorted(values)
    def q(p):
        rank = (len(values) - 1) * p
        lo, hi = math.floor(rank), math.ceil(rank)
        return values[lo] + (values[hi] - values[lo]) * (rank - lo)
    return {"count": len(values), "p50": q(.5), "p95": q(.95),
            "p99": q(.99), "max": values[-1], "mean": sum(values)/len(values)}


def validate(records, mode=None):
    if not records or any(r.get("kind") == "error" for r in records):
        raise ValueError("empty or error-containing run")
    if not any(r.get("kind") == "complete" and r.get("complete") is True for r in records):
        raise ValueError("missing successful complete record")
    if any(r.get("schema") != 1 for r in records):
        raise ValueError("expected schema 1")
    if any(r.get("external_input_events", 0) for r in records):
        raise ValueError("benchmark contaminated by external input")
    mode = mode or records[0].get("mode")
    if mode == "headless":
        if not any(r.get("kind") == "headless_summary" for r in records):
            raise ValueError("missing headless CPU summary")
        return
    startup = next((r for r in records if r.get("kind") == "startup"), None)
    if not startup or not startup.get("scanout_main_ms", 0) > 0:
        raise ValueError("missing real presentedTime scanout")
    phases = {m["phase"]: m["main_ms"] for m in startup["milestones"]}
    required = ["first_scene_ready", "first_offscreen_submitted",
                "first_frame_prepared_before_order_front", "ordered_front",
                "first_submit", "first_present_returned"]
    if any(p not in phases for p in required):
        raise ValueError("missing synchronous startup phase")
    if any(phases[a] > phases[b] for a, b in zip(required, required[1:])):
        raise ValueError("startup phase order violated")
    memory = next((r for r in records if r.get("kind") == "memory" and r.get("phase") == "after_first_frame"), None)
    if not memory:
        raise ValueError("missing first-frame memory observation")
    for key in ("rss_bytes", "phys_footprint_bytes"):
        if key not in memory:
            raise ValueError(f"missing independent memory field {key}")
    expectations = {"scroll": ("scroll_frame", "scroll_summary", "frames", 600),
                    "typing": ("typing_sample", "typing_summary", "insertions", 500)}
    if mode in expectations:
        kind, summary, count_key, expected = expectations[mode]
        samples = [r for r in records if r.get("kind") == kind]
        totals = [r for r in records if r.get("kind") == summary]
        if len(samples) != expected or len(totals) != 1 or totals[0].get(count_key) != expected:
            raise ValueError(f"incomplete {mode}: expected {expected} samples and one summary")
        if [r["sample"]["index"] for r in samples] != list(range(expected)):
            raise ValueError(f"duplicate/missing {mode} sample ids")
        if totals[0].get("external_input_events", 0):
            raise ValueError("benchmark contaminated by external input")
    if mode == "open":
        opened = [r for r in records if r.get("kind") == "file_open" and r.get("phase") == "fully_loaded"]
        if not opened or any(r.get("success") is False for r in opened):
            raise ValueError("file opening did not finish successfully")
    if mode == "memory" and not any(r.get("kind") == "memory_summary" and r.get("sweep_complete") is True for r in records):
        raise ValueError("incomplete memory sweep")


def read_run(path, mode=None):
    records = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    validate(records, mode)
    return records


def run_floor(binary, output, timeout):
    display_preflight()
    result = subprocess.run([str(binary)], cwd=ROOT, capture_output=True, text=True, timeout=timeout)
    output.write_text(result.stdout)
    output.with_suffix(".stderr.txt").write_text(result.stderr)
    if result.returncode:
        raise RuntimeError(f"native floor failed: {result.stdout[-4000:]} {result.stderr[-1000:]}")
    try:
        lines = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        record = next(r for r in lines if r.get("kind") == "native_floor")
    except (json.JSONDecodeError, StopIteration):
        # Accept the unmodified P0 helper too, but keep its receipt boundary explicit.
        marks = {k: float(v) for k, v in re.findall(r"([A-Za-z0-9_+()]+)=([0-9.]+)", result.stdout)}
        if not marks.get("presented(scanout)", 0) > 0:
            raise ValueError(f"native floor missing scanout: {result.stdout!r}")
        record = {"schema": 1, "kind": "native_floor", "mode": "native_floor",
                  "scanout_main_ms": marks["presented(scanout)"], "marks_ms": marks,
                  "boundary": "P0 handler main-queue receipt; inspect callback lag before comparing to actual scanout"}
    if record.get("kind") != "native_floor" or not record.get("scanout_main_ms", 0) > 0:
        raise ValueError(f"native floor failed: {record}")
    record["stdout"] = result.stdout
    record["stderr"] = result.stderr
    output.write_text(json.dumps(record, ensure_ascii=False) + "\n")
    return record["scanout_main_ms"]



def self_test():
    assert summarize([4, 1, 2, 3])["p50"] == 2.5
    assert summarize([]) is None
    for records in ([], [{"schema": 1, "kind": "error"}], [{"schema": 1, "kind": "complete", "complete": True, "mode": "startup"}]):
        try:
            validate(records)
        except ValueError:
            pass
        else:
            raise AssertionError("accepted incomplete telemetry")
    validate([{"schema": 1, "kind": "headless_summary", "mode": "headless"},
              {"schema": 1, "kind": "complete", "complete": True, "mode": "headless"}])
    print("measurement parser/quantiles/completeness self-test passed (no GUI)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/velocity")
    parser.add_argument("--floor", type=Path, help="already-built P0 Swift floor executable; interleaved with warm launches")
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--input", type=Path, help="default: generated ASCII 5000-line comparable workload")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--dry-run", action="store_true", help="print exact commands; never launch a GUI or create results")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--validate", type=Path, nargs="+", help="validate existing application JSONL only")
    parser.add_argument("--only-startup", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if args.validate:
        for path in args.validate:
            read_run(path)
            print(f"valid: {path}")
        return
    if args.runs < 1 or args.timeout < 1:
        parser.error("runs and timeout must be positive")
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output = args.output_dir.resolve() / stamp
    binary = args.binary.resolve()
    workload = args.input.resolve() if args.input else output / "startup-5000.rs"
    matrix = [("startup-coldish", "startup", [])]
    matrix += [(f"startup-warm-{i+1:02}", "startup", []) for i in range(args.runs)]
    if not args.only_startup:
        matrix += [("scroll-default", "scroll", []), ("scroll-4k", "scroll", ["--width", "2048", "--height", "1089"]),
                   ("typing-default", "typing", []), ("typing-4k", "typing", ["--width", "2048", "--height", "1089"]),
                   ("typing-1000", "typing", ["--carets", "1000"]),
                   ("typing-1000-4k", "typing", ["--carets", "1000", "--width", "2048", "--height", "1089"]),
                   ("memory", "memory", [])]
    commands = [(label, mode, [str(binary), f"--bench-{mode}", str(workload), "--lines", "5000", "--timeout", str(args.timeout), "--output", str(output / f"{label}.jsonl"), *extra]) for label, mode, extra in matrix]
    if args.dry_run:
        print(json.dumps({"build": ["cargo", "build", "--release", "-p", "velocity"], "commands": [c for _, _, c in commands], "floor": str(args.floor) if args.floor else None}, indent=2))
        return
    output.mkdir(parents=True, exist_ok=False)
    try:
        display_preflight()
    except RuntimeError as error:
        (output / "preflight.jsonl").write_text(json.dumps({"schema": 1, "kind": "error", "stage": "display_preflight", "complete": False, "message": str(error)}) + "\n")
        raise
    if not args.input:
        workload.write_text("\n".join(f'pub fn line_{i:04}() -> usize {{ let value = {i}; value + 1 }} // native startup workload' for i in range(5000)))
    if not args.skip_build:
        subprocess.run(["cargo", "build", "--release", "-p", "velocity"], cwd=ROOT, check=True)
    warm = []
    floors = []
    for label, mode, command in commands:
        if args.floor and label.startswith("startup-warm"):
            floors.append(run_floor(args.floor.resolve(), output / f"floor-{len(floors)+1:02}.jsonl", args.timeout))
        try:
            display_preflight()
        except RuntimeError as error:
            (output / f"{label}.jsonl").write_text(json.dumps({"schema": 1, "kind": "error", "stage": "display_preflight", "mode": mode, "complete": False, "message": str(error)}) + "\n")
            raise
        print("running:", label, flush=True)
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=args.timeout + 10)
        (output / f"{label}.stderr.txt").write_text(result.stderr)
        if result.returncode:
            raise RuntimeError(f"{label} failed with {result.returncode}: {result.stderr[-4000:]}")
        records = read_run(output / f"{label}.jsonl", mode)
        if label.startswith("startup-warm"):
            warm.append(next(r for r in records if r.get("kind") == "startup"))
    phase_names = list(dict.fromkeys(m["phase"] for run in warm for m in run["milestones"]))
    scanout = summarize([r["scanout_main_ms"] for r in warm])
    floor = summarize(floors)
    summary = {"schema": 1, "kind": "startup_summary", "warm_runs": len(warm),
               "main_to_scanout_ms": scanout,
               "process_to_scanout_ms": summarize([r["scanout_process_ms"] for r in warm]) if all(r["scanout_process_ms"] is not None for r in warm) else None,
               "phases_main_ms": {name: summarize([m["main_ms"] for r in warm for m in r["milestones"] if m["phase"] == name]) for name in phase_names},
               "native_floor_scanout_ms": floor,
               "median_over_floor_ms": scanout["p50"]-floor["p50"] if floor else None,
               "floor_limit": "Use scripts/native-floor.swift for actual presentedTime; the original P0 floor is accepted but measures handler receipt.",
               "coldish_limit": "first process after build; system Metal/driver/font caches are not purged",
               "output": str(output)}
    (output / "startup-summary.jsonl").write_text(json.dumps(summary, ensure_ascii=False) + "\n")
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f"measurement failed: {error}", file=sys.stderr)
        sys.exit(1)
