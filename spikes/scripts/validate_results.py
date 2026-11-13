#!/usr/bin/env python3
"""Check measurement arithmetic/coverage, independently of the report renderer."""
import json,math,pathlib,statistics
root=pathlib.Path(__file__).resolve().parents[1];raw=root/'data/raw'
runs=json.loads((raw/'runs.json').read_text());records=[];process_failures=[]
def check(value):
 if isinstance(value,dict):
  if 'samples_ms' in value:
   samples=value['samples_ms'];assert value['n']==len(samples)
   assert all(math.isfinite(x) and x>=0 for x in samples)
   assert math.isclose(value['median_ms'],statistics.median(samples),rel_tol=1e-12,abs_tol=1e-12)
  for v in value.values():check(v)
 elif isinstance(value,list):
  for v in value:check(v)
for name,run in runs.items():
 if run['status']!='ok':process_failures.append({'command':name,'status':run['status']})
 rows=[json.loads(l) for l in (raw/(name+'.jsonl')).read_text().splitlines() if l.startswith('{')]
 if run['status']=='ok':assert rows,name
 assert all(r['peak_rss_bytes']>0 for r in rows),name
 for row in rows:
  check(row);records.append(row)
  if row.get('status')=='ok' and 'mb_s' in row:
   assert math.isclose(row['mb_s'],row['bytes']/1000/row['time']['median_ms'],rel_tol=1e-12)
  if row.get('bench')=='search_fuzzy':
   steps=row['incremental_typing'];assert steps[-1]['matches']==row['matches']
   assert all(s['matches']<=s['candidates_before'] for s in steps)
for backend in ['fancy','onig']:
 compat=json.loads((root/f'data/syntax-{backend}-compat.json').read_text())
 assert compat['total']==compat['ok']+compat['failed']==len(compat['files'])
 assert compat['failed']==sum(compat['failure_categories'].values())
 d=json.loads((root/f'data/syntax-{backend}-tests.json').read_text())
 assert d['harness_self_check'].startswith('passed')
 assert len({r['language'] for r in d['tests']})==8
 for r in d['tests']:
  if 'result' not in r:assert r['status']=='blocked_syntax_not_loaded';continue
  v=r['result'];assert v['checked_cases']==v['passed_cases']+v['failed_cases']<=r['planned_cases']
  assert v['checked_cells']==v['passed_cells']+v['failed_cells']<=r['planned_cells']
for size in ['100mb','1gb']:
 a=json.loads((raw/f'buffer-ropey-{size}-load.jsonl').read_text());b=json.loads((raw/f'buffer-crop-{size}-load.jsonl').read_text());assert a['bytes']==b['bytes'] and a['lines']==b['lines']
summary={'commands_checked':len(runs),'json_records_checked':len(records),'process_failures':process_failures,'content_errors':[{'backend':r['backend'],'language':r['language'],'mode':r['mode'],'error':r['error']} for r in records if r.get('bench')=='syntax_throughput' and r.get('status')=='error'],'validation':'passed'}
(root/'data/validation.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary,indent=2))
