import base64,copy,json,socket,subprocess,sys,time,urllib.request
from pathlib import Path
root=Path('/var/tmp/codex-browser-scroll');fixtures=Path('crates/js/tests/scroll-into-view-fixtures');screen='codex-browser'
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list'))if x['name']==screen);assert info['state']=='running'
w=next(w for w in json.loads(run('windows',screen))if w['app_id']=='google-chrome');assert '--user-data-dir='+str(root/'chrome-profile') in Path(f'/proc/{w["pid"]}/cmdline').read_text()
def ipc(q):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX)as s:s.connect(info['olo_socket']);s.sendall((json.dumps(q)+'\n').encode());return json.loads(s.makefile().readline())
sys.path.insert(0,'tools/browser');from cdp import Connection
c=Connection(next(x['webSocketDebuggerUrl']for x in json.load(urllib.request.urlopen('http://localhost:9338/json'))if x['type']=='page'))
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 for scale in [1,1.25,1.5,1.75,2]:
  st=copy.deepcopy(original);st['display']['scale']=scale
  for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
  ipc({'type':'set_settings','settings':st});run('place',screen,49,40,'800x687','--window',w['id']);run('focus',screen,'--window',w['id']);time.sleep(.3)
  c.call('Page.navigate',url='data:text/html;base64,'+base64.b64encode((fixtures/'base.html').read_bytes()).decode());time.sleep(.3)
  result=c.call('Runtime.evaluate',expression=(fixtures/'journey.js').read_text(),returnByValue=True);assert 'exceptionDetails'not in result,result
  data=result['result']['value'];assert data['dpr']==scale and data['viewport']==[800,600],data
  rows.append(data);print(scale,'captured',len(data['cases']),flush=True)
 (fixtures/'chrome.json').write_text(json.dumps(rows,indent=2)+'\n')
 for name in ['smooth','idl']:
  c.call('Page.navigate',url='data:text/html;base64,'+base64.b64encode((fixtures/'base.html').read_bytes()).decode());time.sleep(.3)
  run('focus',screen,'--window',w['id'])
  result=c.call('Runtime.evaluate',expression=(fixtures/(name+'.js')).read_text(),returnByValue=True,awaitPromise=True);assert 'exceptionDetails' not in result,result
  (fixtures/('chrome-'+name+'.json')).write_text(json.dumps(result['result']['value'],indent=2)+'\n')
 import hashlib
 manifest={'browser':c.call('Browser.getVersion'),'screen':screen,'scales':[1,1.25,1.5,1.75,2],'viewport':[800,600],'sources':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in fixtures.iterdir() if p.suffix in ['.html','.js','.json'] and p.name!='capture-manifest.json'}}
 (fixtures/'capture-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
finally:ipc({'type':'set_settings','settings':original});c.close()
