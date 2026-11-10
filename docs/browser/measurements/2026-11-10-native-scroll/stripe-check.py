import copy,json,re,socket,subprocess,time
from pathlib import Path
root=Path('/var/tmp/codex-browser-native-scroll');screen='codex-browser';out=root/'stripe';out.mkdir(exist_ok=True)
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']==screen);assert info['state']=='running'
def ipc(req):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(info['olo_socket']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
w=next(w for w in json.loads(run('windows',screen)) if w['app_id']=='olo.browser')
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 for mode in ['light','dark','night']:
  for scale in [1,1.25,1.5,1.75,2]:
   st=copy.deepcopy(original);st['appearance']['theme']='dark' if mode=='night' else mode;st['appearance']['night_light']['mode']='on' if mode=='night' else 'off';st['display']['scale']=scale
   for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
   ipc({'type':'set_settings','settings':st});run('place',screen,49,40,'1000x700','--window',w['id']);time.sleep(1.2)
   log=Path('/run/user/1000/olo-screens/codex-browser-env.log').read_text();data=json.loads(re.findall(r'Browser live eval: (\{[^\n]*\})',log)[-1])
   assert abs(data['dpr']-scale)<1e-6,data
   assert data['canvases'] and abs(data['canvases'][0]['rect'][0]-data['client'][0])<.1,data
   rows.append({'mode':mode,'scale':scale,'data':data,'review':json.loads(run('review',screen,'--window',w['id'],'--json'))})
   run('look',screen,out/f'{mode}-{scale}.png','--window',w['id'],'--no-pointer')
   (out/'results.json').write_text(json.dumps(rows,indent=2))
   print(mode,scale,data,flush=True)
 (out/'log.txt').write_text(log)
finally:ipc({'type':'set_settings','settings':original})
