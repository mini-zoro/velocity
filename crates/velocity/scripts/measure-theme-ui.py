#!/usr/bin/env python3
"""Sequential round-9/round-10 CPU A/B plus popup, 200-annotation and theme-switch budgets."""
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


def main():
    repo = Path(__file__).resolve().parents[3]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, default=repo/'target/release/velocity')
    parser.add_argument('--runs', type=int, default=10)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--resume', action='store_true', help='reuse complete A/B records after verifying frozen binaries and sources')
    args = parser.parse_args()
    if args.runs < 5:
        parser.error('at least five alternating process pairs')
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=args.resume)
    binaries = {}
    for label, source in [('before', args.before), ('after', args.after)]:
        binaries[label] = output/f'velocity-{label}'
        if not args.resume:
            shutil.copy2(source, binaries[label])
    env = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
           'platform': platform.platform(),
           'schedule': 'one process at a time; reversed before/after order each pair; no compilation during series',
           'binary_sha256': {k: hashlib.sha256(p.read_bytes()).hexdigest() for k, p in binaries.items()},
           'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
           'cargo_lock_sha256': hashlib.sha256((repo/'Cargo.lock').read_bytes()).hexdigest(),
           'gui_started': False, 'gpu_started': False,
           'source_sha256': {str(p.relative_to(repo)): hashlib.sha256(p.read_bytes()).hexdigest()
                             for crate in ['velocity', 'vel-ui', 'vel-render', 'vel-platform', 'vel-config', 'vel-session']
                             for p in sorted((repo/'crates'/crate).rglob('*'))
                             if p.is_file() and p.suffix in ['.rs', '.toml', '.wgsl', '.html', '.sublime-settings']}}
    if args.resume:
        previous = json.loads((output/'environment.json').read_text())
        for key in ['binary_sha256', 'cargo_lock_sha256', 'source_sha256', 'head']:
            if previous[key] != env[key]:
                parser.error(f'cannot resume: {key} changed')
        for label, source in [('before', args.before), ('after', args.after)]:
            if hashlib.sha256(source.read_bytes()).hexdigest() != previous['binary_sha256'][label]:
                parser.error(f'cannot resume: {label} input binary changed')
        previous['resumed_at_utc'] = env['utc']
        env = previous
    env['runner_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    (output/'environment.json').write_text(json.dumps(env, indent=2)+'\n')
    records = {'typing': {'before': [], 'after': []}, 'scroll': {'before': [], 'after': []}, 'theme': []}

    def run(workload, label, index):
        raw = output/f'{workload}-{label}-{index}.jsonl'
        mode = {'typing': '--headless-intelligence', 'scroll': '--headless-workbench',
                'theme': '--headless-theme-ui'}[workload]
        cmd = [str(binaries[label]), mode, '--data-dir', str(output/f'profile-{workload}-{label}-{index}'),
               '--output', str(raw), '--timeout', '300']
        if workload == 'typing':
            cmd += ['--review-ui', 'on', '--intel-words', '0']
        reused = args.resume and workload != 'theme' and raw.exists()
        if not reused:
            if raw.exists():
                attempt = 0
                while raw.with_suffix(f'.attempt{attempt}.jsonl').exists():
                    attempt += 1
                raw.rename(raw.with_suffix(f'.attempt{attempt}.jsonl'))
                if raw.with_suffix('.log').exists():
                    raw.with_suffix('.log').rename(raw.with_suffix(f'.attempt{attempt}.log'))
            with raw.with_suffix('.log').open('w') as log:
                result = subprocess.run(cmd, cwd=repo, stdout=log, stderr=subprocess.STDOUT, timeout=330)
            returncode = result.returncode
        else:
            returncode = 0
        rows = [json.loads(line) for line in raw.read_text().splitlines()]
        complete = any(r.get('kind') == 'complete' and r.get('complete') for r in rows)
        if workload == 'theme':
            # This bounded mode publishes its final metrics and enforces budgets before exit.
            complete = any(r.get('kind') == 'theme_ui_cpu' and r.get('popup_bytes') == 2048
                           and r.get('annotations') == 200
                           and r['popup_first_frame_ms']['count'] == 100
                           and r['annotation_frame_ms']['count'] == 100
                           and r['theme_next_frame_ms']['count'] == 50 for r in rows)
        if returncode or not complete:
            raise RuntimeError(f'{workload} {label} {index} failed: inspect {raw}')
        kind = {'typing': 'intelligence_cpu', 'scroll': 'workbench_four_groups', 'theme': 'theme_ui_cpu'}[workload]
        record = next(r for r in rows if r.get('kind') == kind)
        record = {'index': index, 'raw': str(raw), 'record': record}
        if workload == 'theme':
            records['theme'].append(record)
            print('theme', index, record['record']['popup_first_frame_ms']['p95'],
                  record['record']['annotation_frame_ms']['p95'], flush=True)
        else:
            records[workload][label].append(record)
            metric = 'typing_ms' if workload == 'typing' else 'scroll_scene_cpu_ms'
            print(workload, label, index, record['record'][metric]['p50'], flush=True)

    for workload in ['typing', 'scroll']:
        for index in range(1, args.runs+1):
            for label in (['before', 'after'] if index % 2 else ['after', 'before']):
                run(workload, label, index)
    for index in range(1, 6):
        run('theme', 'after', index)
    comparison = {}
    rng = random.Random(10)
    for workload, metric in [('typing', 'typing_ms'), ('scroll', 'scroll_scene_cpu_ms')]:
        comparison[workload] = {}
        for quantile in ['p50', 'p95']:
            before = [r['record'][metric][quantile] for r in records[workload]['before']]
            after = [r['record'][metric][quantile] for r in records[workload]['after']]
            boots = []
            for _ in range(10000):
                indices = rng.choices(range(len(before)), k=len(before))
                boots.append(statistics.median(after[i] for i in indices)/statistics.median(before[i] for i in indices))
            boots.sort()
            percent = (statistics.median(after)/statistics.median(before)-1)*100
            comparison[workload][quantile] = {'before_ms': statistics.median(before),
                                             'after_ms': statistics.median(after), 'change_percent': percent,
                                             'paired_ratio_ci95': [boots[250], boots[9750]],
                                             'within_5_percent': percent <= 5}
    budgets = {}
    for key, budget in [('popup_first_frame_ms', 2), ('annotation_frame_ms', 2), ('theme_next_frame_ms', 50)]:
        budgets[key] = {'budget_ms': budget,
                        'process_p50_median_ms': statistics.median(r['record'][key]['p50'] for r in records['theme']),
                        'process_p95_median_ms': statistics.median(r['record'][key]['p95'] for r in records['theme']),
                        'max_ms': max(r['record'][key]['max'] for r in records['theme'])}
    summary = {'environment': env, 'comparison': comparison, 'budgets': budgets, 'runs': records}
    (output/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    print(json.dumps({'comparison': comparison, 'budgets': budgets}, indent=2), flush=True)
    if not all(q['within_5_percent'] for w in comparison.values() for q in w.values()):
        raise SystemExit('round-9 A/B regression exceeds 5%; raw results retained')


if __name__ == '__main__':
    main()
