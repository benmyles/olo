#!/usr/bin/python3
"""Owned-screen link input regression; production engine/network are Rust.
Start one release olo-browser on a 1280x800 Agent Screen initially at 2x,
eval its environment, then: THIS_SCRIPT NAME /var/tmp/OWNED/output.
No website journey acceptance follows from these authored fixtures.
"""
import copy,http.server,json,os,socket,subprocess,sys,threading,time
from pathlib import Path
from PIL import Image
screen, destination=sys.argv[1:];assert os.environ.get('OLO_SCREEN')==screen
out=Path(destination);out.mkdir(parents=True,exist_ok=True);requests=[]
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  requests.append(self.path)
  if self.path.startswith('/dest/'):
   name='First destination'if self.path.endswith('one')else'Second destination'
   body=f'<title>{name}</title><h1>{name}</h1><a href=/links>Back to links</a>'
  else:
   body='''<title>Link fixture</title><a hidden href=ignored>hidden</a><base href=/dest/><style>a{color:red}a:focus{color:purple}a:hover{color:green}a:active{color:blue}p{line-height:28px}</style><h1>Link fixture</h1><p><a href=one>First link</a> <a href=two>Second link</a></p><p><a href=/unrequested download>Download link</a></p><p><a href=file:///dev/null>Local file link</a></p><p><a target=_blank href=/unrequested>New window link</a></p><p>Ordinary text after the links.</p><div style='height:2px;overflow:hidden'><p style='margin-top:100px'><a href=/unrequested>Clipped link</a></p></div>'''
  data=('<!doctype html><meta charset=utf-8><style>body{margin:20px}h1{font-size:28px}</style>'+body).encode()
  self.send_response(200);self.send_header('Content-Type','text/html; charset=utf-8');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
 def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
base=f'http://127.0.0.1:{server.server_port}'
def run(*args):return subprocess.check_output(['olo-screen',*map(str,args)],text=True)
def ipc(r):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX)as s:
  s.connect(os.environ['OLO_SOCKET']);s.sendall((json.dumps(r)+'\n').encode());return json.loads(s.makefile().readline())
def tree():return next(w for w in json.loads(run('tree',screen,'--json'))if w['app_id']=='olo.browser')
def loaded(text):run('wait',screen,'--text',text,'--timeout',10000);run('wait',screen,'--idle')
def go():run('set',screen,'--name','Address',base+'/links');run('click',screen,'--role','Button','--name','Go');loaded('Link fixture')
def link(name):return next(n for n in tree()['nodes']if n['role']=='Link'and n.get('name')==name)
def shot(name):
 p=out/(name+'.png');run('look',screen,p,'--window',tree()['window'],'--no-pointer');return p
def color(path,rgb):
 with Image.open(path)as im:return sum(n for n,c in im.convert('RGB').getcolors(im.width*im.height)if c==rgb)
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 go();initial=tree();assert link('First link')['value']==base+'/dest/one';assert link('Second link')['value']==base+'/dest/two'
 assert not any(n.get('name')in ['hidden','Clipped link']for n in initial['nodes'])
 for theme in ['light','dark','night']:
  for scale in [1,1.25,1.5,1.75,2]:
   settings=copy.deepcopy(original);settings['appearance']['theme']='dark'if theme=='night'else theme;settings['appearance']['night_light']['mode']='on'if theme=='night'else'off';settings['appearance']['night_light']['temperature']=4500;settings['display']['scale']=scale
   for output in settings['display'].get('outputs',{}).values():output.pop('scale',None)
   assert ipc({'type':'set_settings','settings':settings})['type']=='ok'
   loaded('Link fixture');target=link('First link');x=target['x']+target['width']/2;y=target['y']+target['height']/2
   run('move',screen,x,y);loaded('Link fixture');p=shot(f'{theme}-{scale}-hover');assert color(p,(0,128,0))>10
   review=json.loads(run('review',screen,'--window',tree()['window'],'--json'));assert all(not r['findings']for r in review),review
   n=len(requests);run('click',screen,'--role','Link','--name','Second link');loaded('Second destination');assert requests[n:]==['/dest/two']
   back=link('Back to links');bx=back['x']+back['width']/2;by=back['y']+back['height']/2
   run('down',screen,bx,by);run('wait',screen,'--idle');assert link('Back to links').get('focused'), 'Focus metadata did not update without CSS focus selectors'
   run('up',screen,bx,by);loaded('Link fixture')
   rows.append({'theme':theme,'scale':scale,'review':review,'hover_green_pixels':color(p,(0,128,0))})
 # Held mouse input and release uses the same node identity as agent clicks.
 target=link('First link');x=target['x']+target['width']/2;y=target['y']+target['height']/2
 run('move',screen,x,y);run('down',screen,x,y);run('wait',screen,'--idle');p=shot('pressed');assert color(p,(0,0,255))>10
 run('up',screen,x,y);loaded('First destination');run('click',screen,'--role','Link','--name','Back to links');loaded('Link fixture')
 for name,reason in [('Download link','Downloads are not supported'),('Local file link','cannot navigate to local files'),('New window link','another window')]:
  n=len(requests);run('click',screen,'--role','Link','--name',name);loaded(reason);assert len(requests)==n;go()
 assert '/unrequested'not in requests
 result={'scope':__doc__,'initial':initial,'rows':rows,'requests':requests,'final':tree(),'accepted_websites':0}
 (out/'report.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps({'reviews':len(rows),'requests':len(requests),'report':str(out/'report.json')}))
finally:ipc({'type':'set_settings','settings':original});server.shutdown();server.server_close()
