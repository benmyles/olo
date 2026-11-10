from pathlib import Path
import json
p=Path('/var/tmp/codex-browser-counters/oracle.py').read_text();exec(p[:p.index('token=')])
source=root/'review.html';token=hashlib.sha256(source.read_bytes()).hexdigest()
ws.call('Emulation.setDeviceMetricsOverride',width=1000,height=578,deviceScaleFactor=1,mobile=False)
ws.call('Page.navigate',url=source.as_uri()+'?fontprobe='+token)
for _ in range(100):
 r=ws.call('Runtime.evaluate',expression="location.search==='?fontprobe="+token+"' && document.readyState==='complete' && !!document.getElementById('element')",returnByValue=True)
 if r['result'].get('value'): break
 time.sleep(.02)
else: raise RuntimeError('navigation incomplete')
ws.call('Runtime.evaluate',expression='document.fonts.ready',awaitPromise=True)
ws.call('DOM.enable');ws.call('CSS.enable');doc=ws.call('DOM.getDocument',depth=-1,pierce=True)['root']
rows=[]
def visit(n):
 if n.get('nodeType')==1:
  attrs=dict(zip(n.get('attributes',[])[::2],n.get('attributes',[])[1::2]))
  if 'id' in attrs or n.get('pseudoType'):
   rows.append({'id':attrs.get('id'),'node':n['nodeName'],'pseudo':n.get('pseudoType'),'fonts':ws.call('CSS.getPlatformFontsForNode',nodeId=n['nodeId'])['fonts']})
 for c in n.get('children',[])+n.get('pseudoElements',[]):visit(c)
visit(doc)
Path('/var/tmp/codex-browser-fonts/chrome-fonts.json').write_text(json.dumps({'browser':ws.call('Browser.getVersion'),'source_sha256':token,'rows':rows},indent=2))
print(json.dumps(rows[:8],indent=2));ws.close()
