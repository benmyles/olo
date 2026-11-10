import json,os,socket,subprocess,time
from pathlib import Path
root=Path('/var/tmp/codex-browser-compile-queue');screen='codex-browser'
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
def ask(pid,req):
 with socket.socket(socket.AF_UNIX) as s:
  s.settimeout(5);s.connect(f'/run/user/1000/olo-menus-{pid}.sock');s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
def workers(pid):
 out=[]
 for p in Path(f'/proc/{pid}/task').iterdir():
  try:
   if (p/'comm').read_text().startswith('olo-style-comp'):out.append(int(p.name))
  except FileNotFoundError:pass
 return out
def cpu(pid):
 p=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split();return (int(p[11])+int(p[12]))*1000/os.sysconf('SC_CLK_TCK')
def stop():
 run('wait',screen,'--free')
 for w in json.loads(run('windows',screen)):
  if w['app_id']=='olo.browser':
   assert ask(w['pid'],{'invoke':'quit'})['ok']
   end=time.monotonic()+5
   while Path(f'/proc/{w["pid"]}').exists():
    assert time.monotonic()<end;time.sleep(.01)
rows=[]
for variant in ['candidate']:
 stop();run('wait',screen,'--free')
 binary=root/'browser-before' if variant=='baseline' else Path.cwd()/'target/release/olo-browser'
 pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),binary,root/'stop.html'))
 run('wait',screen,'--role','Button','--name','Compile large stylesheet','--timeout',10000);run('wait',screen,'--idle')
 run('click',screen,'--role','Button','--name','Compile large stylesheet')
 end=time.monotonic()+10
 while not workers(pid):
  assert time.monotonic()<end,'background compile not observed';time.sleep(.001)
 run('wait',screen,'--free');observed=workers(pid);assert observed
 before=cpu(pid);start=time.monotonic_ns();assert ask(pid,{'invoke':'stop'})['ok'];command=time.monotonic_ns()
 end=time.monotonic()+10
 while workers(pid):
  assert time.monotonic()<end;time.sleep(.001)
 elapsed=(time.monotonic_ns()-start)/1e6
 tree=json.loads(run('tree',screen,'--json'))
 assert 'Stopped' in json.dumps(tree),tree
 row=dict(variant=variant,pid=pid,observed_workers=observed,command_ms=(command-start)/1e6,stop_to_workers_exit_ms=elapsed,cpu_after_stop_ms=cpu(pid)-before,tree=tree)
 rows.append(row);(root/'stop-smoke.json').write_text(json.dumps(rows,indent=2)+'\n');print(variant,row['command_ms'],elapsed,row['cpu_after_stop_ms'],flush=True)
stop()
