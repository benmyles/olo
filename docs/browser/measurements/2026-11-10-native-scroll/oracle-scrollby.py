import json,shlex,sys,subprocess,time,urllib.request
from pathlib import Path
r=Path('/var/tmp/codex-browser-native-scroll');screen='codex-browser'
def run(*args):return subprocess.check_output(['olo-screen',*map(str,args)],text=True)
assert next(s for s in json.loads(run('list')) if s['name']==screen)['state']=='running'
w=next(w for w in json.loads(run('windows',screen)) if w['app_id']=='google-chrome');assert '--user-data-dir='+str(r/'chrome-profile') in shlex.split(Path(f'/proc/{w["pid"]}/cmdline').read_bytes().replace(b'\0',b' ').decode());run('wait',screen,'--free')
sys.path.insert(0,'tools/browser');from cdp import Connection
c=Connection(next(x['webSocketDebuggerUrl'] for x in json.load(urllib.request.urlopen('http://127.0.0.1:9338/json')) if x['type']=='page'))
c.call('Page.navigate',url=(r/'probe.html').as_uri());time.sleep(.2)
js="(()=>{let cases=[];for(let expr of ['scrollBy(Infinity,NaN)','scrollBy(-Infinity,0)','scrollTo()','scrollTo(undefined)','scrollTo(null)','scrollTo(1)','scrollTo({})']){scrollTo(120,64);let error=null;try{eval(expr)}catch(e){error=e.name};cases.push({expr,position:[scrollX,scrollY],error})}return cases})()"
out=c.call('Runtime.evaluate',expression=js,returnByValue=True);assert 'exceptionDetails' not in out;data=out['result']['value'];(r/'oracle-scrollby.json').write_text(json.dumps(data,indent=2));print(data);c.close();run('close',screen,'--window',w['id'])
