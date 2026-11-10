import copy,json,sys,socket,subprocess,time,urllib.request
from pathlib import Path
sys.path.insert(0,'tools/browser');from cdp import Connection
root=Path('/var/tmp/codex-browser-overflow');out=root/'stripe-chrome';out.mkdir(exist_ok=True)
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']=='codex-browser');assert info['state']=='running'
w=next(x for x in json.loads(run('windows','codex-browser')) if x['pid']==1575991)
assert '--user-data-dir='+str(root/'chrome-profile') in Path('/proc/1575991/cmdline').read_text()
def ipc(req):
 run('wait','codex-browser','--free')
 with socket.socket(socket.AF_UNIX) as sk:
  sk.connect(info['olo_socket']);sk.sendall((json.dumps(req)+'\n').encode());return json.loads(sk.makefile().readline())
c=Connection(next(x['webSocketDebuggerUrl'] for x in json.load(urllib.request.urlopen('http://localhost:9338/json')) if x['type']=='page'))
run('wait','codex-browser','--free');c.call('Page.navigate',url='https://stripe.com')
# Wait for the actual loaded card image before measuring.
for _ in range(100):
 r=c.call('Runtime.evaluate',expression='!!document.querySelector(".sessions-on-demand-background-image")?.naturalWidth',returnByValue=True)
 if r['result'].get('value'):break
 c.call('Runtime.evaluate',expression='document.querySelector(".sessions-on-demand-background-image")?.scrollIntoView()')
 time.sleep(.2)
else:raise RuntimeError('Stripe image did not load')
c.call('Runtime.evaluate',expression='scrollTo(0,0)')
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 for mode in ['light','dark','night']:
  for scale in [1,1.25,1.5,1.75,2]:
   st=copy.deepcopy(original);st['appearance']['theme']='dark' if mode=='night' else mode;st['appearance']['night_light']['mode']='on' if mode=='night' else 'off';st['display']['scale']=scale
   for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
   ipc({'type':'set_settings','settings':st});run('place','codex-browser',49,40,'1000x665','--window',w['id']);time.sleep(.4)
   run('wait','codex-browser','--free')
   r=c.call('Runtime.evaluate',expression=(root/'live.js').read_text(),returnByValue=True)
   assert 'exceptionDetails' not in r,r
   data=json.loads(r['result']['value']);assert abs(data['dpr']-scale)<1e-6,data
   rows.append({'mode':mode,'scale':scale,'data':data});(out/'results.json').write_text(json.dumps(rows,indent=2))
   run('look','codex-browser',out/f'{mode}-{scale}.png','--window',w['id'],'--no-pointer')
   print(mode,scale,data,flush=True)
finally:ipc({'type':'set_settings','settings':original});c.close()
