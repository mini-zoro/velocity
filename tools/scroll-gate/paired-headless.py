#!/usr/bin/env python3
"""Alternate the existing device-free scroll/typing benches; never starts AppKit/Metal."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--base', type=Path, required=True)
parser.add_argument('--candidate', type=Path, required=True)
parser.add_argument('--pairs', type=int, default=5)
parser.add_argument('--bench', choices=['features', 'typing1', 'typing1000'], action='append')
parser.add_argument('--output', type=Path)
args = parser.parse_args()
root = args.output or Path(tempfile.mkdtemp(prefix='velocity-scroll-headless-'))
root.mkdir(parents=True, exist_ok=True)
measurements = {}
for pair in range(args.pairs):
    for label in (('base', 'candidate') if pair % 2 == 0 else ('candidate', 'base')):
        binary = getattr(args, label).resolve()
        for name, options in [('features', ['--headless-view-features']),
                              ('typing1', ['--headless-smoke', '--carets', '1']),
                              ('typing1000', ['--headless-smoke', '--carets', '1000'])]:
            if args.bench and name not in args.bench:
                continue
            path = root / f'{pair+1:02}-{label}-{name}.jsonl'
            print(path.name, flush=True)
            with path.with_suffix('.stdout').open('w') as stdout:
                subprocess.run([str(binary), *options, '--output', str(path)],
                               stdout=stdout, stderr=subprocess.STDOUT, check=True, timeout=180)
            rows = [json.loads(line) for line in path.read_text().splitlines()]
            if not any(r.get('kind') == 'complete' for r in rows):
                raise ValueError(f'{path}: incomplete bench')
            for record in rows:
                if record['kind'] not in ('highlight_minimap_scroll_cpu', 'highlighted_typing_cpu', 'headless_summary'):
                    continue
                for key in ('stats_ms', 'edit_stats_ms', 'scene_stats_ms', 'edit_and_scene_cpu_ms'):
                    if key not in record:
                        continue
                    for percentile in ('p50', 'p95', 'p99'):
                        metric = f'{name}/{record["kind"]}/{key}/{percentile}'
                        measurements.setdefault(metric, {'base': [], 'candidate': []})[label].append(record[key][percentile])
report = {'pairs': args.pairs, 'boundary': 'existing CPU scenes/keymap benches; no native renderer or scanout',
          'binaries': {label: str(getattr(args, label).resolve()) for label in ('base', 'candidate')},
          'binary_sha256': {label: hashlib.sha256(getattr(args, label).read_bytes()).hexdigest() for label in ('base', 'candidate')},
          'metrics': {}}
for metric, values in measurements.items():
    base, candidate = (statistics.median(values[label]) for label in ('base', 'candidate'))
    report['metrics'][metric] = dict(values, base_median_ms=base, candidate_median_ms=candidate,
                                    delta_pct=(candidate / base - 1) * 100, pass_5pct=candidate <= base * 1.05)
report['pass'] = all(m['pass_5pct'] for m in report['metrics'].values())
(root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
raise SystemExit(0 if report['pass'] else 1)
