import copy,json,os,re,socket,subprocess,time,runpy,statistics,hashlib
from pathlib import Path
root=Path('/var/tmp/codex-browser-scroll');screen='codex-browser'
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']==screen);assert info['state']=='running'
def ipc(req):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(info['olo_socket']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
def usage(pid):
 f=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()
 status=dict(l.split(':',1) for l in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in l)
 return {'cpu_ms':(int(f[11])+int(f[12]))*1000/os.sysconf('SC_CLK_TCK'),'rss_kib':int(status['VmRSS'].split()[0])}
original=ipc({'type':'get_settings'})['settings'];rows=[];scrollbench=runpy.run_path('tools/dev/scroll-check')
try:
 assert not json.loads(run('windows',screen))
 for scale in [1.25,2]:
  st=copy.deepcopy(original);st['appearance']['theme']='light';st['display']['scale']=scale
  for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
  ipc({'type':'set_settings','settings':st})
  for variant in ['before','after','after','before']:
   pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'OLO_BROWSER_TIMING=1',root/f'browser-{variant}',root/'perf.html'))
   try:
    run('wait',screen,'--text','Scroll geometry performance','--timeout',20000);time.sleep(1.5)
    w=next(w for w in json.loads(run('windows',screen))if w['pid']==pid)
    run('place',screen,49,40,'1000x700','--window',w['id']);run('wait',screen,'--idle')
    check=scrollbench['Check'].__new__(scrollbench['Check']);check.sock=info['olo_socket'];check.runtime='/run/user/1000';check.pid=pid;check.at=(900,500);check.sign=1
    scrolling=[check.run(kind,90,900)for kind in ['drag','wheel']]
    assert all('error'not in s for s in scrolling),scrolling
    row={'variant':variant,'scale':scale,'at':[900,500],'scrolling':scrolling};rows.append(row);(root/'latency.json').write_text(json.dumps(rows,indent=2));print(variant,scale,[(s['kind'],s['first_move_ms'])for s in scrolling],flush=True)
   finally:
    run('close',screen,'--window',w['id'])
    end=time.monotonic()+10
    while Path(f'/proc/{pid}').exists()and time.monotonic()<end:time.sleep(.05)
    assert not Path(f'/proc/{pid}').exists()
finally:ipc({'type':'set_settings','settings':original})
