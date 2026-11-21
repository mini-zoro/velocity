#!/usr/bin/env python3
"""Run the comparison matrix serially; preserve JSONL, load and Darwin counters."""

import argparse
import ctypes
import errno
import hashlib
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import tempfile
import time


def counters_reader(env):
    # Same public own-child accounting as crates/velocity/scripts/round21-cpu.py.
    sdk = subprocess.check_output(
        ["xcrun", "--sdk", "macosx", "--show-sdk-path"], text=True, env=env
    ).strip()
    header = (Path(sdk) / "usr/include/sys/resource.h").read_text()
    body = header.split("struct rusage_info_v4 {", 1)[1].split("};", 1)[0]

    class Usage(ctypes.Structure):
        _fields_ = [
            (name, ctypes.c_uint8 * int(length) if ty == "uint8_t" else ctypes.c_uint64)
            for ty, name, length in re.findall(
                r"(uint8_t|uint64_t)\s+(ri_\w+)(?:\[(\d+)\])?;", body
            )
        ]

    proc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True).proc_pid_rusage
    proc.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
    proc.restype = ctypes.c_int
    # The system Python on macOS may omit os.waitid. Darwin sys/wait.h:
    # P_PID == 1, WEXITED == 4, WNOWAIT == 0x20. The aligned buffer is
    # larger than Darwin siginfo_t; we only use the successful wait.
    waitid = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True).waitid
    waitid.argtypes = [ctypes.c_int, ctypes.c_uint, ctypes.c_void_p, ctypes.c_int]
    waitid.restype = ctypes.c_int

    def read(pid):
        info_buffer = (ctypes.c_uint64 * 32)()
        while waitid(1, pid, ctypes.byref(info_buffer), 4 | 0x20):
            error = ctypes.get_errno()
            if error != errno.EINTR:
                return {"wait_error": error}
        info = Usage()
        if proc(pid, 4, ctypes.byref(info)):
            return {"error": ctypes.get_errno()}
        return {"instructions": info.ri_instructions, "cycles": info.ri_cycles}

    return read


def cases(repo, find_roots):
    data = repo / "spikes/data"
    fixtures = data / "sublime-comparison"
    result = {
        f"hl-{path.name}": ["--headless-perf-highlight", path]
        for path in [fixtures / name for name in ("jquery-2.1.4.js", "jquery-3.7.1.js", "sqlite3.c")]
        + [data / "rust-10mb.rs", data / "javascript-10mb.js"]
    }
    result.update({
        "startup-empty": ["--headless-perf-startup"],
        "startup-5k": ["--headless-perf-startup", "--lines", "5000"],
        **{f"open-{name}": ["--headless-perf-file", fixtures / name] for name in [
            "sqlite3.c", "gcc.c", "generated-1m.c", "generated-5m.c", "log-250mb.txt",
            *(f"line-{n}.txt" for n in (10000, 100000, 1000000, 50000000)),
        ]},
        "goto": ["--headless-perf-goto"],
    })
    if find_roots:
        result["find"] = ["--headless-perf-find", *(p.resolve() for p in find_roots)]
    return result


def startup_profile(root, case, home):
    if case != "startup-interactive-restore10":
        return
    # Ten clean files; only the fifth tab is visible and should touch disk at setup.
    if not root.exists():
        (root / "Sessions").mkdir(parents=True)
    files = home / "restore-fixtures"
    files.mkdir(exist_ok=True)
    ids = list(range(1, 11))
    documents, sheets = [], []
    for i in ids:
        file = files / f"tab-{i}.c"
        if not file.exists():
            file.write_text(f"int file_{i}(void) {{ return {i}; }}\n")
        documents.append({
            "id": i, "path": str(file), "encoding": "UTF-8", "bom": False,
            "line_ending": "LF", "dirty": False,
            "disk": {"bytes": file.stat().st_size, "modified_ns": file.stat().st_mtime_ns},
        })
        sheets.append({
            "id": i, "document": i, "transient": False,
            "view": {"selections": [{"a": 4, "b": 4}], "primary": 0, "scroll": [0, 0], "folds": []},
        })
    (root / "Sessions/Session.sublime-session").write_text(json.dumps({
        "version": 1, "documents": documents, "recent_files": [], "recent_folders": [],
        "recent_projects": [], "windows": [{
            "sheets": sheets, "sidebar_visible": True, "workbench": {
                "layout": {"cols": [0, 1], "rows": [0, 1], "cells": [[0, 0, 1, 1]]},
                "groups": [{"sheets": ids, "selected": [5], "focused": 5,
                            "anchor": 5, "tab_scroll": 0}], "active": 0,
            },
        }],
    }) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--before", type=Path, help="alternate with an older harness build")
    parser.add_argument("--find-root", action="append", type=Path, default=[])
    parser.add_argument("--only", action="append", default=[], help="only run this case (repeatable)")
    parser.add_argument("--output", type=Path, help="new directory for raw records; default mktemp")
    parser.add_argument("--runs", type=int, default=5)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--instruction-gate", action="store_true",
                      help="compare existing features/carets1000/smoke1 work; requires --before")
    mode.add_argument("--interactive-startup", action="store_true",
                      help="fresh profiles: empty, restore ten tabs and open sqlite3.c")
    parser.add_argument("--warm-syntax-cache", action="store_true",
                        help="warm the shared source-loader cache for interactive startup runs")
    parser.add_argument("--features-file", type=Path,
                        help="explicit feature-gate input instead of the default 10 MB Rust fixture")
    args = parser.parse_args()
    if args.runs < 5:
        parser.error("--runs must be at least 5")
    repo = Path(__file__).resolve().parent.parent
    if args.instruction_gate:
        if args.before is None:
            parser.error("--instruction-gate requires --before")
        cases_ = {
            "features": ["--headless-view-features"],
            "carets1000": ["--headless-smoke", "--carets", "1000"],
            "smoke1": ["--headless-smoke", "--carets", "1"],
        }
        if args.features_file is not None:
            if not args.features_file.is_file():
                parser.error("--features-file requires a readable fixture")
            cases_["features"].append(args.features_file.resolve())
    elif args.interactive_startup:
        flag = "--headless-perf-startup-interactive"
        cases_ = {
            "startup-interactive-empty": [flag],
            "startup-interactive-restore10": [flag, "--restore-session"],
            "startup-interactive-sqlite": [flag, repo / "spikes/data/sublime-comparison/sqlite3.c"],
        }
    else:
        cases_ = cases(repo, args.find_root)
    if unknown := set(args.only) - cases_.keys():
        parser.error(f"unknown cases {sorted(unknown)}; choices: {', '.join(cases_)}")
    if args.only:
        cases_ = {name: cases_[name] for name in args.only}
    if args.warm_syntax_cache and not args.interactive_startup:
        parser.error("--warm-syntax-cache requires --interactive-startup")
    if args.features_file is not None and not args.instruction_gate:
        parser.error("--features-file requires --instruction-gate")
    sqlite = cases_.get("startup-interactive-sqlite")
    if sqlite and not sqlite[-1].is_file():
        parser.error(f"missing {sqlite[-1]}; use --only to run the other cases")
    binaries = {"candidate": args.binary.resolve()}
    if args.before:
        binaries = {"before": args.before.resolve(), **binaries}
    for binary in binaries.values():
        if not os.access(binary, os.X_OK):
            parser.error(f"{binary} is not executable")
    if args.output is None:
        root = Path(tempfile.mkdtemp(prefix="velocity-headless-"))
    else:
        root = args.output.resolve()
        root.mkdir()
    for subdir in ("home", "tmp", "logs"):
        (root / subdir).mkdir()
    home = Path.home()
    env = {
        **os.environ, "HOME": str(root / "home"), "SHELL": "/bin/zsh",
        "RUSTUP_HOME": os.getenv("RUSTUP_HOME", str(home / ".rustup")),
        "CARGO_HOME": os.getenv("CARGO_HOME", str(home / ".cargo")),
        "TMPDIR": str(root / "tmp"), "VELOCITY_LOG_DIR": str(root / "logs"),
    }
    read_counters = counters_reader(env)
    (root / "run.json").write_text(json.dumps({
        "binaries": {k: {"path": str(v), "sha256": hashlib.sha256(v.read_bytes()).hexdigest()}
                     for k, v in binaries.items()},
        "cases": cases_, "runs": args.runs, "warm_syntax_cache": args.warm_syntax_cache,
    }, default=str, indent=2) + "\n")
    print(f"Raw logs: {root}", flush=True)
    # Populate the disposable syntax cache once. All timed starts are new
    # processes, with no claim that OS, storage or asset caches are cold.
    if not args.interactive_startup or args.warm_syntax_cache:
        subprocess.run(
            [str(args.binary.resolve()), "--headless-perf-startup", "--data-dir",
             env["HOME"] + "/profile", "--output", str(root / "warmup.jsonl")],
            env=env, cwd=repo, stdout=subprocess.DEVNULL, stdin=subprocess.DEVNULL, check=True,
        )
    failures = 0
    counts = {case: {label: [] for label in binaries} for case in cases_}
    for run in range(1, args.runs + 1):
        labels = list(binaries) if run % 2 else list(reversed(binaries))
        for name, flags in cases_.items():
            for label in labels:
                stem = root / f"{name}--{label}--{run}"
                uptime = subprocess.check_output(["/usr/bin/uptime"], text=True, env=env).strip()
                at = time.monotonic()
                records = stem.with_name(stem.name + ".jsonl")
                profile, child_env = Path(env["HOME"]) / "profile", env
                if args.interactive_startup:
                    profile = stem.with_suffix(".profile")
                    startup_profile(profile, name, Path(env["HOME"]))
                    if not args.warm_syntax_cache:
                        fresh_tmp = stem.with_suffix(".tmp")
                        fresh_tmp.mkdir()
                        child_env = {**env, "TMPDIR": str(fresh_tmp)}
                with stem.with_name(stem.name + ".stderr").open("w") as stderr:
                    child = subprocess.Popen(
                        [str(binaries[label]), *map(str, flags), "--timeout", "600", "--data-dir",
                         str(profile), "--output", str(records)],
                        env=child_env, cwd=repo, stdout=subprocess.DEVNULL, stderr=stderr,
                        stdin=subprocess.DEVNULL,
                    )
                    try:
                        usage = read_counters(child.pid)
                    finally:
                        # Reap after proc_pid_rusage has read the zombie's counters.
                        _, status, rusage = os.wait4(child.pid, 0)
                        code = child.returncode = os.waitstatus_to_exitcode(status)
                stem.with_name(stem.name + ".json").write_text(json.dumps({
                    "uptime": uptime, "returncode": code,
                    "process_ms": (time.monotonic() - at) * 1000,
                    "user_s": rusage.ru_utime, "system_s": rusage.ru_stime, **usage,
                }) + "\n")
                rows = [json.loads(line) for line in records.read_text().splitlines()] \
                    if records.exists() else []
                records_ok = code == 0 and any(row.get("complete") is True for row in rows)
                if args.interactive_startup:
                    complete = any(row.get("phase") == "interactive_setup_ready" for row in rows)
                    if label == "before":
                        # An instrumentation-only baseline may emit just the phase boundary.
                        records_ok = code == 0 and complete
                    else:
                        startup = next((r for r in rows if r.get("kind") == "interactive_startup"), {})
                        records_ok = records_ok and complete and (
                            name != "startup-interactive-restore10"
                            or (startup.get("tabs") == 10 and startup.get("lazy_tabs") == 9)
                        )
                failures += not records_ok
                if records_ok and usage.get("instructions", 0) > 0:
                    counts[name][label].append(usage["instructions"])
                print(f"{name} {label} {run}/{args.runs}: exit {code}", flush=True)
    if args.instruction_gate:
        summary = {}
        for name, entry in counts.items():
            before, candidate = entry["before"], entry["candidate"]
            if len(before) != args.runs or len(candidate) != args.runs:
                failures += 1
                continue
            change = (statistics.median(b / a for a, b in zip(before, candidate)) - 1) * 100
            summary[name] = {
                "before_median": statistics.median(before),
                "candidate_median": statistics.median(candidate),
                "median_paired_pct": change, "pass": change <= 1,
            }
            failures += change > 1
        print(json.dumps(summary, indent=2))
        (root / "instruction-gate.json").write_text(json.dumps(summary, indent=2) + "\n")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
