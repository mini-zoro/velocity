#!/usr/bin/env python3
"""Paired bootstrap intervals over processes, never individual query samples."""
import argparse
import collections
import json
import math
from pathlib import Path
import random
import statistics as st

p = argparse.ArgumentParser()
p.add_argument('files', nargs='+', type=Path)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
groups = collections.defaultdict(dict)
for file in a.files:
    for row in map(json.loads, file.read_text().splitlines()):
        operation = row['operation']
        key = f"{row['case']}/{operation}"
        if 'ns' in row:
            samples, unit = row['ns'], 'ns'
        elif 'total_ns' in row:
            samples, unit = [row['total_ns'] / row['count']], 'ns'
        else:
            samples, unit = [row['bytes'] / row['count']], 'bytes'
        groups[key][(row['pair'], row['side'])] = (samples, unit)
        if 'init_ns' in row:
            groups[f"{row['case']}/construction"][(row['pair'], row['side'])] = ([row['init_ns']], 'ns')


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def summarize(records):
    pairs = sorted({pair for pair, _ in records})
    assert all((pair, side) in records for pair in pairs for side in 'AB')
    ratios = [st.median(records[pair, 'B'][0]) / max(1e-12, st.median(records[pair, 'A'][0])) for pair in pairs]
    rng = random.Random(110011)
    draws = [st.median(rng.choices(ratios, k=len(ratios))) for _ in range(20000)]
    out = {
        'pairs': len(pairs), 'unit': next(iter(records.values()))[1],
        'paired_change_percent': (st.median(ratios) - 1) * 100,
        'paired_ratio_ci95': [percentile(draws, .025), percentile(draws, .975)],
    }
    for side in 'AB':
        values = [value for pair in pairs for value in records[pair, side][0]]
        out[side] = {'median': st.median(values), 'p95': percentile(values, .95), 'max': max(values), 'samples': len(values)}
    return out


result = {key: summarize(records) for key, records in groups.items()}
a.output.write_text(json.dumps(result, indent=2) + '\n')
for key, row in result.items():
    print(f"{key}: A {row['A']['median']:.3f} B {row['B']['median']:.3f} {row['unit']}; paired {row['paired_change_percent']:+.2f}% CI {row['paired_ratio_ci95']}")
