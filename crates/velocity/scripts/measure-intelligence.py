#!/usr/bin/env python3
"""Compare the identical round-7/8 parser probe, then measure index/popup CPU workloads."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import platform
import shutil
import statistics
import subprocess


def main():
    repo = Path(__file__).resolve().parents[3]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--old', type=Path, required=True)
    parser.add_argument('--new', type=Path, default=repo / 'target/release/velocity')
    parser.add_argument('--runs', type=int, default=5)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    if args.runs < 5:
        parser.error('at least five independent processes per workload')
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    environment = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                   'platform': platform.platform(), 'gui_started': False, 'gpu_started': False,
                   'schedule': 'one process at a time; alternate/reverse old-new parser pairs, then intelligence',
                   'reference': 'git archive 5b128ba; identical parse_bench.rs plus CLI/main probe hook; public syntax submodule copied',
                   'head': subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),
                   'binaries': {}, 'source_sha256': {}}
    for source in sorted((repo / 'crates/velocity/src').glob('*.rs')):
        environment['source_sha256'][str(source.relative_to(repo))] = hashlib.sha256(source.read_bytes()).hexdigest()
    resources = sorted((repo/'crates/velocity/resources').rglob('*'))
    for source in resources + [repo/'crates/velocity/Cargo.toml',repo/'crates/vel-commands/src/lib.rs']:
        if source.is_file():
            environment['source_sha256'][str(source.relative_to(repo))] = hashlib.sha256(source.read_bytes()).hexdigest()
    for label, source in [('old', args.old), ('new', args.new)]:
        binary = output / ('velocity-' + label)
        shutil.copy2(source, binary)
        environment['binaries'][label] = {'source': str(source),
            'sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
    (output/'environment.json').write_text(json.dumps(environment, indent=2)+'\n')
    runs = {'old': [], 'new': [], 'index': [], 'index_100k': []}

    def run(label, number, mode, words=None):
        path = output/f'{label}-{number}.jsonl'
        binary = output/('velocity-old' if label == 'old' else 'velocity-new')
        command = [str(binary), mode, '--data-dir', str(output/'profiles'/f'{label}-{number}'),
                   '--output', str(path), '--timeout', '300']
        if words is not None:
            command.extend(['--intel-words',str(words)])
        with path.with_suffix('.log').open('w') as log:
            subprocess.run(command, cwd=repo, stdout=log, stderr=subprocess.STDOUT,
                           timeout=330, check=True)
        rows = [json.loads(line) for line in path.read_text().splitlines()]
        if not any(r['kind']=='complete' and r.get('complete') for r in rows):
            raise RuntimeError(f'{label} {number} did not complete')
        if label in ['old','new']:
            record = next(r for r in rows if r['kind']=='ten_file_parser_memory_cpu')
        else:
            memory = next(r for r in rows if r['kind']=='memory')
            timing = next(r for r in rows if r['kind']=='intelligence_cpu')
            record = {'phys_footprint_bytes':memory['phys_footprint_bytes'],
                      'rss_bytes':memory['rss_bytes'], 'typing_ms':timing['typing_ms'],
                      'definition_ms':timing['definition_ms']}
        runs[label].append(record)
        print(label, number, json.dumps({k:v for k,v in record.items() if k not in ['schema','run_id','pid']}),flush=True)

    for number in range(1,args.runs+1):
        for label in (['old','new'] if number % 2 else ['new','old']):
            run(label,number,'--headless-parse-memory')
    for label, words in [('index',0),('index_100k',100000)]:
        for number in range(1,args.runs+1):
            run(label,number,'--headless-intelligence',words)
    summary = {'environment':environment,'runs':runs,'median':{}}
    for label, records in runs.items():
        result = {'phys_footprint_bytes':statistics.median(r['phys_footprint_bytes'] for r in records),
                  'rss_bytes':statistics.median(r['rss_bytes'] for r in records)}
        if label in ['old','new']:
            result.update({k:statistics.median(r[k] for r in records) for k in ['cpu_ms','wall_ms']})
        else:
            result.update({key:{stat:statistics.median(r[key][stat] for r in records)
                               for stat in ['p50','p95','max']} for key in ['typing_ms','definition_ms']})
        summary['median'][label]=result
    old,new = summary['median']['old'],summary['median']['new']
    summary['parser_change'] = {'cpu_percent':100*(new['cpu_ms']/old['cpu_ms']-1),
        'phys_footprint_percent':100*(new['phys_footprint_bytes']/old['phys_footprint_bytes']-1),
        'phys_footprint_bytes':new['phys_footprint_bytes']-old['phys_footprint_bytes']}
    summary['index_memory_change_vs_round7_bytes'] = summary['median']['index']['phys_footprint_bytes']-old['phys_footprint_bytes']
    summary['synthetic_100k_memory_change_vs_round7_bytes'] = summary['median']['index_100k']['phys_footprint_bytes']-old['phys_footprint_bytes']
    (output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print(json.dumps({k:v for k,v in summary.items() if k not in ['runs','environment']},indent=2),flush=True)


if __name__=='__main__':
    main()
