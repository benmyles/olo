import json,shlex,sys,subprocess,time,urllib.request
from pathlib import Path
sys.path.insert(0,'tools/browser');from cdp import Connection
root=Path('/var/tmp/codex-browser-classic');screen='codex-browser'
def run(*args):return subprocess.check_output(['olo-screen',*map(str,args)],text=True)
assert next(s for s in json.loads(run('list')) if s['name']==screen)['state']=='running'
w=next(w for w in json.loads(run('windows',screen)) if w['app_id']=='google-chrome');assert '--user-data-dir='+str(root/'chrome-profile') in shlex.split(Path(f'/proc/{w["pid"]}/cmdline').read_bytes().replace(b'\0',b' ').decode());run('wait',screen,'--free');run('place',screen,49,40,'1000x700','--window',w['id'])
c=Connection(next(x['webSocketDebuggerUrl'] for x in json.load(urllib.request.urlopen('http://127.0.0.1:9338/json')) if x['type']=='page'))
def js(s):
 r=c.call('Runtime.evaluate',expression=s,returnByValue=True);assert 'exceptionDetails' not in r,r;return r['result'].get('value')
c.call('Page.navigate',url=(root/'probe.html').as_uri());time.sleep(.3);rows=[]
for h,b in [('','direction:rtl'),('direction:rtl','direction:ltr'),('overflow:auto;direction:ltr','direction:rtl'),('overflow:auto;direction:rtl','direction:ltr')]:
 js(f"document.documentElement.style.cssText={json.dumps(h)};document.body.style.cssText={json.dumps(b)};document.querySelector('#content').style.height='2000px';scrollTo(0,0)")
 data=js("(()=>{let v=document.querySelector('#vw').getBoundingClientRect();let a={client:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth,vw:[v.x,v.width],x:scrollX};scrollTo(-100,0);a.negative=scrollX;scrollTo(100,0);a.positive=scrollX;return a})()")
 rows.append({'html':h,'body':b,'data':data})
(root/'rtl.json').write_text(json.dumps(rows,indent=2));print(json.dumps(rows,indent=2))
