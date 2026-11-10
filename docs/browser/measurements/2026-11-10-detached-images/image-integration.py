import subprocess,json,re,time
from pathlib import Path
root=Path('/var/tmp/codex-browser-live');screen='codex-browser';rows=[]
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
page=Path.cwd()/'docs/browser/measurements/2026-11-10-detached-images/detached-image.html'
assert not json.loads(run('windows',screen))
for variant,expected in [('before','[true,1,0,0,0,0]'),('after','[true,1,17,91,203,255]')]:
 pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'XDG_CACHE_HOME='+str(root/'warm-cache'/variant),root/f'browser-{variant}',page))
 try:
  run('wait',screen,'--text',expected,'--timeout',15000)
  rows.append({'variant':variant,'expected':expected,'tree':json.loads(run('tree',screen,'--json')),'log':Path('/run/user/1000/olo-screens/codex-browser-env.log').read_text()})
  print(variant,expected,flush=True)
 finally:
  for w in json.loads(run('windows',screen)):
   assert w['pid']==pid;run('close',screen,'--window',w['id'])
  end=time.monotonic()+10
  while Path(f'/proc/{pid}').exists() and time.monotonic()<end:time.sleep(.05)
  assert not Path(f'/proc/{pid}').exists()
(root/'image-integration.json').write_text(json.dumps(rows,indent=2))
