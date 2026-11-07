(() => {
  const c = CASE_DATA;
  document.getElementById('sheet').textContent = c.css;
  const main = document.querySelector('main');
  main.removeAttribute('id');
  main.innerHTML = c.html;
  const cs = getComputedStyle(document.getElementById('probe'));
  const value = cs.clipPath;
  if (value === 'none') return JSON.stringify({dpr:devicePixelRatio, value, samples:[]});
  const start = value.indexOf('inset(');
  if (start !== 0) throw Error(value);
  let depth = 0, end = -1;
  for (let i = 5; i < value.length; i++) {
    if (value[i] === '(') depth++;
    if (value[i] === ')' && --depth === 0) { end = i; break; }
  }
  if (end < 0) throw Error(value);
  const body = value.slice(6,end), tokens = [];
  let token = '';
  depth = 0;
  for (const ch of body) {
    if (!depth && (/\s/.test(ch) || ch === '/')) {
      if (token) tokens.push(token);
      token = '';
      if (ch === '/') tokens.push(ch);
    } else {
      token += ch;
      if (ch === '(') depth++;
      if (ch === ')') depth--;
    }
  }
  if (token) tokens.push(token);
  function quad(a) {
    if (!a.length || a.length > 4) throw Error(JSON.stringify(a));
    return [a[0],a[1]??a[0],a[2]??a[0],a[3]??a[1]??a[0]];
  }
  const round = tokens.indexOf('round'), slash = tokens.indexOf('/');
  const offsets = quad(round < 0 ? tokens : tokens.slice(0,round));
  const rx = round < 0 ? ['0px','0px','0px','0px'] : quad(tokens.slice(round+1,slash<0?undefined:slash));
  const ry = slash < 0 ? rx : quad(tokens.slice(slash+1));
  const containing = document.createElement('div'), probe = document.createElement('div');
  containing.style.cssText = 'all:initial;position:absolute;top:0;left:0;height:0;pointer-events:none';
  probe.style.cssText = 'all:initial;position:absolute;top:0;width:0;height:0;pointer-events:none';
  containing.appendChild(probe);
  document.body.appendChild(containing);
  const samples = [80,200,350.5].map(basis => {
    containing.style.width = basis+'px';
    const values = [...offsets,...rx,...ry].map((v,i) => {
      probe.style.left = '';
      probe.style.left = v;
      if (!probe.style.left) throw Error(v);
      const actual = getComputedStyle(probe).left;
      if (!actual.endsWith('px')) throw Error(actual);
      const number = parseFloat(actual);
      return i<4 ? number : Math.max(0,number);
    });
    return {basis,values};
  });
  containing.remove();
  return JSON.stringify({dpr:devicePixelRatio,value,reference:value.slice(end+1).trim()||'border-box',samples});
})()
