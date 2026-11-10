(async()=>{
 const inner=document.getElementById('inner'),target=document.getElementById('target');
 const frame=()=>new Promise(r=>requestAnimationFrame(r));
 const frames=async n=>{for(let i=0;i<n;i++)await frame()};
 const waitEnd=()=>new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(new Error('scrollend timeout')),3000);inner.addEventListener('scrollend',()=>{clearTimeout(timer);resolve()},{once:true})});
 for(const el of [inner,target])el.removeAttribute('style');
 inner.scrollTo({left:0,top:0,behavior:'instant'});await frames(3);
 const events=[];inner.addEventListener('scroll',()=>events.push('scroll'));inner.addEventListener('scrollend',()=>events.push('end'));
 const finished=waitEnd();const completion=target.scrollIntoView({container:'nearest',behavior:'smooth'});
 const start=[inner.scrollLeft,inner.scrollTop];await frames(8);
 const intermediate=inner.scrollLeft>0&&inner.scrollLeft<280&&inner.scrollTop>0&&inner.scrollTop<300;
 await finished;const completed=await completion;const end=[inner.scrollLeft,inner.scrollTop],ordered=events[0]==='scroll'&&events[events.length-1]==='end';
 const cancelled=inner.scrollTo({left:0,top:0,behavior:'smooth'});await frames(4);const replacement=await inner.scrollTo({left:42,top:23,behavior:'instant'});const cancellation=await cancelled;await frames(35);
 const interrupted=[inner.scrollLeft,inner.scrollTop];
 return globalThis.smoothResult={start,intermediate,end,ordered,interrupted,completed,cancellation,replacement};
})()
