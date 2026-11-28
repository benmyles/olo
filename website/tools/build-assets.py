#!/usr/bin/python3
"""Makes the website's assets from Olo's own sources, into website/assets:

  - the pictures, from docs/img: parts of 2x pictures shown at their own
    size (NAME-2x.webp and its half, NAME-1x.webp: never scaled by the
    browser), the desktop's plate at the widths the page asks for, the
    Dunes wallpapers (lossless, dithered: below), and DeskCalc's two
    pictures from an Agent Screen;
  - the app icons (data/icons/svg), as drawn;
  - the mark (data/icons/svg/olo.svg) without its contact shadow, and
    "Olo" set from Sora as tools/mkbrand.py sets it (ExtraBold, -0.03 em,
    its l a plain bar), as a vector in the brand's two inks;
  - the fonts, subset to what the page writes (needs fontTools: give its
    Python with FONTTOOLS_PY, e.g. a venv in /var/tmp; skipped without).

Run from anywhere:  /usr/bin/python3 website/tools/build-assets.py
Only what is missing or older than its source is made again.
"""
import os
import random
import re
import subprocess
import sys

from PIL import Image, ImageMath

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(ROOT, "website", "assets")
# Pictures made for the website alone (readme-shots with OLO_SHOTS_OUT:
# Rex, Agent Screens and Token Pool at the column's size, the looks in
# both modes), not kept in docs/img: WEBSITE_SHOTS=DIR, where they were
# written. Without them, what was made from them is kept as it is.
SHOTS = os.environ.get("WEBSITE_SHOTS")
IMG = os.path.join(ROOT, "docs", "img")
# The pack's 21:9 band: the shape the hero shows it in.
WALL = os.path.join(ROOT, "data", "wallpapers", "ultrawide")
ICONS = os.path.join(ROOT, "data", "icons", "svg")
FONTS = os.path.join(ROOT, "data", "fonts")


def stale(out, *src):
    return not os.path.exists(out) or any(os.path.getmtime(s) > os.path.getmtime(out) for s in src)


def webp(src, name, widths, quality=82, crop=None, lossless=False):
    """`src` at each width (its shape kept), as assets/img/NAME-W.webp."""
    os.makedirs(os.path.join(OUT, "img"), exist_ok=True)
    im = None
    for w in widths:
        out = os.path.join(OUT, "img", f"{name}-{w}.webp")
        if not stale(out, src, __file__):
            continue
        if im is None:
            im = Image.open(src)
            im = im.convert("RGBA" if im.mode in ("RGBA", "LA", "P") else "RGB")
            if crop:
                im = im.crop(crop)
        small = im if im.width == w else im.resize((w, round(im.height * w / im.width)), Image.LANCZOS)
        small.save(out, "WEBP", quality=quality, method=6, lossless=lossless)
        print(f"  {os.path.relpath(out, ROOT)}  {small.size[0]}x{small.size[1]}  {os.path.getsize(out) // 1024} KB")


def wallpaper(src, name, widths):
    """The pack's wallpaper at each width, as assets/img/NAME-W.webp,
    lossless. Its gradients are dithered (tools/restorewallpaper.py), and
    any lossy encoder smooths a dither away and gives the sky its 8-bit
    bands back (WebP at 100 and AVIF at 95 both did, measured): so each
    width is resized in floating point and dithered again to 8 bits, each
    pixel rounded up or down by chance in proportion to its fraction (one
    chance for its three channels: no colour noise), as the restorer does.
    Never wider than the source: nothing is enlarged."""
    os.makedirs(os.path.join(OUT, "img"), exist_ok=True)
    im = None
    for w in widths:
        out = os.path.join(OUT, "img", f"{name}-{w}.webp")
        if not stale(out, src, __file__):
            continue
        if im is None:
            im = Image.open(src).convert("RGB")
        if w >= im.width:
            small = im
        else:
            h = round(im.height * w / im.width)
            # floor(v + u), u uniform in (0, 1): F to L truncates (and clips).
            u = Image.frombytes("L", (w, h), random.Random(w).randbytes(w * h)).convert("F")
            u = u.point(lambda v: v / 256 + 1 / 512)
            chans = []
            for c in im.split():
                f = c.convert("F").resize((w, h), Image.LANCZOS)
                chans.append(ImageMath.lambda_eval(lambda a: a["f"] + a["u"], f=f, u=u).convert("L"))
            small = Image.merge("RGB", chans)
        small.save(out, "WEBP", lossless=True, quality=100, method=6)
        print(f"  {os.path.relpath(out, ROOT)}  {small.size[0]}x{small.size[1]}  {os.path.getsize(out) // 1024} KB")


PICTURES = [
    # (file in docs/img, name, widths): the desktop's plate, the one
    # picture shown smaller than its own size (it says so).
    ("olo-desktop.png", "desktop", [1280, 1920, 2560]),
    ("olo-desktop-dark.png", "desktop-dark", [1280, 1920, 2560]),
]

# The themes sheet: three looks across, two down (README, "Themes and looks").
LOOKS = ["classic", "platinum", "beos", "amigaos", "aqua", "tokyo-night"]
# Each look's own picture: the Pictures window's corner of readme-shots'
# themes scene, at 2x (docs/img/olo-look-NAME.png, cut from its 4K grab at
# 80,120 for 1440 x 810), from its window's frame, shown at its own size.
LOOK_PART = (40, 40, 1376, 606)

APP_ICONS = [
    "rex", "terminal", "agent-screens", "token-pool", "tracker", "styled-edit", "show-image",
    "calculator", "calendar", "todo", "passwords", "phone", "telegram", "weather", "screenshot",
    "activity", "doctor", "software-update", "logs", "drive-setup", "disk-usage", "diskbackup",
    "speed-test", "kache",
    # The page's own: Deskbar's sections, the desktop's icons.
    "desktop", "performance", "themes", "installer", "application",
    "olo", "query", "ai", "document", "software-update", "development",
    # The User's Guide's: its callouts' and its chapter openers' icons.
    "idea", "info", "keyboard", "warning", "preferences", "workspaces", "windows",
    "deskbar", "display", "sound", "notifications",
]




# Pictures at their own size: a part of a 2x picture (in its pixels), kept
# as NAME-2x.webp and, halved, NAME-1x.webp. The page shows them with
# density descriptors at their size in points, so the browser never scales
# them: one picture pixel to a device pixel at 1x and at 2x.
ONE_TO_ONE = [
    # (file, name, (x, y, w, h) in the file's pixels; w and h even)
    ("olo-palette.png", "palette-1to1", (1320, 432, 652, 410)),
    ("olo-palette-dark.png", "palette-dark-1to1", (1320, 432, 652, 410)),
    ("olo-tracker.png", "tracker-1to1", (60, 60, 652, 410)),
    ("olo-tracker-dark.png", "tracker-dark-1to1", (60, 60, 652, 410)),
    # Rex, Agent Screens and Token Pool made at the column's own size,
    # 688 x 431 points under their tabs (readme-shots with
    # OLO_SHOTS_WINDOW=688x452, both modes; cut at their frames).
    ("olo-site-rex-light.png", "rex-1to1", (0, 0, 1376, 862)),
    ("olo-site-rex-dark.png", "rex-dark-1to1", (0, 0, 1376, 862)),
    ("olo-site-screens-light.png", "screens-1to1", (0, 0, 1376, 862)),
    ("olo-site-screens-dark.png", "screens-dark-1to1", (0, 0, 1376, 862)),
    ("olo-site-pool-light.png", "pool-1to1", (0, 0, 1376, 862)),
    ("olo-site-pool-dark.png", "pool-dark-1to1", (0, 0, 1376, 862)),
    ("olo-installer.png", "installer-1to1", (445, 130, 1242, 830)),
]


def source(src):
    """`src` (in docs/img), else its name in WEBSITE_SHOTS, else None."""
    if os.path.exists(src):
        return src
    other = os.path.join(SHOTS, os.path.basename(src)) if SHOTS else None
    return other if other and os.path.exists(other) else None


def one_to_one(src, name, part):
    out2, out1 = (os.path.join(OUT, "img", f"{name}-{k}x.webp") for k in (2, 1))
    found = source(src)
    if not found:
        if not os.path.exists(out2):
            print(f"  {name}: no {os.path.basename(src)} (WEBSITE_SHOTS=DIR) and nothing made from it")
        return
    src = found
    if not stale(out2, src, __file__) and not stale(out1, src, __file__):
        return
    x, y, w, h = part
    im = Image.open(src).convert("RGB").crop((x, y, x + w, y + h))
    im.save(out2, "WEBP", quality=84, method=6)
    im.resize((w // 2, h // 2), Image.LANCZOS).save(out1, "WEBP", quality=86, method=6)
    print(f"  {name}: {w // 2}x{h // 2} pt, {os.path.getsize(out2) // 1024} + {os.path.getsize(out1) // 1024} KB")


def app_shots():
    """Each app's Get Info picture (website/index.html, Apps): the window
    below its tab, at its own size, light and dark, from readme-shots'
    website-apps scene (`OLO_SHOTS_OUT=DIR tools/dev/readme-shots
    website-apps`), given as WEBSITE_APP_SHOTS=DIR; kept as they are
    without it. Their sizes in points are the buttons' data-w and data-h."""
    folder = os.environ.get("WEBSITE_APP_SHOTS")
    if not folder:
        print("  kept: no WEBSITE_APP_SHOTS")
        return
    for f in sorted(os.listdir(folder)):
        m = re.fullmatch(r"([a-z-]+)-(light|dark)\.png", f)
        if m:
            w, h = Image.open(os.path.join(folder, f)).size
            one_to_one(os.path.join(folder, f), f"app-{m[1]}-{m[2]}", (0, 0, w // 2 * 2, h // 2 * 2))


def pictures():
    for src, name, part in ONE_TO_ONE:
        one_to_one(os.path.join(IMG, src), name, part)
    for look in LOOKS:
        # Light and dark (readme-shots' themes scene, and with OLO_SHOTS_FLIP=1):
        # Tokyo Night's own is its dark; its light is Tokyo Night Light.
        light = f"olo-look-{look}-light.png" if look == "tokyo-night" else f"olo-look-{look}.png"
        dark = f"olo-look-{look}.png" if look == "tokyo-night" else f"olo-look-{look}-dark.png"
        for src, mode in ((light, ""), (dark, "-dark")):
            one_to_one(os.path.join(IMG, src), f"look-{look}{mode}-1to1", LOOK_PART)
            # Its window's tab and frame corner, beside its name in the list.
            one_to_one(os.path.join(IMG, src), f"tab-{look}{mode}", (40, 40, 192, 56))

    for src, name, widths in PICTURES:
        webp(os.path.join(IMG, src), name, widths)
    for light in ("olo-dunes", "olo-dunes-dark"):
        wallpaper(os.path.join(WALL, light + ".webp"), light.replace("olo-", "wall-"), [1280, 1920, 2880, 3840])
    # DeskCalc, from a screen of Olo's own (website/README.md: how it was taken).
    # (At 1x too, 276 wide, made from the 2x by halving it here, once, so
    # that a 1x screen isn't left to shrink the 2x as it draws.)
    webp(os.path.join(ROOT, "website", "tools", "deskcalc.png"), "deskcalc", [276, 552], lossless=True)
    webp(os.path.join(ROOT, "website", "tools", "deskcalc-dark.png"), "deskcalc-dark", [276, 552], lossless=True)


# The sizes a drawing's `fromN` and `uptoN` classes name (crates/xdg/src/svg.rs).
STEPS = [16, 24, 32, 48, 64, 96, 128, 256, 512]


def size_sheet(svg):
    """Olo's per-size rule as media queries on the drawing's own width:
    `fromN` hidden under N, `uptoN` hidden from the step after N. (By CSS
    pixels: a browser doesn't tell an <img>'s drawing its screen's scale,
    so a 2x screen gets the 1x drawing of that size.)"""
    css = []
    for i, n in enumerate(STEPS):
        if n > STEPS[0] and re.search(rf"\bfrom{n}\b", svg):
            css.append(f"@media (max-width:{n - 0.02}px){{.from{n}{{display:none}}}}")
        if i + 1 < len(STEPS) and re.search(rf"\bupto{n}\b", svg):
            css.append(f"@media (min-width:{STEPS[i + 1]}px){{.upto{n}{{display:none}}}}")
    return "".join(css)


def icons():
    os.makedirs(os.path.join(OUT, "icons"), exist_ok=True)
    for name in sorted(set(APP_ICONS)):
        src, out = os.path.join(ICONS, name + ".svg"), os.path.join(OUT, "icons", name + ".svg")
        if stale(out, src, __file__):
            svg = open(src).read()
            svg = re.sub(r"<!--.*?-->\n?", "", svg, flags=re.S)
            # A browser draws every layer of a drawing at once, the stand-ins
            # for small sizes over the detail they stand in for: each layer
            # is shown where Olo's renderer shows it (docs/ICONS.md).
            sized = size_sheet(svg)
            if not sized:
                pass
            elif "</style>" in svg:
                svg = svg.replace("</style>", sized + "</style>", 1)
            else:
                svg = re.sub(r"(<svg\b[^>]*>)", lambda m: m.group(1) + "\n<style>" + sized + "</style>", svg, count=1)
            open(out, "w").write(svg)
    # The mark for the logo: the drawing without its contact shadow (a logo
    # stands on nothing), as mkbrand's `drawn` takes it.
    src, out = os.path.join(ICONS, "olo.svg"), os.path.join(OUT, "brand", "mark.svg")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    if stale(out, src, __file__):
        svg = open(src).read()
        svg = re.sub(r"<!--.*?-->\n?", "", svg, flags=re.S)
        a = svg.index('<ellipse class="from48"')
        b = svg.index("/>", a) + 2
        assert "url(#shadow)" in svg[a:b], "the drawing's first large-only ellipse is its shadow"
        svg = svg[:a] + svg[b:]
        open(out, "w").write(svg)
        print(f"  {os.path.relpath(out, ROOT)}")


FONTTOOLS_SCRIPT = r'''
import sys, json
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.boundsPen import BoundsPen
from fontTools.varLib.instancer import instantiateVariableFont

src, out = sys.argv[1], sys.argv[2]
def at(weight):
    f = TTFont(src)
    return instantiateVariableFont(f, {"wght": weight}, inplace=False)

olo, sub = at(800), at(600)
upm = olo["head"].unitsPerEm

def glyph(font, ch):
    gs = font.getGlyphSet()
    name = font.getBestCmap()[ord(ch)]
    g = gs[name]
    pen = SVGPathPen(gs)
    g.draw(pen)
    b = BoundsPen(gs)
    g.draw(b)
    return name, g.width, pen.getCommands(), b.bounds

# "Olo": O, then the l's stem alone (its flag left off: its columns at the
# foot, from the baseline to the ascender), then o. -0.03 em between each.
track = -0.03 * upm
parts, x = [], 0
_, w, d, _ = glyph(olo, "O")
parts.append((x, d)); x += w + track
gs = olo.getGlyphSet(); lname = olo.getBestCmap()[ord("l")]
# The stem's left and right where the l meets the baseline: the outline's
# points on or near y=0.
pts = []
from fontTools.pens.recordingPen import RecordingPen
rp = RecordingPen(); gs[lname].draw(rp)
for op, args in rp.value:
    for p in args:
        pts.append(p)
foot = [p[0] for p in pts if abs(p[1]) < 2]
s0, s1 = min(foot), max(foot)
top = max(p[1] for p in pts)
stem = f"M{x + s0:.1f} 0H{x + s1:.1f}V{top:.1f}H{x + s0:.1f}Z"
lw = gs[lname].width
parts.append((0, stem)); x += lw + track
_, w, d, b = glyph(olo, "o")
parts.append((x, d)); x += w
# Ink box of "Olo".
_, _, _, ob = glyph(olo, "O")
x0 = ob[0]
x1 = x - w + b[2]
cap = olo["OS/2"].sCapHeight
# Its ink from top to foot: the rounds overshoot the cap height and the
# baseline (O and o by some 20 units each way), and are never cut off.
y0, y1 = min(ob[1], b[1], 0), max(ob[3], b[3], top, cap)
print(json.dumps({"upm": upm, "parts": parts, "x0": x0, "x1": x1, "cap": cap, "y0": y0, "y1": y1,
                  "desktop": [(c,) + glyph(sub, c)[1:3] + (glyph(sub, c)[3],) for c in "DESKTOP"]}))
'''


def wordmark():
    """ "Olo" as mkbrand sets it (Sora ExtraBold, -0.03 em, its l a plain
    bar), written as SVG paths in the brand's ink for light and for dark:
    the mark and "Olo", as Olo's own About sets them (the lockup with
    DESKTOP under it is too small to read at a page's logo size)."""
    out = os.path.join(OUT, "brand", "olo.svg")
    py = os.environ.get("FONTTOOLS_PY")
    if not py or not stale(out, os.path.join(FONTS, "Sora[wght].ttf"), __file__):
        return
    import json
    data = json.loads(subprocess.run([py, "-c", FONTTOOLS_SCRIPT, os.path.join(FONTS, "Sora[wght].ttf"), out],
                                     check=True, capture_output=True, text=True).stdout)
    upm, cap = data["upm"], data["cap"]
    # Glyph space has y up: flip into SVG's, baseline at `cap`.
    paths = []
    for dx, d in data["parts"]:
        paths.append(f'<path transform="translate({dx:.1f} 0)" d="{d}"/>')
    olo_w = data["x1"] - data["x0"]
    # DESKTOP: SemiBold, tracked so its ink is 0.98 of Olo's (mkbrand), its
    # cap height set by that width; 0.30 cap below Olo.
    glyphs = data["desktop"]
    adv = sum(g[1] for g in glyphs)
    ink_l = glyphs[0][3][0]
    ink_r = glyphs[-1][1] - glyphs[-1][3][2]
    # width = s * (adv - ink_l - ink_r + 6*track), track = 0.32 em (in the scaled font)
    target = 0.98 * olo_w
    s = target / (adv - ink_l - ink_r + 6 * 0.32 * upm)
    track = 0.32 * upm
    gap = 0.30 * cap
    sub_cap = cap * s
    x = data["x0"] + (olo_w - target) / 2 - ink_l * s
    sub = []
    for c, w, d, _b in glyphs:
        sub.append(f'<path transform="translate({x:.1f} 0) scale({s:.4f})" d="{d}"/>')
        x += (w + track) * s
    h = cap + gap + sub_cap
    # Everything in glyph space (y up), flipped once; Olo's baseline at y=0,
    # DESKTOP's at -(gap + sub_cap).
    # The brand's colours (BRAND.md): Charcoal and Desktop Gray on light;
    # #F4F6FA and #A1A7B3 on dark.
    # The box is the ink's, overshoots and all (it was the cap height to
    # the baseline, and cut the O's and o's rounds flat; Ben, 2026-11-27).
    y0, y1 = data["y0"], data["y1"]
    for name, ink in (("olo.svg", "#0B0D14"), ("olo-on-dark.svg", "#F4F6FA")):
        svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{data["x0"]:.0f} 0 {olo_w:.0f} {y1 - y0:.0f}">'
               f'<g fill="{ink}" transform="translate(0 {y1:.0f}) scale(1 -1)">{"".join(paths)}</g></svg>')
        open(os.path.join(OUT, "brand", name), "w").write(svg)
        print(f"  website/assets/brand/{name}  ({olo_w:.0f} x {y1 - y0:.0f}: cap {cap:.0f}, {y1 - cap:.0f} above it, {-y0:.0f} below the baseline)")


UNICODES = ("U+0020-007E,U+00A0-00FF,U+0131,U+0152-0153,U+02C6,U+02DA,U+02DC,U+2010-2027,U+2030-203A,"
            "U+2044,U+20AC,U+2122,U+2190-2193,U+2212,U+2215,U+2026")


def fonts():
    py = os.environ.get("FONTTOOLS_PY")
    if not py:
        print("  fonts: skipped (no FONTTOOLS_PY)")
        return
    d = os.path.join(OUT, "fonts")
    os.makedirs(d, exist_ok=True)
    tmp = os.path.join("/var/tmp", f"olo-website-fonts-{os.getuid()}")
    os.makedirs(tmp, exist_ok=True)
    bindir = os.path.dirname(py)
    inst = [py, "-m", "fontTools.varLib.instancer"]
    subset = [py, "-m", "fontTools.subset"]
    jobs = [
        ("Sora[wght].ttf", "wght=300:800", "sora.woff2", UNICODES, "kern,liga,calt,case,tnum,lnum"),
        ("NotoSans[wght].ttf", "wght=400:700", "noto-sans.woff2", UNICODES, "kern,liga,calt,tnum,lnum"),
        ("NotoSansMono[wght].ttf", "wght=400:600", "noto-mono.woff2", UNICODES + ",U+2500-257F,U+25B8,U+2713,U+00D7,U+00F7", "kern"),
        ("NotoSansSymbols2-Regular.ttf", None, "symbols2.woff2", "U+2318,U+2325,U+21E7,U+232B,U+2713,U+25B8", ""),
        ("NotoSansSymbols[wght].ttf", "wght=400", "symbols1.woff2", "U+2303", ""),
    ]
    for src, axes, name, uni, feats in jobs:
        s, out = os.path.join(FONTS, src), os.path.join(d, name)
        if not stale(out, s, __file__):
            continue
        mid = s
        if axes:
            mid = os.path.join(tmp, name + ".ttf")
            subprocess.run(inst + [s, axes, "-o", mid, "-q"], check=True)
        args = subset + [mid, f"--unicodes={uni}", "--flavor=woff2", f"--output-file={out}"]
        args.append(f"--layout-features={feats}" if feats else "--layout-features=")
        subprocess.run(args, check=True)
        print(f"  {os.path.relpath(out, ROOT)}  {os.path.getsize(out) // 1024} KB")


# The Looks window's icon well: each of Olo's icon sets
# (olo_theme::looks::ICON_SETS), these icons, drawn by Olo's own renderer
# as Olo draws them (crates/xdg/examples/svgicon: hinted for the size, in
# the set's colours), at 64 points, 1x and 2x, one strip a set.
ICON_SETS = ["classic", "platinum", "beos", "amiga", "aqua", "flat"]
SET_ICONS = ["folder", "home", "disk", "trash-full", "terminal", "tracker", "styled-edit", "mail",
             "music", "image", "calendar", "calculator", "preferences", "network", "printer", "rex"]


def icon_sets():
    """assets/img/iconset-SET-{1x,2x}.webp. A set's own drawing of an icon
    (data/themes/SET/icons/svg) is drawn as it is; else Classic's, in the
    set's colours (its recolour.txt), as Olo does. Needs the svgicon
    example built (`jato build -p olo-xdg --example svgicon`; SVGICON=PATH
    for another): skipped without it."""
    import tempfile

    tool = os.environ.get("SVGICON") or os.path.join(ROOT, "target", "debug", "examples", "svgicon")
    if not os.path.exists(tool):
        print(f"  skipped: no {os.path.relpath(tool, ROOT)}")
        return
    env = dict(os.environ, OLO_DATA=os.path.join(ROOT, "data"))
    for st in ICON_SETS:
        own = os.path.join(ROOT, "data", "themes", st, "icons")
        srcs = []
        for n in SET_ICONS:
            mine = os.path.join(own, "svg", n + ".svg")
            srcs.append((n, mine if os.path.exists(mine) else os.path.join(ICONS, n + ".svg"), not os.path.exists(mine) and st != "classic"))
        rules = [os.path.join(own, "recolour.txt")] if os.path.exists(os.path.join(own, "recolour.txt")) else []
        outs = {k: os.path.join(OUT, "img", f"iconset-{st}-{k}x.webp") for k in (1, 2)}
        if not any(stale(o, *[x[1] for x in srcs], *rules, __file__) for o in outs.values()):
            continue
        with tempfile.TemporaryDirectory(dir="/var/tmp") as tmp:
            for k, px in ((1, 64), (2, 128)):
                strip = Image.new("RGBA", (px * len(srcs), px), (0, 0, 0, 0))
                for i, (n, src, themed) in enumerate(srcs):
                    png = os.path.join(tmp, f"{n}-{px}.png")
                    subprocess.run([tool] + (["--theme", st] if themed else []) + [src, png, str(px)], check=True, env=env)
                    strip.paste(Image.open(png).convert("RGBA"), (i * px, 0))
                strip.save(outs[k], "WEBP", lossless=True, quality=100, method=6)
                print(f"  {os.path.relpath(outs[k], ROOT)}  {strip.size[0]}x{strip.size[1]}  {os.path.getsize(outs[k]) // 1024} KB")


if __name__ == "__main__":
    print("pictures"); pictures()
    print("icons"); icons()
    print("icon sets"); icon_sets()
    print("apps"); app_shots()
    print("wordmark"); wordmark()
    print("fonts"); fonts()
