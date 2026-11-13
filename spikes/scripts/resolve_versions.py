#!/usr/bin/env python3
"""Snapshot the newest non-yanked stable releases from the crates.io API."""
import concurrent.futures, json, pathlib, subprocess, datetime
root = pathlib.Path(__file__).resolve().parents[1]
names = 'ropey crop syntect tree-sitter tree-sitter-rust tree-sitter-javascript regex ignore grep-searcher grep-regex nucleo-matcher cosmic-text serde serde_json anyhow walkdir libc'.split()
def query(name):
 key=(name if len(name)==1 else '2/'+name if len(name)==2 else '3/'+name[0]+'/'+name if len(name)==3 else name[:2]+'/'+name[2:4]+'/'+name)
 url='https://index.crates.io/'+key
 cache=root/'data'/('index-'+name+'.jsonl')
 versions=None
 if cache.exists():
  try:versions=[json.loads(l) for l in cache.read_text().splitlines()]
  except ValueError:pass
 if versions is None:
  tmp=cache.with_suffix('.tmp')
  subprocess.run(['curl','--http1.1','-fsSL','--retry','4','--retry-all-errors','--max-time','40',url,'-o',str(tmp)],check=True)
  versions=[json.loads(l) for l in tmp.read_text().splitlines()]
  tmp.replace(cache)
 versions=[v for v in versions if not v.get('yanked') and '-' not in v['vers']]
 v=max(versions, key=lambda v:tuple(map(int,v['vers'].split('.'))))
 return name, {'version':v['vers'],'index_url':url,'checksum':v['cksum'],'rust_version':v.get('rust_version')}
with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool: versions=dict(pool.map(query,names))
out={'queried_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'crates':versions}
p=root/'data/crate_versions.json'; p.write_text(json.dumps(out,indent=2)+'\n')
for n,v in versions.items(): print(n, v['version'])
