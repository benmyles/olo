import json,os,re,runpy,socket,subprocess,time,hashlib
from pathlib import Path
root=Path('/var/tmp/codex-browser-counters');screen='codex-browser'
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
def wins():return [w for w in json.loads(run('windows',screen)) if w['app_id']=='olo.browser']
def ask(pid,r):
 with socket.socket(socket.AF_UNIX) as s:
  s.settimeout(5);s.connect(f'/run/user/1000/olo-menus-{pid}.sock');s.sendall((json.dumps(r)+'\n').encode());return json.loads(s.makefile().readline())
def stop():
 for w in wins():
  ask(w['pid'],{'invoke':'quit'})
  deadline=time.monotonic()+5
  while Path(f'/proc/{w["pid"]}').exists() and time.monotonic()<deadline:time.sleep(.02)
  assert not Path(f'/proc/{w["pid"]}').exists()
for tag in ['counters','hidden','ordinary']:
 content='<div class=row>A paragraph with ordinary text.</div>'*1000
 style='body{font:16px Arial;margin:32px;counter-reset:n 0}.row{padding:2px}'
 if tag!='ordinary':style+='.row::before{content:counter(n) ". ";counter-increment:n}'
 if tag=='hidden':style+='.row::before{display:none}'
 (root/(tag+'.html')).write_text('<!doctype html><title>Counter performance '+tag+'</title><style>'+style+'</style>'+content)
bench=runpy.run_path('tools/dev/scroll-check');rows=[]
screeninfo=next(s for s in json.loads(run('list')) if s['name']==screen);assert screeninfo['state']=='running'
log=Path('/run/user/1000/olo-screens/codex-browser-env.log')
for tag in ['counters','hidden','ordinary']:
 for variant in ['baseline','candidate','candidate','baseline']:
  stop();run('wait',screen,'--free')
  binary=root/'olo-browser-before' if variant=='baseline' else Path.cwd()/'target/release/olo-browser'
  pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'OLO_BROWSER_TIMING=1','OLO_WEB_TIMING=1',binary,root/(tag+'.html')).strip())
  run('wait',screen,'--role','Document','--timeout',10000);run('wait',screen,'--idle')
  w=next(w for w in wins() if w['pid']==pid)
  run('place',screen,49,40,'1000x700','--window',w['id']);run('wait',screen,'--idle')
  compilers_before=subprocess.run(['ps','-C','rustc','-o','pid=,etimes=,pcpu='],capture_output=True,text=True).stdout
  start=log.stat().st_size
  for i in range(7):
   assert ask(pid,{'invoke':'reload'})['ok']
   run('wait',screen,'--idle')
  text=log.read_text()[start:]
  loads=[int(x) for x in re.findall(r'Browser: static page ready (\d+) ms',text)]
  assert len(loads)==7,(tag,variant,loads)
  idle=json.loads(subprocess.check_output(['/usr/bin/python3','tools/dev/cost',str(pid),'2','--json']))
  c=bench['Check'].__new__(bench['Check']);c.sock=screeninfo['olo_socket'];c.runtime='/run/user/1000';c.pid=pid;c.at=(500,400);c.sign=1
  run('wait',screen,'--free')
  scrolling=[c.run(k,90,900) for k in ['drag','wheel']]
  row=dict(compilers_before=compilers_before,compilers_after=subprocess.run(['ps','-C','rustc','-o','pid=,etimes=,pcpu='],capture_output=True,text=True).stdout,tag=tag,variant=variant,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),worker_ready_ms=loads,idle=idle,scrolling=scrolling,log=text)
  rows.append(row);print(tag,variant,loads,scrolling,flush=True)
  (root/'costs.json').write_text(json.dumps(rows,indent=2)+'\n')
stop()
