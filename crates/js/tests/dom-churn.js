
const rows = [];
for (let round=0;round<8;round++) {
  const begin=performance.now(), p=document.createElement('div');
  document.body.append(p);
  for(let j=0;j<1000;j++) { const e=document.createElement('span');e.textContent='entry '+j;p.append(e); }
  const built=performance.now();
  const f=document.createDocumentFragment();
  while(p.firstChild) f.append(p.firstChild);
  p.append(f);
  const moved=performance.now();
  p.textContent='';p.remove();
  rows.push({build:built-begin,move:moved-built,remove:performance.now()-moved});
}
globalThis.benchResult=rows;
console.log('OWNERSHIP_BENCH '+JSON.stringify(rows));
