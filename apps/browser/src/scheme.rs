//! Pages in dark mode. NetPositive tells Servo the desktop's theme, so
//! `prefers-color-scheme: dark` works, but Servo 0.7 takes system colours
//! (CanvasText, LinkText, Field…) from that preference even for pages
//! that never chose a colour scheme, and doesn't paint the canvas a dark
//! page's Canvas colour. So this script gives every document a
//! zero-specificity style (the page's own rules win): `color-scheme:
//! light` for pages that don't support dark, and a Canvas background for
//! those that do (`<meta name=color-scheme>` or `color-scheme` on the
//! root).

pub fn script() -> String {
    r#"(function() {
  var style = null, query = matchMedia('(prefers-color-scheme: dark)');
  function apply() {
    var root = document.documentElement;
    if (!root) return;
    var own = '', meta = document.querySelector('meta[name="color-scheme"]');
    if (meta) own += meta.content;
    try { own += ' ' + getComputedStyle(root).colorScheme; } catch (e) {}
    var css = !query.matches ? ''
      : /\bdark\b/.test(own) ? ':where(html) { background-color: Canvas; } ::selection { background-color: Highlight; color: HighlightText; }'
      : ':where(html) { color-scheme: light; }';
    if (!style) {
      if (!css) return;
      style = document.createElement('style');
    }
    if (style.textContent !== css) style.textContent = css;
    if (!style.isConnected) (document.head || root).appendChild(style);
  }
  apply();
  document.addEventListener('DOMContentLoaded', apply);
  if (query.addEventListener) query.addEventListener('change', apply);
})();"#
        .into()
}
