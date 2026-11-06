#!/usr/bin/python3
"""Capture actual native Wayland-scale Chrome pixels on an OWNED Agent Screen.

olo-screen run NAME olo-chrome --user-data-dir=/var/tmp/OWNED/profile --remote-debugging-port=18739 about:blank
eval "$(olo-screen env NAME)"
/usr/bin/python3 capture.py /var/tmp/OWNED/capture --screen NAME [--port 18739]

Emulated DPR is deliberately not used: it does not match native border snapping.
Only this screen's settings/profile are touched; original settings are restored.
"""
import argparse,base64,hashlib,http.server,json,os,socket,struct,subprocess,sys,threading,urllib.parse,urllib.request
from pathlib import Path
HERE=Path(__file__).resolve().parent
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('out',type=Path);parser.add_argument('--screen',required=True);parser.add_argument('--port',type=int,default=18742)
args=parser.parse_args();OUT=args.out.resolve();assert OUT.is_relative_to('/var/tmp');OUT.mkdir(parents=True,exist_ok=True)
assert os.environ.get('OLO_SCREEN')==args.screen,'eval the owned screen environment first'
screen=next(s for s in json.loads(subprocess.check_output(['olo-screen','list']))if s['name']==args.screen)
assert screen['state']=='running'and screen['olo_socket']==os.environ['OLO_SOCKET']
PORT=args.port
class WS:
 def __init__(self,url):
  u=urllib.parse.urlsplit(url);assert u.hostname in ['127.0.0.1','localhost']
  self.s=socket.create_connection((u.hostname,u.port),timeout=30);key=base64.b64encode(os.urandom(16)).decode()
  self.s.sendall(f'GET {u.path} HTTP/1.1\r\nHost: {u.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode())
  data=b''
  while not data.endswith(b'\r\n\r\n'):data+=self.s.recv(1)
  assert data.startswith(b'HTTP/1.1 101 '),data
  accept=base64.b64encode(hashlib.sha1((key+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest())
  assert accept in data
  self.id=0
 def read(self,n):
  b=b''
  while len(b)<n:
   part=self.s.recv(n-len(b))
   if not part:raise EOFError('Chrome disconnected')
   b+=part
  return b
 def call(self,method,**params):
  self.id+=1;data=json.dumps(dict(id=self.id,method=method,params=params)).encode();mask=os.urandom(4);n=len(data)
  header=bytes([0x81,0x80|n])if n<126 else bytes([0x81,0xfe])+struct.pack('!H',n)if n<65536 else bytes([0x81,0xff])+struct.pack('!Q',n)
  self.s.sendall(header+mask+bytes(v^mask[i%4]for i,v in enumerate(data)))
  message=b''
  while True:
   a,b=self.read(2);n=b&127
   if n==126:n=struct.unpack('!H',self.read(2))[0]
   elif n==127:n=struct.unpack('!Q',self.read(8))[0]
   assert not b&128
   chunk=self.read(n)
   if a&15==8:raise EOFError('Chrome closed WebSocket')
   assert a&15 in [0,1],a
   message+=chunk
   if not a&128:continue
   reply=json.loads(message);message=b''
   if reply.get('id')!=self.id:continue
   if 'error'in reply:raise RuntimeError(reply['error'])
   return reply['result']
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  if self.path=='/font':data=(HERE.parent/'inline-fixtures/plain.ttf').read_bytes();mime='font/ttf'
  elif self.path=='/':data=b'<!doctype html><title>Olo native paint reference</title>';mime='text/html'
  else:self.send_error(404);return
  self.send_response(200);self.send_header('Content-Type',mime);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
def free():
 subprocess.run(['olo-screen','wait',args.screen,'--free','--timeout','10000'],check=True,stdout=subprocess.DEVNULL)
def ipc(req):
 if req['type']!='get_settings':free()
 with socket.socket(socket.AF_UNIX)as s:
  s.connect(screen['olo_socket']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
# Verify this DevTools endpoint belongs to a Chrome on the named screen before navigation.
browser=WS(json.load(urllib.request.urlopen(f'http://127.0.0.1:{PORT}/json/version'))['webSocketDebuggerUrl'])
pid=next(p['id']for p in browser.call('SystemInfo.getProcessInfo')['processInfo']if p['type']=='browser')
wins=json.loads(subprocess.check_output(['olo-screen','windows',args.screen]));assert any(w['pid']==pid and w['app_id']=='google-chrome'for w in wins),'DevTools browser is not on the owned screen'
version=browser.call('Browser.getVersion');gpu=browser.call('SystemInfo.getInfo')['gpu'];browser.s.close()
target=next(t for t in json.load(urllib.request.urlopen(f'http://127.0.0.1:{PORT}/json'))if t['type']=='page'and t['url']=='about:blank')
ws=WS(target['webSocketDebuggerUrl']);free();ws.call('Emulation.clearDeviceMetricsOverride')
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 free();ws.call('Page.navigate',url=f'http://127.0.0.1:{server.server_port}/')
 ws.call('Runtime.evaluate',expression='new Promise(r=>document.readyState==="complete"?r():addEventListener("load",()=>r(),{once:true}))',awaitPromise=True)
 base=(HERE/'base.css').read_text();inputs=json.loads((HERE/'inputs.json').read_text())
 for scale in [1,1.25,1.5,1.75,2]:
  st=ipc({'type':'get_settings'})['settings'];st['display']['scale']=scale
  for v in st['display']['outputs'].values():v.pop('scale',None)
  assert ipc({'type':'set_settings','settings':st})['type']=='ok'
  subprocess.run(['olo-screen','wait',args.screen,'--idle','--timeout','10000'],check=True,stdout=subprocess.DEVNULL)
  for case in inputs:
   free()
   source=f'<!doctype html><style>@font-face{{font-family:"Olo Inline plain";src:url("/font")}}{base}{case["css"]}</style>{case["body"]}'
   parent='<iframe style="position:absolute;left:0;top:0;width:100px;height:100px;border:0"></iframe>'
   expression=f"""(async()=>{{document.documentElement.innerHTML={json.dumps('<head></head><body>'+parent+'</body>')};const f=document.querySelector('iframe');const loaded=new Promise(r=>f.onload=r);f.srcdoc={json.dumps(source)};await loaded;const d=f.contentDocument,w=f.contentWindow;await Promise.all([...d.images].map(i=>i.decode()));await d.fonts.load("20px 'Olo Inline plain'");await d.fonts.ready;if(!d.fonts.check("20px 'Olo Inline plain'"))throw Error('font unavailable');await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));return {{dpr:devicePixelRatio,frameDpr:w.devicePixelRatio,width:w.innerWidth,height:w.innerHeight,boxes:[...d.body.children].map(e=>{{const r=e.getBoundingClientRect(),s=w.getComputedStyle(e);return {{tag:e.localName,x:r.x,y:r.y,width:r.width,height:r.height,borderWidths:[s.borderTopWidth,s.borderRightWidth,s.borderBottomWidth,s.borderLeftWidth]}};}})}};}})()"""
   result=ws.call('Runtime.evaluate',expression=expression,awaitPromise=True,returnByValue=True);assert 'exceptionDetails'not in result,result
   metrics=result['result']['value'];assert metrics['dpr']==scale and metrics['frameDpr']==scale and metrics['width']==100 and metrics['height']==100,metrics
   image=base64.b64decode(ws.call('Page.captureScreenshot',format='png',fromSurface=True,captureBeyondViewport=False,clip=dict(x=0,y=0,width=100,height=100,scale=1))['data'])
   name=f'{case["id"]}-{scale}.png';(OUT/name).write_bytes(image);rows.append(dict(file=name,sha256=hashlib.sha256(image).hexdigest(),metrics=metrics))
finally:
 ipc({'type':'set_settings','settings':original});ws.s.close();server.shutdown();server.server_close()
report=dict(version=version,gpu=gpu,method='Actual native Wayland output scale; no DevTools device metrics override. 100x100 iframe, ready original font, checked native DPR and frame dimensions.',rows=rows,source_hashes={str(p.relative_to(HERE.parent.parent)):hashlib.sha256(p.read_bytes()).hexdigest()for p in [HERE/'inputs.json',HERE/'assets.json',HERE/'base.css',HERE/'capture.py',HERE.parent/'inline-fixtures/plain.ttf']})
(OUT/'capture.json').write_text(json.dumps(report,indent=2)+'\n');print(f'Saved {len(rows)} native-scale independent Chrome pixel observations in {OUT}')
