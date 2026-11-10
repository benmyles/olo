#!/usr/bin/python3
"""Capture component geometry in an already running, owned Olo Chrome screen."""
import argparse, base64, copy, hashlib, json, shlex, socket, subprocess, sys, time, urllib.request
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
js='''(async ({html,cases})=>{document.open();document.write(html);document.close();await document.fonts.load(getComputedStyle(document.body).font);await document.fonts.ready;const ids=['p','a','n','b','q','c','m','d'],rows=[];for(const c of cases){for(const id of ids)document.getElementById(id).setAttribute('style',c.styles[id]||'');rows.push({id:c.id,rects:Object.fromEntries(ids.map(id=>{const r=document.getElementById(id).getBoundingClientRect();return[id,[r.x,r.y,r.width,r.height]]}))})}return rows})'''
original=ipc({'type':'get_settings'})['settings']
try:
 for htmlfile,casefile,fontfile,output in [('base.html','cases.json','plain.ttf','chrome.json'),('ligature.html','ligature-cases.json','ligature.ttf','ligature-chrome.json')]:
  html=(root/htmlfile).read_text();cases=json.loads((root/casefile).read_text());font=(root.parent/'inline-fixtures'/fontfile).read_bytes();live=html.replace('url('+fontfile+')','url(data:font/ttf;base64,'+base64.b64encode(font).decode()+')');rows=[]
  for scale in [1,1.25,1.5,1.75,2]:
   st=copy.deepcopy(original);st['display']['scale']=scale
   for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
   ipc({'type':'set_settings','settings':st});deadline=time.monotonic()+10
   while c.call('Runtime.evaluate',expression='devicePixelRatio',returnByValue=True)['result']['value']!=scale:
    assert time.monotonic()<deadline;time.sleep(.1)
   r=c.call('Runtime.evaluate',expression=js+'('+json.dumps({'html':live,'cases':cases})+')',awaitPromise=True,returnByValue=True);assert 'exceptionDetails' not in r,r
   rows.extend(dict(scale=scale,**r) for r in r['result']['value'])
  (root/output).write_text(json.dumps({'chrome':c.call('Browser.getVersion'),'html_sha256':hashlib.sha256(html.encode()).hexdigest(),'cases_sha256':hashlib.sha256((root/casefile).read_bytes()).hexdigest(),'font_sha256':hashlib.sha256(font).hexdigest(),'rows':rows},indent=2)+'\n')
  print(output,len(rows),'independent observations',flush=True)
finally:ipc({'type':'set_settings','settings':original});c.call('Target.closeTarget',targetId=target);c.close()
