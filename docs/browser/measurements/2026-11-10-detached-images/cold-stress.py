import os,json,subprocess,resource
from pathlib import Path
root=Path('/var/tmp/codex-browser-live');rows=[]
def no_core():resource.setrlimit(resource.RLIMIT_CORE,(0,0))
for i in range(12):
 for mode,flags in [('default',None),('sealed','--no-extensible-ro-snapshot'),('no-snapshot',None)]:
  env=dict(os.environ);env['XDG_CACHE_HOME']=str(root/'cold-cache'/f'{mode}-{i}')
  env.pop('OLO_V8_FLAGS',None);env.pop('OLO_NO_SNAPSHOT',None)
  if flags:env['OLO_V8_FLAGS']=flags
  if mode=='no-snapshot':env['OLO_NO_SNAPSHOT']='1'
  r=subprocess.run(['target/release/examples/dom_lifetime_bench'],env=env,capture_output=True,text=True,timeout=15,preexec_fn=no_core)
  row={'mode':mode,'iteration':i,'returncode':r.returncode,'stdout':r.stdout,'stderr':r.stderr};rows.append(row)
  (root/'cold-stress.json').write_text(json.dumps(rows,indent=2));print(mode,i,r.returncode,r.stderr[-160:],flush=True)
