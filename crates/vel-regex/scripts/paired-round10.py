#!/usr/bin/env python3
"""Alternate complete production workers against the unmodified main build.

Build worker_probe in both checkouts, copying only that example into the baseline.
Both persistent processes warm once; exactly one worker runs at a time. Report
95% percentile bootstrap CIs for medians and paired speedup, resampling whole
pairs (20,000 draws, fixed seed). All samples, input and binary digests are saved.
"""
import argparse
import hashlib
import json
import random
import statistics
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
FIXTURES = {
    "rust": ("Rust", "spikes/data/rust-10mb.rs"),
    "javascript": ("JavaScript", "spikes/data/javascript-10mb.js"),
    "cpp": ("C++", "prototypes/sregex/data/cpp-5mb.cpp"),
    "python": ("Python", "prototypes/sregex/data/python-5mb.py"),
    "json": ("JSON", "spikes/data/json-20mb.json"),
    "markdown": ("Markdown", "spikes/data/markdown-5mb.md"),
    "yaml": ("YAML", "prototypes/sregex/data/yaml-5mb.yaml"),
    "html": ("HTML", "prototypes/sregex/data/html-5mb.html"),
}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_digest(root):
    digest = hashlib.sha256()
    for name in ("vel-regex", "vel-scope", "vel-syntax", "vel-highlight"):
        crate = root / "crates" / name
        for path in sorted([crate / "Cargo.toml", *crate.joinpath("src").rglob("*.rs")]):
            digest.update(str(path.relative_to(root)).encode() + b"\0" + path.read_bytes())
    return digest.hexdigest()


def interval(samples):
    ordered = sorted(samples)
    return [ordered[int(.025 * len(ordered))], ordered[int(.975 * len(ordered))]]


def summarize(pairs, size):
    left = [size / 1e6 / p["before"]["seconds"] for p in pairs]
    right = [size / 1e6 / p["after"]["seconds"] for p in pairs]
    ratios = [b / a for a, b in zip(left, right)]
    rng = random.Random(1031)
    medians = [[], [], []]
    for _ in range(20000):
        indices = rng.choices(range(len(pairs)), k=len(pairs))
        for target, values in zip(medians, (left, right, ratios)):
            target.append(statistics.median(values[i] for i in indices))
    return {name: {"median": statistics.median(values), "ci95": interval(boot)}
            for name, values, boot in zip(
                ("before_mb_per_second", "after_mb_per_second", "paired_speedup"),
                (left, right, ratios), medians)}


class Probe:
    def __init__(self, binary, definitions, fixture, syntax):
        self.process = subprocess.Popen(
            [str(binary), str(definitions), str(fixture), syntax],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        self.ready = json.loads(self.process.stdout.readline())

    def run(self):
        self.process.stdin.write("run\n")
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError(f"probe exited: {self.process.wait()}")
        return json.loads(line)

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.write("quit\n")
            self.process.stdin.flush()
        if self.process.wait() != 0:
            raise RuntimeError("probe failed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, default=ROOT / "target/release/examples/worker_probe")
    parser.add_argument("--data", type=Path, default=ROOT)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--languages", nargs="+", default=["rust", "javascript", "cpp"])
    parser.add_argument("--pairs", type=int, default=10)
    args = parser.parse_args()
    if args.pairs < 2:
        parser.error("--pairs must be at least 2")
    definitions = ROOT / "third_party/sublime-packages"
    result = {"case": "alternating_full_production_workers", "pairs": args.pairs,
              "bootstrap": {"draws": 20000, "seed": 1031, "unit": "whole_pair", "interval": "percentile_95"},
              "before_binary_sha256": sha(args.before), "after_binary_sha256": sha(args.after),
              "after_source_sha256": source_digest(ROOT),
              "baseline_commit": "fcab5c3a372ef63c67866d61cc11ffd080ca0b0c", "timings": []}
    for language in args.languages:
        syntax, file = FIXTURES[language]
        fixture = args.data / file
        probes = {}
        try:
            # Warm sequentially; a warmup is excluded from every reported sample.
            for name, binary in (("before", args.before), ("after", args.after)):
                probes[name] = Probe(binary, definitions, fixture, syntax)
            assert probes["before"].ready == probes["after"].ready
            pairs = []
            for run in range(args.pairs):
                order = ["before", "after"] if run % 2 == 0 else ["after", "before"]
                pair = {"order": order}
                for name in order:
                    pair[name] = probes[name].run()
                pairs.append(pair)
                print(f'{language} pair {run + 1}: A {pair["before"]["seconds"]:.6f}s B {pair["after"]["seconds"]:.6f}s', flush=True)
            row = {"language": language, "file": file, "sha256": sha(fixture), **probes["before"].ready, "samples": pairs,
                   **summarize(pairs, fixture.stat().st_size)}
            result["timings"].append(row)
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps({k: row[k] for k in ("language", "before_mb_per_second", "after_mb_per_second", "paired_speedup")}), flush=True)
        finally:
            for probe in probes.values():
                probe.close()


if __name__ == "__main__":
    main()
