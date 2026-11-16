#!/usr/bin/env python3
"""Native scroll metrics and paired gates; Python standard library, usable with -I."""
import argparse
import json
import math
from pathlib import Path
import re
import statistics
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
METRICS = {
    'frames': 'higher',
    'receipt_to_submit_p50_ms': 'lower',
    'receipt_to_submit_p95_ms': 'lower',
    'receipt_to_submit_p99_ms': 'lower',
    'frame_interval_p50_ms': 'lower',
    'frame_interval_p95_ms': 'lower',
    'frame_interval_p99_ms': 'lower',
    'gaps_over_20_ms': 'lower',
    'gaps_over_50_ms': 'lower',
    'main_thread_busy_pct': 'lower',
}


def finite(value, name, positive=False):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ValueError(f'{name} must be a finite number')
    if value < 0 or (positive and value == 0):
        raise ValueError(f'{name} must be {"positive" if positive else "nonnegative"}')
    return float(value)


def records(path):
    result = []
    for number, line in enumerate(Path(path).read_text().splitlines(), 1):
        if line.strip():
            try:
                record = json.loads(line)
            except ValueError as error:
                raise ValueError(f'{path}:{number}: {error}') from error
            if not isinstance(record, dict):
                raise ValueError(f'{path}:{number}: expected an object')
            result.append(record)
    return result


def percentile(values, p):
    """Linear interpolation at (n-1)*p, with no rounding before gate comparisons."""
    if not values:
        raise ValueError('no samples for percentile')
    ordered = sorted(values)
    rank = (len(ordered) - 1) * p / 100
    lo, hi = math.floor(rank), math.ceil(rank)
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (rank - lo)


def sample_busy(path):
    """Count disjoint run-loop mach_msg subtrees of the FIRST sample thread.

    The flat symbol-frequency summary and other threads are deliberately ignored.
    A drawable/semaphore wait remains busy, even inside an event-loop callback.
    """
    lines = Path(path).read_text(errors='replace').splitlines()
    try:
        begin = next(i for i, line in enumerate(lines) if 'Call graph:' in line) + 1
    except StopIteration as error:
        raise ValueError(f'{path}: missing sample call graph') from error
    root = None
    stack = []
    idle = 0
    row_pattern = re.compile(r'^([\s+|!:]*)(\d+)\s+(.+)$')
    for line in lines[begin:]:
        match = row_pattern.match(line)
        if not match:
            if root is not None and ('Total number in stack' in line or 'Sort by top of stack' in line):
                break
            continue
        depth, count, name = len(match[1]), int(match[2]), match[3]
        is_thread = re.match(r'Thread[_ ]', name) is not None
        if root is None:
            if not is_thread:
                continue
            root = (depth, count, name)
            stack = [(depth, name, False)]
            continue
        if is_thread or depth <= root[0]:
            break
        while stack and stack[-1][0] >= depth:
            stack.pop()
        accounted = any(row[2] for row in stack)
        mach = re.match(r'(?:_+)?mach_msg(?:2|_overwrite|_trap|_rpc_from_kernel_proper)*\b', name) is not None
        # The nearest non-mach caller must be the idle run-loop receive path.
        # Testing merely for any CFRunLoop ancestor mislabels nextDrawable waits.
        caller = stack[-1][1] if stack else ''
        runloop_receive = re.search(r'CFRunLoopServiceMachPort|CFRunLoopRun\b|DPSNextEvent|DPSGetNextEvent', caller)
        this_idle = bool(mach and runloop_receive and not accounted)
        if this_idle:
            idle += count
        stack.append((depth, name, this_idle or accounted))
    if root is None or root[1] <= 0 or idle > root[1]:
        raise ValueError(f'{path}: invalid/missing first-thread sample counts')
    return {'total_samples': root[1], 'idle_samples': idle,
            'busy_samples': root[1] - idle,
            'main_thread_busy_pct': 100 * (root[1] - idle) / root[1],
            'thread': root[2], 'boundary': 'first thread; only run-loop mach_msg receive is idle'}


def poster_window(events, telemetry):
    starts = [r for r in events if r.get('kind') == 'poster_start']
    ends = [r for r in events if r.get('kind') == 'poster_end']
    if len(starts) != 1 or len(ends) != 1 or ends[0].get('complete') is not True:
        raise ValueError('poster must have one start and one successful end')
    start, end = starts[0], ends[0]
    duration = finite(start['duration_s'], 'duration', True)
    rate = finite(start['rate_hz'], 'input rate', True)
    if end.get('pid') != start.get('pid'):
        raise ValueError('poster pid changed')
    pids = {r['pid'] for r in telemetry if 'pid' in r}
    if pids and pids != {start['pid']}:
        raise ValueError('poster/telemetry pid mismatch')
    run_ids = {r['run_id'] for r in telemetry if 'run_id' in r}
    if len(run_ids) != 1:
        raise ValueError('expected one telemetry run_id')
    # Legacy Reporter run_id is UNIX microseconds at Reporter creation, just after
    # main entry. Preserve this small launch alignment error in the report.
    epoch = int(next(iter(run_ids)).split('-')[0]) / 1_000_000
    start_ms = (finite(start['start_unix_s'], 'poster start', True) - epoch) * 1000
    elapsed = finite(end['elapsed_s'], 'poster elapsed', True)
    expected = math.floor(duration * rate + 0.5)
    if end.get('movement_events') != expected:
        raise ValueError('poster did not deliver the requested movement-event count')
    if abs(elapsed - duration) > max(0.05, duration * 0.01):
        raise ValueError('poster runtime differs from requested duration by >1% or 50 ms')
    if finite(end.get('max_lateness_ms'), 'poster maximum lateness') > 50:
        raise ValueError('poster missed an input deadline by >50 ms')
    if finite(end.get('late_slots'), 'poster late slots') > expected * 0.01:
        raise ValueError('poster missed >1% of input slots')
    return start_ms, duration, rate


def absolute_gate(metrics, ideal, refresh):
    checks = {
        'frames_at_least_95pct_ideal': (metrics['frames'], '>=', ideal * 0.95),
        'frame_interval_p99_at_most_two_refreshes': (metrics['frame_interval_p99_ms'], '<=', 2000 / refresh),
        'zero_gaps_over_50ms': (metrics['gaps_over_50_ms'], '<=', 0),
        'receipt_to_submit_p95_at_most_4ms': (metrics['receipt_to_submit_p95_ms'], '<=', 4),
        'main_thread_busy_at_most_35pct': (metrics['main_thread_busy_pct'], '<=', 35),
    }
    return {name: {'value': value, 'operator': op, 'limit': limit,
                   'pass': value >= limit if op == '>=' else value <= limit}
            for name, (value, op, limit) in checks.items()}


def analyze(jsonl, sample, refresh=None, duration=10, start_ms=0, poster=None, input_hz=144):
    telemetry = records(jsonl)
    if not telemetry:
        raise ValueError('empty telemetry')
    if any(r.get('kind') == 'error' for r in telemetry):
        raise ValueError('application reported an error; inspect telemetry')
    if any(r.get('mode', 'interactive') != 'interactive' for r in telemetry):
        raise ValueError('native gate requires interactive real-event telemetry')
    if refresh is None:
        rates = {float(r[key]) for r in telemetry for key in ('display_refresh_hz', 'refresh_hz') if key in r}
        if len(rates) != 1:
            raise ValueError('pass --refresh-hz or log one display_refresh_hz')
        refresh = rates.pop()
    refresh = finite(refresh, 'display refresh', True)
    if poster:
        start_ms, duration, input_hz = poster_window(records(poster), telemetry)
    duration = finite(duration, 'duration', True)
    input_hz = finite(input_hz, 'input rate', True)
    stop_ms = start_ms + duration * 1000
    frames = []
    explicit = True
    frame_kind = 'native_scroll_frame' if any(r.get('kind') == 'native_scroll_frame' for r in telemetry) else 'input_latency'
    for record in telemetry:
        if record.get('kind') != frame_kind:
            continue
        latency = finite(record['receipt_to_submit_ms'], 'receipt_to_submit_ms')
        if 'submit_main_ms' in record:
            at = finite(record['submit_main_ms'], 'submit_main_ms')
        else:
            explicit = False
            at = finite(record['logged_main_ms'], 'logged_main_ms')
        if start_ms <= at <= stop_ms:
            frames.append((at, latency))
    if len(frames) < 2:
        raise ValueError('fewer than two input frames inside the scrolling window')
    stamps = [at for at, _ in frames]
    if any(b <= a for a, b in zip(stamps, stamps[1:])):
        raise ValueError('input frame timestamps must increase strictly')
    intervals = [b - a for a, b in zip(stamps, stamps[1:])]
    # Include quiet leading/trailing edges in gap and p99 statistics: freezing
    # before the first or after the last frame must never improve the gate.
    edge_intervals = [stamps[0] - start_ms, stop_ms - stamps[-1]]
    all_intervals = intervals + edge_intervals
    latencies = [latency for _, latency in frames]
    busy = sample_busy(sample)
    metrics = {'frames': len(frames), 'gaps_over_20_ms': sum(v > 20 for v in all_intervals),
               'gaps_over_50_ms': sum(v > 50 for v in all_intervals),
               'main_thread_busy_pct': busy['main_thread_busy_pct']}
    for p in (50, 95, 99):
        metrics[f'receipt_to_submit_p{p}_ms'] = percentile(latencies, p)
        metrics[f'frame_interval_p{p}_ms'] = percentile(all_intervals, p)
    ideal = duration * min(refresh, input_hz)
    checks = absolute_gate(metrics, ideal, refresh)
    return {'metrics': metrics, 'display_refresh_hz': refresh, 'input_hz': input_hz,
            'duration_s': duration, 'ideal_frames': ideal, 'frame_coverage_pct': len(frames) / ideal * 100,
            'window_main_ms': [start_ms, stop_ms], 'sample': busy, 'checks': checks,
            'pass': all(c['pass'] for c in checks.values()),
            'timestamp_boundary': 'queue submit' if explicit else 'input_latency.logged_main_ms (legacy submit-log proxy)',
            'window_alignment': 'Reporter run_id UNIX epoch (legacy: creation just after main entry)' if poster else 'explicit main-relative start',
            'files': {'jsonl': str(jsonl), 'sample': str(sample), 'poster': str(poster) if poster else None}}


def delta(base, candidate, direction):
    change = (candidate - base) / base * 100 if base else (0.0 if candidate == 0 else None)
    passes = candidate >= base * 0.95 if direction == 'higher' else candidate <= base * 1.05
    return {'base': base, 'candidate': candidate, 'delta_pct': change, 'direction': direction,
            'pass': passes, 'zero_baseline': base == 0}


def paired(manifest_path):
    manifest_path = Path(manifest_path)
    manifest = json.loads(manifest_path.read_text())
    if manifest.get('complete') is not True:
        raise ValueError('manifest is incomplete; all requested trials must finish')
    groups = {}
    runs = []
    seen = set()
    for entry in manifest['runs']:
        if entry['label'] not in ('base', 'candidate'):
            raise ValueError('run label must be base or candidate')
        identity = (entry['workload'], entry['gesture'], entry['label'], entry['trial'])
        if identity in seen:
            raise ValueError('duplicate paired trial')
        seen.add(identity)
        def path(key):
            p = Path(entry[key])
            return p if p.is_absolute() else manifest_path.parent / p
        result = analyze(path('jsonl'), path('sample'), manifest.get('refresh_hz'),
                         poster=path('poster'))
        runs.append(dict(entry, result=result))
        key = (entry['workload'], entry['gesture'])
        groups.setdefault(key, {'base': [], 'candidate': []})[entry['label']].append(result)
    summaries = []
    expected_runs = manifest.get('runs_each', 3)
    if 'workloads' in manifest and 'gestures' in manifest:
        expected_groups = {(w, g) for w in manifest['workloads'] for g in manifest['gestures']}
        if set(groups) != expected_groups:
            raise ValueError('missing requested workload/gesture groups')
    for (workload, gesture), labels in sorted(groups.items()):
        if any(len(labels[label]) != expected_runs for label in ('base', 'candidate')):
            raise ValueError(f'{workload}/{gesture}: incomplete pairs')
        if any({trial for w, g, lab, trial in seen if (w, g, lab) == (workload, gesture, label)}
               != set(range(1, expected_runs + 1)) for label in ('base', 'candidate')):
            raise ValueError(f'{workload}/{gesture}: incorrect trial numbers')
        if len({(r['duration_s'], r['input_hz'], r['display_refresh_hz'])
                for values in labels.values() for r in values}) != 1:
            raise ValueError(f'{workload}/{gesture}: mismatched duration/input/display rates')
        medians = {label: {metric: statistics.median(r['metrics'][metric] for r in values)
                           for metric in METRICS} for label, values in labels.items()}
        comparisons = {metric: delta(medians['base'][metric], medians['candidate'][metric], direction)
                       for metric, direction in METRICS.items()}
        absolute = all(r['pass'] for r in labels['candidate'])
        summaries.append({'workload': workload, 'gesture': gesture, 'runs_each': expected_runs,
                          'medians': medians, 'comparisons': comparisons,
                          'candidate_all_runs_pass_absolute_gate': absolute,
                          'pass': absolute and all(c['pass'] for c in comparisons.values())})
    if not summaries:
        raise ValueError('manifest contains no runs')
    return {'schema': 1, 'groups': summaries, 'runs': runs, 'pass': all(g['pass'] for g in summaries),
            'regression_limit_pct': 5, 'manifest': str(manifest_path)}


def markdown(report):
    rows = ['# Native scroll gate', '', f'Gate: **{"PASS" if report["pass"] else "FAIL"}**', '']
    if 'groups' not in report:
        rows += ['| Metric | Value |', '|---|---:|']
        rows += [f'| {key} | {value:.4f} |' for key, value in report['metrics'].items()]
    else:
        for group in report['groups']:
            rows += [f'## {group["workload"]} / {group["gesture"]}', '',
                     f'Candidate absolute gate, every run: {group["candidate_all_runs_pass_absolute_gate"]}', '',
                     '| Metric | Base median | Candidate median | Delta % | ≤5% regression |', '|---|---:|---:|---:|---|']
            for name, comp in group['comparisons'].items():
                pct = f'{comp["delta_pct"]:+.2f}' if comp['delta_pct'] is not None else 'new nonzero (base=0)'
                rows.append(f'| {name} | {comp["base"]:.4f} | {comp["candidate"]:.4f} | {pct} | {comp["pass"]} |')
            rows.append('')
    rows += ['', 'Frame intervals include the leading/trailing quiet edges. Legacy timestamps are submit-log proxies;',
             'see report.json for each run’s timestamp boundary, raw paths, checks, and main-thread counts.', '']
    return '\n'.join(rows)


def self_test():
    class Tests(unittest.TestCase):
        def test_fixture_metrics(self):
            result = analyze(HERE / 'fixtures/events.jsonl', HERE / 'fixtures/main.sample.txt', duration=1, refresh=10)
            self.assertEqual(result['metrics']['frames'], 10)
            self.assertEqual(result['metrics']['main_thread_busy_pct'], 27)
            self.assertAlmostEqual(result['metrics']['receipt_to_submit_p95_ms'], 4.0)
            self.assertEqual(result['metrics']['gaps_over_50_ms'], 10)
            self.assertFalse(result['pass'])
        def test_main_only_and_no_double_counting(self):
            busy = sample_busy(HERE / 'fixtures/main.sample.txt')
            self.assertEqual((busy['total_samples'], busy['idle_samples']), (1000, 730))
        def test_percentile_and_thresholds(self):
            self.assertEqual(percentile([1, 2, 3, 4], 50), 2.5)
            m = {'frames': 1368, 'frame_interval_p99_ms': 2000 / 144, 'gaps_over_50_ms': 0,
                 'receipt_to_submit_p95_ms': 4, 'main_thread_busy_pct': 35}
            self.assertTrue(all(c['pass'] for c in absolute_gate(m, 1440, 144).values()))
            for name in m:
                bad = dict(m)
                bad[name] += -1 if name == 'frames' else 0.001
                self.assertFalse(all(c['pass'] for c in absolute_gate(bad, 1440, 144).values()))
        def test_regression_zero_baseline(self):
            self.assertTrue(delta(100, 105, 'lower')['pass'])
            self.assertFalse(delta(100, 105.01, 'lower')['pass'])
            self.assertFalse(delta(0, 1, 'lower')['pass'])
            self.assertTrue(delta(0, 0, 'lower')['pass'])
            self.assertTrue(delta(100, 95, 'higher')['pass'])
            self.assertFalse(delta(100, 94.99, 'higher')['pass'])
        def test_invalid_numbers(self):
            for value in (float('nan'), float('inf'), -1, True, '3'):
                with self.assertRaises(ValueError):
                    finite(value, 'test')
        def test_empty_percentile(self):
            with self.assertRaises(ValueError):
                percentile([], 95)
        def test_real_rate_window_and_paired_evidence(self):
            with tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                sample = HERE / 'fixtures/main.sample.txt'
                events = root / 'events.jsonl'
                rows = [{'kind': 'input_latency', 'pid': 42, 'run_id': '1000000000-42',
                         'mode': 'interactive', 'submit_main_ms': (i + 0.5) * 1000 / 144,
                         'receipt_to_submit_ms': 2} for i in range(1440)]
                events.write_text(''.join(json.dumps(r) + '\n' for r in rows))
                poster = root / 'poster.jsonl'
                post = [{'kind': 'poster_start', 'pid': 42, 'start_unix_s': 1000,
                         'duration_s': 10, 'rate_hz': 144},
                        {'kind': 'poster_end', 'pid': 42, 'complete': True, 'elapsed_s': 10,
                         'movement_events': 1440, 'late_slots': 0, 'max_lateness_ms': 0.5}]
                poster.write_text(''.join(json.dumps(r) + '\n' for r in post))
                result = analyze(events, sample, refresh=144, poster=poster)
                self.assertTrue(result['pass'])
                self.assertEqual(result['metrics']['frames'], 1440)
                self.assertEqual(result['timestamp_boundary'], 'queue submit')
                manifest = {'complete': True, 'runs_each': 3, 'refresh_hz': 144, 'runs': []}
                for trial in range(1, 4):
                    for label in ('base', 'candidate'):
                        manifest['runs'].append({'label': label, 'trial': trial, 'workload': 'large',
                                                 'gesture': 'finger', 'jsonl': str(events),
                                                 'sample': str(sample), 'poster': str(poster)})
                path = root / 'manifest.json'
                path.write_text(json.dumps(manifest))
                self.assertTrue(paired(path)['pass'])
                manifest['runs'].pop()
                path.write_text(json.dumps(manifest))
                with self.assertRaises(ValueError):
                    paired(path)
                # A long quiet tail must fail even if the reported inter-frame
                # intervals before it were perfect.
                events.write_text(''.join(json.dumps(r) + '\n' for r in rows[:1300]))
                frozen = analyze(events, sample, refresh=144, poster=poster)
                self.assertFalse(frozen['pass'])
                self.assertEqual(frozen['metrics']['gaps_over_50_ms'], 1)
                post[-1]['movement_events'] = 1
                poster.write_text(''.join(json.dumps(r) + '\n' for r in post))
                with self.assertRaises(ValueError):
                    analyze(events, sample, refresh=144, poster=poster)
    return unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(Tests)).wasSuccessful()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--self-test', action='store_true')
    parser.add_argument('--manifest', type=Path)
    parser.add_argument('--jsonl', type=Path)
    parser.add_argument('--sample', type=Path)
    parser.add_argument('--poster', type=Path)
    parser.add_argument('--refresh-hz', type=float)
    parser.add_argument('--input-hz', type=float, default=144)
    parser.add_argument('--duration', type=float, default=10)
    parser.add_argument('--start-ms', type=float, default=0)
    parser.add_argument('--report', type=Path, help='write JSON here and Markdown to the same stem')
    args = parser.parse_args()
    if args.self_test:
        return 0 if self_test() else 1
    try:
        if args.manifest:
            result = paired(args.manifest)
        elif args.jsonl and args.sample:
            result = analyze(args.jsonl, args.sample, args.refresh_hz, args.duration, args.start_ms, args.poster, args.input_hz)
        else:
            parser.error('use --manifest or both --jsonl and --sample')
        output = json.dumps(result, indent=2, allow_nan=False) + '\n'
        if args.report:
            args.report.write_text(output)
            args.report.with_suffix('.md').write_text(markdown(result))
            print(f'{"PASS" if result["pass"] else "FAIL"}: {args.report}')
        else:
            print(output, end='')
        return 0 if result['pass'] else 1
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f'scroll-gate: invalid/incomplete evidence: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
