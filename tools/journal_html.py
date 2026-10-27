#!/usr/bin/env python3
"""Build docs/journal/journal.html: the screenshot journal as one
self-contained page (every image embedded), for following progress and
for sharing. journal.py runs this after each `add`; run it by hand after
editing entries.json.

    journal_html.py [OUT.html] [--last N]     (--last: only the newest N, for a quick look)

Images are embedded as high-quality WebP at full resolution (text stays
crisp; the page is a fraction of the PNGs' size). Re-encoding is cached
in docs/journal/.cache/ (ignored by git), so rebuilding is quick.
"""
import base64, datetime, hashlib, html, io, json, os, sys

from PIL import Image

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
DIR = os.path.join(ROOT, "docs", "journal")
CACHE = os.path.join(DIR, ".cache")
QUALITY = 90


def webp(path):
    """The image at `path` as WebP bytes, cached by content."""
    data = open(path, "rb").read()
    key = hashlib.sha256(data).hexdigest()[:24]
    cached = os.path.join(CACHE, f"{key}-q{QUALITY}.webp")
    if os.path.exists(cached):
        return open(cached, "rb").read(), Image.open(path).size
    im = Image.open(io.BytesIO(data))
    size = im.size
    if im.mode not in ("RGB", "RGBA"):
        im = im.convert("RGBA")
    out = io.BytesIO()
    im.save(out, "WEBP", quality=QUALITY, method=6)
    os.makedirs(CACHE, exist_ok=True)
    open(cached, "wb").write(out.getvalue())
    return out.getvalue(), size


def day_name(d):
    return datetime.date.fromisoformat(d).strftime("%A, %B %-d, %Y")


STYLE = """
:root { --bg: #f4f2ee; --card: #ffffff; --text: #1d1d1f; --muted: #6b6b70; --rule: #dcd8d0;
        --accent: #2a5db0; --tab: #ffcb00; --shadow: 0 1px 2px rgba(0,0,0,.06), 0 4px 16px rgba(0,0,0,.06); }
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) { --bg: #17181b; --card: #212226; --text: #ececef; --muted: #9a9aa2;
    --rule: #34353a; --accent: #8cb4ff; --shadow: 0 1px 2px rgba(0,0,0,.4); }
}
:root[data-theme="dark"] { --bg: #17181b; --card: #212226; --text: #ececef; --muted: #9a9aa2;
  --rule: #34353a; --accent: #8cb4ff; --shadow: 0 1px 2px rgba(0,0,0,.4); }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--text);
  font: 16px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans", Helvetica, Arial, sans-serif; }
header { padding: 48px 16px 24px; max-width: 1100px; margin: 0 auto; }
header h1 { margin: 0 0 6px; font-size: 34px; letter-spacing: -.02em; }
header h1 span { background: var(--tab); color: #000; padding: 0 10px; border-radius: 4px 4px 0 0; }
header p { margin: 4px 0; color: var(--muted); }
nav { max-width: 1100px; margin: 0 auto; padding: 0 16px 8px; display: flex; flex-wrap: wrap; gap: 8px; }
nav a { color: var(--accent); text-decoration: none; font-size: 14px; border: 1px solid var(--rule);
  padding: 3px 10px; border-radius: 999px; background: var(--card); }
main { max-width: 1100px; margin: 0 auto; padding: 0 16px 64px; }
h2 { font-size: 22px; margin: 40px 0 16px; padding-bottom: 6px; border-bottom: 1px solid var(--rule); }
article { background: var(--card); border-radius: 10px; box-shadow: var(--shadow); margin: 0 0 28px; overflow: hidden; }
article .text { padding: 16px 20px 12px; }
article h3 { margin: 0; font-size: 19px; }
article time { color: var(--muted); font-size: 13px; font-variant-numeric: tabular-nums; }
article p { margin: 6px 0 0; }
article img { display: block; width: 100%; height: auto; cursor: zoom-in; border-top: 1px solid var(--rule); background: var(--bg); }
article img.narrow { width: auto; max-width: 100%; margin: 0 auto; }
#zoom { position: fixed; inset: 0; background: rgba(0,0,0,.85); display: none; overflow: auto; cursor: zoom-out; z-index: 9; }
#zoom.on { display: block; }
#zoom img { display: block; margin: 0 auto; max-width: none; }
footer { text-align: center; color: var(--muted); font-size: 13px; padding: 24px 16px 48px; }
@media (max-width: 600px) { header { padding-top: 28px; } header h1 { font-size: 26px; } article .text { padding: 12px 14px 10px; } }
"""

SCRIPT = """
var z = document.getElementById('zoom'), zi = z.querySelector('img');
document.addEventListener('click', function(e) {
  var t = e.target;
  if (t.tagName === 'IMG' && t.closest('article')) {
    zi.src = t.src; zi.style.width = (t.naturalWidth / 2) + 'px'; zi.alt = t.alt;
    z.classList.add('on'); z.scrollTop = 0;
  } else if (z.contains(t)) z.classList.remove('on');
});
document.addEventListener('keydown', function(e) { if (e.key === 'Escape') z.classList.remove('on'); });
"""


def build(out_path, last=None):
    entries = json.load(open(os.path.join(DIR, "entries.json")))
    if last:
        entries = sorted(entries, key=lambda e: e["time"])[-last:]
    order = sorted(enumerate(entries), key=lambda ie: (ie[1]["time"], ie[0]), reverse=True)
    days = []
    for _, e in order:
        d = e["time"][:10]
        if not days or days[-1][0] != d:
            days.append((d, []))
        days[-1][1].append(e)
    first, last = entries and min(e["time"] for e in entries), entries and max(e["time"] for e in entries)
    parts = ["<!doctype html><html lang='en'><head><meta charset='utf-8'>",
             "<meta name='viewport' content='width=device-width, initial-scale=1'>",
             "<title>Olo Journal</title>",
             f"<style>{STYLE}</style></head><body>",
             "<header><h1><span>Olo</span> journal</h1>",
             "<p>A BeOS-inspired desktop for Linux, written in Rust: every visible change, newest first.</p>",
             f"<p>{len(entries)} screenshots, {day_name(first[:10])} to {day_name(last[:10])}. "
             "Click a picture to see it at full size.</p></header>" if entries else "</header>",
             "<nav>"]
    parts += [f"<a href='#d{d}'>{datetime.date.fromisoformat(d).strftime('%b %-d')} · {len(es)}</a>" for d, es in days]
    parts.append("</nav><main>")
    for d, es in days:
        parts.append(f"<h2 id='d{d}'>{html.escape(day_name(d))}</h2>")
        for e in es:
            path = os.path.join(DIR, e["image"])
            if not os.path.exists(path):
                continue
            data, (w, h) = webp(path)
            src = "data:image/webp;base64," + base64.b64encode(data).decode()
            # Device pixels at 2×: small crops show at their real size.
            narrow = " class='narrow'" if w / 2 < 1068 else ""
            style = f" style='width:{w // 2}px'" if narrow else ""
            parts.append("<article><div class='text'>"
                         f"<time datetime='{e['time']}'>{e['time'][11:16]}</time>"
                         f"<h3>{html.escape(e['title'])}</h3>"
                         + (f"<p>{html.escape(e['caption'])}</p>" if e.get("caption") else "")
                         + f"</div><img{narrow}{style} loading='lazy' decoding='async' src='{src}' width='{w}' height='{h}' alt='{html.escape(e['title'], quote=True)}'></article>")
    parts.append("</main><footer>Generated by tools/journal_html.py from docs/journal/entries.json.</footer>")
    parts.append(f"<div id='zoom'><img alt=''></div><script>{SCRIPT}</script></body></html>")
    open(out_path, "w").write("".join(parts))
    return out_path


if __name__ == "__main__":
    a = sys.argv[1:]
    last = None
    if "--last" in a:
        i = a.index("--last")
        last = int(a[i + 1])
        del a[i:i + 2]
    out = build(a[0] if a else os.path.join(DIR, "journal.html"), last)
    print(out, f"{os.path.getsize(out) / 1e6:.1f} MB")
