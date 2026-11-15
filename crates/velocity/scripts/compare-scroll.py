#!/usr/bin/env python3
"""Alternate isolated old/new CPU view processes; bootstrap run medians, keep raw data."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import platform
import random
import shutil
import statistics
import subprocess


def interval(values):
    rng = random.Random(8)
    samples = sorted(statistics.median(rng.choices(values, k=len(values)))
                     for _ in range(10000))
    return [samples[249], samples[9749]]


def main():
    repo = Path(__file__).resolve().parents[3]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--old', type=Path, required=True)
    parser.add_argument('--new', type=Path, required=True)
    parser.add_argument('--pairs', type=int, default=10)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    if args.pairs < 10:
        parser.error('at least ten pairs required')
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    environment = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                   'platform': platform.platform(), 'gui_started': False,
                   'workload': '--headless-view-features; identical default Rust 10MB fixture',
                   'fixture_sha256': hashlib.sha256((repo/'spikes/data/rust-10mb.rs').read_bytes()).hexdigest(),
                   'schedule': 'alternating old/new, reversed on odd pairs; one child at a time',
                   'confidence': '95% bootstrap of independent process scroll p50s, seed 8',
                   'binaries': {}}
    for label, source in [('old', args.old), ('new', args.new)]:
        binary = output / ('velocity-' + label)
        shutil.copy2(source, binary)
        environment['binaries'][label] = {'source': str(source),
            'sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
    (output / 'environment.json').write_text(json.dumps(environment, indent=2) + '\n')
    runs = {'old': [], 'new': []}
    print(output, flush=True)
    for pair in range(args.pairs):
        for label in (['old', 'new'] if pair % 2 == 0 else ['new', 'old']):
            path = output / f'{label}-{pair + 1}.jsonl'
            with path.with_suffix('.log').open('w') as log:
                subprocess.run([str(output / ('velocity-' + label)),
                    '--headless-view-features', '--output', str(path)],
                    cwd=repo, stdout=log, stderr=subprocess.STDOUT,
                    timeout=240, check=True)
            rows = [json.loads(line) for line in path.read_text().splitlines()]
            row = next(r for r in rows if r['kind'] == 'highlight_minimap_scroll_cpu')
            runs[label].append(row['stats_ms'])
            print(label, pair + 1, row['stats_ms'], flush=True)
    summary = {'environment': environment, 'runs': runs}
    for label in runs:
        medians = [r['p50'] for r in runs[label]]
        summary[label] = {'median_process_p50_ms': statistics.median(medians),
                          'p50_95pct_ci_ms': interval(medians),
                          'median_process_p95_ms': statistics.median(r['p95'] for r in runs[label])}
    delta = 100 * (summary['new']['median_process_p50_ms'] /
                   summary['old']['median_process_p50_ms'] - 1)
    nonoverlap = summary['new']['p50_95pct_ci_ms'][0] > summary['old']['p50_95pct_ci_ms'][1]
    summary.update({'p50_change_percent': delta, 'regression_over_5pct_nonoverlapping':
                    delta > 5 and nonoverlap})
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary, indent=2), flush=True)


if __name__ == '__main__':
    main()
