#!/usr/bin/python3
"""Capture component geometry in an already running, owned Olo Chrome screen."""
import argparse, copy, hashlib, json, shlex, socket, subprocess, sys, time, urllib.request
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('--screen',required=True);parser.add_argument('--port',type=int,required=True);args=parser.parse_args()
root=Path(__file__).resolve().parent;repo=root.parents[3]
sys.path.insert(0,str(repo/'tools/browser'));from cdp import Connection
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']==args.screen);assert info['state']=='running'
chrome=next(w for w in json.loads(run('windows',args.screen)) if w['app_id']=='google-chrome')
cmd=shlex.split(Path(f'/proc/{chrome["pid"]}/cmdline').read_text().replace('\0',' '))
assert any(x==f'--remote-debugging-port={args.port}' for x in cmd)
assert any(x.startswith('--user-data-dir=') and '/.config/' not in x for x in cmd)
def ipc(req):
 run('wait',args.screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(info['olo_socket']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
def tabs():return json.load(urllib.request.urlopen(f'http://localhost:{args.port}/json'))
c=Connection(next(t['webSocketDebuggerUrl'] for t in tabs() if t['type']=='page'))
run('wait',args.screen,'--free');target=c.call('Target.createTarget',url='about:blank',background=True)['targetId'];c.close()
c=Connection(next(t['webSocketDebuggerUrl'] for t in tabs() if t['id']==target))
js='''({html,cases})=>{document.open();document.write(html);document.close();const ids=['grid','a','ac','al','b','bc','bl','chain','n1','n2','n3','n4','n5','end'];return cases.map(c=>{for(const id of ids)document.getElementById(id).setAttribute('style',c.styles[id]||'');return {id:c.id,boxes:Object.fromEntries(ids.map(id=>{const e=document.getElementById(id),r=e.getBoundingClientRect(),s=getComputedStyle(e);return[id,{rect:[r.x,r.y,r.width,r.height],paddingRight:s.paddingRight}]}))}})}'''
original=ipc({'type':'get_settings'})['settings'];rows=[]
f=root;html=(f/'base.html').read_text();cases=json.loads((f/'cases.json').read_text())
try:
 for scale in [1,1.25,1.5,1.75,2]:
  st=copy.deepcopy(original);st['display']['scale']=scale
  for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
  ipc({'type':'set_settings','settings':st});deadline=time.monotonic()+10
  while c.call('Runtime.evaluate',expression='devicePixelRatio',returnByValue=True)['result']['value']!=scale:assert time.monotonic()<deadline;time.sleep(.1)
  result=c.call('Runtime.evaluate',expression='('+js+')('+json.dumps({'html':html,'cases':cases})+')',returnByValue=True);assert 'exceptionDetails' not in result,result
  rows.extend(dict(scale=scale,**x) for x in result['result']['value'])
 (f/'chrome.json').write_text(json.dumps({'version':c.call('Browser.getVersion'),'html_sha256':hashlib.sha256(html.encode()).hexdigest(),'cases_sha256':hashlib.sha256((f/'cases.json').read_bytes()).hexdigest(),'rows':rows},indent=2)+'\n');print(len(rows),'Chrome observations')
finally:ipc({'type':'set_settings','settings':original});c.call('Target.closeTarget',targetId=target);c.close()
