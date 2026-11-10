(() => {
  const result = [];
  for (const [tag, prop] of [['ol', 'start'], ['li', 'value']]) {
    const el = document.createElement(tag);
    result.push([tag, 'missing', el[prop], typeof el[prop]]);
    for (const value of [' +12x', '-4x', '2147483647', '2147483648', '-2147483648', '-2147483649', '1.5', '\u000b1', '\u00a01', '+', '']) {
      el.setAttribute(prop, value);
      result.push([tag, value, el[prop]]);
    }
    for (const value of [12.9, -12.9, 2147483648, -2147483649, 4294967297, NaN, Infinity, null, undefined, ' +12 ', '12x', true]) {
      el[prop] = value;
      result.push([tag, String(value), el[prop], el.getAttribute(prop)]);
    }
    for (const value of [1n, Symbol('x')]) {
      let error = '';
      try { el[prop] = value; } catch (e) { error = e.name; }
      result.push([tag, typeof value, error]);
    }
    el.removeAttribute(prop);
    result.push([tag, 'removed', el[prop]]);
  }
  const ol = document.createElement('ol');
  ol.reversed = true;
  result.push(['reversed', ol.reversed, ol.hasAttribute('reversed'), ol.start]);
  ol.reversed = false;
  result.push(['forward', ol.reversed, ol.hasAttribute('reversed'), ol.start]);
  return result;
})()
