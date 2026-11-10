import copy,json,re,socket,subprocess,time,sys,urllib.request
from pathlib import Path
root=Path('/var/tmp/codex-browser-vertical');variant=sys.argv[1];out=root/variant;out.mkdir(exist_ok=True)
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']=='codex-browser');assert info['state']=='running'
def ipc(req):
 run('wait','codex-browser','--free')
 with socket.socket(socket.AF_UNIX) as sk:
  sk.connect(info['olo_socket']);sk.sendall((json.dumps(req)+'\n').encode());return json.loads(sk.makefile().readline())
deadline=time.monotonic()+20
while True:
 native=next((w for w in json.loads(run('windows','codex-browser')) if w['app_id']=='olo.browser'),None)
 if native:break
 assert time.monotonic()<deadline,'Browser window did not appear'
 time.sleep(.1)
chrome=next(w for w in json.loads(run('windows','codex-browser')) if w['app_id']=='google-chrome')
assert '--user-data-dir='+str(root/'chrome-profile') in Path(f'/proc/{chrome["pid"]}/cmdline').read_text()
sys.path.insert(0,'tools/browser');from cdp import Connection
c=Connection(next(t['webSocketDebuggerUrl'] for t in json.load(urllib.request.urlopen('http://localhost:9338/json')) if t['type']=='page' and t['url'].startswith('https://stripe.com')))
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 for mode in ['light','dark','night']:
  for scale in [1,1.25,1.5,1.75,2]:
   st=copy.deepcopy(original);st['appearance']['theme']='dark' if mode=='night' else mode;st['appearance']['night_light']['mode']='on' if mode=='night' else 'off';st['display']['scale']=scale
   for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
   ipc({'type':'set_settings','settings':st})
   run('place','codex-browser',49,40,'1000x665','--window',chrome['id'])
   run('place','codex-browser',49,40,'1000x700','--window',native['id']);run('focus','codex-browser','--window',native['id']);run('move','codex-browser',750,310);time.sleep(1)
   deadline=time.monotonic()+10
   while True:
    log=Path('/run/user/1000/olo-screens/codex-browser-env.log').read_text();matches=re.findall(r'Browser live eval: (\{[^\n]*\})',log);data=json.loads(matches[-1]) if matches else {}
    if data.get('dpr')==scale and data.get('scroll')==500 and len(data.get('boxes',[]))>1 and bool(data.get('experiments')):break
    assert time.monotonic()<deadline,data;time.sleep(.1)
   run('wait','codex-browser','--free');ref=c.call('Runtime.evaluate',expression=(root/'matrix-probe.js').read_text(),returnByValue=True);assert 'exceptionDetails' not in ref,ref
   ref=json.loads(ref['result']['value']);assert ref['dpr']==scale and ref['scroll']==500 and data['experiments']==ref['experiments'] and data['viewport']==ref['viewport'] and data['client']==ref['client'],(data,ref)
   run('look','codex-browser',out/f'{mode}-{scale}-native.png','--window',native['id'],'--no-pointer')
   run('focus','codex-browser','--window',chrome['id'])
   c.call('Runtime.evaluate',expression='new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))',awaitPromise=True)
   time.sleep(.2)
   run('look','codex-browser',out/f'{mode}-{scale}-chrome.png','--window',chrome['id'],'--no-pointer')
   rows.append({'mode':mode,'scale':scale,'native':data,'chrome':ref,'review':json.loads(run('review','codex-browser','--window',native['id'],'--json'))});(out/'results.json').write_text(json.dumps(rows,indent=2))
   print(mode,scale,'first heading widths',data['boxes'][0]['rect'][2],ref['boxes'][0]['rect'][2],flush=True)
finally:ipc({'type':'set_settings','settings':original});c.close()
