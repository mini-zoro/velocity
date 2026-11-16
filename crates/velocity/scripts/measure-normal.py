#!/usr/bin/env python3
"""Frozen serial AB/BA process pairs for the normal-file typing and scroll gates."""
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


def summarize(before, after):
    ratios = [b / a for a, b in zip(before, after)]
    rng = random.Random(12)
    paired, marginal = [], []
    for _ in range(20_000):
        indices = rng.choices(range(len(ratios)), k=len(ratios))
        paired.append(statistics.median(ratios[i] for i in indices))
        marginal.append(statistics.median(after[i] for i in indices) /
                        statistics.median(before[i] for i in indices))
    paired.sort()
    marginal.sort()
    return {'before_ms': statistics.median(before), 'after_ms': statistics.median(after),
            'paired_change_percent': 100 * (statistics.median(ratios) - 1),
            'paired_ratio_ci95': [paired[500], paired[19500]],
            'ratio_of_medians': statistics.median(after) / statistics.median(before),
            'ratio_of_medians_ci95': [marginal[500], marginal[19500]],
            'pairs': list(zip(before, after))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--pairs', type=int, default=10)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    if args.pairs < 10:
        parser.error('at least ten alternating pairs required')
    repo = Path(__file__).resolve().parents[3]
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binaries = {}
    for label in ['before', 'after']:
        binaries[label] = output / ('velocity-' + label)
        shutil.copy2(getattr(args, label), binaries[label])
    environment = {
        'platform': platform.platform(),
        'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
        'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
        'binary_sha256': {k: hashlib.sha256(p.read_bytes()).hexdigest() for k, p in binaries.items()},
        'runner_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'source_sha256': {str(p.relative_to(repo)): hashlib.sha256(p.read_bytes()).hexdigest()
                          for crate in ['velocity', 'vel-fileio']
                          for p in sorted((repo / 'crates' / crate).rglob('*'))
                          if p.is_file() and p.suffix in ['.rs', '.toml', '.sublime-settings', '.sublime-keymap', '.sublime-commands']},
        'cargo_lock_sha256': hashlib.sha256((repo/'Cargo.lock').read_bytes()).hexdigest(),
        'fixture_sha256': hashlib.sha256((repo/'spikes/data/rust-10mb.rs').read_bytes()).hexdigest(),
        'schedule': 'AB, BA repeated; independent processes; one process at a time; no concurrent compilation/tests',
        'confidence': '20,000 paired process bootstrap resamples, seed 12; both p50 CI upper bounds <=1.05',
        'gui_started': False, 'gpu_started': False,
    }
    (output/'environment.json').write_text(json.dumps(environment, indent=2)+'\n')
    shutil.copy2(Path(__file__), output/'measure-normal.py')
    env = dict(os.environ)
    env.pop('VELOCITY_REVIEW_PROFILE', None)
    summary = {'environment': environment, 'pairs': args.pairs}
    for suite in ['typing', 'scroll']:
        records = {'before': [], 'after': []}
        for pair in range(args.pairs):
            for label in (['before', 'after'] if pair % 2 == 0 else ['after', 'before']):
                raw = output/f'{suite}-{pair+1:02}-{label}.jsonl'
                command = [str(binaries[label])]
                if suite == 'typing':
                    command += ['--headless-intelligence', '--review-ui', 'on', '--intel-words', '0']
                else:
                    command += ['--headless-view-features']
                command += ['--data-dir', str(output/f'profile-{suite}-{pair}-{label}'),
                            '--output', str(raw), '--timeout', '300']
                with raw.with_suffix('.log').open('w') as log:
                    subprocess.run(command, cwd=repo, env=env, stdout=log, stderr=subprocess.STDOUT,
                                   timeout=330, check=True)
                rows = [json.loads(line) for line in raw.read_text().splitlines()]
                assert any(r.get('kind') == 'complete' and r.get('complete') for r in rows), raw
                kind = 'intelligence_cpu' if suite == 'typing' else 'highlight_minimap_scroll_cpu'
                record = next(r for r in rows if r.get('kind') == kind)
                stats = record['typing_ms' if suite == 'typing' else 'stats_ms']
                assert stats['count'] >= 500, (raw, stats)
                records[label].append(stats)
                print(suite, pair+1, label, stats['p50'], stats['p95'], flush=True)
        result = {}
        for quantile in ['p50', 'p95']:
            result[quantile] = summarize([r[quantile] for r in records['before']],
                                         [r[quantile] for r in records['after']])
        result['p50_ci_upper_within_5_percent'] = max(
            result['p50']['paired_ratio_ci95'][1], result['p50']['ratio_of_medians_ci95'][1]) <= 1.05
        summary[suite] = result
        (output/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    summary['passed'] = all(summary[s]['p50_ci_upper_within_5_percent'] for s in ['typing', 'scroll'])
    (output/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    print(json.dumps({k: v for k, v in summary.items() if k != 'environment'}, indent=2), flush=True)
    if not summary['passed']:
        raise SystemExit('normal-file p50 CI upper bound exceeds +5%')


if __name__ == '__main__':
    main()
