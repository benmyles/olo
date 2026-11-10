from common import *
import copy
variant=sys.argv[1];out=root/variant;out.mkdir(exist_ok=True);n,w=windows();c=chrome()
info=next(x for x in json.loads(run('list'))if x['name']==screen);assert info['state']=='running'
def ipc(q):
 run('wait',screen,'--free')
 with socket.socket(socket.AF_UNIX)as s:s.connect(info['olo_socket']);s.sendall((json.dumps(q)+'\n').encode());return json.loads(s.makefile().readline())
probe='''(()=>{const box=document.querySelector('.testimonial-carousel__cards');const rect=e=>{const r=e.getBoundingClientRect();return [r.x,r.y,r.width,r.height]};return {experiments:(document.querySelector('meta[name=experiment-treatments]')?.getAttribute('content')||'').split(',').map(x=>x.split('.').slice(0,2).join('.')).sort(),viewport:[innerWidth,innerHeight],client:[document.documentElement.clientWidth,document.documentElement.clientHeight],dpr:devicePixelRatio,scroll:[scrollX,scrollY],active:document.querySelector('.testimonial-carousel__navigation-button--active')?.getAttribute('aria-label'),offset:box.scrollLeft,container:rect(box),cards:Array.from(document.querySelectorAll('.testimonial-card')).map(rect)}})()'''
original=ipc({'type':'get_settings'})['settings'];rows=[]
try:
 for mode in ['light','dark','night']:
  for scale in [1,1.25,1.5,1.75,2]:
   st=copy.deepcopy(original);st['appearance']['theme']='dark'if mode=='night'else mode;st['appearance']['night_light']['mode']='on'if mode=='night'else'off';st['display']['scale']=scale
   for o in st['display'].get('outputs',{}).values():o.pop('scale',None)
   ipc({'type':'set_settings','settings':st});run('place',screen,49,40,'1000x665','--window',w['id']);run('place',screen,49,40,'1000x700','--window',n['id'])
   reset='''(()=>{document.querySelector('.testimonial-carousel__cards').scrollTo({left:0,top:0,behavior:'instant'});scrollTo({left:0,top:8111,behavior:'instant'});return true})()'''
   native(reset);evaluate(c,reset);time.sleep(.3)
   if variant=='after':native("(()=>{globalThis.__scrollSettled=false;document.querySelector('.testimonial-carousel__cards').addEventListener('scrollend',()=>globalThis.__scrollSettled=true,{once:true});return true})()")
   run('focus',screen,'--window',n['id']);action=json.loads(run('click',screen,'--window',n['id'],'--role','Button','--name','Show testimonial from Jobber'));time.sleep(.85)
   if variant=='after':
    deadline=time.monotonic()+10
    while not native('globalThis.__scrollSettled'):
     assert time.monotonic()<deadline,'No scrollend after the button click'
     time.sleep(.1)
   run('wait',screen,'--idle');time.sleep(.4)
   a=native(probe);run('wait',screen,'--idle');assert a['active']=='Show testimonial from '+('Jobber'if variant=='after'else'Mindbody'),a
   run('look',screen,out/f'{mode}-{scale}-native.png','--window',n['id'],'--no-pointer');review=json.loads(run('review',screen,'--window',n['id'],'--json'))
   run('focus',screen,'--window',w['id']);ref_action=json.loads(run('click',screen,'--window',w['id'],'--role','Button','--name','Show testimonial from Jobber'));time.sleep(.85)
   c.call('Runtime.evaluate',expression='new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))',awaitPromise=True)
   b=evaluate(c,probe);assert b['active']=='Show testimonial from Jobber',b
   for key in ['experiments','viewport','client','dpr']:assert a[key]==b[key],(key,a,b)
   assert all(abs(x-y)<.001 for x,y in zip(a['scroll'],b['scroll'])),(a,b)
   if variant=='after':assert abs(a['offset']-b['offset'])<.02,(a,b)
   run('look',screen,out/f'{mode}-{scale}-chrome.png','--window',w['id'],'--no-pointer')
   rows.append({'mode':mode,'scale':scale,'native':a,'chrome':b,'review':review,'native_action':action,'chrome_action':ref_action});(out/'results.json').write_text(json.dumps(rows,indent=2));print(mode,scale,a['active'],a['offset'],b['offset'],flush=True)
finally:ipc({'type':'set_settings','settings':original});c.close()
