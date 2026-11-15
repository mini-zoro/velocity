#!/usr/bin/env python3
"""Serial, alternating, process-isolated pairs. Build both sides before invocation."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
p = argparse.ArgumentParser()
p.add_argument('--suite', choices=['legacy', 'budgets', 'first', 'highlight'], required=True)
p.add_argument('--executables', type=Path, default=Path('/tmp/velocity-r11'))
p.add_argument('--data', type=Path, default=ROOT / '../velocity/spikes/data')
p.add_argument('--case', action='append', dest='only_cases')
p.add_argument('--pairs', type=int, default=5)
p.add_argument('--resume', action='store_true')
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
assert a.pairs >= 5
budget = ROOT / 'target/release/examples/budgets'
if a.suite == 'legacy':
    # The parent prepares its Unicode fixture before filtering all actual workers out.
    subprocess.run([str(a.executables / 'text-before'), '--impl', 'vel-text', '--only', 'prepare'], check=True, cwd=ROOT)
    cases = [('text-load-' + corpus, 'text', ['--worker', 'vel-text', corpus, 'load', str(path)]) for corpus, path in [
        ('ascii-100mb', a.data / 'text-100mb.txt'), ('ascii-1gb', a.data / 'text-1gb.txt'), ('unicode-100mb', Path(tempfile.gettempdir()) / 'velocity-vel-text-unicode-100mb.txt')]]
    cases += [('text-ops', 'text', ['--worker', 'vel-text', 'ascii-1gb', 'ops', str(a.data / 'text-1gb.txt')])]
    cases += [(op, 'buffer', ['--worker', op, str(a.data / 'text-100mb.txt')]) for op in ['type_10000', 'type_100000', 'find_10000', 'undo_10000', 'delete_50mb', 'undo_50mb', 'map_100000', 'undo_metadata']]
elif a.suite == 'budgets':
    cases = [(op, 'budget', [op]) for op in ['load', 'insert', 'word', 'end', 'window']]
elif a.suite == 'highlight':
    cases = [(op, 'highlight', [op]) for op in ['deep-viewport', 'long-viewport']]
else:
    fixture = a.data / 'text-r11-4gb-ascii.txt'
    subprocess.run([str(budget), 'prepare', str(fixture), '4000000000'], check=True)
    cases = [('first', 'budget', ['first'])]
a.output.parent.mkdir(parents=True, exist_ok=True)
done = {(x['case'], x['pair'], x['side']) for x in map(json.loads, a.output.read_text().splitlines())} if a.resume and a.output.exists() else set()
with a.output.open('a' if a.resume else 'w') as out:
    for case, group, arguments in cases:
        if a.only_cases and case not in a.only_cases: continue
        for pair in range(a.pairs):
            for side in (['A', 'B'] if pair % 2 == 0 else ['B', 'A']):
                if (case, pair, side) in done: continue
                print(f'{case} pair {pair + 1}/{a.pairs} {side}', file=sys.stderr, flush=True)
                if group == 'highlight':
                    command = [str(ROOT / 'target/release/examples/huge_bench'), *arguments, side]
                elif group == 'budget':
                    command = [str(budget), *arguments, side]
                    if case == 'load': command += [str(a.data / 'text-1gb.txt')]
                    if case == 'first': command += [str(fixture)]
                else:
                    command = [str(a.executables / (f'{group}-' + ('before' if side == 'A' else 'after'))), *arguments]
                env = dict(os.environ, VEL_TEXT_BENCH_RUN=str(pair + 1), VEL_BUFFER_BENCH_RUN=str(pair + 1))
                result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True, check=True)
                for line in result.stdout.splitlines():
                    row = json.loads(line)
                    row.update(case=case, pair=pair, side=side, order='AB' if pair % 2 == 0 else 'BA')
                    out.write(json.dumps(row) + '\n')
                out.flush()
