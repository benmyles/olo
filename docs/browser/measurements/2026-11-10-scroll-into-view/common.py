import json, re, socket, subprocess, sys, time, urllib.request
from pathlib import Path
root=Path('/var/tmp/codex-browser-scroll');screen='codex-browser'
sys.path.insert(0,str(Path.cwd()/'tools/browser'));from cdp import Connection
def run(*a):return subprocess.check_output(['olo-screen',*map(str,a)],text=True)
def windows():
 info=next(x for x in json.loads(run('list')) if x['name']==screen);assert info['state']=='running'
 w=json.loads(run('windows',screen));n=next(x for x in w if x['app_id']=='olo.browser');c=next(x for x in w if x['app_id']=='google-chrome')
 assert any(str(root/('browser-'+v)) in Path(f'/proc/{n["pid"]}/cmdline').read_text() for v in ['before','after'])
 assert '--user-data-dir='+str(root/'chrome-profile') in Path(f'/proc/{c["pid"]}/cmdline').read_text()
 return n,c
def chrome():return Connection(next(t['webSocketDebuggerUrl'] for t in json.load(urllib.request.urlopen('http://localhost:9338/json')) if t['type']=='page' and t['url'].startswith('https://stripe.com')))
def evaluate(c,js):
 r=c.call('Runtime.evaluate',expression=js,returnByValue=True,awaitPromise=True);assert 'exceptionDetails' not in r,r;return r['result'].get('value')
def native(js):
 n,_=windows();nonce=str(time.time_ns());code=f'(()=>{{const nonce={json.dumps(nonce)};if(globalThis.__probeNonce===nonce)return globalThis.__probeResult;const data=({js});globalThis.__probeNonce=nonce;return globalThis.__probeResult=JSON.stringify({{nonce,data}})}})()';(root/'probe.js').write_text(code)
 run('wait',screen,'--free');run('focus',screen,'--window',n['id']);run('move',screen,730,310);run('move',screen,731,311)
 end=time.monotonic()+15
 while True:
  rows=re.findall(r'Browser live eval: (\{[^\n]*\})',Path('/run/user/1000/olo-screens/codex-browser-env.log').read_text())
  if rows:
   value=json.loads(rows[-1])
   if value.get('nonce')==nonce:return value['data']
  assert time.monotonic()<end,'Native diagnostic timed out'
  time.sleep(.1)
