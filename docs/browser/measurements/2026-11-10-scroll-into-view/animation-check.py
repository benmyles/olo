import subprocess,json,time,sys,hashlib
from pathlib import Path
r=Path('/var/tmp/codex-browser-scroll');screen='codex-browser';variant=sys.argv[1]
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list'))if x['name']==screen);assert info['state']=='running'
assert not any(w['app_id']=='olo.browser'for w in json.loads(run('windows',screen)))
pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'OLO_BROWSER_TIMING=1',r/'browser-after',r/'animation.html'))
time.sleep(1);w=next(w for w in json.loads(run('windows',screen))if w['pid']==pid)
try:
 run('place',screen,49,40,'1000x700','--window',w['id']);run('wait',screen,'--idle');time.sleep(.5)
 action=json.loads(run('click',screen,'--window',w['id'],'--role','Button','--name','Start smooth scroll'))
 time.sleep(2)
 log=Path('/run/user/1000/olo-screens/codex-browser-env.log').read_text();(r/('animation-'+variant+'.log')).write_text(log)
 records=[json.loads(line.split('SCROLL_ANIMATION ',1)[1])for line in log.splitlines()if 'SCROLL_ANIMATION 'in line]
 data={'pid':pid,'action':action,'binary_sha256':hashlib.sha256((r/'browser-after').read_bytes()).hexdigest(),'records':records};(r/('animation-'+variant+'.json')).write_text(json.dumps(data,indent=2));print(data)
finally:run('close',screen,'--window',w['id'])
