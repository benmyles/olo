import json,sys,urllib.request,subprocess,hashlib,shlex,time,copy,os,socket
from pathlib import Path
sys.path.insert(0,'tools/browser');from cdp import Connection
root=Path('/var/tmp/codex-browser-grid');screen='codex-browser';pid=3088066
args=shlex.split(Path(f'/proc/{pid}/cmdline').read_bytes().replace(b'\0',b' ').decode())
assert '--user-data-dir=/var/tmp/codex-browser-grid/chrome-profile' in args
assert next(s for s in json.loads(subprocess.check_output(['olo-screen','list'])) if s['name']==screen)['state']=='running'
assert any(w['pid']==pid and w['app_id']=='google-chrome' for w in json.loads(subprocess.check_output(['olo-screen','windows',screen])))
def ipc(req):
 subprocess.run(['olo-screen','wait',screen,'--free'],check=True,stdout=subprocess.DEVNULL)
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(os.environ['OLO_SOCKET']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
page=next(t for t in json.load(urllib.request.urlopen('http://127.0.0.1:18749/json')) if t['type']=='page');ws=Connection(page['webSocketDebuggerUrl'])
original=ipc({'type':'get_settings'})['settings'];rows=[];token=hashlib.sha256((root/'grid.html').read_bytes()).hexdigest()
try:
 ws.call('Emulation.clearDeviceMetricsOverride');ws.call('Page.navigate',url=(root/'grid.html').as_uri()+'?oracle='+token)
 for _ in range(100):
  r=ws.call('Runtime.evaluate',expression="location.search==='?oracle="+token+"' && document.readyState==='complete'",returnByValue=True)
  if r['result'].get('value'):break
  time.sleep(.02)
 else:raise RuntimeError('navigation incomplete')
 for scale in [1,1.25,1.5,1.75,2]:
  settings=copy.deepcopy(original);settings['display']['scale']=scale
  for output in settings['display'].get('outputs',{}).values():output.pop('scale',None)
  assert ipc({'type':'set_settings','settings':settings})['type']=='ok'
  subprocess.run(['olo-screen','wait',screen,'--idle'],check=True,stdout=subprocess.DEVNULL)
  for _ in range(100):
   dpr=ws.call('Runtime.evaluate',expression='devicePixelRatio',returnByValue=True)['result']['value']
   if abs(dpr-scale)<.001:break
   time.sleep(.02)
  else:raise RuntimeError(f'DPR {dpr} != {scale}')
  ws.call('Runtime.evaluate',expression='document.fonts.ready.then(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))))',awaitPromise=True)
  data=ws.call('Runtime.evaluate',expression="Array.from(document.querySelectorAll('.grid'),e=>({id:e.id,columns:getComputedStyle(e).gridTemplateColumns,rows:getComputedStyle(e).gridTemplateRows,rects:Array.from([e,...e.children],n=>({id:n.id,rect:Array.from(['x','y','width','height'],k=>n.getBoundingClientRect()[k])}))}))",returnByValue=True)['result']['value']
  rows.append({'scale':scale,'cases':data})
 result={'source_sha256':token,'browser':ws.call('Browser.getVersion'),'method':'actual Wayland output scale, no CDP density emulation; fonts and two frames awaited','rows':rows}
 (root/'chrome.json').write_text(json.dumps(result,indent=2));print(len(rows)*len(rows[0]['cases']),'case/scale observations')
finally:ipc({'type':'set_settings','settings':original});ws.close()
