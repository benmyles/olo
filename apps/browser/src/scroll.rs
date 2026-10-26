//! The page's scroll bars. Servo 0.7 draws none and tells the embedder
//! nothing about scrolling, so a script in the top frame reports where
//! the document is scrolled and how big it is, and NetPositive draws
//! BeOS scroll bars beside the page; moving one scrolls the page with
//! `scrollTo`.

/// The top document's scroll position, size and viewport, in CSS px.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Deserialize)]
#[serde(from = "[f32; 6]")]
pub struct Metrics {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub view_width: f32,
    pub view_height: f32,
}

impl From<[f32; 6]> for Metrics {
    fn from([x, y, width, height, view_width, view_height]: [f32; 6]) -> Self {
        Metrics { x, y, width, height, view_width, view_height }
    }
}

impl Metrics {
    /// Whether it's wider than the window (a horizontal bar is needed).
    pub fn wide(&self) -> bool {
        self.width > self.view_width + 0.5
    }
}

pub fn marker(token: &str) -> String {
    format!("\u{1}tabula-scroll-{token}:")
}

/// Reports `[x, y, width, height, viewWidth, viewHeight]` whenever any of
/// them changes, at most once a frame.
pub fn script(token: &str) -> String {
    let marker = serde_json::to_string(&marker(token)).unwrap_or_default();
    format!(
        r#"(function() {{
  if (window.top !== window) return;
  var log = console.log.bind(console), K = {marker}, last = '', queued = false;
  function report() {{
    queued = false;
    var e = document.scrollingElement || document.documentElement;
    if (!e) return;
    var w = innerWidth, h = innerHeight;
    var m = [Math.round(scrollX), Math.round(scrollY), Math.max(e.scrollWidth, w), Math.max(e.scrollHeight, h), w, h].join();
    if (m === last) return;
    last = m;
    log(K + '[' + m + ']');
  }}
  function soon() {{
    if (!queued) {{ queued = true; requestAnimationFrame(report); }}
  }}
  addEventListener('scroll', soon, {{passive: true}});
  addEventListener('resize', soon);
  addEventListener('load', soon);
  document.addEventListener('DOMContentLoaded', function() {{
    soon();
    // Content that grows or shrinks without a resize.
    if (window.ResizeObserver) {{
      var o = new ResizeObserver(soon);
      o.observe(document.documentElement);
      if (document.body) o.observe(document.body);
    }} else {{
      setInterval(soon, 1000);
    }}
  }});
}})();"#
    )
}

/// Scroll the page to `x`, `y` (CSS px).
pub fn scroll_to(x: f32, y: f32) -> String {
    format!("scrollTo({}, {})", x.round(), y.round())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reports() {
        let m: Metrics = serde_json::from_str("[0,120,800,3000,800,600]").unwrap();
        assert_eq!(m.y, 120.0);
        assert_eq!(m.height, 3000.0);
        assert!(!m.wide());
        assert_eq!(scroll_to(0.4, 99.6), "scrollTo(0, 100)");
    }
}
