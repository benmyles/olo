#!/usr/bin/python3
"""Native-browser fixture checks on an OWN Agent Screen; no site acceptance.

Start a release olo-browser on the screen, eval `olo-screen env NAME`, then:
  /usr/bin/python3 tools/browser/network-smoke.py NAME /var/tmp/OWNED/output
Only this screen's settings/windows change. Original settings are restored.
The test fixture server is Python; the browser's HTTP/HTML/CSS path is Rust.
"""
import copy, hashlib, http.server, json, os, socket, subprocess, sys, threading, time
from pathlib import Path
from PIL import Image
screen, destination = sys.argv[1:]
assert os.environ.get('OLO_SCREEN') == screen, 'Eval the owned screen environment first'
out = Path(destination); out.mkdir(parents=True, exist_ok=True)
requests = []
release = threading.Event()
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        requests.append(self.path)
        if self.path == '/start':
            self.send_response(302); self.send_header('Location', '/dir/page'); self.end_headers(); return
        if self.path == '/stall': release.wait(60); return
        if self.path == '/assets/first.css': body = b'.note{background-color:blue}'; mime = 'text/css'
        elif self.path == '/assets/last.css': body = b'.note{background-color:green}'; mime = 'text/css'
        elif self.path == '/plain': body = b'<script>&literal'; mime = 'text/plain; charset=utf-8'
        else:
            body = ('''<!doctype html><meta charset=utf-8><title>Network fixture · Résumé</title><link rel=stylesheet href=first.css><base href=/assets/><style>body{margin:20px}h1{font-size:28px}.note{padding:12px;width:80vw;background-color:red;color:white}p{line-height:24px}</style><link rel=stylesheet href=last.css><h1>Résumé over HTTP</h1><div class=note>Linked stylesheets apply in order</div><p>Redirects, a late document base and UTF-8 meta decoding.</p>''' + ''.join(f'<p>Network row {i}: office alpha beta gamma.</p>' for i in range(500))).encode()
            mime = 'text/html'
        self.send_response(200); self.send_header('Content-Type', mime); self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *args): pass
server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
base = f'http://127.0.0.1:{server.server_port}'
def run(*args): return subprocess.check_output(['olo-screen', *map(str,args)], text=True)
def free(): run('wait', screen, '--free', '--timeout', 10000)
def ipc(request):
    free()
    with socket.socket(socket.AF_UNIX) as s:
        s.connect(os.environ['OLO_SOCKET']); s.sendall((json.dumps(request)+'\n').encode()); return json.loads(s.makefile().readline())
def tree(): return next(w for w in json.loads(run('tree', screen, '--json')) if w['app_id'] == 'olo.browser')
def until(predicate):
    end = time.monotonic()+10
    while not predicate():
        assert time.monotonic() < end, 'Fixture condition timed out'
        time.sleep(.01)
def go(path):
    run('set', screen, '--name', 'Address', base+path); run('click', screen, '--role', 'Button', '--name', 'Go')
def loaded():
    run('wait', screen, '--text', 'Network fixture · Résumé', '--timeout', 10000)
    run('wait', screen, '--idle', '--timeout', 10000)
original = ipc({'type':'get_settings'})['settings']
rows = []
try:
    go('/start'); loaded()
    first = tree(); assert next(n['value'] for n in first['nodes'] if n['name']=='Address').endswith('/dir/page')
    assert requests == ['/start', '/dir/page', '/assets/first.css', '/assets/last.css'], requests
    assert any(n.get('name')=='Résumé over HTTP' for n in first['nodes'])
    out.joinpath('initial-tree.json').write_text(json.dumps(first,indent=2)+'\n')
    n = len(requests); run('invoke', screen, 'reload'); until(lambda:len(requests)>=n+3); loaded()
    assert requests[n:] == ['/dir/page','/assets/first.css','/assets/last.css']
    n=len(requests)
    for theme in ['light','dark','night']:
        for scale in [1,1.25,1.5,1.75,2]:
            settings = copy.deepcopy(original); settings['appearance']['theme'] = 'dark' if theme=='night' else theme; settings['appearance']['night_light']['mode'] = 'on' if theme=='night' else 'off'; settings['appearance']['night_light']['temperature'] = 4500; settings['display']['scale'] = scale
            for output in settings['display'].get('outputs',{}).values(): output.pop('scale',None)
            response=ipc({'type':'set_settings','settings':settings}); assert response['type']=='ok', response
            loaded(); window = tree(); wid = window['window']
            review = json.loads(run('review',screen,'--window',wid,'--json'))
            assert all(not r['findings'] for r in review), review
            image = out/f'{theme}-{scale}.png'; run('look',screen,image,'--window',wid,'--no-pointer')
            with Image.open(image) as im:
                green = sum(count for count,color in im.convert('RGB').getcolors(im.width*im.height) if color==(0,128,0))
            assert green > 1000, 'Last linked stylesheet did not paint green'
            rows.append({'theme':theme,'scale':scale,'review':review,'green_pixels':green,'image_sha256':hashlib.sha256(image.read_bytes()).hexdigest()})
    assert len(requests)==n, "Theme/scale changes refetched resources"
    # Width-dependent CSS restyles from retained rules, without another fetch.
    n=len(requests); run('resize',screen,'900x650'); loaded(); assert len(requests)==n
    before=time.monotonic(); go('/stall'); until(lambda:requests[-1]=='/stall')
    run('click',screen,'--role','Button','--name','Stop'); run('wait',screen,'--text','Stopped','--timeout',1000)
    stopped = tree(); go('/start'); loaded(); recovered=time.monotonic()-before
    go('/plain'); run('wait',screen,'--text','<script>&literal','--timeout',10000)
    plain=tree(); assert any(n.get('name')=='<script>&literal' for n in plain['nodes'])
    go('/start'); loaded()
    result={'scope':__doc__,'script_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'requests':requests,'reviews':rows,'stop_then_new_navigation_seconds':recovered,'stopped':stopped,'plain':plain,'final_tree':tree(),'website_acceptance':False}
    out.joinpath('report.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({'reviews':len(rows),'stop_then_new_navigation_seconds':recovered,'report':str(out/'report.json')}))
finally:
    ipc({'type':'set_settings','settings':original}); release.set(); server.shutdown(); server.server_close()
