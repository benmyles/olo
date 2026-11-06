#!/usr/bin/python3
"""Native image nodes and visuals, live mode/scale changes on an owned Agent Screen.
Start one release Browser first; SCREEN /var/tmp/OWNED/output.
Component observations only; not website compatibility or pixel acceptance.
"""
import json,os,socket,copy,subprocess,base64,http.server,threading
from pathlib import Path
from PIL import Image
import sys
screen=sys.argv[1];out=Path(sys.argv[2]).resolve();assert out.is_relative_to('/var/tmp');out.mkdir(parents=True,exist_ok=True)
assert os.environ.get('OLO_SCREEN')==screen
assets=json.loads((Path(__file__).resolve().parents[2]/'crates/web/tests/image-paint-fixtures/assets.json').read_text())
url=next(u for u,a in assets.items() if a['name']=='opaque');png=base64.b64decode(url.split(',',1)[1]);requests=[]
source='<!doctype html><title>Native image pipeline</title><style>body{margin:20px}h1{font-size:28px}p{line-height:28px}.large{display:block;border-width:2px;border-style:solid;border-color:green;padding:4px;margin:12px}</style><h1>Native image pipeline</h1><p>Inline <img src="/image.png#one" alt="Inline colours" width=32 height=32> image.</p><img class=large src="/image.png" alt="Pixel colours" width=160 height=120><p><a href="/dest"><img src="/image.png#two" alt="Linked colours" width=64 height=64></a></p>'
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  requests.append(self.path);data=png if self.path=='/image.png' else b'<!doctype html><h1>Image destination</h1>' if self.path=='/dest' else source.encode()
  self.send_response(200);self.send_header('Content-Type','image/png' if self.path=='/image.png' else 'text/html; charset=utf-8');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
 def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
base=f'http://127.0.0.1:{server.server_port}'
def run(*args):return subprocess.check_output(['olo-screen',*map(str,args)],text=True)
def ipc(body):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(os.environ['OLO_SOCKET']);s.sendall((json.dumps(body)+'\n').encode());return json.loads(s.makefile().readline())
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 windows=[w for w in json.loads(run('windows',screen)) if w['app_id']=='olo.browser']
 assert len(windows)==1,'Start exactly one Browser on this owned screen'
 run('focus',screen,'--window',windows[0]['id'])
 run('set',screen,'--name','Address',base+'/');run('click',screen,'--role','Button','--name','Go')
 for mode in ['light','dark','night']:
  for scale in [1,1.25,1.5,1.75,2]:
   settings=copy.deepcopy(original);settings['appearance']['theme']='dark' if mode=='night' else mode;settings['appearance']['night_light']['mode']='on' if mode=='night' else 'off';settings['appearance']['night_light']['temperature']=4500;settings['display']['scale']=scale
   for output in settings['display'].get('outputs',{}).values():output.pop('scale',None)
   assert ipc({'type':'set_settings','settings':settings})['type']=='ok'
   run('resize',screen,'1000x700');run('wait',screen,'--text','Pixel colours','--timeout',10000);run('wait',screen,'--idle')
   w=next(w for w in json.loads(run('tree',screen,'--json')) if w['app_id']=='olo.browser')
   images=[n for n in w['nodes'] if n['role']=='Image'];links=[n for n in w['nodes'] if n['role']=='Link'];assert {n['name'] for n in images}=={'Inline colours','Pixel colours'};assert any(n['name']=='Linked colours' for n in links)
   review=json.loads(run('review',screen,'--window',w['window'],'--json'));assert all(not r['findings'] for r in review),review
   path=out/f'{mode}-{scale}.png';run('look',screen,path,'--window',w['window'],'--no-pointer')
   with Image.open(path) as im:
    counts={}
    for n,(r,g,b,a) in im.convert('RGBA').getcolors(im.width*im.height):
     if a==255:counts[(r,g,b)]=counts.get((r,g,b),0)+n
   assert counts.get((255,0,0),0)>100 and counts.get((0,255,0),0)>100,counts.get((255,0,0))
   rows.append(dict(mode=mode,scale=scale,tree=w,review=review,red_pixels=counts[(255,0,0)],green_pixels=counts[(0,255,0)]))
 assert requests==['/','/image.png'],requests
 run('click',screen,'--role','Link','--name','Linked colours');run('wait',screen,'--text','Image destination','--timeout',10000);run('wait',screen,'--idle')
 assert requests==['/','/image.png','/dest'],requests
finally:
 ipc({'type':'set_settings','settings':original});server.shutdown();server.server_close()
(out/'report.json').write_text(json.dumps(dict(scope='Owned native demo image visuals/structured nodes; all modes/scales, no website or Chrome pixel acceptance.',rows=rows,requests=requests,source=source),indent=2)+'\n');print('Passed',len(rows),'native reviews')
