#!/usr/bin/python3
"""Capture localhost-only Chrome pixels on an OWNED Olo Agent Screen.

Start: olo-screen run OWN_SCREEN olo-chrome --remote-debugging-port=18739 about:blank
Then: /usr/bin/python3 capture.py /var/tmp/OWN_CAPTURE [18739]
Only an isolated screen profile may be used. This script never launches Chrome.
"""
import base64,hashlib,http.server,json,os,socket,struct,sys,threading,urllib.parse,urllib.request
from pathlib import Path
HERE=Path(__file__).resolve().parent
OUT=Path(sys.argv[1]).resolve();assert OUT.is_relative_to('/var/tmp');OUT.mkdir(parents=True,exist_ok=True)
PORT=int(sys.argv[2])if len(sys.argv)>2 else 18739
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
  elif self.path=='/':data=b'<!doctype html><title>Olo independent paint reference</title>';mime='text/html'
  else:self.send_error(404);return
  self.send_response(200);self.send_header('Content-Type',mime);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=server.serve_forever,daemon=True).start()
targets=json.load(urllib.request.urlopen(f'http://127.0.0.1:{PORT}/json'))
target=next(t for t in targets if t['type']=='page' and t['url']=='about:blank')
ws=WS(target['webSocketDebuggerUrl']);version=ws.call('Browser.getVersion')
ws.call('Page.navigate',url=f'http://127.0.0.1:{server.server_port}/')
ws.call('Runtime.evaluate',expression='new Promise(r=>document.readyState==="complete"?r():addEventListener("load",()=>r(),{once:true}))',awaitPromise=True)
base=(HERE/'base.css').read_text();inputs=json.loads((HERE/'inputs.json').read_text());rows=[]
for scale in [1,1.5,2]:
 ws.call('Emulation.setDeviceMetricsOverride',width=100,height=100,deviceScaleFactor=scale,mobile=False)
 for case in inputs:
  source=f'<style>@font-face{{font-family:"Olo Inline plain";src:url("/font")}}{base}{case["css"]}</style>{case["body"]}'
  expression=f'''(async()=>{{document.documentElement.innerHTML={json.dumps('<head></head><body>'+source+'</body>')};await document.fonts.load("20px 'Olo Inline plain'");await document.fonts.ready;if(!document.fonts.check("20px 'Olo Inline plain'"))throw Error('font unavailable');await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));return {{dpr:devicePixelRatio,width:innerWidth,height:innerHeight}};}})()'''
  result=ws.call('Runtime.evaluate',expression=expression,awaitPromise=True,returnByValue=True)
  assert 'exceptionDetails'not in result,result
  assert result['result']['value']==dict(dpr=scale,width=100,height=100),result
  image=base64.b64decode(ws.call('Page.captureScreenshot',format='png',fromSurface=True,captureBeyondViewport=False)['data'])
  name=f'{case["id"]}-{scale}.png';(OUT/name).write_bytes(image)
  rows.append(dict(file=name,sha256=hashlib.sha256(image).hexdigest(),metrics=result['result']['value']))
ws.s.close();server.shutdown();server.server_close()
(OUT/'capture.json').write_text(json.dumps(dict(version=version,rows=rows,scope='12 authored static fixtures at 3 emulated DPRs on owned Olo Chrome. Fixed original font; pixels, not websites or arbitrary typography.'),indent=2)+'\n')
print(f'Saved {len(rows)} independent Chrome pixel observations in {OUT}')
