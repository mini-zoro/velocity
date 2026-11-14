#!/usr/bin/env python3
"""Serial, five-run production measurements. Never calls sysctl."""
import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

CRATE = Path(__file__).resolve().parents[1]
ROOT = CRATE.parents[1]
BIN = ROOT / "target/release/regex-bench"
HIGHLIGHT = ROOT / "target/release/examples/highlight_bench"


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], cwd=ROOT, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--phase", choices=["all", "diff", "throughput", "profile", "cold", "highlight", "parser"], default="all")
    args = parser.parse_args()
    run("cargo", "build", "--release", "-p", "vel-syntax", "--bins", "-p", "vel-highlight", "--example", "highlight_bench")
    if args.phase in ("all", "diff"):
        run(BIN, "--diff-only", "--verify-matches", "--output", CRATE / "differential.json")
        data = json.loads((CRATE / "differential.json").read_text())
        data["official"] = []
        (CRATE / "large-differential.json").write_text(json.dumps(data, indent=2) + "\n")
    if args.phase in ("all", "throughput"):
        run(BIN, "--bench-only", "--output", CRATE / "throughput.json")
    if args.phase in ("all", "profile"):
        run(BIN, "--bench-only", "--quick", "--profile", "--output", CRATE / "contexts.json")
        run(BIN, "--contexts-only", "--output", CRATE / "focused-contexts.json")
    if args.phase in ("all", "cold"):
        runs = []
        with tempfile.TemporaryDirectory(prefix="velocity-regex-cold-") as temporary:
            for index in range(5):
                path = Path(temporary) / f"{index}.json"
                command = [str(BIN), "--cold-only", "--output", str(path)]
                # macOS /usr/bin/time -l calls blocked sysctl kern.clockrate.
                # wait4 gives this child's exact rusage without querying sysctl.
                usage = None
                if hasattr(os, "wait4"):
                    child = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    _, status, usage = os.wait4(child.pid, 0)
                    child.returncode = os.waitstatus_to_exitcode(status)
                    stdout, stderr = child.communicate()
                    if child.returncode:
                        raise RuntimeError(f"cold process failed: {stdout}\n{stderr}")
                else:
                    run(*command, capture_output=True, text=True)
                data = json.loads(path.read_text())
                data["rss_bytes"] = usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024) if usage else None
                runs.append(data)
        (CRATE / "compile.json").write_text(json.dumps({"runs": runs}, indent=2) + "\n")
    if args.phase in ("all", "highlight"):
        run(HIGHLIGHT)
        run(HIGHLIGHT, "--background-only")
        run(HIGHLIGHT, "--worker-edits-only")
    if args.phase in ("all", "parser"):
        run(ROOT / "target/release/syntax-bench")


if __name__ == "__main__":
    main()
