#!/usr/bin/env python3
"""Pair the old single-root loader over a staged tree with direct virtual loading.

Pass roots_probe built from main with its two load_cached_roots calls changed to
load_cached, and the unchanged candidate roots_probe. Staging is outside timers.
Each process measures a cold binary-cache miss, then 20 full content-hashed hits.
"""
import argparse
import json
import os
from pathlib import Path
import random
import statistics
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, default=ROOT / "target/release/examples/roots_probe")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pairs", type=int, default=5)
    args = parser.parse_args()
    samples = []
    with tempfile.TemporaryDirectory(prefix="velocity-round10-stage-") as scratch:
        stage = Path(scratch)
        packages = ROOT / "third_party/sublime-packages"
        for path in sorted(packages.rglob("*")):
            if path.is_file() and (path.suffix in (".sublime-syntax", ".tmLanguage") or path.name.endswith(".tmLanguage.json")):
                destination = stage / path.relative_to(packages)
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.symlink_to(path)
        fallback = stage / "Default/Plain Text.sublime-syntax"
        fallback.parent.mkdir(exist_ok=True)
        fallback.unlink(missing_ok=True)
        fallback.write_text("name: Plain Text\nfile_extensions: [txt]\nscope: text.plain\ncontexts: { main: [] }\n")
        for run in range(args.pairs):
            order = ["before", "after"] if run % 2 == 0 else ["after", "before"]
            pair = {"order": order}
            for name in order:
                env = dict(os.environ)
                env.pop("VEL_SYNTAX_ROOT", None)
                if name == "before":
                    env["VEL_SYNTAX_ROOT"] = str(stage)
                binary = args.before if name == "before" else args.after
                pair[name] = json.loads(subprocess.check_output([str(binary)], env=env, text=True))
            assert pair["before"]["syntaxes"] == pair["after"]["syntaxes"] == 204
            samples.append(pair)
            print(f'loader pair {run + 1}: cold A {pair["before"]["cold_build_ms"]:.2f}ms B {pair["after"]["cold_build_ms"]:.2f}ms', flush=True)
    result = {"case": "alternating_content_hashed_loading", "staging_timed": False, "samples": samples}
    rng = random.Random(1017)
    for field in ("cold_build_ms", "warm_ms_including_content_hashes"):
        values = {name: [p[name][field] if field == "cold_build_ms" else statistics.median(p[name][field]) for p in samples] for name in ("before", "after")}
        boot = {name: [] for name in values}
        for _ in range(20000):
            indices = rng.choices(range(len(samples)), k=len(samples))
            for name in values:
                boot[name].append(statistics.median(values[name][i] for i in indices))
        result[field] = {}
        for name, vals in values.items():
            dist = sorted(boot[name])
            result[field][name] = {"median": statistics.median(vals), "ci95": [dist[500], dist[19500]]}
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "samples"}))


if __name__ == "__main__":
    main()
