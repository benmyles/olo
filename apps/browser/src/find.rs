//! Find in the page. Servo 0.7 has no find API, so a script searches the
//! top frame's shown text from the current selection on, selects the
//! match and scrolls it to the middle; the selection is the cursor, so
//! Find Again goes on from it and a click moves where the next search
//! starts.

/// Shared by every window, like the Find window on BeOS.
#[derive(Debug, Clone)]
pub struct Search {
    pub text: String,
    pub case: bool,
    pub words: bool,
    pub wrap: bool,
    pub backwards: bool,
}

impl Default for Search {
    fn default() -> Self {
        Search { text: String::new(), case: false, words: false, wrap: true, backwards: false }
    }
}

/// Evaluates to "found" or "none".
pub fn script(s: &Search) -> String {
    let q = serde_json::to_string(&s.text).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(function(q, cs, words, back, wrap) {{
  if (!q) return 'none';
  var skip = {{SCRIPT: 1, STYLE: 1, NOSCRIPT: 1, TEMPLATE: 1}}, seen = new Map();
  function shown(el) {{
    if (!seen.has(el)) {{
      var st = getComputedStyle(el);
      seen.set(el, st.display !== 'none' && st.visibility !== 'hidden' && el.getClientRects().length > 0);
    }}
    return seen.get(el);
  }}
  var nodes = [], starts = [], text = '';
  var walk = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT);
  for (var n = walk.nextNode(); n; n = walk.nextNode()) {{
    var p = n.parentElement;
    if (!p || skip[p.tagName] || !shown(p)) continue;
    starts.push(text.length);
    nodes.push(n);
    text += n.data;
  }}
  var hay = cs ? text : text.toLowerCase(), needle = cs ? q : q.toLowerCase();
  function at(node, off) {{
    var i = nodes.indexOf(node);
    return i < 0 ? -1 : starts[i] + off;
  }}
  var sel = getSelection(), from = back ? hay.length : 0;
  if (sel.rangeCount) {{
    var r = sel.getRangeAt(0), o = back ? at(r.startContainer, r.startOffset) : at(r.endContainer, r.endOffset);
    if (o >= 0) from = o;
  }}
  var word = /[\p{{L}}\p{{N}}_]/u;
  function whole(i) {{
    if (!words) return true;
    var a = text[i - 1], b = text[i + needle.length];
    return !(a && word.test(a)) && !(b && word.test(b));
  }}
  function look(start, limit) {{
    var i = start;
    while (true) {{
      i = back ? (i < 0 ? -1 : hay.lastIndexOf(needle, i)) : hay.indexOf(needle, i);
      if (i < 0 || (back ? i < limit : i > limit)) return -1;
      if (whole(i)) return i;
      i += back ? -1 : 1;
    }}
  }}
  var found = back ? look(from - needle.length, 0) : look(from, hay.length);
  if (found < 0 && wrap) found = back ? look(hay.length - needle.length, from) : look(0, from);
  if (found < 0) return 'none';
  function point(i, end) {{
    for (var k = nodes.length - 1; k >= 0; k--)
      if (end ? starts[k] < i : starts[k] <= i) return [nodes[k], i - starts[k]];
    return [nodes[0], 0];
  }}
  var range = document.createRange(), a = point(found, false), b = point(found + needle.length, true);
  range.setStart(a[0], a[1]);
  range.setEnd(b[0], b[1]);
  sel.removeAllRanges();
  sel.addRange(range);
  var box = range.getBoundingClientRect();
  if (box.top < 0 || box.bottom > innerHeight || box.left < 0 || box.right > innerWidth)
    scrollBy(box.left < 0 || box.right > innerWidth ? box.left - innerWidth / 2 : 0, box.top - innerHeight / 2);
  return 'found';
}})({q}, {case}, {words}, {back}, {wrap})"#,
        case = s.case,
        words = s.words,
        back = s.backwards,
        wrap = s.wrap,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_the_text() {
        let s = Search { text: "a'b\"</script>".into(), ..Default::default() };
        let js = script(&s);
        assert!(js.ends_with(r#"})("a'b\"</script>", false, false, false, true)"#), "{js}");
    }
}
