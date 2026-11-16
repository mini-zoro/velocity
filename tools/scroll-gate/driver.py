#!/usr/bin/env python3
"""Build the HID poster and alternate isolated native base/candidate scroll trials."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import selectors
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


def build_poster(destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    source = HERE / 'scrollhid.swift'
    if not destination.exists() or destination.stat().st_mtime < source.stat().st_mtime:
        subprocess.run(['swiftc', '-O', '-module-cache-path', str(destination.parent / 'swift-cache'),
                        str(source), '-o', str(destination)], check=True)
    return destination


def make_workloads(directory, source):
    directory.mkdir(parents=True, exist_ok=True)
    if not source.is_file():
        raise ValueError(f'missing 10 MB fixture: {source}; pass --large-file PATH')
    data = source.read_bytes()
    if not 9_000_000 <= len(data) <= 11_000_000:
        raise ValueError(f'10 MB workload is {len(data)} bytes, expected 9..11 MB')
    boundary = data.rfind(b'\n', 0, 1_000_001)
    if boundary < 900_000:
        raise ValueError('10 MB fixture has no suitable 1 MB source prefix')
    normal = directory / 'rust-1mb.rs'
    normal.write_bytes(data[:boundary + 1])
    small = directory / 'rust-200-lines.rs'
    small.write_text(''.join(f'fn line_{i:03}() {{ let value = {i}; }} // scroll fixture\n' for i in range(200)))
    return [('rust-10mb', source.resolve()), ('rust-1mb', normal.resolve()), ('rust-200-lines', small.resolve())]


def order(runs):
    # AB, BA, AB counterbalances drift while each trial remains adjacent to its pair.
    for trial in range(runs):
        for label in (('base', 'candidate') if trial % 2 == 0 else ('candidate', 'base')):
            yield trial + 1, label


def discover(directory, pid, since=None):
    if directory is None or not directory.is_dir():
        return []
    found = []
    for path in directory.glob('*.jsonl'):
        try:
            if since is not None and path.stat().st_mtime < since:
                continue
            with path.open() as stream:
                for _ in range(20):
                    line = stream.readline()
                    if not line:
                        break
                    if json.loads(line).get('pid') == pid:
                        found.append(path)
                        break
        except (OSError, ValueError):
            continue
    return found


def stop_app(app, poster):
    if app.poll() is None:
        try:
            subprocess.run([str(poster), '--quit-pid', str(app.pid)], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=5, check=False)
        except (OSError, subprocess.TimeoutExpired):
            pass  # A stuck quit helper must not prevent process cleanup.
        try:
            app.wait(timeout=5)
        except subprocess.TimeoutExpired:
            app.terminate()
            try:
                app.wait(timeout=3)
            except subprocess.TimeoutExpired:
                app.kill()
                app.wait(timeout=3)


def run_one(args, binary, file, gesture, directory, poster, legacy):
    directory.mkdir(parents=True)
    telemetry_dir = directory / 'telemetry'
    telemetry_dir.mkdir()
    env = dict(os.environ, VELOCITY_LOG_DIR=str(telemetry_dir))
    cmd = [str(binary), '-n', '--data-dir', str(directory / 'data'), '--log-input',
           '--lines', '0', '--width', str(args.width), '--height', str(args.height), str(file)]
    (directory / 'command.json').write_text(json.dumps({'argv': cmd, 'VELOCITY_LOG_DIR': str(telemetry_dir)}, indent=2))
    app = None
    hid = None
    sampler = None
    with (directory / 'app.stdout').open('w') as stdout, (directory / 'app.stderr').open('w') as stderr, \
            (directory / 'poster.stderr').open('w') as hid_stderr, (directory / 'sample.stderr').open('w') as sample_stderr:
        try:
            launched = time.time()
            app = subprocess.Popen(cmd, env=env, stdout=stdout, stderr=stderr)
            deadline = time.monotonic() + args.startup_timeout
            while True:
                if app.poll() is not None:
                    raise ValueError(f'app exited before scrolling ({app.returncode}); see {directory}')
                logs = discover(telemetry_dir, app.pid, launched) or discover(legacy, app.pid, launched)
                if logs and any('"environment"' in p.read_text() for p in logs):
                    break
                if time.monotonic() > deadline:
                    raise ValueError('no startup telemetry; old binaries need --legacy-output-dir PATH')
                time.sleep(0.1)
            until = time.monotonic() + args.settle
            while time.monotonic() < until:
                if app.poll() is not None:
                    raise ValueError('app exited during settling')
                time.sleep(min(0.1, max(0, until - time.monotonic())))
            poster_cmd = [str(poster), '--pid', str(app.pid), '--duration', str(args.duration),
                          '--rate', str(args.input_hz), '--delta', str(args.delta)]
            if gesture == 'momentum':
                poster_cmd.append('--momentum')
            if args.x is not None:
                poster_cmd.extend(['--x', str(args.x), '--y', str(args.y)])
            hid = subprocess.Popen(poster_cmd, stdout=subprocess.PIPE, stderr=hid_stderr, text=True)
            selector = selectors.DefaultSelector()
            selector.register(hid.stdout, selectors.EVENT_READ)
            try:
                if not selector.select(timeout=10):
                    raise ValueError('poster did not start within 10 seconds')
                first = hid.stdout.readline()
            finally:
                selector.close()
            if not first or json.loads(first).get('kind') != 'poster_start':
                raise ValueError(f'poster refused target/permissions; see {directory / "poster.stderr"}')
            # The poster announces a start 100 ms ahead; keep the four-second
            # sample entirely inside the scrolling interval, after that deadline.
            time.sleep(max(0, json.loads(first)['start_unix_s'] - time.time()))
            sample_file = directory / 'main.sample.txt'
            sampler = subprocess.Popen(['/usr/bin/sample', str(app.pid), '4', '-file', str(sample_file)],
                                       stdout=sample_stderr, stderr=sample_stderr)
            rest, _ = hid.communicate(timeout=args.duration + 10)
            (directory / 'poster.jsonl').write_text(first + rest)
            if hid.returncode:
                raise ValueError(f'poster exited {hid.returncode}; see poster.stderr')
            sampler.wait(timeout=10)
            if sampler.returncode or not sample_file.exists():
                raise ValueError(f'sample failed ({sampler.returncode}); see sample.stderr')
            if app.poll() is not None:
                raise ValueError('app exited during scrolling')
            # Let the last submitted input record flush; this tail is excluded by poster bounds.
            time.sleep(0.2)
        finally:
            try:
                for child in (hid, sampler):
                    if child is not None and child.poll() is None:
                        child.terminate()
                        try:
                            child.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            child.kill()
                            child.wait(timeout=3)
            finally:
                if app is not None:
                    stop_app(app, poster)
    logs = discover(telemetry_dir, app.pid, launched)
    origin = 'VELOCITY_LOG_DIR'
    if not logs:
        logs = discover(legacy, app.pid, launched)
        origin = 'legacy-output-dir'
    if len(logs) != 1:
        raise ValueError(f'expected one pid-matched JSONL, found {len(logs)} in {origin}')
    destination = directory / 'events.jsonl'
    shutil.copy2(logs[0], destination)
    (directory / 'telemetry-source.json').write_text(json.dumps({'source': str(logs[0]), 'contract': origin, 'pid': app.pid}))
    return {'jsonl': str(destination), 'sample': str(directory / 'main.sample.txt'),
            'poster': str(directory / 'poster.jsonl')}


def self_test():
    class Tests(unittest.TestCase):
        def test_failed_quit_helper_still_terminates_app(self):
            from unittest.mock import Mock, patch
            app = Mock()
            app.poll.return_value = None
            app.wait.side_effect = [subprocess.TimeoutExpired('app', 5), 0]
            with patch('subprocess.run', side_effect=subprocess.TimeoutExpired('quit', 5)):
                stop_app(app, Path('/unused/poster'))
            app.terminate.assert_called_once()
            app.kill.assert_not_called()
        def test_alternation(self):
            self.assertEqual(list(order(3)), [(1, 'base'), (1, 'candidate'), (2, 'candidate'), (2, 'base'), (3, 'base'), (3, 'candidate')])
        def test_workloads_and_pid_filter(self):
            with tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                fixture = root / 'large.rs'
                fixture.write_text('fn f() {}\n' * 1_000_000)
                files = make_workloads(root / 'fixtures', fixture)
                self.assertEqual(len(files), 3)
                self.assertEqual(len(files[2][1].read_text().splitlines()), 200)
                self.assertLessEqual(files[1][1].stat().st_size, 1_000_001)
                (root / 'a.jsonl').write_text('{"pid":42}\n')
                (root / 'b.jsonl').write_text('{"pid":43}\n')
                self.assertEqual(discover(root, 42), [root / 'a.jsonl'])
                self.assertEqual(discover(root, 44), [])
                stale = root / 'stale.jsonl'
                stale.write_text('{"pid":42}\n')
                os.utime(stale, (1, 1))
                self.assertEqual(discover(root, 42, since=2), [root / 'a.jsonl'])
    return unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(Tests)).wasSuccessful()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', type=Path)
    parser.add_argument('--candidate', type=Path)
    parser.add_argument('--runs', type=int, default=3)
    parser.add_argument('--refresh-hz', type=float, default=144)
    parser.add_argument('--input-hz', type=float, default=144)
    parser.add_argument('--duration', type=float, default=10)
    parser.add_argument('--settle', type=float, default=5)
    parser.add_argument('--startup-timeout', type=float, default=45)
    parser.add_argument('--width', type=int, default=1600)
    parser.add_argument('--height', type=int, default=1000)
    parser.add_argument('--delta', type=int, default=8)
    parser.add_argument('--x', type=float)
    parser.add_argument('--y', type=float)
    parser.add_argument('--large-file', type=Path, default=ROOT / 'spikes/data/rust-10mb.rs')
    parser.add_argument('--workload', choices=['rust-10mb', 'rust-1mb', 'rust-200-lines'], action='append')
    parser.add_argument('--gesture', choices=['finger', 'momentum'], action='append')
    parser.add_argument('--legacy-output-dir', type=Path)
    parser.add_argument('--base-legacy-output-dir', type=Path)
    parser.add_argument('--candidate-legacy-output-dir', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--poster', type=Path, help='use this prebuilt poster')
    parser.add_argument('--build-poster', action='store_true', help='build poster and exit')
    parser.add_argument('--self-test', action='store_true', help='headless analyzer/driver checks, no native launch')
    args = parser.parse_args()
    if args.self_test:
        status = subprocess.run([sys.executable, '-I', str(HERE / 'analyze.py'), '--self-test'], check=False).returncode
        return 0 if self_test() and status == 0 else 1
    if sys.platform != 'darwin':
        parser.error('native driver needs macOS; analyzer --self-test works headlessly')
    try:
        poster = args.poster.resolve() if args.poster else build_poster(HERE / '.build/scrollhid')
        if args.build_poster:
            print(poster)
            return 0
        if not args.base or not args.candidate:
            parser.error('--base and --candidate are required')
        numeric = [args.refresh_hz, args.input_hz, args.duration, args.settle, args.startup_timeout]
        numeric += [n for n in (args.x, args.y) if n is not None]
        if not all(math.isfinite(n) for n in numeric):
            parser.error('all numeric arguments must be finite')
        binaries = {label: getattr(args, label).resolve() for label in ('base', 'candidate')}
        if any(not binary.is_file() or not os.access(binary, os.X_OK) for binary in binaries.values()):
            parser.error('both binaries must be executable')
        if not 1 <= args.runs <= 100 or not 5 <= args.duration <= 600 or args.settle < 0 or args.startup_timeout <= 0:
            parser.error('runs 1..100, duration 5..600, settle >=0, startup-timeout >0 required')
        if not 1 <= args.input_hz <= 1000 or not 1 <= args.refresh_hz <= 1000:
            parser.error('input and display rates must be 1..1000 Hz')
        if not 64 <= args.width <= 8192 or not 64 <= args.height <= 8192:
            parser.error('window dimensions must be 64..8192 logical points')
        if not 1 <= args.delta <= 100 or (args.x is None) != (args.y is None):
            parser.error('delta 1..100 and either both --x/--y or neither required')
        output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix='velocity-scroll-gate-'))
        if args.output:
            output.mkdir(parents=True, exist_ok=False)
        workloads = make_workloads(output / 'workloads', args.large_file)
        manifest = {'schema': 1, 'runs_each': args.runs, 'refresh_hz': args.refresh_hz,
                    'input_hz': args.input_hz, 'binaries': {k: str(v) for k, v in binaries.items()},
                    'binary_sha256': {k: hashlib.sha256(v.read_bytes()).hexdigest() for k, v in binaries.items()},
                    'window_logical_points': [args.width, args.height], 'complete': False,
                    'workloads': [name for name, _ in workloads if not args.workload or name in args.workload],
                    'gestures': args.gesture or ['finger', 'momentum'], 'runs': []}
        manifest_path = output / 'manifest.json'
        manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
        print(f'Evidence: {output}', flush=True)
        for workload, file in workloads:
            if args.workload and workload not in args.workload:
                continue
            for gesture in args.gesture or ['finger', 'momentum']:
                for trial, label in order(args.runs):
                    directory = output / workload / gesture / f'{trial:02}-{label}'
                    print(f'{workload} / {gesture} / pair {trial} / {label}', flush=True)
                    legacy = getattr(args, f'{label}_legacy_output_dir') or args.legacy_output_dir
                    evidence = run_one(args, binaries[label], file, gesture, directory, poster, legacy)
                    manifest['runs'].append(dict(evidence, workload=workload, gesture=gesture, trial=trial, label=label))
                    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
        manifest['complete'] = True
        manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
        return subprocess.run([sys.executable, '-I', str(HERE / 'analyze.py'), '--manifest', str(manifest_path),
                               '--report', str(output / 'report.json')], check=False).returncode
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'scroll-gate: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
