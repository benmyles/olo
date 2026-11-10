import os,json,subprocess,time
from pathlib import Path
root=Path('/var/tmp/codex-browser-live');screen='codex-browser';log=Path('/run/user/1000/olo-screens/codex-browser-env.log');rows=[]
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
assert not json.loads(run('windows',screen))
for i in range(32):
 for mode,variant,flags in [('production','after','')]:
  cache=root/'browser-cold'/f'{mode}-{i}'
  pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'XDG_CACHE_HOME='+str(cache),'OLO_V8_FLAGS='+flags,root/f'browser-{variant}',root/'ownership-bench.html'))
  deadline=time.monotonic()+8
  while Path(f'/proc/{pid}').exists() and 'OWNERSHIP_BENCH' not in log.read_text() and time.monotonic()<deadline:time.sleep(.05)
  text=log.read_text();passed=Path(f'/proc/{pid}').exists() and 'OWNERSHIP_BENCH' in text
  rows.append(dict(mode=mode,iteration=i,pid=pid,passed=passed,log=text));(root/'browser-cold-final.json').write_text(json.dumps(rows,indent=2))
  print(mode,i,pid,passed,text[-100:],flush=True)
  for w in json.loads(run('windows',screen)):
   assert w['pid']==pid
   run('close',screen,'--window',w['id'])
  end=time.monotonic()+10
  while Path(f'/proc/{pid}').exists() and time.monotonic()<end:time.sleep(.05)
  assert not Path(f'/proc/{pid}').exists()
