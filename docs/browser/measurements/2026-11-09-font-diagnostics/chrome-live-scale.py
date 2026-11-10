from pathlib import Path
import json,copy,os,socket,base64
p=Path('/var/tmp/codex-browser-counters/oracle.py').read_text();exec(p[:p.index('token=')])
out=Path('/var/tmp/codex-browser-fonts');source=root/'review.html';token=hashlib.sha256(source.read_bytes()).hexdigest()
def ipc(req):
 subprocess.run(['olo-screen','wait',screen,'--free'],check=True,stdout=subprocess.DEVNULL)
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(os.environ['OLO_SOCKET']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 ws.call('Emulation.clearDeviceMetricsOverride')
 ws.call('Page.navigate',url=source.as_uri()+'?live='+token)
 for _ in range(100):
  r=ws.call('Runtime.evaluate',expression="location.search==='?live="+token+"' && document.readyState==='complete'",returnByValue=True)
  if r['result'].get('value'):break
  time.sleep(.02)
 else:raise RuntimeError('navigation incomplete')
 for scale in [1,1.25,1.5,1.75,2]:
  settings=copy.deepcopy(original);settings['display']['scale']=scale
  for output in settings['display'].get('outputs',{}).values():output.pop('scale',None)
  assert ipc({'type':'set_settings','settings':settings})['type']=='ok'
  subprocess.run(['olo-screen','place',screen,'49','40','1000x822','--window','1'],check=True,stdout=subprocess.DEVNULL)
  subprocess.run(['olo-screen','wait',screen,'--idle'],check=True,stdout=subprocess.DEVNULL)
  for _ in range(100):
   q=ws.call('Runtime.evaluate',expression='devicePixelRatio',returnByValue=True)['result']['value']
   if abs(q-scale)<.001:break
   time.sleep(.02)
  else:raise RuntimeError(f'Chrome did not reach native DPR {scale}: {q}')
  ws.call('Runtime.evaluate',expression='document.fonts.ready.then(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))))',awaitPromise=True)
  # Snapshot only; no emulation of device density or CSS zoom.
  r=ws.call('Runtime.evaluate',expression="({dpr:devicePixelRatio,viewport:[innerWidth,innerHeight],rects:Array.from(document.querySelectorAll('[id]'),e=>({id:e.id,rect:Array.from(['x','y','width','height'],k=>e.getBoundingClientRect()[k])})),border:getComputedStyle(document.querySelector('section')).borderTopWidth})",returnByValue=True)['result']['value'];r['requested_scale']=scale
  (out/f'chrome-live-{scale}.png').write_bytes(base64.b64decode(ws.call('Page.captureScreenshot',format='png')['data']))
  rows.append(r)
finally:
 ipc({'type':'set_settings','settings':original});ws.close()
(out/'chrome-live-geometry.json').write_text(json.dumps(rows,indent=2));print(json.dumps([{k:v for k,v in row.items() if k!='rects'} for row in rows]))
