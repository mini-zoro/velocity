#!/usr/bin/env python3
import datetime,json,pathlib,subprocess
root=pathlib.Path(__file__).resolve().parents[1]
checks={'hardware':['system_profiler','SPHardwareDataType'],'os':['sw_vers'],'rustc':['rustc','--version','--verbose'],'cargo':['cargo','--version'],'power':['pmset','-g','batt'],'memory':['vm_stat']}
result={'measured_at':datetime.datetime.now().astimezone().isoformat()}
for name,cmd in checks.items():
 p=subprocess.run(cmd,capture_output=True,text=True)
 text='\n'.join(l for l in p.stdout.splitlines() if not any(s in l for s in ['Serial Number','UUID','UDID']))
 result[name]={'status':p.returncode,'stdout':text,'stderr':p.stderr}
(root/'data/machine.json').write_text(json.dumps(result,indent=2)+'\n')
