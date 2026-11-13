#!/usr/bin/env python3
"""Serial release runs, per-process time -l, timeout kills the process group."""
import argparse,datetime,json,os,pathlib,signal,subprocess,time,shutil
ROOT=pathlib.Path(__file__).resolve().parents[1]; RAW=ROOT/'data/raw'
def jobs():
 out=[]
 def add(name,group,cmd,timeout=240):out.append((name,group,cmd,timeout))
 for backend in ['fancy','onig']:
  b=f'target/release/bench-syntax-{backend}'
  for mode in ['compat','tests','startup']:add(f'syntax-{backend}-{mode}','syntax-'+mode,[b,mode],240)
 for size in ['100mb','1gb']:
  for engine in ['ropey','crop','string']:
   for mode in (['load'] if engine=='string' else ['load','ops']):add(f'buffer-{engine}-{size}-{mode}','buffer',['target/release/bench-buffer','--file',f'data/text-{size}.txt','--engine',engine,'--mode',mode],360)
 for mode in ['regex','files','fuzzy']:add('search-'+mode,'search',['target/release/bench-search',mode],240)
 add('text','text',['target/release/bench-text'],180)
 for language in ['Rust','JavaScript']:add('tree-'+language,'tree',['target/release/bench-syntax-fancy','tree','--language',language],240)
 for backend in ['fancy','onig']:
  for language in ['Rust','JavaScript','JSON','Markdown']:
   for mode in ['parse','highlight']:
    add(f'throughput-{backend}-{language}-{mode}','throughput',[f'target/release/bench-syntax-{backend}','throughput','--language',language,'--mode',mode],240)
    if language=='Markdown':add(f'throughput-{backend}-{language}-{mode}-bundled','throughput',[f'target/release/bench-syntax-{backend}','throughput','--language',language,'--mode',mode,'--source','bundled'],240)
 return out
def main():
 p=argparse.ArgumentParser();p.add_argument('--only',default='all');p.add_argument('--force',action='store_true');a=p.parse_args();RAW.mkdir(exist_ok=True)
 probe=subprocess.run(['/usr/bin/time','-l','/usr/bin/true'],capture_output=True,text=True)
 use_time=probe.returncode==0
 (RAW/'rss-method.json').write_text(json.dumps({'time_l_supported':use_time,'probe_stderr':probe.stderr,'fallback':'benchmark libc::getrusage(RUSAGE_SELF).ru_maxrss; macOS unit bytes'},indent=2)+'\n')
 summary_path=RAW/'runs.json';summary=json.loads(summary_path.read_text()) if summary_path.exists() else {}
 selected=a.only.split(',')
 for name,group,cmd,timeout in jobs():
  if 'all' not in selected and name not in selected and group not in selected:continue
  if not a.force and name in summary and summary[name]['status']=='ok':continue
  for path in [RAW/(name+'.jsonl'),RAW/(name+'.time.txt')]:
   if path.exists():
    backup=ROOT/'data/backups'/str(time.time_ns())/path.relative_to(ROOT);backup.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(path,backup)
  print(f'[{datetime.datetime.now().isoformat(timespec="seconds")}] {name}',flush=True)
  start=time.monotonic();status='ok';code=None
  with (RAW/(name+'.jsonl')).open('w') as out,(RAW/(name+'.time.txt')).open('w') as err:
   proc=subprocess.Popen((['/usr/bin/time','-l'] if use_time else [])+cmd,cwd=ROOT,stdout=out,stderr=err,start_new_session=True)
   try:code=proc.wait(timeout=timeout);status='ok' if code==0 else 'failed'
   except subprocess.TimeoutExpired:
    os.killpg(proc.pid,signal.SIGKILL);proc.wait();status='timeout'
  row={'command':cmd,'cwd':str(ROOT),'started_at':datetime.datetime.now().isoformat(),'wall_seconds':time.monotonic()-start,'status':status,'exit_code':code,'timeout_seconds':timeout}
  text=(RAW/(name+'.time.txt')).read_text(errors='replace')
  import re
  m=re.search(r'(\d+)\s+maximum resident set size',text);row['time_peak_rss_bytes']=int(m[1]) if m else None
  records=[]
  for line in (RAW/(name+'.jsonl')).read_text(errors='replace').splitlines():
   try: records.append(json.loads(line))
   except ValueError:pass
  row['json_rows']=len(records)
  row['process_peak_rss_bytes']=max((r.get('peak_rss_bytes',0) for r in records),default=0)
  row['rss_method']='time -l + getrusage' if use_time else 'getrusage (time -l sysctl forbidden)'
  if status=='ok' and not records:row['status']='missing_output'
  summary[name]=row;tmp=summary_path.with_suffix('.tmp');tmp.write_text(json.dumps(summary,indent=2)+'\n');os.replace(tmp,summary_path)
  print(f'  {status}: {row["wall_seconds"]:.2f}s, rows={row["json_rows"]}, peak RSS={row["process_peak_rss_bytes"]}',flush=True)
if __name__=='__main__':main()
