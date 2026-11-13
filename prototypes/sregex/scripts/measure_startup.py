#!/usr/bin/env python3
"""Run serially, after throughput benchmarking, in fresh subprocesses."""
import json
import shutil
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXE = ROOT / 'target/release/sregex-prototype'
LANGS = ['rust','javascript','python','cpp','json','markdown','yaml','html']

def write(path, value):
    if path.exists():
        b = ROOT / 'backups' / f'{time.time_ns()}-{path.name}'
        b.parent.mkdir(exist_ok=True)
        shutil.copy2(path,b)
    temp = path.with_suffix('.tmp')
    temp.write_text(json.dumps(value, ensure_ascii=False, indent=2)+'\n')
    temp.replace(path)

for rep in range(3):
    subprocess.run([str(EXE),'compile','--output',f'output/compile-{rep}.json'],cwd=ROOT,check=True)
memory = []
for lang in LANGS:
    for engine in ['onig','fancy','multi']:
        p = subprocess.run([str(EXE),'memory','--languages',lang,'--engine',engine],cwd=ROOT,check=True,text=True,capture_output=True)
        memory.append(json.loads(p.stdout))
        print(lang,engine,'RSS MiB',round(memory[-1]['loaded_rss_bytes']/2**20,1),flush=True)
        write(ROOT/'output/memory.json',memory)
