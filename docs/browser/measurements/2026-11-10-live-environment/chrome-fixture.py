import copy,json,os,re,socket,subprocess,time,sys,shlex,urllib.request
from pathlib import Path
sys.path.insert(0,'tools/browser')
from cdp import Connection
root=Path('/var/tmp/codex-browser-viewport');screen='codex-browser';out=root/'chrome';out.mkdir(exist_ok=True)
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']==screen);assert info['state']=='running'
def ipc(req):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(info['olo_socket']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
w=next(w for w in json.loads(run('windows',screen)) if w['app_id']=='google-chrome')
args=shlex.split(Path(f'/proc/{w["pid"]}/cmdline').read_bytes().replace(b'\0',b' ').decode())
assert '--user-data-dir=/var/tmp/codex-browser-viewport/chrome-profile' in args
pages=json.load(urllib.request.urlopen('http://127.0.0.1:9338/json'))
c=Connection(next(p['webSocketDebuggerUrl'] for p in pages if p['type']=='page' and p['url'].endswith('/viewport.html')))
def js(source):return c.call('Runtime.evaluate',expression=source,returnByValue=True)['result']['value']
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 for mode in ['light','dark','night']:
  for scale in [1,1.25,1.5,1.75,2]:
   st=copy.deepcopy(original);st['appearance']['theme']='dark' if mode=='night' else mode;st['appearance']['night_light']['mode']='on' if mode=='night' else 'off';st['display']['scale']=scale
   for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
   ipc({'type':'set_settings','settings':st});run('place',screen,49,40,'1000x700','--window',w['id']);time.sleep(.7)
   data=js('result')
   rows.append({'mode':mode,'scale':scale,'data':data})
   run('look',screen,out/f'{mode}-{scale}.png','--window',w['id'],'--no-pointer')
   print(mode,scale,data['size'],data['box'],data['canvas'],data['matches'],flush=True)
 for width in [800,1100,800,1000]:
  run('place',screen,49,40,f'{width}x700','--window',w['id']);time.sleep(.6)
  rows.append({'width':width,'data':js('result')})
 (out/'results.json').write_text(json.dumps(rows,indent=2))
finally:
 ipc({'type':'set_settings','settings':original})
 run('close',screen,'--window',w['id'])
