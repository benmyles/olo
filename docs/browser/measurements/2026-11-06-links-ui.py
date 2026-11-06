#!/usr/bin/python3
"""Frozen before/after native scroll + idle samples on an OWN Agent Screen.
Args: NAME /var/tmp/OWNED with browser-before/browser-after; OUTPUT.json.
Eval the screen's environment first. Uses only the local scroll fixture.
Client-frame marks are not compositor presentation or optical latency.
"""
import json,os,socket,subprocess,sys,hashlib
from pathlib import Path
screen,owned,output=sys.argv[1:4];root=Path(owned);assert os.environ['OLO_SCREEN']==screen
variants=sys.argv[4:]or["before","after"]
rows=[r for r in json.loads(Path(output).read_text())["rows"]if r["variant"]=="before"]if variants==["after"]else[]
def run(*args):return subprocess.check_output(args,text=True)
def cmd(*args):return run('olo-screen',*map(str,args))
cmd('wait',screen,'--free')
with socket.socket(socket.AF_UNIX)as s:
 s.connect(os.environ['OLO_SOCKET']);s.sendall(b'{"type":"get_settings"}\n');settings=json.loads(s.makefile().readline())['settings']
settings['display']['scale']=1;settings['appearance']['theme']='light';settings['appearance']['night_light']['mode']='off'
cmd('wait',screen,'--free')
with socket.socket(socket.AF_UNIX)as s:
 s.connect(os.environ['OLO_SOCKET']);s.sendall((json.dumps({'type':'set_settings','settings':settings})+'\n').encode());assert json.loads(s.makefile().readline())['type']=='ok'
for window in json.loads(cmd('windows',screen)):
 if window['app_id']=='olo.browser':cmd('close',screen,'--window',window['id'])
for variant in variants:
 binary=root/f'browser-{variant}';pid=int(cmd('run',screen,binary,Path('crates/web/tests/paint-fixtures/scroll.html').absolute()))
 cmd('wait',screen,'--text','Scroll fixture','--timeout',10000);cmd('resize',screen,'1000x700');cmd('place',screen,49,41)
 samples=[]
 for repeat in range(3):
  cmd('invoke',screen,'reload');cmd('wait',screen,'--text','Scroll fixture','--timeout',10000);cmd('wait',screen,'--idle')
  destination=root/f'{variant}-scroll-{repeat}.json'
  subprocess.run(['/usr/bin/python3','docs/browser/measurements/2026-11-06-edge-ui-replay.py',screen,str(destination)],stdout=subprocess.DEVNULL,check=True);samples.append(json.loads(destination.read_text()))
 idle=json.loads(run('/usr/bin/python3','tools/dev/cost',str(pid),'3','--json'))
 status=dict(line.split(':',1)for line in Path(f'/proc/{pid}/status').read_text().splitlines()if ':'in line)
 rows.append({'variant':variant,'binary_sha256':hashlib.file_digest(binary.open('rb'),'sha256').hexdigest(),'rss_kib':int(status['VmRSS'].split()[0]),'hwm_kib':int(status['VmHWM'].split()[0]),'idle':idle,'samples':samples})
 for window in json.loads(cmd('windows',screen)):
  if window['pid']==pid:cmd('close',screen,'--window',window['id'])
Path(output).write_text(json.dumps({'scope':__doc__,'rows':rows},indent=2)+'\n');print(json.dumps([{k:v for k,v in r.items()if k not in ['samples','idle']}for r in rows]))
