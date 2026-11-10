import copy,json,os,re,socket,subprocess,time,sys
from pathlib import Path
root=Path('/var/tmp/codex-browser-viewport');screen='codex-browser'
variant=sys.argv[1];out=root/variant;out.mkdir(exist_ok=True)
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
original=ipc({'type':'get_settings'})['settings']
rows=[];pid=None
try:
 assert not json.loads(run('windows',screen))
 settings=copy.deepcopy(original);settings['appearance']['theme']='light';settings['display']['scale']=1
 for o in settings['display'].get('outputs',{}).values():o.pop('scale',None)
 ipc({'type':'set_settings','settings':settings})
 pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'OLO_BROWSER_TIMING=1','XDG_CACHE_HOME='+str(root/'cache'/variant),root/f'browser-{variant}',root/'viewport.html'))
 run('wait',screen,'--text','Live viewport','--timeout',20000)
 run('wait',screen,'--idle')
 w=next(w for w in json.loads(run('windows',screen)) if w['pid']==pid)
 for mode in ['light','dark','night']:
  for scale in [1,1.25,1.5,1.75,2]:
   st=copy.deepcopy(settings);st['appearance']['theme']='dark' if mode=='night' else mode;st['appearance']['night_light']['mode']='on' if mode=='night' else 'off';st['display']['scale']=scale
   before=usage(pid);begin=time.monotonic()
   ipc({'type':'set_settings','settings':st})
   run('place',screen,49,40,'1000x700','--window',w['id'])
   time.sleep(.8)
   log=Path('/run/user/1000/olo-screens/codex-browser-env.log').read_text()
   matches=re.findall(r'VIEWPORT (\{[^\n]*\})',log)
   assert matches,log[-2000:]
   data=json.loads(matches[-1]);after=usage(pid)
   row={'mode':mode,'scale':scale,'data':data,'before':before,'after':after,'elapsed_ms':1000*(time.monotonic()-begin),'review':json.loads(run('review',screen,'--window',w['id'],'--json'))}
   run('look',screen,out/f'{mode}-{scale}.png','--window',w['id'],'--no-pointer')
   rows.append(row);(out/'results.json').write_text(json.dumps(rows,indent=2))
   print(mode,scale,data['size'],data['box'],data['canvas'],data['matches'],data['contentCalls'],data['deviceCalls'],flush=True)
 for width in [800,1100,800,1000]:
  run('place',screen,49,40,f'{width}x700','--window',w['id']);time.sleep(.6)
  log=Path('/run/user/1000/olo-screens/codex-browser-env.log').read_text();data=json.loads(re.findall(r'VIEWPORT (\{[^\n]*\})',log)[-1])
  rows.append({'width':width,'data':data})
 (out/'results.json').write_text(json.dumps(rows,indent=2))
 (out/'log.txt').write_text(log)
 (out/'idle.json').write_text(subprocess.check_output(['/usr/bin/python3','tools/dev/cost',str(pid),'3','--json'],text=True))
finally:
 if pid:
  for w in json.loads(run('windows',screen)):
   assert w['pid']==pid;run('close',screen,'--window',w['id'])
  end=time.monotonic()+10
  while Path(f'/proc/{pid}').exists() and time.monotonic()<end:time.sleep(.05)
  assert not Path(f'/proc/{pid}').exists()
 ipc({'type':'set_settings','settings':original})
