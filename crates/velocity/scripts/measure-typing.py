#!/usr/bin/env python3
"""Frozen, alternating process pairs for the typing regression gate (no GUI/GPU)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import shutil
import statistics
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--runs', type=int, default=10)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    if args.runs < 10:
        parser.error('at least ten alternating pairs required')
    repo = Path(__file__).resolve().parents[3]
    output = args.output_dir.resolve()
    output.mkdir(parents=True)
    binaries = {}
    for label in ['before', 'after']:
        binaries[label] = output / f'velocity-{label}'
        shutil.copy2(getattr(args, label), binaries[label])
    environment = {
        'platform': platform.platform(),
        'source_sha256': {str(p.relative_to(repo)): hashlib.sha256(p.read_bytes()).hexdigest()
                          for crate in ['velocity', 'vel-ui']
                          for p in sorted((repo / 'crates' / crate).rglob('*'))
                          if p.is_file() and p.suffix in ['.rs', '.toml', '.sublime-settings', '.sublime-keymap']},
        'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
        'binary_sha256': {k: hashlib.sha256(p.read_bytes()).hexdigest() for k, p in binaries.items()},
        'schedule': 'AB, BA, repeated; one process at a time; no concurrent compilation',
        'gui_started': False, 'gpu_started': False,
        'runner_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'cargo_lock_sha256': hashlib.sha256((repo / 'Cargo.lock').read_bytes()).hexdigest(),
    }
    shutil.copy2(Path(__file__), output / 'measure-typing.py')
    (output / 'environment.json').write_text(json.dumps(environment, indent=2) + '\n')
    records = {'before': [], 'after': []}
    env = dict(os.environ)
    env.pop('VELOCITY_REVIEW_PROFILE', None)
    for pair in range(args.runs):
        for label in (['before', 'after'] if pair % 2 == 0 else ['after', 'before']):
            raw = output / f'{pair + 1:02}-{label}.jsonl'
            cmd = [str(binaries[label]), '--headless-intelligence', '--review-ui', 'on',
                   '--intel-words', '0', '--data-dir', str(output / f'profile-{pair}-{label}'),
                   '--output', str(raw), '--timeout', '300']
            with raw.with_suffix('.log').open('w') as log:
                subprocess.run(cmd, cwd=repo, env=env, stdout=log, stderr=subprocess.STDOUT,
                               timeout=330, check=True)
            rows = [json.loads(line) for line in raw.read_text().splitlines()]
            assert any(r.get('kind') == 'complete' and r.get('complete') for r in rows), raw
            record = next(r for r in rows if r.get('kind') == 'intelligence_cpu')
            records[label].append(record)
            print(pair + 1, label, record['typing_ms']['p50'], flush=True)
    result = {}
    for quantile in ['p50', 'p95']:
        before = [r['typing_ms'][quantile] for r in records['before']]
        after = [r['typing_ms'][quantile] for r in records['after']]
        ratios = [b / a for a, b in zip(before, after)]
        rng = random.Random(11)
        paired_boots, medians_boots = [], []
        for _ in range(20000):
            indices = rng.choices(range(args.runs), k=args.runs)
            paired_boots.append(statistics.median(ratios[i] for i in indices))
            medians_boots.append(statistics.median(after[i] for i in indices) /
                                 statistics.median(before[i] for i in indices))
        paired_boots.sort()
        medians_boots.sort()
        result[quantile] = {
            'before_ms': statistics.median(before), 'after_ms': statistics.median(after),
            'paired_change_percent': 100 * (statistics.median(ratios) - 1),
            'paired_ratio_ci95': [paired_boots[500], paired_boots[19500]],
            'ratio_of_medians': statistics.median(after) / statistics.median(before),
            'ratio_of_medians_ci95': [medians_boots[500], medians_boots[19500]],
            'pairs': list(zip(before, after)),
        }
    passed = max(result['p50']['paired_ratio_ci95'][1],
                 result['p50']['ratio_of_medians_ci95'][1]) <= 1.05
    summary = {'environment': environment, 'runs': args.runs, 'typing': result,
               'p50_ci_upper_within_5_percent': passed}
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary, indent=2), flush=True)
    if not passed:
        raise SystemExit('typing p50 CI upper bound exceeds +5%')


if __name__ == '__main__':
    main()
