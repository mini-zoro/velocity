#!/usr/bin/env python3
"""Alternating, sequential Git/diff/spelling A/B and native full-history CPU measurements."""
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
    parser.add_argument('--binary', type=Path, default=repo/'target/release/velocity')
    parser.add_argument('--runs', type=int, default=10)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    if args.runs < 5:
        parser.error('at least five process pairs')
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = output/'velocity'
    shutil.copy2(args.binary, binary)
    env = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
           'platform': platform.platform(), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
           'schedule': 'one process at a time; reverse on/off order each pair; no compilation during series',
           'head': subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),
           'gui_started': False, 'gpu_started': False,
           'cargo_lock_sha256': hashlib.sha256((repo/'Cargo.lock').read_bytes()).hexdigest(),
           'source_sha256': {str(p.relative_to(repo)): hashlib.sha256(p.read_bytes()).hexdigest()
               for crate in ['velocity','vel-session','vel-commands','vel-config']
               for p in sorted((repo/'crates'/crate).rglob('*'))
               if p.is_file() and (p.suffix in ['.rs','.toml'] or '/resources/' in str(p))}}
    (output/'environment.json').write_text(json.dumps(env,indent=2)+'\n')
    records = {'off': [], 'on': [], 'history': []}

    def run(label, index):
        profile = output/f'profile-{label}-{index}'
        raw = output/f'{label}-{index}.jsonl'
        command = [str(binary), '--headless-review-history' if label == 'history' else '--headless-intelligence',
                   '--data-dir', str(profile), '--output', str(raw), '--timeout','300']
        if label != 'history':
            command += ['--review-ui',label,'--intel-words','0']
        with raw.with_suffix('.log').open('w') as log:
            subprocess.run(command,cwd=repo,stdout=log,stderr=subprocess.STDOUT,timeout=330,check=True)
        rows = [json.loads(line) for line in raw.read_text().splitlines()]
        if not any(r.get('kind')=='complete' and r.get('complete') for r in rows):
            raise RuntimeError(f'{label} {index}: missing completion')
        kinds = ['full_history_save','full_history_restore'] if label=='history' else ['intelligence_cpu']
        selected = [r for r in rows if r.get('kind') in kinds]
        records[label].append({'index':index,'raw':str(raw),'records':selected})
        if label=='history':
            print(label,index,selected[0]['worker_ms'],selected[1]['first_window_cpu_ms'],flush=True)
        else:
            print(label,index,selected[0]['typing_ms'],flush=True)

    for index in range(1,args.runs+1):
        for label in (['off','on'] if index%2 else ['on','off']):
            run(label,index)
    for index in range(1,6):
        run('history',index)
    medians = {label: {stat:statistics.median(r['records'][0]['typing_ms'][stat] for r in runs)
                      for stat in ['p50','p95','max']} for label,runs in records.items() if label!='history'}
    ratios = {}
    randomizer = random.Random(9)
    for stat in ['p50','p95']:
        before=[r['records'][0]['typing_ms'][stat] for r in records['off']]
        after=[r['records'][0]['typing_ms'][stat] for r in records['on']]
        boot=sorted(statistics.median(randomizer.choices(after,k=len(after)))/statistics.median(randomizer.choices(before,k=len(before))) for _ in range(10000))
        ratios[stat]={'percent':(statistics.median(after)/statistics.median(before)-1)*100,'ratio_ci95':[boot[250],boot[9750]]}
    summary={'environment':env,'process_medians':medians,'active_vs_disabled':ratios,
             'history': {'worker_p50_ms':statistics.median(r['records'][0]['worker_ms']['p50'] for r in records['history']),
                         'worker_p95_ms':statistics.median(r['records'][0]['worker_ms']['p95'] for r in records['history']),
                         'worker_max_ms':max(r['records'][0]['worker_ms']['max'] for r in records['history']),
                         'restore_median_ms':statistics.median(r['records'][1]['first_window_cpu_ms'] for r in records['history']),
                         'restore_max_ms':max(r['records'][1]['first_window_cpu_ms'] for r in records['history'])},
             'runs':records}
    (output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print(json.dumps({k:v for k,v in summary.items() if k not in ['environment','runs']},indent=2),flush=True)


if __name__=='__main__':
    main()
