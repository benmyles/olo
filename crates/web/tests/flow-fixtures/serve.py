#!/usr/bin/python3
"""Local independent Chrome probe. Run on an owned Olo Agent Screen only.

Usage: /usr/bin/python3 serve.py /var/tmp/OWNED_CAPTURE_DIR [PORT]
Then: olo-screen run OWN_SCREEN olo-chrome http://127.0.0.1:PORT/
Stop the screen and this server after saving the three reports.
"""
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
from pathlib import Path
import json,sys
HERE=Path(__file__).resolve().parent;ROOT=HERE.parents[3]
OUT=Path(sys.argv[1]).resolve();assert OUT.is_relative_to('/var/tmp');OUT.mkdir(parents=True,exist_ok=True)
PORT=int(sys.argv[2])if len(sys.argv)>2 else 18736
class Handler(BaseHTTPRequestHandler):
 def do_GET(self):
  routes={'/':(HERE/'probe.html','text/html'),'/inputs':(HERE/'inputs.json','application/json'),'/base':(HERE/'base.css','text/css'),'/font':(HERE.parent/'inline-fixtures/plain.ttf','font/ttf')}
  if self.path not in routes:self.send_error(404);return
  p,mime=routes[self.path];data=p.read_bytes();self.send_response(200);self.send_header('Content-Type',mime);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
 def do_POST(self):
  if self.path!='/result':self.send_error(404);return
  n=int(self.headers['Content-Length'])
  if not 0<n<16*1024*1024:self.send_error(413);return
  report=json.loads(self.rfile.read(n));dpr=report['devicePixelRatio'];assert dpr in [1,1.5,2]
  (OUT/f'chrome-{dpr}.json').write_text(json.dumps(report,indent=2)+'\n');self.send_response(200);self.end_headers();self.wfile.write(b'ok')
print(f'Owned-screen probe: http://127.0.0.1:{PORT}/',flush=True)
ThreadingHTTPServer(('127.0.0.1',PORT),Handler).serve_forever()
