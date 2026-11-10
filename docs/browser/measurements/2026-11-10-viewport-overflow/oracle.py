import copy,json,shlex,socket,subprocess,sys,time,urllib.request
from pathlib import Path
sys.path.insert(0,'tools/browser')
from cdp import Connection
root=Path('/var/tmp/codex-browser-scrollbars');screen='codex-browser'
def run(*args):return subprocess.check_output(['olo-screen',*map(str,args)],text=True)
info=next(x for x in json.loads(run('list')) if x['name']==screen);assert info['state']=='running'
w=next(x for x in json.loads(run('windows',screen)) if x['app_id']=='google-chrome')
assert '--user-data-dir='+str(root/'chrome-profile') in shlex.split(Path(f'/proc/{w["pid"]}/cmdline').read_bytes().replace(b'\0',b' ').decode())
def ipc(req):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(info['olo_socket']);s.sendall((json.dumps(req)+'\n').encode());return json.loads(s.makefile().readline())
c=Connection(next(x['webSocketDebuggerUrl'] for x in json.load(urllib.request.urlopen('http://127.0.0.1:9338/json')) if x['type']=='page'))
def js(s):
 r=c.call('Runtime.evaluate',expression=s,returnByValue=True)
 assert 'exceptionDetails' not in r,r
 return r['result'].get('value')
c.call('Page.navigate',url=(root/'probe.html').as_uri());time.sleep(.3)
measure="""(() => {
 const box=e=>{const r=e.getBoundingClientRect(),s=getComputedStyle(e);return {rect:[r.x,r.y,r.width,r.height],client:[e.clientLeft,e.clientTop,e.clientWidth,e.clientHeight],offset:[e.offsetWidth,e.offsetHeight],scroll:[e.scrollWidth,e.scrollHeight],overflow:[s.overflowX,s.overflowY],scrollbarWidth:s.scrollbarWidth,gutter:s.scrollbarGutter}};
 return {dpr:devicePixelRatio,inner:[innerWidth,innerHeight],visual:[visualViewport.width,visualViewport.height],html:box(document.documentElement),body:box(document.body),percent:box(document.querySelector('#percent')),vw:box(document.querySelector('#vw')),fixed:box(document.querySelector('#fixed')),nested:box(document.querySelector('#nested')),inside:box(document.querySelector('#inside')),query:matchMedia('(width:1000px)').matches};
})()"""
cases=[
 ('short','','',200,100),
 ('tall','','',2000,100),
 ('root-scroll','overflow:scroll','',200,100),
 ('root-y-scroll','overflow-y:scroll','',200,100),
 ('root-hidden','overflow:hidden','',2000,100),
 ('body-hidden','','overflow:hidden',2000,100),
 ('body-scroll','','overflow:scroll',200,100),
 ('root-clip','overflow:clip','',2000,100),
 ('root-x-hidden','overflow-x:hidden','',2000,100),
 ('stable-short','scrollbar-gutter:stable','',200,100),
 ('stable-both','scrollbar-gutter:stable both-edges','',200,100),
 ('body-gutter','','scrollbar-gutter:stable both-edges',200,100),
 ('root-thin','scrollbar-width:thin','',2000,100),
 ('root-none','scrollbar-width:none','',2000,100),
 ('body-thin','','scrollbar-width:thin',2000,100),
 ('root-rtl','direction:rtl','',2000,100),
 ('body-rtl','','direction:rtl',2000,100),
 ('wide','','',200,120),
 ('wide-tall','','',2000,120),
 ('root-none-wide','scrollbar-width:none','',2000,120),
]
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 for scale in [1,1.25,1.5,1.75,2]:
  st=copy.deepcopy(original);st['display']['scale']=scale;st['appearance']['theme']='light'
  for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
  ipc({'type':'set_settings','settings':st});run('place',screen,49,40,'1000x700','--window',w['id']);time.sleep(.3)
  for name,html,body,height,width in cases:
   js(f"document.documentElement.style.cssText={json.dumps(html)};document.body.style.cssText={json.dumps(body)};document.querySelector('#content').style.cssText='height:{height}px;width:{width}%'")
   data=js(measure);rows.append({'scale':scale,'case':name,'data':data})
  run('look',screen,root/f'chrome-{scale}.png','--window',w['id'],'--no-pointer')
finally:ipc({'type':'set_settings','settings':original})
(root/'chrome.json').write_text(json.dumps(rows,indent=2))
for r in rows:
 if r['scale']==1:print(r['case'],r['data']['inner'],r['data']['html']['client'],r['data']['percent']['rect'],r['data']['vw']['rect'],r['data']['visual'],r['data']['nested']['client'])
