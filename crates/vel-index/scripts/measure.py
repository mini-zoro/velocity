#!/usr/bin/env python3
"""Release evidence for lane B round 6. No sysctl/time wrapper; >=5 samples per timing."""
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def run(example, arguments):
    process = subprocess.Popen([str(ROOT / 'target/release/examples' / example), *arguments],
                               cwd=ROOT, stdout=subprocess.PIPE, text=True)
    rows = []
    for line in process.stdout:
        if line.startswith('{'):
            record = json.loads(line)
            rows.append(record)
            if record['case'] in ('corpus_cold', 'corpus_restart', 'million_memory'):
                print(json.dumps(record), flush=True)
    if process.wait() != 0:
        raise RuntimeError(f'{example} failed')
    return rows


def write(path, rows):
    path = ROOT / path
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(rows, indent=2) + '\n')
    temporary.replace(path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--corpus', type=Path)
    parser.add_argument('--skip-highlight', action='store_true')
    args = parser.parse_args()
    subprocess.run([str(ROOT / 'target/release/examples/index_bench'), 'prepare'], check=True)
    prior = ROOT / 'crates/vel-index/round6-timings.json'
    retained = [row for row in json.loads(prior.read_text()) if row['case'] in ('corpus_cold','corpus_restart')] if prior.exists() and not args.corpus else []
    rows = []
    for process in range(5):
        sample = run('index_bench', ['queries'])
        for row in sample:
            row['process'] = process
        rows.extend(sample)
    rows.extend(run('index_bench', ['edit']))
    rows.extend(retained)
    write('crates/vel-index/round6-timings.json', rows)
    write('crates/vel-complete/round6-timings.json', run('complete_bench', []))
    if not args.skip_highlight:
        saved_path = ROOT / 'crates/vel-highlight/timings.json'
        saved = saved_path.read_bytes()
        try:
            samples = run('highlight_bench', ['--worker-edits-only'])
            samples.extend(run('highlight_bench', ['--background-only']))
            write('crates/vel-highlight/round6-timings.json', samples)
        finally:
            temporary = saved_path.with_suffix('.restore')
            temporary.write_bytes(saved)
            temporary.replace(saved_path)
    if args.corpus:
        rows.extend(run('index_bench', ['corpus', str(args.corpus)]))
        write('crates/vel-index/round6-timings.json', rows)


if __name__ == '__main__':
    main()
