from pathlib import Path
import json,base64
p=Path('/var/tmp/codex-browser-counters/oracle.py').read_text();exec(p[:p.index('token=')])
source=root/'review.html';token=hashlib.sha256(source.read_bytes()).hexdigest();out=Path('/var/tmp/codex-browser-fonts')
ws.call('Page.navigate',url=source.as_uri()+'?fontprobe='+token)
for _ in range(100):
 r=ws.call('Runtime.evaluate',expression="location.search==='?fontprobe="+token+"' && document.readyState==='complete' && !!document.getElementById('element')",returnByValue=True)
 if r['result'].get('value'):break
 time.sleep(.02)
else:raise RuntimeError('navigation incomplete')
rows=[]
for scale in [1,1.25,1.5,1.75,2]:
 ws.call('Emulation.setDeviceMetricsOverride',width=1000,height=700,deviceScaleFactor=scale,mobile=False)
 ws.call('Runtime.evaluate',expression='document.fonts.ready.then(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))))',awaitPromise=True)
 r=ws.call('Runtime.evaluate',expression="({dpr:devicePixelRatio,rects:Array.from(document.querySelectorAll('[id]'),e=>({id:e.id,rect:Array.from(['x','y','width','height'],k=>e.getBoundingClientRect()[k]),text:Array.from(e.childNodes).filter(n=>n.nodeType===Node.TEXT_NODE).map(n=>{let r=new Range();r.selectNodeContents(n);let b=r.getBoundingClientRect();return {text:n.textContent,rect:[b.x,b.y,b.width,b.height]}})}))})",returnByValue=True)['result']['value']
 assert r['dpr']==scale
 (out/f'chrome-{scale}.png').write_bytes(base64.b64decode(ws.call('Page.captureScreenshot',format='png')['data']))
 rows.append(r)
(out/'chrome-geometry.json').write_text(json.dumps({'source_sha256':token,'browser':ws.call('Browser.getVersion'),'viewport':[1000,700],'rows':rows},indent=2));ws.close()
