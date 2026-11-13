#!/usr/bin/env python3
"""Create deterministic corpora. All writes and downloads stay under spikes/data."""
import argparse, hashlib, json, os, pathlib, subprocess
ROOT=pathlib.Path(__file__).resolve().parents[1]; DATA=ROOT/'data'
def sha(p):
 h=hashlib.sha256()
 with p.open('rb') as f:
  for b in iter(lambda:f.read(8*1024*1024),b''): h.update(b)
 return h.hexdigest()
def repeat_to(p, seed, size):
 if p.exists() and p.stat().st_size==size:return
 with p.with_suffix('.tmp').open('wb') as f:
  n=0
  while n<size:
   block=seed[:min(len(seed),size-n)]; f.write(block); n+=len(block)
 os.replace(p.with_suffix('.tmp'),p)
def main():
 DATA.mkdir(exist_ok=True)
 pkg=DATA/'Packages'
 if not pkg.exists():subprocess.run(['git','clone','--depth','1','https://github.com/sublimehq/Packages',str(pkg)],check=True)
 upstream=DATA/'syntect-upstream'
 if not upstream.exists():subprocess.run(['git','clone','--depth','1','https://github.com/trishume/syntect',str(upstream)],check=True)
 registry=pathlib.Path(os.environ.get('BENCH_SOURCE_REGISTRY',str(pathlib.Path.home()/'.cargo/registry/src')))
 manifest=DATA/'source_manifest.json'
 if manifest.exists():sources=json.loads(manifest.read_text())
 else:
  sources=[]; total=0
  for p in sorted(registry.glob('*/*/src/**/*.rs')):
   if p.is_file() and p.stat().st_size<500_000:
    b=p.read_bytes(); sources.append({'path':str(p),'sha256':hashlib.sha256(b).hexdigest()});total+=len(b)
    if total>=12_000_000:break
  if total==0:raise RuntimeError('No real Rust source in '+str(registry))
  manifest.write_text(json.dumps(sources,indent=2)+'\n')
 rust=b''.join(b'\n// SOURCE: '+pathlib.Path(s['path']).name.encode()+b'\n'+pathlib.Path(s['path']).read_bytes()+b'\n' for s in sources)
 repeat_to(DATA/'rust-10mb.rs',rust,10_000_000)
 ascii_seed=rust.decode('utf8').encode('ascii','replace')
 repeat_to(DATA/'text-100mb.txt',ascii_seed,100_000_000)
 repeat_to(DATA/'text-1gb.txt',ascii_seed,1_000_000_000)
 js=[]
 for name in ['lodash.js','lodash.min.js']:
  p=DATA/name
  if not p.exists():
   url='https://raw.githubusercontent.com/lodash/lodash/4.17.21/lodash.js' if name=='lodash.js' else 'https://cdn.jsdelivr.net/npm/lodash@4.17.21/lodash.min.js'
   subprocess.run(['curl','--http1.1','-fsSL','--retry','3','--retry-all-errors','--max-time','45',url,'-o',str(p)],check=True)
  js.append(p.read_bytes()+b'\n')
 repeat_to(DATA/'javascript-10mb.js',b'\n'.join(js),10_000_000)
 j=DATA/'json-20mb.json'
 if not j.exists():
  row=json.dumps({'name':'editor benchmark','path':'src/runtime/main.rs','line':12345,'enabled':True,'tags':['Rust','JSON','syntax'],'nested':{'text':'hello world','values':[1,2,3,4,5] }},separators=(',',':')).encode()
  n=(20_000_000-3)//(len(row)+1)
  with j.open('wb') as f:f.write(b'[\n');f.write((row+b',\n')*(n-1));f.write(row+b'\n]\n')
 mdpaths=sorted(registry.glob('*/*/README.md'))
 mdseed=b'\n\n'.join(p.read_bytes() for p in mdpaths[:80])
 repeat_to(DATA/'markdown-5mb.md',mdseed,5_000_000)
 corpus={p.name:{'bytes':p.stat().st_size,'sha256':sha(p)} for p in [DATA/n for n in ['text-100mb.txt','text-1gb.txt','rust-10mb.rs','javascript-10mb.js','json-20mb.json','markdown-5mb.md','lodash.js','lodash.min.js']]}
 corpus['packages_commit']=subprocess.check_output(['git','-C',str(pkg),'rev-parse','HEAD'],text=True).strip()
 corpus['syntect_example_commit']=subprocess.check_output(['git','-C',str(upstream),'rev-parse','HEAD'],text=True).strip()
 corpus['rust_source_files']=len(sources);corpus['buffer_encoding']='ASCII-transliterated real crate sources; byte offset == Unicode scalar offset'
 (DATA/'corpora.json').write_text(json.dumps(corpus,indent=2)+'\n')
 print(json.dumps(corpus,indent=2))
if __name__=='__main__':main()
