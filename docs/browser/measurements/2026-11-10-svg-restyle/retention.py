import json,os,re,socket,subprocess,time,copy
from pathlib import Path
root=Path('/var/tmp/codex-browser-svg-refresh');screen='codex-browser'
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list'))if x['name']==screen);assert info['state']=='running'
def ipc(q):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:s.connect(info['olo_socket']);s.sendall((json.dumps(q)+'\n').encode());return json.loads(s.makefile().readline())
def usage(pid):
 f=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split();s=dict(l.split(':',1)for l in Path(f'/proc/{pid}/status').read_text().splitlines()if':'in l)
 return {'cpu_ms':(int(f[11])+int(f[12]))*1000/os.sysconf('SC_CLK_TCK'),'rss_kib':int(s['VmRSS'].split()[0])}
html=(root/'perf.html').read_text().split('<script>')[0]+'''<script>let count=0;const chosen=document.getElementById('chosen');function frame(){chosen.classList.toggle('selected');if(++count%100===0)console.log('SVG_MEMORY '+JSON.stringify({frame:count,time:performance.now()}));if(count<1200)requestAnimationFrame(frame)}setTimeout(()=>requestAnimationFrame(frame),800)</script>'''
(root/'retention.html').write_text(html);original=ipc({'type':'get_settings'})['settings'];settings=copy.deepcopy(original);settings['display']['scale']=2
for o in settings['display'].get('outputs',{}).values():o.pop('scale',None)
ipc({'type':'set_settings','settings':settings});out=[]
try:
 assert not json.loads(run('windows',screen))
 for variant in ['before','after']:
  pid=int(run('run',screen,'env','OLO_DATA='+str(Path.cwd()),'OLO_BROWSER_TIMING=1','XDG_CACHE_HOME='+str(root/'cache'/variant),root/f'browser-{variant}',root/'retention.html'))
  w=None
  try:
   run('wait',screen,'--text','SVG refresh performance','--timeout',20000);w=next(x for x in json.loads(run('windows',screen))if x['pid']==pid);run('place',screen,49,40,'1000x700','--window',w['id'])
   samples=[];seen=0;deadline=time.monotonic()+45
   while seen<1200:
    log=Path('/run/user/1000/olo-screens/codex-browser-env.log').read_text();rows=re.findall(r'SVG_MEMORY (\{[^\n]*\})',log)
    if rows:
     x=json.loads(rows[-1])
     if x['frame']>seen:samples.append(x|usage(pid));seen=x['frame']
    assert time.monotonic()<deadline,'No completion'
    time.sleep(.03)
   run('wait',screen,'--idle');time.sleep(.3);idle=json.loads(subprocess.check_output(['/usr/bin/python3','tools/dev/cost',str(pid),'3','--json']))
   out.append({'variant':variant,'samples':samples,'settled':usage(pid),'idle':idle});(root/'retention.json').write_text(json.dumps(out,indent=2));(root/f'retention-{variant}.log').write_text(log)
   print(variant,[(x['frame'],x['rss_kib'])for x in samples],flush=True)
  finally:
   if w:run('close',screen,'--window',w['id'])
finally:ipc({'type':'set_settings','settings':original})
