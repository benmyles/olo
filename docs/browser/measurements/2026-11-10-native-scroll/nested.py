import copy,json,os,shlex,socket,subprocess,time,sys,urllib.request
from pathlib import Path
root=Path('/var/tmp/codex-browser-native-scroll');screen='codex-browser';variant=sys.argv[1];out=root/'nested';out.mkdir(exist_ok=True)
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']==screen);assert info['state']=='running'
def ipc(req):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(info['olo_socket']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
original=ipc({'type':'get_settings'})['settings'];rows=[];w=None
try:
 if variant=='chrome':
  w=next(x for x in json.loads(run('windows',screen)) if x['app_id']=='google-chrome')
  assert '--user-data-dir='+str(root/'chrome-profile') in shlex.split(Path(f'/proc/{w["pid"]}/cmdline').read_bytes().replace(b'\0',b' ').decode())
  sys.path.insert(0,'tools/browser');from cdp import Connection
  c=Connection(next(x['webSocketDebuggerUrl'] for x in json.load(urllib.request.urlopen('http://127.0.0.1:9338/json')) if x['type']=='page'))
  (out/'version.json').write_text(json.dumps(c.call('Browser.getVersion'),indent=2))
  c.call('Page.navigate',url=(root/'nested.html').as_uri());time.sleep(.3)
 else:
  assert not json.loads(run('windows',screen))
  pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'XDG_CACHE_HOME='+str(root/'cache'/variant),root/f'browser-{variant}',root/'nested.html'))
  run('wait',screen,'--text','Nested wheel','--timeout',20000);run('wait',screen,'--idle')
  w=next(x for x in json.loads(run('windows',screen)) if x['pid']==pid)
 for scale in [1,1.25,1.5,1.75,2]:
  st=copy.deepcopy(original);st['appearance']['theme']='light';st['display']['scale']=scale
  for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
  ipc({'type':'set_settings','settings':st});run('place',screen,49,40,'1000x700','--window',w['id']);run('wait',screen,'--idle')
  run('click',screen,'--role','Button','--name','Block nested wheel');run('move',screen,350,330)
  before=run('tree',screen,'--contains','Nested position');run('wheel',screen,3);run('wait',screen,'--idle');time.sleep(.2)
  blocked=run('tree',screen,'--contains','Nested position');assert before==blocked,(scale,before,blocked)
  run('click',screen,'--role','Button','--name','Allow nested wheel');run('move',screen,350,330);run('wheel',screen,3);run('wait',screen,'--idle');time.sleep(.2)
  moved=run('tree',screen,'--contains','Nested position');assert before!=moved,(scale,moved)
  rows.append({'scale':scale,'blocked':blocked,'moved':moved});(out/'results.json').write_text(json.dumps(rows,indent=2));print(rows[-1],flush=True)

finally:
 if w:
  run('close',screen,'--window',w['id'])
  end=time.monotonic()+10
  while Path(f'/proc/{w["pid"]}').exists() and time.monotonic()<end:time.sleep(.05)
 ipc({'type':'set_settings','settings':original})
