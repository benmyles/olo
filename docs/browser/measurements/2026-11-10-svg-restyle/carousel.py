from common import *
n,w=windows();c=chrome();rows=[]
probe="""(()=>{const b=document.querySelector('.testimonial-carousel__cards');return {offset:b.scrollLeft,active:document.querySelector('.testimonial-carousel__navigation-button--active')?.getAttribute('aria-label'),buttons:Array.from(document.querySelectorAll('.testimonial-carousel__navigation-button')).map(e=>({name:e.getAttribute('aria-label'),class:e.className,color:getComputedStyle(e).color,svg:Array.from(e.querySelectorAll('svg,path')).slice(0,4).map(s=>({tag:s.tagName,fill:getComputedStyle(s).fill,color:getComputedStyle(s).color,opacity:getComputedStyle(s).opacity}))}))}})()"""
for company in ['Substack','Lightspeed','Mindbody','Jobber']:
 name='Show testimonial from '+company
 native("(()=>{globalThis.__done=false;document.querySelector('.testimonial-carousel__cards').addEventListener('scrollend',()=>globalThis.__done=true,{once:true});return true})()")
 run('click',screen,'--window',n['id'],'--role','Button','--name',name);time.sleep(1.5)
 deadline=time.monotonic()+10
 while not native('globalThis.__done'):
  assert time.monotonic()<deadline;time.sleep(.1)
 run('wait',screen,'--idle');a=native(probe)
 run('focus',screen,'--window',w['id']);run('click',screen,'--window',w['id'],'--role','Button','--name',name);time.sleep(1.5);b=evaluate(c,probe)
 assert a['active']==b['active']==name,(a,b);assert abs(a['offset']-b['offset'])<.02,(a,b)
 rows.append({'company':company,'native':a,'chrome':b});(root/'carousel.json').write_text(json.dumps(rows,indent=2));print(company,a['offset'],b['offset'],flush=True)
c.close()
