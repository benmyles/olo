import json,sys,urllib.request,subprocess,hashlib,shlex,time
from pathlib import Path
sys.path.insert(0,'tools/browser');from cdp import Connection
root=Path('/var/tmp/codex-browser-lists');screen='codex-browser';pid=2268640
args=shlex.split(Path(f'/proc/{pid}/cmdline').read_bytes().replace(b'\0',b' ').decode())
assert '--user-data-dir=/var/tmp/codex-browser-resume/chrome-profile' in args
assert next(s for s in json.loads(subprocess.check_output(['olo-screen','list'])) if s['name']==screen)['state']=='running'
assert any(w['pid']==pid and w['app_id']=='google-chrome' for w in json.loads(subprocess.check_output(['olo-screen','windows',screen])))
subprocess.run(['olo-screen','wait',screen,'--free'],check=True,stdout=subprocess.DEVNULL)
page=next(t for t in json.load(urllib.request.urlopen('http://127.0.0.1:18749/json')) if t['type']=='page');ws=Connection(page['webSocketDebuggerUrl'])
token=hashlib.sha256((root/'probe.html').read_bytes()).hexdigest();ws.call('Page.navigate',url=(root/'probe.html').as_uri()+'?oracle='+token)
for _ in range(100):
 r=ws.call('Runtime.evaluate',expression="location.search==='?oracle="+token+"' && document.readyState==='complete' && !!document.getElementById('p3')",returnByValue=True)
 if r['result'].get('value'):break
 time.sleep(.02)
ws.call('Accessibility.enable');ax=ws.call('Accessibility.getFullAXTree')['nodes'];nodes={n['nodeId']:n for n in ax};rows=[]
for n in ax:
 if n.get('role',{}).get('value')!='listitem':continue
 d=ws.call('DOM.describeNode',backendNodeId=n['backendDOMNodeId'])['node'];attrs=dict(zip(d.get('attributes',[])[::2],d.get('attributes',[])[1::2]));markers=[nodes[c]['name']['value'] for c in n.get('childIds',[]) if nodes[c].get('role',{}).get('value')=='ListMarker'];rows.append({'id':attrs.get('id'),'markers':markers})
result={'browser':ws.call('Browser.getVersion'),'source_sha256':hashlib.sha256((root/'probe.html').read_bytes()).hexdigest(),'rows':rows,'dom':ws.call('Runtime.evaluate',expression="Array.from(document.querySelectorAll('ol,li'),e=>({id:e.id,tag:e.tagName,start:e.start,value:e.value,reversed:e.reversed,display:getComputedStyle(e).display}))",returnByValue=True)['result']['value']}
reflection=(root/'reflection.js').read_text(); value=ws.call('Runtime.evaluate',expression=reflection,returnByValue=True); assert 'exceptionDetails' not in value,value; (root/'reflection.json').write_text(json.dumps(value['result']['value'],indent=2))
(root/'chrome.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2));ws.close()
