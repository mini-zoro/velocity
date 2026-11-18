#!/usr/bin/env python3
"""Pair the ignored App keystroke harness in prebuilt test executables; never starts a GUI.

Inject the identical app_lane_b_round19_perf.rs module into comparison revisions.
All compared App::scene implementations use normal native font initialization.
This measures warmed keys with native metrics, never startup.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--base", type=Path, required=True, help="385dcdd test executable")
parser.add_argument("--before", type=Path, help="round-18 test executable")
parser.add_argument("--candidate", type=Path, required=True)
parser.add_argument("--pairs", type=int, default=9)
parser.add_argument("--output", type=Path)
args = parser.parse_args()
assert args.pairs > 0
root = args.output or Path(tempfile.mkdtemp(prefix="velocity-app-keys-"))
root.mkdir(parents=True, exist_ok=True)
binaries = {"main": args.base.resolve()}
if args.before:
    binaries["before"] = args.before.resolve()
binaries["candidate"] = args.candidate.resolve()
hashes = {key: hashlib.sha256(path.read_bytes()).hexdigest() for key, path in binaries.items()}
# A future harness may warm the login provider. Each child still receives a
# private HOME/SHELL without mutating this process or the developer's profiles.
login = root.resolve() / "login"
login.mkdir(exist_ok=True)
shell = login / "login-shell"
shell.write_text('#!/bin/sh\nexec /bin/sh -c "$3"\n')
shell.chmod(0o700)
environment = {**os.environ, "HOME": str(login), "SHELL": str(shell), "ZDOTDIR": str(login)}
for key in ["BASH_ENV", "ENV"]:
    environment.pop(key, None)
records = []
for pair in range(args.pairs):
    labels = list(binaries)
    if pair % 2:
        labels.reverse()
    for label in labels:
        result = subprocess.run(
            [str(binaries[label]), "--ignored", "--exact", "app::lane_b_round19_perf::app_key_cost", "--nocapture"],
            env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, timeout=180, check=True,
        )
        (root / f"{pair + 1:02}-{label}.log").write_text(result.stdout)
        rows = [line.removeprefix("APP_KEY_COST ") for line in result.stdout.splitlines()
                if line.startswith("APP_KEY_COST ")]
        assert len(rows) == 1, f"missing/ambiguous App measurement: {label}"
        records.append({"pair": pair, "label": label, "result": json.loads(rows[0])})
summary = {}
for case in ["1", "2"]:
    values = {label: statistics.median(row["result"][case]["instructions_per_key"]
                                      for row in records if row["label"] == label)
              for label in binaries}
    values["after_delta_pct"] = 100 * (values["candidate"] / values["main"] - 1)
    if args.before:
        values["before_delta_pct"] = 100 * (values["before"] / values["main"] - 1)
    values["pass_1_5pct"] = values["after_delta_pct"] <= 1.5
    summary[case] = values
for label, path in binaries.items():
    assert hashlib.sha256(path.read_bytes()).hexdigest() == hashes[label], f"binary changed during pairing: {path}"
report = {
    "boundary": "Calling-thread retired instructions through App dispatch_key_binding, committed input or named command, changed_state and App::scene per key; native font metrics; 200 warm keys excluded, 1000 measured alternating type/delete keys; two shared-buffer views and actual wrapped rows; no window/renderer. Whole-process per-key counts are also retained, including background workers.",
    "pairs": args.pairs, "records": records, "summary": summary,
    "binaries": {label: {"path": str(path), "sha256": hashes[label]} for label, path in binaries.items()},
}
(root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(summary, indent=2))
raise SystemExit(0 if all(case["pass_1_5pct"] for case in summary.values()) else 1)
