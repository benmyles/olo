import copy,json,shlex,socket,subprocess,sys,time,urllib.request
from pathlib import Path
root=Path('/var/tmp/codex-browser-native-scroll');screen='codex-browser'
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']==screen);assert info['state']=='running'
w=next(x for x in json.loads(run('windows',screen)) if x['app_id']=='google-chrome')
assert '--user-data-dir='+str(root/'chrome-profile') in shlex.split(Path(f'/proc/{w["pid"]}/cmdline').read_bytes().replace(b'\0',b' ').decode())
def ipc(req):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(info['olo_socket']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
sys.path.insert(0,'tools/browser');from cdp import Connection
c=Connection(next(x['webSocketDebuggerUrl'] for x in json.load(urllib.request.urlopen('http://127.0.0.1:9338/json')) if x['type']=='page'))
def js(s):
 r=c.call('Runtime.evaluate',expression=s,returnByValue=True);assert 'exceptionDetails' not in r,r;return r['result'].get('value')
(root/'probe.html').write_text('<!doctype html><style>html,body{margin:0}#wide{width:2000px;height:2000px;background:linear-gradient(90deg,red,blue)}#fixed{position:fixed;left:10px;top:10px;width:100px;height:50px;background:green}</style><div id=wide>Wide</div><div id=fixed>Fixed</div>')
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 run('wait',screen,'--free');c.call('Page.navigate',url=(root/'probe.html').as_uri());time.sleep(.3)
 for scale in [1,1.25,1.5,1.75,2]:
  st=copy.deepcopy(original);st['display']['scale']=scale
  for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
  ipc({'type':'set_settings','settings':st});run('place',screen,49,40,'1000x700','--window',w['id']);run('wait',screen,'--idle')
  for direction in ['ltr','rtl']:
   row={'scale':scale,'direction':direction};js(f'document.documentElement.style.direction="{direction}";scrollTo(0,0)')
   row['data']=js('(()=>{let out={dpr:devicePixelRatio,inner:[innerWidth,innerHeight],client:[visualViewport.width,visualViewport.height],steps:[]};for(let x of [-10000,-1.5,-.5,-.25,0,.25,.5,1,1.5,10000]){scrollTo(x,Math.abs(x));out.steps.push([x,scrollX,scrollY])}return out})()');rows.append(row)
 (root/'oracle.json').write_text(json.dumps(rows,indent=2));print(json.dumps(rows,indent=2))
finally:ipc({'type':'set_settings','settings':original});c.close()
