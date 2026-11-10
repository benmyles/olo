import json,sys,urllib.request,subprocess,hashlib,shlex,time
from pathlib import Path
sys.path.insert(0,'tools/browser');from cdp import Connection
root=Path('/var/tmp/codex-browser-counters');screen='codex-browser';pid=2268640
args=shlex.split(Path(f'/proc/{pid}/cmdline').read_bytes().replace(b'\0',b' ').decode())
assert '--user-data-dir=/var/tmp/codex-browser-resume/chrome-profile' in args
assert next(s for s in json.loads(subprocess.check_output(['olo-screen','list'])) if s['name']==screen)['state']=='running'
assert any(w['pid']==pid and w['app_id']=='google-chrome' for w in json.loads(subprocess.check_output(['olo-screen','windows',screen])))
subprocess.run(['olo-screen','wait',screen,'--free'],check=True,stdout=subprocess.DEVNULL)
page=next(t for t in json.load(urllib.request.urlopen('http://127.0.0.1:18749/json')) if t['type']=='page');ws=Connection(page['webSocketDebuggerUrl'])
token=hashlib.sha256((root/'probe.html').read_bytes()).hexdigest()
ws.call('Page.navigate',url=(root/'probe.html').as_uri()+'?oracle='+token)
for _ in range(100):
 r=ws.call('Runtime.evaluate',expression="location.search==='?oracle="+token+"' && document.readyState==='complete' && !!document.getElementById('increment-intermediate-overflow')",returnByValue=True)
 if r['result'].get('value'):break
 time.sleep(.02)
ws.call('Accessibility.enable');ax=ws.call('Accessibility.getFullAXTree')['nodes'];nodes={n['nodeId']:n for n in ax}
descs={n['nodeId']:ws.call('DOM.describeNode',backendNodeId=n['backendDOMNodeId'])['node'] for n in ax if 'backendDOMNodeId' in n}
def ident(n):
 d=descs.get(n['nodeId'],{});attrs=dict(zip(d.get('attributes',[])[::2],d.get('attributes',[])[1::2]));return attrs.get('id')
def generated(n,owner):
 if ident(n) not in [None,owner]:return ''
 if n.get('role',{}).get('value')=='ListMarker':return n['name']['value']
 if n.get('role',{}).get('value')=='StaticText':return n['name']['value'] if 'backendDOMNodeId' not in n else ''
 return ''.join(generated(nodes[c],owner) for c in n.get('childIds',[]))
rows=[{'id':ident(n),'generated':generated(n,ident(n))} for n in ax if ident(n)]
result={'browser':ws.call('Browser.getVersion'),'source_sha256':hashlib.sha256((root/'probe.html').read_bytes()).hexdigest(),'rows':rows,'ax':ax}
(root/'chrome.json').write_text(json.dumps(result,indent=2));print(json.dumps(rows,indent=2));ws.close()
