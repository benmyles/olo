import json,socket,subprocess,time
from pathlib import Path
root=Path('/var/tmp/codex-browser-native-scroll');name='browser-scroll-reload';pid=int(__import__('sys').argv[1])
env=json.loads(Path('/var/tmp/olo-dev/'+name+'/env.json').read_text())
def ask(path,data):
 with socket.socket(socket.AF_UNIX) as s:
  s.settimeout(3);s.connect(path);s.sendall((json.dumps(data)+'\n').encode());return json.loads(s.makefile().readline())
def app(data):return ask('/run/user/1000/olo-menus-'+str(pid)+'.sock',data)
def tree():return app({'tree':True})
def run(*a):return subprocess.check_output(['/usr/bin/python3','tools/dev/tsession',*map(str,a)],text=True)
def wait(text):
 end=time.monotonic()+20
 while time.monotonic()<end:
  try:
   t=tree()
   if any(text in n.get('name','') for w in t for n in w['nodes']):return t
  except (OSError,ValueError):pass
  time.sleep(.05)
 raise AssertionError((text,tree()))
wait('Viewport scrolling');w=next(w for w in ask(env['OLO_SOCKET'],{'type':'list_windows'})['windows'] if w['pid']==pid)
n=next(n for v in tree() for n in v['nodes'] if n.get('role')=='Button' and n.get('name')=='Move both axes')
f=w['content'];run('click',name,f['x']+n['x']+n['width']/2,f['y']+n['y']+n['height']/2)
out={'moved':wait('Position 640, 720')}
app({'invoke':'home'});wait('Olo Browser');app({'invoke':'back'});out['back']=wait('Position 640, 720')
app({'invoke':'reload'});out['page_reload']=wait('Position 640, 720')
# Finish the current release workspace, then reload only this owned session.
with (root/'workspace-build-final.log').open('w') as f:subprocess.run(['cargo','build','--release','--workspace'],stdout=f,stderr=subprocess.STDOUT,check=True)
run('reload',name);time.sleep(.5);out['session_reload']=wait('Position 640, 720')
(root/'reload.json').write_text(json.dumps(out,indent=2));print({k:[n['name'] for w in v for n in w['nodes'] if n.get('name','').startswith('Position')] for k,v in out.items()})
run('stop',name)
