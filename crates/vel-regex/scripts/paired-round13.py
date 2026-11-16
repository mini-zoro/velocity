#!/usr/bin/env python3
"""Alternate unchanged-baseline/candidate binaries; bootstrap whole process pairs.

Worker mode reuses the round-10 persistent protocol. Loading runs identical direct
roots (including the virtual fallback) on both sides. Export runs identical 10k
highlighted lines. Buffer runs the real perf harness worker on the 100 MB fixture.
No builds/tests should run concurrently with these measurements.
"""
import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import random
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("worker_pairs", Path(__file__).with_name("paired-round10.py"))
WORKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WORKER)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def quantile(values, q):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(q * len(ordered)) - 1)]


def describe(pairs, field, statistic=statistics.median):
    """Keep dependent within-process observations together in every bootstrap draw."""
    rng = random.Random(1313)
    values = {name: [p[name][field] for p in pairs] for name in ("before", "after")}
    pooled = lambda groups: [x for group in groups for x in (group if isinstance(group, list) else [group])]
    boots = {name: [] for name in values}
    ratios = []
    for _ in range(20000):
        indices = rng.choices(range(len(pairs)), k=len(pairs))
        for name in values:
            boots[name].append(statistic(pooled([values[name][i] for i in indices])))
        ratios.append(statistics.median(
            statistic(pooled([values["before"][i]])) / statistic(pooled([values["after"][i]])) for i in indices))
    result = {name: {"estimate": statistic(pooled(groups)), "ci95": [quantile(boots[name], .025), quantile(boots[name], .975)]}
              for name, groups in values.items()}
    result["paired_speedup"] = {"estimate": statistics.median(
        statistic(pooled([a])) / statistic(pooled([b])) for a, b in zip(values["before"], values["after"])),
        "ci95": [quantile(ratios, .025), quantile(ratios, .975)]}
    return result


def describe_worker(pairs):
    times = describe(pairs, "seconds")
    rates = describe(pairs, "mb_per_second")
    return {**{name: {"median_mb_s": rates[name]["estimate"], "ci95": rates[name]["ci95"]} for name in ("before", "after")},
            "paired_speedup": times["paired_speedup"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=("worker", "loading", "export", "buffer"))
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, required=True)
    parser.add_argument("--baseline-commit", required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pairs", type=int, default=10)
    parser.add_argument("--languages", nargs="+", default=list(WORKER.FIXTURES))
    parser.add_argument("--cases", nargs="+", help="buffer scenarios for a targeted follow-up")
    args = parser.parse_args()
    if args.pairs < 5:
        parser.error("at least five complete pairs are required")
    if args.cases and args.kind != "buffer":
        parser.error("--cases applies only to buffer measurements")
    result = {"kind": args.kind, "baseline_commit": args.baseline_commit,
              "before_sha256": sha(args.before), "after_sha256": sha(args.after),
              "worker_source_sha256": WORKER.source_digest(ROOT),
              "bootstrap": {"draws": 20000, "seed": 1313, "unit": "whole_pair", "interval": "percentile_95"}, "cases": []}
    if args.kind == "worker":
        for language in args.languages:
            syntax, relative = WORKER.FIXTURES[language]
            fixture = args.data / relative
            probes = {}
            try:
                for name, binary in (("before", args.before), ("after", args.after)):
                    probes[name] = WORKER.Probe(binary.resolve(), ROOT / "third_party/sublime-packages", fixture, syntax)
                assert probes["before"].ready == probes["after"].ready
                pairs = []
                for i in range(args.pairs):
                    order = ["before", "after"] if i % 2 == 0 else ["after", "before"]
                    pair = {"order": order}
                    for name in order:
                        pair[name] = probes[name].run()
                    pairs.append(pair)
                    print(f'{language} pair {i+1}: A {pair["before"]["seconds"]:.4f}s B {pair["after"]["seconds"]:.4f}s', flush=True)
                summary = describe_worker(pairs)
                result["cases"].append({"name": language, "input_sha256": sha(fixture), **probes["before"].ready, "pairs": pairs, "summary": summary})
            finally:
                for probe in probes.values():
                    probe.close()
            args.output.write_text(json.dumps(result, indent=2) + "\n")
    else:
        cases = [args.kind] if args.kind != "buffer" else ["type_10000", "type_100000", "find_10000", "undo_10000", "delete_50mb", "undo_50mb", "map_100000", "undo_metadata"]
        if args.cases:
            if not set(args.cases) <= set(cases):
                parser.error("unknown buffer scenario")
            cases = args.cases
        for case in cases:
            pairs = []
            for i in range(args.pairs):
                order = ["before", "after"] if i % 2 == 0 else ["after", "before"]
                pair = {"order": order}
                for name in order:
                    binary = args.before if name == "before" else args.after
                    env = dict(os.environ)
                    env.pop("VEL_SYNTAX_ROOT", None)
                    command = [str(binary.resolve())]
                    if args.kind == "buffer":
                        command += ["--worker", case, str(args.data)]
                        env["VEL_BUFFER_BENCH_RUN"] = str(i+1)
                    measured = json.loads(subprocess.check_output(command, env=env, text=True))
                    if args.kind == "loading":
                        assert measured["syntaxes"] == 204
                    if args.kind == "export":
                        # Export's writer has dropped; validate the delivered file too.
                        path = Path(measured["print_file"])
                        assert path.stat().st_size == measured["html_bytes"]
                        measured["html_sha256"] = sha(path)
                        path.unlink()
                    pair[name] = measured
                if args.kind == "export":
                    assert pair["before"]["html_sha256"] == pair["after"]["html_sha256"]
                pairs.append(pair)
                print(f'{case} pair {i+1} complete', flush=True)
            if args.kind == "loading":
                summary = {"cold_ms": describe(pairs, "cold_build_ms"),
                           "warm_median_ms": describe(pairs, "warm_ms_including_content_hashes"),
                           "warm_p95_ms": describe(pairs, "warm_ms_including_content_hashes", lambda xs: quantile(xs, .95))}
            elif args.kind == "export":
                summary = {"median_ms": describe(pairs, "milliseconds"), "p95_ms": describe(pairs, "milliseconds", lambda xs: quantile(xs, .95))}
            else:
                field = "total_ns" if "total_ns" in pairs[0]["after"] else "bytes"
                summary = {field: describe(pairs, field)}
            result["cases"].append({"name": case, "pairs": pairs, "summary": summary})
            args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({c["name"]: c["summary"] for c in result["cases"]}), flush=True)


if __name__ == "__main__":
    main()
