import json,os,re,subprocess,time,socket,hashlib,copy,runpy
from pathlib import Path
root=Path('/var/tmp/codex-browser-live');screen='codex-browser';log=Path('/run/user/1000/olo-screens/codex-browser-env.log')
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
def windows():return json.loads(run('windows',screen))
def stop():
 for w in windows():
  assert w['app_id'] in ('olo.browser','google-chrome'),w
  run('close',screen,'--window',w['id'])
  end=time.monotonic()+15
  while Path(f'/proc/{w["pid"]}').exists() and time.monotonic()<end:time.sleep(.05)
  assert not Path(f'/proc/{w["pid"]}').exists(),w
info=next(s for s in json.loads(run('list')) if s['name']==screen);assert info['state']=='running'
def ipc(body):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(info['olo_socket']);s.sendall((json.dumps(body)+'\n').encode());return json.loads(s.makefile().readline())
def usage(pid):
 fields=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()
 st=dict(l.split(':',1) for l in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in l)
 return {'cpu_ms':(int(fields[11])+int(fields[12]))*1000/os.sysconf('SC_CLK_TCK'),'rss_kib':int(st['VmRSS'].split()[0]),'peak_rss_kib':int(st['VmHWM'].split()[0])}
original=ipc({'type':'get_settings'})['settings'];rows=[];scrollbench=runpy.run_path('tools/dev/scroll-check')
try:
 stop()
 for variant in ['before','after']:
  pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'XDG_CACHE_HOME='+str(root/'warm-cache'/variant),root/f'browser-{variant}',root/'ownership-bench.html'))
  run('wait',screen,'--text','Ownership benchmark complete','--timeout',15000)
  run('wait',screen,'--idle');stop()
 for scale in [1.25,2]:
  st=copy.deepcopy(original);st['display']['scale']=scale
  for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
  assert ipc({'type':'set_settings','settings':st})['type']=='ok'
  for variant in ['before','after','after','before']:
   binary=root/f'browser-{variant}'
   comp_before=ipc({'type':'timings'})
   pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'XDG_CACHE_HOME='+str(root/'warm-cache'/variant),binary,root/'ownership-bench.html'))
   run('wait',screen,'--text','Ownership benchmark complete','--timeout',15000)
   run('wait',screen,'--idle')
   text=log.read_text();bench=json.loads(re.search(r'OWNERSHIP_BENCH (\[.*\])',text)[1])
   ready=int(re.search(r'Browser: scripted page ready (\d+) ms',text)[1])
   idle=json.loads(subprocess.check_output(['/usr/bin/python3','tools/dev/cost',str(pid),'3','--json']))
   scroll=scrollbench['Check'].__new__(scrollbench['Check']);scroll.sock=info['olo_socket'];scroll.runtime='/run/user/1000';scroll.pid=pid;scroll.at=(500,400);scroll.sign=1
   run('wait',screen,'--free')
   scrolling=[scroll.run(kind,90,900) for kind in ['drag','wheel']]
   comp_after=ipc({'type':'timings'})
   rows.append({'compositor_before':comp_before,'compositor_after':comp_after,'scrolling':scrolling,'variant':variant,'scale':scale,'pid':pid,'binary_sha256':hashlib.file_digest(binary.open('rb'),'sha256').hexdigest(),'bench_ms':bench,'ready_ms':ready,'usage':usage(pid),'idle':idle})
   (root/'costs.json').write_text(json.dumps(rows,indent=2));print(variant,scale,ready,rows[-1]['usage'],flush=True)
   stop()
finally:
 stop();ipc({'type':'set_settings','settings':original})
