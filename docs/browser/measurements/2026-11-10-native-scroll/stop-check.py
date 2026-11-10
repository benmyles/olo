import json,socket,subprocess,time,sys
from pathlib import Path
root=Path('/var/tmp/codex-browser-native-scroll'); screen='codex-browser'
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
assert not json.loads(run('windows',screen))
pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'OLO_SCROLL_DEBUG=1','XDG_CACHE_HOME='+str(root/'cache/stopping'),root/'browser-after',root/'slow-wheel.html'))
try:
 run('wait',screen,'--text','Slow wheel decision');run('wait',screen,'--idle');time.sleep(.3)
 w=next(w for w in json.loads(run('windows',screen)) if w['pid']==pid)
 run('move',screen,400,350);run('wheel',screen,3);time.sleep(.05);t=time.monotonic();run('invoke',screen,'stop');latency=(time.monotonic()-t)*1000
 stopped=run('tree',screen,'--contains','Stopped');time.sleep(1)
 with socket.socket(socket.AF_UNIX) as s:
  s.connect('/run/user/1000/olo-menus-'+str(pid)+'.sock');s.sendall(b'{"frames":true}\n');marks=json.loads(s.makefile().readline())
 out={'stop_ms':latency,'status':stopped,'marks':marks,'log':Path('/run/user/1000/olo-screens/codex-browser-env.log').read_text()}
 (root/(sys.argv[1]+'.json')).write_text(json.dumps(out,indent=2))
 print('stop',latency,'last scrolls',[(m['scrolls']) for w in marks for m in w.get('marks',[]) if m['scrolls']][-3:],out['log'][-1200:])
finally:run('close',screen,'--window',w['id'])
