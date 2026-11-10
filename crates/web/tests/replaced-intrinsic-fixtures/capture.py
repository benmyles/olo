import argparse,base64,io,json,sys,urllib.request,subprocess,hashlib,copy,socket,time
from pathlib import Path
sys.path.insert(0,'tools/browser')
from cdp import Connection
parser=argparse.ArgumentParser(description='Capture loaded/broken image geometry on owned codex-browser Chrome, port 9338. Run from repository root.')
parser.add_argument('chrome_pid',type=int);args=parser.parse_args()
root=Path(__file__).resolve().parent
assert '--user-data-dir=/var/tmp/codex-browser-overflow/chrome-profile' in Path(f'/proc/{args.chrome_pid}/cmdline').read_text()
subprocess.run(['olo-screen','wait','codex-browser','--free'],check=True,stdout=subprocess.DEVNULL)
tabs=json.load(urllib.request.urlopen('http://localhost:9338/json'))
c=Connection(next(t['webSocketDebuggerUrl'] for t in tabs if t['type']=='page'))
from PIL import Image
buf=io.BytesIO();Image.new('RGBA',(2,1),(255,0,0,255)).save(buf,format='PNG')
root.joinpath('image.png').write_bytes(buf.getvalue())
uri='data:image/png;base64,'+base64.b64encode(buf.getvalue()).decode()
cases=[]
for name,rule in [('percent-max','max-width:100%'),('calc-max','max-width:calc(100% + 40px)'),('percent-width','width:100%'),('calc-width','width:calc(100% + 50px)'),('minimum','max-width:100%;min-width:70px'),('calc-minimum','max-width:100%;min-width:calc(50% + 60px)'),('fixed-control',''),('fixed-max','max-width:90px')]:
 for sizing in ['300px','min-content','max-content']:
  cases.append({'id':name+'-'+sizing,'html':f'<div id=grid><picture id=picture><img src="{uri}" id=image width=2464 height=800></picture><div id=overlay></div></div>','css':f'#grid{{display:grid;width:{sizing};height:100px}}#picture,#overlay{{grid-area:1/1}}img{{display:block;height:100px;{rule}}}#overlay{{overflow:hidden}}','measure':['grid','picture','image','overlay']})
for case in list(cases):
 broken=copy.deepcopy(case);broken['id']='broken-'+case['id'];broken['broken']=True;broken['html']=broken['html'].replace('src="'+uri+'"','');cases.append(broken)
root.joinpath('cases.json').write_text(json.dumps(cases,indent=2)+'\n')
base=Path('crates/web/tests/flow-fixtures/base.css').read_text()
js="(async cases => {const rows=[];for (const c of cases) {document.head.innerHTML='<style>'+BASE+c.css+'</style>';document.body.innerHTML=c.html;if(!c.broken)await document.querySelector('img').decode();rows.push({id:c.id,rects:Object.fromEntries(c.measure.map(id=>{const r=document.getElementById(id).getBoundingClientRect();return [id,{width:r.width,height:r.height}]}))})}return rows})".replace('BASE',json.dumps(base))
# Owned tab only; discard Stripe scripts before constructing the fixtures.
c.call('Page.navigate',url='about:blank')
import time
time.sleep(.2)
info=next(x for x in json.loads(subprocess.check_output(['olo-screen','list'])) if x['name']=='codex-browser')
def ipc(req):
 subprocess.run(['olo-screen','wait','codex-browser','--free'],check=True,stdout=subprocess.DEVNULL)
 with socket.socket(socket.AF_UNIX) as sk:
  sk.connect(info['olo_socket']);sk.sendall((json.dumps(req)+'\n').encode());return json.loads(sk.makefile().readline())
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 for scale in [1,1.25,1.5,1.75,2]:
  st=copy.deepcopy(original);st['display']['scale']=scale
  for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
  ipc({'type':'set_settings','settings':st});time.sleep(.3)
  assert c.call('Runtime.evaluate',expression='devicePixelRatio',returnByValue=True)['result']['value']==scale
  r=c.call('Runtime.evaluate',expression=js+'('+json.dumps(cases)+')',returnByValue=True,awaitPromise=True)
  assert 'exceptionDetails' not in r,r
  rows.extend(dict(scale=scale,**row) for row in r['result']['value'])
finally:ipc({'type':'set_settings','settings':original})
out={'chrome':c.call('Browser.getVersion'),'cases_sha256':hashlib.sha256(root.joinpath('cases.json').read_bytes()).hexdigest(),'rows':rows}
root.joinpath('chrome.json').write_text(json.dumps(out,indent=2)+'\n')
for row in out['rows'][:48]:print(row['id'],[r['width'] for r in row['rects'].values()])
c.close()
