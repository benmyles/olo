from common import *
n,w=windows();c=chrome();run('wait',screen,'--free')
jar=json.loads(Path.home().joinpath('.local/state/olo/screens',screen,'olo-browser/cookies.json').read_text())
selected=[x for x in jar if x['domain']=='stripe.com' and x['name'] in ['cid','__Secure-sid']];assert len(selected)==2
for old in c.call('Network.getCookies',urls=['https://stripe.com/'])['cookies']:
 if old['name'] in ['cid','__Secure-sid']:c.call('Network.deleteCookies',name=old['name'],domain=old['domain'],path=old['path'])
for x in selected:
 args=dict(name=x['name'],value=x['value'],url='https://stripe.com/',path=x['path'],secure=x['secure'],httpOnly=x['http_only'])
 if not x['host_only']:args['domain']=x['domain']
 if x['expires']:args['expires']=x['expires']
 assert c.call('Network.setCookie',**args)['success']
c.call('Page.reload',ignoreCache=True);time.sleep(3)
run('place',screen,49,40,'1000x700','--window',n['id']);run('place',screen,49,40,'1000x665','--window',w['id'])
js="({experiments:(document.querySelector('meta[name=experiment-treatments]')?.getAttribute('content')||'').split(',').map(x=>x.split('.').slice(0,2).join('.')).sort(),viewport:[innerWidth,innerHeight],client:[document.documentElement.clientWidth,document.documentElement.clientHeight]})"
a=native(js);b=evaluate(c,js);assert a==b,(a,b);(root/'environment.json').write_text(json.dumps(a,indent=2));print('Aligned experiment labels and viewport',flush=True)
for y in [4200,5000,5800,6600,7400,8111]:
 native(f'(()=>{{scrollTo(0,{y});return scrollY}})()');evaluate(c,f'scrollTo({{top:{y},behavior:"instant"}})');time.sleep(.3)
js="document.querySelector('button[aria-label=\"Show testimonial from Jobber\"]').click()"
native('(()=>{'+js+';return true})()');run('wait',screen,'--idle');time.sleep(.8)
a=native("({active:document.querySelector('.testimonial-carousel__navigation-button--active')?.getAttribute('aria-label'),offset:document.querySelector('.testimonial-carousel__cards').scrollLeft,cards:Array.from(document.querySelectorAll('.testimonial-card')).map(e=>e.getBoundingClientRect().x)})")
print(a);(root/'live-first.json').write_text(json.dumps(a,indent=2));c.close()
