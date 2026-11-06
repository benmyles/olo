#!/usr/bin/python3
"""Local independent probe server. Run Chrome only on an owned Agent Screen.
Usage: serve.py /var/tmp/OWNED_DIR (writes port and chrome-DPR.json there).
"""
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from pathlib import Path
import json
import sys

here = Path(__file__).resolve().parent
out = Path(sys.argv[1]).resolve()
assert out.is_relative_to('/var/tmp')
out.mkdir(parents=True, exist_ok=True)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        routes = {'/': (here / 'probe.html', 'text/html'),
                  '/inputs': (here / 'inputs.json', 'application/json'),
                  '/base': (here.parent / 'flow-fixtures/base.css', 'text/css'),
                  '/font': (here.parent / 'inline-fixtures/plain.ttf', 'font/ttf')}
        if self.path not in routes:
            self.send_error(404)
            return
        path, mime = routes[self.path]
        data = path.read_bytes()
        self.send_response(200)
        self.send_header('Content-Type', mime)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        if self.path != '/result':
            self.send_error(404)
            return
        count = int(self.headers['Content-Length'])
        assert 0 < count < 8 * 1024 * 1024
        report = json.loads(self.rfile.read(count))
        dpr = report['devicePixelRatio']
        assert dpr in [1, 1.25, 1.5, 1.75, 2]
        (out / f'chrome-{dpr:g}.json').write_text(json.dumps(report, indent=2) + '\n')
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'ok')

    def log_message(self, *args):
        pass


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
(out / 'port').write_text(str(server.server_port))
print(server.server_port, flush=True)
server.serve_forever()
