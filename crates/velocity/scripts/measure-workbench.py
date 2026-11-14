#!/usr/bin/env python3
"""Record sequential round-7 CPU runs with one release binary and all raw samples."""
import argparse
import datetime
import hashlib
import json
import math
import platform
from pathlib import Path
import shutil
import subprocess


def quantiles(values):
    ordered = sorted(values)
    def at(p):
        rank = p * (len(ordered) - 1)
        lo = math.floor(rank)
        hi = min(lo + 1, len(ordered) - 1)
        return ordered[lo] + (ordered[hi] - ordered[lo]) * (rank - lo)
    return dict(count=len(values), p50=at(.5), p95=at(.95), p99=at(.99), max=max(values))


def main():
    repo = Path(__file__).resolve().parents[3]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runs', type=int, default=5)
    parser.add_argument('--width', type=int, default=3840)
    parser.add_argument('--height', type=int, default=2160)
    parser.add_argument('--binary', type=Path, default=repo / 'target/release/velocity')
    parser.add_argument('--output-dir', type=Path)
    args = parser.parse_args()
    if args.runs < 1 or not args.binary.is_file():
        parser.error('positive --runs and an existing release --binary are required')
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S.%fZ')
    output = args.output_dir or repo / 'crates/velocity/output' / f'round7-cpu-{stamp}'
    output.mkdir(parents=True, exist_ok=False)
    output = output.resolve()
    binary = output / 'velocity'
    shutil.copy2(args.binary, binary)
    files = [repo / 'Cargo.lock']
    for crate in ['velocity', 'vel-ui', 'vel-config', 'vel-commands', 'vel-platform', 'vel-build',
                  'vel-session', 'vel-buffer', 'vel-highlight', 'vel-syntax']:
        files += list((repo / 'crates' / crate / 'src').glob('*.rs'))
        files += [repo / 'crates' / crate / 'Cargo.toml']
    environment = dict(
        captured_utc=stamp, platform=platform.platform(), machine=platform.machine(),
        rustc=subprocess.check_output(['rustc', '--version'], text=True).strip(),
        head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
        binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
        source_hashes={str(p.relative_to(repo)): hashlib.sha256(p.read_bytes()).hexdigest() for p in files},
        note='One copied binary, sequential runs, all outliers retained; lane B may be active.',
        gui_started=False, gpu_started=False)
    (output / 'environment.json').write_text(json.dumps(environment, indent=2) + '\n')
    print(output, flush=True)
    runs = []
    typing, scrolling, saves, restores = [], [], [], []
    for index in range(1, args.runs + 1):
        path = output / f'run-{index}.jsonl'
        with path.with_suffix('.stdout.log').open('w') as log:
            result = subprocess.run([str(binary), '--headless-workbench', '--width', str(args.width),
                                     '--height', str(args.height), '--output', str(path)], cwd=repo,
                                    stdout=log, stderr=subprocess.STDOUT, timeout=240)
        if result.returncode:
            raise SystemExit(f'run {index} failed ({result.returncode}): {path.with_suffix(".stdout.log")}')
        rows = {r['kind']: r for r in map(json.loads, path.read_text().splitlines())}
        if not rows.get('complete', {}).get('complete'):
            raise SystemExit(f'run {index} lacks completion')
        scene = rows['workbench_four_groups']
        typing += scene['typing_samples_ms']
        scrolling += scene['scroll_samples_ms']
        saves += rows['session_save']['samples_ms']
        restores += [rows['session_restore']['first_window_cpu_ready_ms']]
        current = dict(save_ms=rows['session_save']['save_worker_ms'],
                       restore_ms=rows['session_restore']['first_window_cpu_ready_ms'],
                       first_scene_ms=rows['session_restore']['first_scene_cpu_ready_ms'],
                       typing_ms=scene['typing_edit_and_scene_cpu_ms'], scroll_ms=scene['scroll_scene_cpu_ms'],
                       data_dir=rows['session_save']['data_dir'])
        runs.append(current)
        print(f'run {index}: {json.dumps(current)}', flush=True)
    summary = dict(environment=environment, runs=runs, logical_size=[args.width, args.height],
                   pooled_ms=dict(save=quantiles(saves), restore=quantiles(restores),
                                  typing=quantiles(typing), scroll=quantiles(scrolling)),
                   budgets_ms=dict(save=20, restore=300, typing_p50=2, typing_p95=4, scroll_p95=2),
                   raw_directory=str(output), native_results_pending=True)
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary['pooled_ms']), flush=True)


if __name__ == '__main__':
    main()
