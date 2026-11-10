import json,shlex,subprocess,sys,time,urllib.request
from pathlib import Path
sys.path.insert(0,'tools/browser')
from cdp import Connection
root=Path('/var/tmp/codex-browser-scrollbars'); screen='codex-browser'
def run(*args):return subprocess.check_output(['olo-screen',*map(str,args)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']==screen); assert info['state']=='running'
w=next(x for x in json.loads(run('windows',screen)) if x['app_id']=='google-chrome')
assert '--user-data-dir='+str(root/'chrome-profile') in shlex.split(Path(f'/proc/{w["pid"]}/cmdline').read_bytes().replace(b'\0',b' ').decode())
c=Connection(next(x['webSocketDebuggerUrl'] for x in json.load(urllib.request.urlopen('http://127.0.0.1:9338/json')) if x['type']=='page'))
def js(s):
 r=c.call('Runtime.evaluate',expression=s,returnByValue=True); assert 'exceptionDetails' not in r,r;return r['result'].get('value')
c.call('Page.navigate',url=(root/'overflow.html').as_uri());time.sleep(.3)
run('place',screen,49,40,'1000x700','--window',w['id'])
cases=[
 ('body-propagates','','width:120px;height:40px;overflow:hidden',''),
 ('root-auto-body-hidden','width:200px;height:80px;overflow:auto','width:120px;height:40px;overflow:hidden',''),
 ('root-hidden-body-visible','width:200px;height:80px;overflow:hidden','width:120px;height:40px',''),
 ('root-clip-body-visible','width:200px;height:80px;overflow:clip','width:120px;height:40px',''),
 ('root-auto-body-visible','width:200px;height:80px;overflow:auto','width:120px;height:40px',''),
 ('nested-x-clip','','','overflow-x:clip'),
 ('nested-y-clip','','','overflow-y:clip'),
 ('nested-clip-auto','','','overflow-x:clip;overflow-y:auto'),
 ('nested-auto-clip','','','overflow-x:auto;overflow-y:clip'),
 ('body-own-scroll','overflow:auto','width:120px;height:40px;overflow:auto',''),
 ('body-display-none','','display:none;overflow:hidden',''),
 ('root-display-none','display:none;overflow:hidden','',''),
]
rows=[]
for name,html,body,box in cases:
 js(f'document.documentElement.style.cssText={json.dumps(html)};document.body.style.cssText={json.dumps(body)};document.querySelector("#box").style.cssText={json.dumps(box)}')
 data=js('''(()=>{const ids=['html','body','#box','#content'];return {inner:[innerWidth,innerHeight],size:[document.documentElement.scrollWidth,document.documentElement.scrollHeight],elements:ids.map(id=>{const e=document.querySelector(id),s=getComputedStyle(e),r=e.getBoundingClientRect();return {id,rect:[r.x,r.y,r.width,r.height],overflow:[s.overflowX,s.overflowY],width:s.scrollbarWidth,gutter:s.scrollbarGutter}}),hits:[[20,20],[200,20],[20,200],[200,200]].map(([x,y])=>{const e=document.elementFromPoint(x,y);return e&&(e.id||e.tagName.toLowerCase())})}})()''')
 rows.append({'case':name,'html':html,'body':body,'box':box,'data':data})
print(json.dumps(rows,indent=2));(root/'overflow-chrome.json').write_text(json.dumps(rows,indent=2))
values=['visible','clip','hidden','auto','scroll'];pairs=[]
for x in values:
 for y in values:
  pairs.append(js(f'''(()=>{{const e=document.querySelector('#box');e.style.cssText='overflow-x:{x};overflow-y:{y}';const s=getComputedStyle(e);return {{input:['{x}','{y}'],computed:[s.overflowX,s.overflowY]}}}})()'''))
(root/'axis-chrome.json').write_text(json.dumps(pairs,indent=2))
