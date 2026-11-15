#!/usr/bin/env python3
"""Reproduce five independent release runs; fixtures are isolated temporary Git repos."""
import argparse
import json
import math
import shutil
import statistics
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def make_fixtures():
    base = Path(tempfile.mkdtemp(prefix="velocity-r7-git-"))
    fixtures = []
    for name in ["sublime-packages", "generated-50000"]:
        folder = base / name
        if name == "sublime-packages":
            shutil.copytree(ROOT / "third_party/sublime-packages", folder,
                            ignore=shutil.ignore_patterns(".git"))
        else:
            folder.mkdir()
            for directory in range(100):
                part = folder / f"folder-{directory:03d}"
                part.mkdir()
                for index in range(500):
                    (part / f"file-{index:04d}.txt").write_text(
                        f"fixture {directory} {index}: tracked file contents with a stable baseline.\n")

        def git(*args):
            return subprocess.run(["git", "-C", str(folder), "-c", "user.name=Fixture",
                                   "-c", "user.email=fixture@example.invalid", "-c",
                                   "commit.gpgsign=false", *args], check=True, capture_output=True)
        git("init", "-b", "main")
        git("add", ".")
        git("commit", "-m", "benchmark fixture")
        files = git("ls-files", "-z").stdout.split(b"\0")[:-1]
        fixtures.append(dict(name=name, path=str(folder), files=len(files), modify=files[0].decode()))
    return fixtures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--fixtures-json", type=Path)
    args = parser.parse_args()
    assert args.runs >= 5
    fixtures = json.loads(args.fixtures_json.read_text()) if args.fixtures_json else make_fixtures()
    outputs = {name: [] for name in ["vel-buffer", "vel-diff", "vel-spell", "vel-git"]}
    for run in range(1, args.runs + 1):
        for crate, binary in [("vel-buffer", "history_bench"), ("vel-diff", "diff_bench"),
                              ("vel-spell", "spell_bench")]:
            result = subprocess.run([str(ROOT / "target/release/examples" / binary)],
                                    check=True, capture_output=True, text=True)
            for line in result.stdout.splitlines():
                row = json.loads(line)
                outputs[crate].append(dict(run=run, **row))
        for fixture in fixtures:
            result = subprocess.run([str(ROOT / "target/release/examples/git_bench"),
                                     fixture["path"], fixture["modify"]],
                                    check=True, capture_output=True, text=True)
            outputs["vel-git"].append(dict(run=run, fixture=fixture["name"], files=fixture["files"],
                                           **json.loads(result.stdout)))
        print(f"Completed run {run}/{args.runs}", flush=True)
    for crate, rows in outputs.items():
        (ROOT / "crates" / crate / "round7-timings.json").write_text(json.dumps(rows, indent=2) + "\n")

    def stats(values):
        return dict(median=statistics.median(values), p95=sorted(values)[math.ceil(len(values) * .95)-1],
                    samples=len(values))
    summary = {}
    for case in ["one_byte", "100_byte"]:
        rows = [r for r in outputs["vel-buffer"] if r["case"] == case]
        summary["history_" + case] = dict(export_ns=stats([r["export_ns"] for r in rows]),
                                         serialize_ns=stats([r["serialize_ns"] for r in rows]),
                                         serialized_bytes=max(r["serialized_bytes"] for r in rows),
                                         raw_edit_bytes=rows[0]["raw_edit_bytes"])
    for crate, initial in [("vel-diff", "initial_ns"), ("vel-spell", "viewport_ns")]:
        rows = outputs[crate]
        summary[crate] = {initial: stats([r[initial] for r in rows]),
                          "edit_ns": stats([n for r in rows for n in r["edit_ns"]])}
    for fixture in fixtures:
        rows = [r for r in outputs["vel-git"] if r["fixture"] == fixture["name"]]
        summary["git_" + fixture["name"]] = dict(files=fixture["files"],
                                                  cold_ns=stats([r["cold_ns"] for r in rows]),
                                                  warm_ns=stats([r["warm_ns"] for r in rows]))
    (ROOT / "crates/vel-git/round7-summary.json").write_text(json.dumps(summary, indent=2)+"\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
