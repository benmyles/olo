(async()=>{
 const el=document.getElementById('inner'),target=document.getElementById('target');
 const attempt=async fn=>{try{const r=await fn();return r===undefined?'undefined':r}catch(e){return e.name}};
 const order=[];const opts={};for(const k of ['behavior','block','container','inline'])Object.defineProperty(opts,k,{get(){order.push(k);return undefined}});
 const into=await attempt(()=>target.scrollIntoView(opts));
 const result={into,order,lengths:[Element.prototype.scrollIntoView.length,Element.prototype.scrollTo.length,scrollTo.length],
  invalid:await Promise.all(['behavior','block','container','inline'].map(k=>attempt(()=>target.scrollIntoView({[k]:'invalid'})))),
  brands:await Promise.all(['scrollIntoView','scrollTo','scrollBy'].map(k=>attempt(()=>Element.prototype[k].call({},10,20)))),
  symbol:await attempt(()=>target.scrollIntoView({block:Symbol('x')})),
  getterThrow:await attempt(()=>target.scrollIntoView({get block(){throw new RangeError()}}))};
 el.scrollTo({left:42,top:23,behavior:'instant'});for(const arg of [undefined,null,{},7])el.scrollTo(arg);
 result.omitted=[el.scrollLeft,el.scrollTop];el.scrollBy(Infinity,NaN);result.nonfinite=[el.scrollLeft,el.scrollTop];
 const calls=[];el.scrollTo({get behavior(){calls.push('behavior');return 'instant'},get left(){calls.push('left');return 15},get top(){calls.push('top');return 20}});result.getters=calls;
 result.last=[el.scrollLeft,el.scrollTop];return globalThis.idlResult=result;
})()
