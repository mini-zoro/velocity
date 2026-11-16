#!/usr/bin/env python3
"""Repeat the production editor huge/long-line/hex CPU probes in independent serial processes."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import shutil
import statistics
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--file', type=Path, required=True, help='Prepared 4 GB ASCII log')
    parser.add_argument('--runs', type=int, default=5)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    if args.runs < 5:
        parser.error('at least five independent processes required')
    repo = Path(__file__).resolve().parents[3]
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = output/'velocity'
    shutil.copy2(args.binary, binary)
    environment = {'platform': platform.platform(), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                   'runner_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                   'file': str(args.file.resolve()), 'source_bytes': args.file.stat().st_size,
                   'schedule': 'serial independent processes, no lane-A compilation/tests during measurement',
                   'boundary': 'wall time for CPU editor/scene work; no native rendering, GPU or scanout; setup excluded',
                   'gui_started': False, 'gpu_started': False}
    records = []
    for run in range(args.runs):
        raw = output/f'{run+1:02}.jsonl'
        with raw.with_suffix('.log').open('w') as log:
            subprocess.run([str(binary), '--headless-large-files', str(args.file.resolve()),
                            '--data-dir', str(output/f'profile-{run}'), '--output', str(raw), '--timeout', '180'],
                           cwd=repo, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=200)
        rows = [json.loads(line) for line in raw.read_text().splitlines()]
        assert any(r.get('kind') == 'large_files_complete' and r.get('complete') for r in rows), raw
        records.append({r['kind']: r for r in rows if r['kind'] in [
            'large_file_prefix_scene', 'long_line_typing_scene', 'long_line_horizontal_scene', 'binary_4gb_scene']})
        print(run+1, {k: r.get('elapsed_ms', r.get('stats_ms', {}).get('p95'))
                      for k, r in records[-1].items()}, flush=True)
    result = {}
    for kind, budget in [('large_file_prefix_scene', 100), ('long_line_typing_scene', 4),
                         ('long_line_horizontal_scene', 2), ('binary_4gb_scene', None)]:
        values = [r[kind].get('elapsed_ms', r[kind].get('stats_ms', {}).get('p95')) for r in records]
        ordered = sorted(values)
        result[kind] = {'process_values_ms': values, 'median_ms': statistics.median(values),
                        'nearest_rank_p95_ms': ordered[math.ceil(.95*len(values))-1],
                        'maximum_ms': max(values), 'budget_ms': budget,
                        'all_observed_pass': budget is None or max(values) <= budget}
    summary = {'environment': environment, 'runs': args.runs, 'results': result,
               'records': records, 'passed': all(r['all_observed_pass'] for r in result.values())}
    (output/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    print(json.dumps(result, indent=2), flush=True)
    if not summary['passed']:
        raise SystemExit('an observed UI budget was exceeded')


if __name__ == '__main__':
    main()
