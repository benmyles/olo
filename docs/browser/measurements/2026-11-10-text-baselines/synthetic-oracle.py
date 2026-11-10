import shlex,json,sys,urllib.request,subprocess,hashlib,time,socket,os,copy,base64
from pathlib import Path
sys.path.insert(0,'tools/browser');from cdp import Connection
root=Path('/var/tmp/codex-browser-text');screen='codex-browser';pid=3489964
def run(*args):return subprocess.check_output(['olo-screen',*map(str,args)],text=True)
assert '--user-data-dir=/var/tmp/codex-browser-text/chrome-profile' in shlex.split(Path(f'/proc/{pid}/cmdline').read_bytes().replace(b'\0',b' ').decode())
assert next(s for s in json.loads(run('list')) if s['name']==screen)['state']=='running'
window=next(w for w in json.loads(run('windows',screen)) if w['pid']==pid and w['app_id']=='google-chrome')
run('wait',screen,'--free')
target=next(t for t in json.load(urllib.request.urlopen('http://127.0.0.1:18749/json')) if t['type']=='page');ws=Connection(target['webSocketDebuggerUrl'])
def ipc(req):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(os.environ['OLO_SOCKET']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
def evaluate(expr):return ws.call('Runtime.evaluate',expression=expr,returnByValue=True)['result'].get('value')
original=ipc({'type':'get_settings'})['settings'];rows=[];source=root/'synthetic.html';token=hashlib.sha256(source.read_bytes()).hexdigest()
try:
 ws.call('Emulation.clearDeviceMetricsOverride');ws.call('Page.navigate',url=source.as_uri()+'?oracle='+token)
 deadline=time.monotonic()+10
 while not evaluate("location.search==='?oracle="+token+"' && document.readyState==='complete'"):
  assert time.monotonic()<deadline;time.sleep(.02)
 for scale in [1,1.25,1.5,1.75,2]:
  settings=copy.deepcopy(original);settings['display']['scale']=scale
  for o in settings['display'].get('outputs',{}).values():o.pop('scale',None)
  assert ipc({'type':'set_settings','settings':settings})['type']=='ok'
  run('place',screen,49,40,'1000x822','--window',window['id']);run('wait',screen,'--idle')
  deadline=time.monotonic()+10
  while evaluate('devicePixelRatio')!=scale:
   assert time.monotonic()<deadline;time.sleep(.02)
  ws.call('Runtime.evaluate',expression='document.fonts.ready.then(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))))',awaitPromise=True)
  row=evaluate("({scale:devicePixelRatio,viewport:[innerWidth,innerHeight],cases:Array.from(document.querySelectorAll('.case'),e=>{let m=e.querySelector('.mark').getBoundingClientRect(),r=e.getBoundingClientRect(),s=e.querySelector('span').getBoundingClientRect();return {id:e.id,box:[r.x,r.y,r.width,r.height],baseline:m.bottom,span:[s.x,s.y,s.width,s.height],lineHeight:getComputedStyle(e).lineHeight}})})")
  (root/f'synthetic-chrome-{scale}.png').write_bytes(base64.b64decode(ws.call('Page.captureScreenshot',format='png',captureBeyondViewport=True,clip={'x':0,'y':0,'width':1000,'height':650,'scale':1})['data']))
  rows.append(row)
 ws.call('DOM.enable');ws.call('CSS.enable');doc=ws.call('DOM.getDocument')['root']['nodeId'];fonts={}
 for key in ['case-0']:
  node=ws.call('DOM.querySelector',nodeId=doc,selector='#'+key)['nodeId'];fonts[key]=ws.call('CSS.getPlatformFontsForNode',nodeId=node)
 (root/'synthetic-chrome.json').write_text(json.dumps({'browser':ws.call('Browser.getVersion'),'source_sha256':token,'fonts':fonts,'scales':rows},indent=2)+'\n')
finally:ipc({'type':'set_settings','settings':original});ws.close()
print([(r['scale'],len(r['cases'])) for r in rows])
