#!/usr/bin/env python3
"""Alternating headless CPU pairs; collect macOS instructions before reaping."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--before", type=Path, required=True)
parser.add_argument("--after", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--pairs", type=int, default=9)
parser.add_argument("--baseline", default="5497418")
args = parser.parse_args()
args.output.mkdir(parents=True)
root = args.output.resolve()
home = root / "home"
home.mkdir()
env = dict(os.environ, HOME=str(home), SHELL="/bin/sh", VELOCITY_CACHE_DIR=str(root / "cache"),
           VELOCITY_LOG_DIR=str(root / "logs"), TMPDIR=str(root))
sdk = subprocess.check_output(["xcrun", "--sdk", "macosx", "--show-sdk-path"], text=True).strip()
header = (Path(sdk) / "usr/include/sys/resource.h").read_text()
body = header.split("struct rusage_info_v4 {", 1)[1].split("};", 1)[0]

class Usage(ctypes.Structure):
    _fields_ = [
        (name, (ctypes.c_uint8 * int(length) if length else ctypes.c_uint8)
         if ty == "uint8_t" else ctypes.c_uint64)
        for ty, name, length in re.findall(r"(uint8_t|uint64_t)\s+(ri_\w+)(?:\[(\d+)\])?;", body)
    ]

proc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
proc.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
proc.proc_pid_rusage.restype = ctypes.c_int
bins = {label: getattr(args, label).resolve() for label in ("before", "after")}
modes = {
    "typing1": ["--headless-smoke", "--carets", "1"],
    "typing1000": ["--headless-smoke", "--carets", "1000"],
    "features": ["--headless-view-features"],
}
samples = {mode: {label: [] for label in bins} for mode in modes}
for pair in range(args.pairs):
    for label in (["before", "after"] if pair % 2 == 0 else ["after", "before"]):
        for mode, flags in modes.items():
            name = f"{pair:02}-{label}-{mode}"
            records = root / f"{name}.jsonl"
            started = time.monotonic()
            usage = Usage()
            with (root / f"{name}.log").open("w") as log:
                child = subprocess.Popen([str(bins[label]), *flags, "--data-dir", str(root / name),
                                          "--output", str(records)],
                                         env=env, stdout=log, stderr=subprocess.STDOUT,
                                         stdin=subprocess.DEVNULL)
                try:
                    os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOWAIT)
                    if proc.proc_pid_rusage(child.pid, 4, ctypes.byref(usage)):
                        raise OSError(ctypes.get_errno(), "proc_pid_rusage")
                finally:
                    code = child.wait()
            assert code == 0 and usage.ri_instructions > 0, name
            assert any(json.loads(line)["kind"] == "complete"
                       for line in records.read_text().splitlines()), name
            samples[mode][label].append({"instructions": usage.ri_instructions,
                                         "seconds": time.monotonic() - started,
                                         "load": os.getloadavg()})
            print(name, usage.ri_instructions, flush=True)
summary = {}
for mode, data in samples.items():
    base = [s["instructions"] for s in data["before"]]
    current = [s["instructions"] for s in data["after"]]
    summary[mode] = {
        "before": statistics.median(base), "after": statistics.median(current),
        "paired_percent": (statistics.median(b / a for a, b in zip(base, current)) - 1) * 100,
    }
(root / "report.json").write_text(json.dumps({
    "baseline": args.baseline, "pairs": args.pairs, "samples": samples, "summary": summary,
    "sha256": {label: hashlib.sha256(path.read_bytes()).hexdigest() for label, path in bins.items()},
}, indent=2) + "\n")
print(json.dumps(summary, indent=2))
