#!/usr/bin/env python3
"""Add alternating scrolling pairs, retaining every observation from a normal-file series."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--previous', type=Path, required=True)
    parser.add_argument('--pairs', type=int, default=20)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    if args.pairs < 10 or args.pairs % 2:
        parser.error('use an even number of additional pairs, at least ten')
    previous = args.previous.resolve()
    summary = json.loads((previous / 'summary.json').read_text())
    original_pairs = summary['pairs']
    if not isinstance(original_pairs, int):
        parser.error('previous must be one original measure-normal.py series')
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    repo = Path(__file__).resolve().parents[3]
    binaries = {}
    for label in ['before', 'after']:
        source = previous / ('velocity-' + label)
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        if digest != summary['environment']['binary_sha256'][label]:
            parser.error('frozen binary changed: ' + label)
        binaries[label] = output / source.name
        shutil.copy2(source, binaries[label])
    frozen_runner = previous / 'measure-normal.py'
    if hashlib.sha256(frozen_runner.read_bytes()).hexdigest() != summary['environment']['runner_sha256']:
        parser.error('original runner changed')
    summarize = runpy.run_path(str(frozen_runner))['summarize']
    env = dict(os.environ)
    env.pop('VELOCITY_REVIEW_PROFILE', None)
    stats = {'before': [], 'after': []}
    for pair in range(original_pairs, original_pairs + args.pairs):
        order = ['before', 'after'] if pair % 2 == 0 else ['after', 'before']
        for label in order:
            raw = output / f'scroll-{pair+1:02}-{label}.jsonl'
            command = [str(binaries[label]), '--headless-view-features', '--data-dir',
                       str(output / f'profile-scroll-{pair}-{label}'), '--output', str(raw),
                       '--timeout', '300']
            with raw.with_suffix('.log').open('w') as log:
                subprocess.run(command, cwd=repo, env=env, stdout=log,
                               stderr=subprocess.STDOUT, timeout=330, check=True)
            rows = [json.loads(line) for line in raw.read_text().splitlines()]
            assert any(row.get('kind') == 'complete' and row.get('complete') for row in rows)
            record = next(row for row in rows if row.get('kind') == 'highlight_minimap_scroll_cpu')
            value = record['stats_ms']
            assert value['count'] >= 500
            stats[label].append(value)
            print('scroll', pair + 1, label, value['p50'], value['p95'], flush=True)
    combined = {}
    for quantile in ['p50', 'p95']:
        pairs = summary['scroll'][quantile]['pairs']
        before = [pair[0] for pair in pairs] + [row[quantile] for row in stats['before']]
        after = [pair[1] for pair in pairs] + [row[quantile] for row in stats['after']]
        combined[quantile] = summarize(before, after)
    combined['p50_ci_upper_within_5_percent'] = max(
        combined['p50']['paired_ratio_ci95'][1],
        combined['p50']['ratio_of_medians_ci95'][1]) <= 1.05
    summary['scroll'] = combined
    summary['pairs'] = {'typing': original_pairs, 'scroll': original_pairs + args.pairs}
    summary['extension'] = {'previous': str(previous), 'additional_scroll_pairs': args.pairs,
                            'all_previous_samples_retained': True,
                            'reason': 'initial scroll p50 confidence upper bound exceeded +5%',
                            'runner_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                            'gui_started': False, 'gpu_started': False}
    summary['passed'] = all(summary[name]['p50_ci_upper_within_5_percent'] for name in ['typing', 'scroll'])
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps({name: summary[name] for name in ['pairs', 'typing', 'scroll', 'passed']}, indent=2), flush=True)
    if not summary['passed']:
        raise SystemExit('pooled normal-file p50 CI upper bound exceeds +5%')


if __name__ == '__main__':
    main()
