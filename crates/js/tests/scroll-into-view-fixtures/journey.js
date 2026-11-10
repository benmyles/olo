(()=>{
 const outer=document.getElementById('outer'),inner=document.getElementById('inner'),target=document.getElementById('target');
 const cases=[
  ['default',undefined],['true',true],['false',false],['null',null],
  ['start',{block:'start',inline:'start'}],['center',{block:'center',inline:'center'}],['end',{block:'end',inline:'end'}],['nearest',{block:'nearest',inline:'nearest'}],
  ['container-nearest',{block:'start',inline:'start',container:'nearest'}],
  ['already-visible',{block:'nearest',inline:'nearest'},'visible'],
  ['oversized-cover',{block:'nearest',inline:'nearest'},'cover'],
  ['oversized-before',{block:'nearest',inline:'nearest'},'large-before'],
  ['oversized-after',{block:'nearest',inline:'nearest'},'large-after'],
  ['margins-padding',{block:'start',inline:'end'},'spacing'],
  ['logical-spacing',{block:'center',inline:'center'},'logical'],
  ['hidden-container',{block:'end',inline:'end'},'hidden'],
  ['clip-container',{block:'start',inline:'start'},'clip'],
  ['display-none',{block:'start'},'none'],
  ['fixed-target',{block:'start',inline:'start'},'fixed'],
  ['rtl-start',{block:'start',inline:'start'},'rtl'],
  ['rtl-end',{block:'end',inline:'end'},'rtl'],
  ['rtl-nearest',{block:'nearest',inline:'nearest'},'rtl'],
  ['vertical-rl-start',{block:'start',inline:'start'},'vertical-rl'],
  ['vertical-lr-end',{block:'end',inline:'end'},'vertical-lr'],
  ['inner-padding',{block:'end',inline:'end'},'padding'],
  ['transformed-target',{block:'center',inline:'center'},'transform'],
  ['negative-margin',{block:'start',inline:'start'},'negative'],
  ['fractional',{block:'center',inline:'center'},'fraction'],
 ];
 const out=[];
 for(const [name,opt,style]of cases){
  if(globalThis.caseOnly && name!==globalThis.caseOnly)continue;
  for(const el of [outer,inner,target,document.documentElement])el.removeAttribute('style');
  if(style==='rtl')inner.style.direction='rtl';
  if(style==='vertical-rl'||style==='vertical-lr')inner.style.writingMode=style;
  if(style==='padding')inner.style.padding='13px 19px';
  if(style==='transform')target.style.transform='translate(13px,17px)';
  if(style==='negative')target.style.scrollMargin='-13px -17px';
  if(style==='visible')target.style.cssText='left:30px;top:25px';
  if(style==='cover')target.style.cssText='left:-20px;top:-20px;width:500px;height:400px';
  if(style==='large-before')target.style.cssText='left:-350px;top:-250px;width:500px;height:400px';
  if(style==='large-after')target.style.cssText='left:30px;top:25px;width:500px;height:400px';
  if(style==='spacing'){target.style.scrollMargin='11px 13px 17px 19px';inner.style.scrollPadding='10% 7px 9px 5%';outer.style.scrollPadding='5px';document.documentElement.style.scrollPadding='23px';}
  if(style==='logical'){target.style.cssText='scroll-margin-block:11px 17px;scroll-margin-inline:19px 13px';inner.style.cssText='scroll-padding-block:10% 9px;scroll-padding-inline:5% 7px';}
  if(style==='hidden')inner.style.overflow='hidden';
  if(style==='clip')inner.style.overflow='clip';
  if(style==='none')target.style.display='none';
  if(style==='fixed')target.style.cssText='position:fixed;left:900px;top:750px';
  if(style==='fraction')target.style.cssText='left:420.25px;top:300.25px;width:100.5px;height:60.5px';
  scrollTo({left:0,top:0,behavior:'instant'});outer.scrollTo({left:0,top:0,behavior:'instant'});inner.scrollTo({left:0,top:0,behavior:'instant'});
  target.scrollIntoView(opt);
  const r=target.getBoundingClientRect();out.push({name,styles:[target,inner,outer].map(e=>[getComputedStyle(e).direction,getComputedStyle(e).writingMode]),offsets:[scrollX,scrollY,outer.scrollLeft,outer.scrollTop,inner.scrollLeft,inner.scrollTop],rect:[r.x,r.y,r.width,r.height]});
 }
 return {dpr:devicePixelRatio,viewport:[innerWidth,innerHeight],cases:out};
})()
