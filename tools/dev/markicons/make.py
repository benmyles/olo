#!/usr/bin/python3
"""Other people's marks (Chrome, Slack, Telegram, GitHub), drawn for each icon set.

    make.py write             write every set's drawings into the repository
    make.py sheet OUT.png     render them all (svgicon, as Olo draws them) at 128, 48,
                              40, 32, 20 and 16 pixels on a light and a dark ground

A mark faces whoever looks at it, in every set: a logo is never sheared or turned
(docs/ICONS.md, "Other people's marks"). What carries it is the set's: a thick
badge or tile standing, its edge showing to the left and above (classic, and the
sets after it: Platinum's ink and lilac greys, BeOS's and AmigaOS's flat faces in
their palettes, Aqua's gel); in Flat, the mark alone on its plain disc or tile.
Build `svgicon` first (cargo build --release -p olo-xdg --example svgicon).
"""
import colorsys, math, os, subprocess, sys
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))

def n(v):
    s = ("%.2f" % v).rstrip("0").rstrip(".")
    return s if s not in ("", "-0") else "0"
def rgb(h): return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))
def hexc(c): return "#%02x%02x%02x" % tuple(max(0, min(255, int(round(v)))) for v in c)
def mix(a, b, t):
    a, b = rgb(a), rgb(b)
    return hexc(tuple(a[i] + (b[i] - a[i]) * t for i in range(3)))
def dark(c, t): return mix(c, "#000000", t)
def light(c, t): return mix(c, "#ffffff", t)
def hls(c, dh=0, ms=1.0, dl=0.0):
    r, g, b = (v / 255 for v in rgb(c))
    h, l, s = colorsys.rgb_to_hls(r, g, b)
    r, g, b = colorsys.hls_to_rgb((h + dh / 360) % 1, max(0, min(1, l + dl)), max(0, min(1, s * ms)))
    return hexc((r * 255, g * 255, b * 255))

# --- the sets ----------------------------------------------------------------
AMIGA = ["#959595", "#000000", "#ffffff", "#3b67a2", "#7b7b7b", "#afafaf", "#aa907c", "#ffa997", "#ff8800", "#eecc44", "#4a9a4a", "#22407a", "#6b4a2a", "#cc3333", "#8fb4e8", "#444444"]
def nearest(c, pens):
    r, g, b = rgb(c)
    return min(pens, key=lambda p: (2 * (rgb(p)[0] - r)) ** 2 + (4 * (rgb(p)[1] - g)) ** 2 + (3 * (rgb(p)[2] - b)) ** 2)
BEOS = {"#ecb22e": "#ffcc33", "#fbbc04": "#ffcc00", "#e01e5a": "#cc0066", "#2eb67d": "#33cc66", "#36c5f0": "#33ccff", "#2aabee": "#3399ff", "#4285f4": "#3366ff", "#ea4335": "#ff3333", "#34a853": "#339966"}
def cube(c):
    if c in BEOS: return BEOS[c]
    r, g, b = rgb(c)
    if max(r, g, b) - min(r, g, b) < 18:
        v = round((r + g + b) / 3 / 255 * 31) * 255 / 31
        return hexc((v, v, v))
    return hexc(tuple(round(v / 51) * 51 for v in (r, g, b)))
AMIGA_OF = {"#d0d7de": "#afafaf", "#4285f4": "#3b67a2", "#2aabee": "#3b67a2", "#36c5f0": "#8fb4e8", "#ecb22e": "#eecc44", "#fbbc04": "#eecc44", "#e01e5a": "#cc3333", "#ea4335": "#cc3333", "#2eb67d": "#4a9a4a", "#34a853": "#4a9a4a"}
def lilac(c):
    "Platinum: greys a lilac grey, everything a little softer."
    r, g, b = rgb(c)
    if max(r, g, b) - min(r, g, b) < 18:
        l = (r + g + b) / 3 / 255
        rr, gg, bb = colorsys.hls_to_rgb(240 / 360, l, 0.24 if 0.08 < l < 0.97 else 0.0)
        return hexc((rr * 255, gg * 255, bb * 255))
    return hls(c, 0, 0.9, 0.02)
def aqua(c):
    r, g, b = rgb(c)
    if max(r, g, b) - min(r, g, b) < 18:
        l = (r + g + b) / 3 / 255
        rr, gg, bb = colorsys.hls_to_rgb(212 / 360, l, 0.10 if l < 0.97 else 0.0)
        return hexc((rr * 255, gg * 255, bb * 255))
    return hls(c, 0, 1.06, 0.02)

SETS = {
    # id: (folder, ink, outline opacity, colour, gradients, soft shadow, gloss, solid)
    "classic": dict(folder="data/icons/svg", ink="#1b1b1f", ink_a=1, col=lambda c: c, grad=True, shadow=True, gloss=0, solid=True),
    "platinum": dict(folder="data/themes/platinum/icons/svg", ink="#0b0b16", ink_a=1, col=lilac, grad=True, shadow=True, gloss=0, solid=True),
    "beos": dict(folder="data/themes/beos/icons/svg", ink="#000000", ink_a=1, col=cube, grad=False, shadow=False, gloss=0, solid=True),
    "amiga": dict(folder="data/themes/amiga/icons/svg", ink="#000000", ink_a=1, col=lambda c: AMIGA_OF.get(c) or nearest(c, AMIGA), grad=False, shadow=False, gloss=0, solid=True),
    "aqua": dict(folder="data/themes/aqua/icons/svg", ink="#30465f", ink_a=0.55, col=aqua, grad=True, shadow=True, gloss=1, solid=True),
    "flat": dict(folder="data/themes/flat/icons/svg", ink="#000000", ink_a=0, col=lambda c: c, grad=False, shadow=False, gloss=0, solid=False),
}

# --- a drawing ---------------------------------------------------------------
LEAN = -0.125   # a mark's front plane rises 1 in 8: half as much as the set's other standing things

class D:
    def __init__(self, st, title):
        self.st, self.title, self.defs, self.items, self.k = st, title, [], [], 0
        self.level = []      # what lies on the desk (the shadow): not leant
    def col(self, c): return self.st["col"](c)
    def rim(self, c, t=0.4):
        "The colour of the edge of a face of colour c: the set's colour for it, darkened, in the set's colours again."
        return self.col(dark(self.col(c), t))
    def add(self, *items):
        for i in items:
            if isinstance(i, (list, tuple)): self.add(*i)
            elif i: self.items.append(i)
    def clip(self, shape):
        self.k += 1
        self.defs.append('<clipPath id="c%d">%s</clipPath>' % (self.k, shape))
        return "c%d" % self.k
    def grad(self, x1, y1, x2, y2, stops):
        "A linear gradient in the drawing's units: stops of (offset, colour, opacity)."
        self.k += 1
        self.defs.append('<linearGradient id="g%d" x1="%s" y1="%s" x2="%s" y2="%s" gradientUnits="userSpaceOnUse">%s</linearGradient>' % (
            self.k, n(x1), n(y1), n(x2), n(y2), "".join('<stop offset="%s" stop-color="%s"%s/>' % (n(o), c, "" if a == 1 else ' stop-opacity="%s"' % n(a)) for o, c, a in stops)))
        return "url(#g%d)" % self.k
    def shadow(self, cx, cy, rx):
        if not self.st["shadow"]: return
        self.k += 1
        self.defs.append('<radialGradient id="g%d" cx="%s" cy="%s" r="%s" gradientUnits="userSpaceOnUse" gradientTransform="translate(0 %s) scale(1 .13)"><stop offset="0" stop-opacity=".32"/><stop offset="1" stop-opacity="0"/></radialGradient>' % (
            self.k, n(cx), n(cy), n(rx), n(cy * 0.87)))
        self.level.append('<ellipse class="from48" cx="%s" cy="%s" rx="%s" ry="%s" fill="url(#g%d)"/>' % (n(cx), n(cy), n(rx), n(rx * 0.14), self.k))
    def svg(self, manner):
        st = self.st
        o = ".o{paint-order:stroke;stroke:%s;stroke-width:1.2;stroke-linejoin:round%s}" % (st["ink"], "" if st["ink_a"] == 1 else ";stroke-opacity:%s" % n(st["ink_a"])) if st["ink_a"] else ".o{paint-order:stroke;stroke:none}"
        return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n<!-- %s (%s). -->\n<style>%s</style>\n%s%s\n</svg>\n' % (
            self.title, manner, o, ("<defs>\n%s\n</defs>\n" % "\n".join(self.defs)) if self.defs else "", "\n".join(self.level + self.body())))
    def body(self):
        if not self.st["solid"]: return self.items
        # (Leant about the middle of the canvas, so the thing stays where it was drawn.)
        return ['<g transform="matrix(1 %s 0 1 0 %s)">' % (n(LEAN), n(-LEAN * 34))] + self.items + ["</g>"]

def circle(cx, cy, r, fill, c=None, extra=""):
    return '<circle%s cx="%s" cy="%s" r="%s" fill="%s"%s/>' % (' class="%s"' % c if c else "", n(cx), n(cy), n(r), fill, extra)
def path(d, fill, c=None, extra=""):
    return '<path%s fill="%s" d="%s"%s/>' % (' class="%s"' % c if c else "", fill, d, extra)
def group(items, transform=None, c=None, clip=None):
    a = (' class="%s"' % c if c else "") + (' transform="%s"' % transform if transform else "") + (' clip-path="url(#%s)"' % clip if clip else "")
    return "<g%s>%s</g>" % (a, "".join(items))
def pt(cx, cy, r, deg):
    "The point at `deg` (clockwise on screen from 3 o'clock) on a circle."
    return cx + r * math.cos(math.radians(deg)), cy + r * math.sin(math.radians(deg))

# --- the marks, each facing forward, in a circle of radius r about cx, cy -----
def chrome(d, cx, cy, r):
    "Chrome's: three arms turning round a blue eye, each edge a tangent of the white ring."
    red, yellow, green, blue = d.col("#ea4335"), d.col("#fbbc04"), d.col("#34a853"), d.col("#4285f4")
    a = r * 0.47                                   # the white ring's radius, which the arms' inner edges touch
    def arm(k, fill):
        # From the ring's point T (at -90 + 120k), along the tangent to the rim P, round the rim to the
        # next arm's P, back along that arm's tangent to its T, and round the ring to where it began.
        t0, p0, p1, t1 = -90 + 120 * k, -30 + 120 * k, 90 + 120 * k, 30 + 120 * k
        (tx, ty), (px, py), (qx, qy), (ux, uy) = pt(cx, cy, a, t0), pt(cx, cy, r, p0), pt(cx, cy, r, p1), pt(cx, cy, a, t1)
        return path("M%s %sL%s %sA%s %s 0 0 1 %s %sL%s %sA%s %s 0 0 0 %s %sZ" % (n(tx), n(ty), n(px), n(py), n(r), n(r), n(qx), n(qy), n(ux), n(uy), n(a), n(a), n(tx), n(ty)), fill)
    # (Yellow first from the top's tangent: yellow right, green below and left, red above.)
    return [circle(cx, cy, r, red), arm(0, yellow), arm(1, green), circle(cx, cy, a, d.col("#ffffff")), circle(cx, cy, a * 0.79, blue)]

PLANE = "M54.3 118.8c35-15.2 58.3-25.3 70-30.2 33.3-13.9 40.3-16.3 44.8-16.4 1 0 3.2.2 4.7 1.4 1.2 1 1.5 2.3 1.7 3.3s.4 3.1.2 4.7c-1.8 19-9.6 65.1-13.6 86.3-1.7 9-5 12-8.2 12.3-7 .6-12.3-4.6-19-9-10.6-6.9-16.5-11.2-26.8-18-11.9-7.8-4.2-12.1 2.6-19.1 1.8-1.8 32.5-29.8 33.1-32.3.1-.3.1-1.5-.6-2.1-.7-.6-1.7-.4-2.5-.2-1.1.2-17.9 11.4-50.6 33.5-4.8 3.3-9.1 4.9-13 4.8-4.3-.1-12.5-2.4-18.7-4.4-7.5-2.4-13.5-3.7-13-7.9.3-2.2 3.3-4.4 9-6.7z"
def telegram(d, cx, cy, r):
    "Telegram's: the paper plane, white on its blue."
    def plane(k, c):
        # (k times its size about its own middle, which is a little left of and below the disc's.)
        s = r / 120 * k
        return group([path(PLANE, d.col("#ffffff"))], "translate(%s %s) scale(%s)" % (n(cx - 112 * s), n(cy - 123 * s), n(s)), c)
    return [circle(cx, cy, r, d.col("#2aabee")), plane(1.0, "from48"), plane(1.2, "upto32")]

CAT = "M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"
def github(d, cx, cy, r):
    "GitHub's: the cat, white in its dark disc, a white rim round it so it shows on a dark ground."
    m = r if d.st["solid"] else r * 0.93
    return [circle(cx, cy, r, d.col("#ffffff")), group([path(CAT, d.col("#24292f"))], "translate(%s %s) scale(%s)" % (n(cx - m), n(cy - m), n(m / 8)))]

SLACK = [("#36c5f0", "M12.9 32.3h32.3a12.9 12.9 0 0 1 0 25.8h-32.3a12.9 12.9 0 0 1 0-25.8zM45.2 25.8h-12.9v-12.9a12.9 12.9 0 1 1 12.9 12.9z"),
         ("#2eb67d", "M64.7 12.9a12.9 12.9 0 0 1 25.8 0v32.3a12.9 12.9 0 0 1-25.8 0zM97 45.2v-12.9h12.9a12.9 12.9 0 1 1-12.9 12.9z"),
         ("#ecb22e", "M77.6 64.7h32.3a12.9 12.9 0 0 1 0 25.8h-32.3a12.9 12.9 0 0 1 0-25.8zM77.6 97h12.9v12.9a12.9 12.9 0 1 1-12.9-12.9z"),
         ("#e01e5a", "M32.3 77.6a12.9 12.9 0 0 1 25.8 0v32.3a12.9 12.9 0 0 1-25.8 0zM25.8 77.6v12.9h-12.9a12.9 12.9 0 1 1 12.9-12.9z")]
def slack(d, cx, cy, w, c=None):
    "Slack's: four pieces of four colours, `w` across."
    s = w / 122.8
    return group([path(p, d.col(f)) for f, p in SLACK], "translate(%s %s) scale(%s)" % (n(cx - w / 2), n(cy - w / 2), n(s)), c)

# --- what carries a mark --------------------------------------------------------
def shade(d, cx, cy, r, clip):
    "A face's light: lighter toward the top left, darker away from it (sets with soft faces)."
    if not d.st["grad"]: return []
    g = d.grad(cx - r * 0.72, cy - r * 0.72, cx + r * 0.72, cy + r * 0.72, [(0, "#ffffff", 0.30), (0.45, "#ffffff", 0), (0.55, "#000000", 0), (1, "#000000", 0.22)])
    return [group(['<rect x="%s" y="%s" width="%s" height="%s" fill="%s"/>' % (n(cx - r - 1), n(cy - r - 1), n(2 * r + 2), n(2 * r + 2), g)], clip=clip)]
def gel(d, cx, cy, r, clip, tile=False):
    "Aqua's shine: a light lens over the upper half of a face, and a glow coming up from below."
    if not d.st["gloss"]: return []
    top = d.grad(cx, cy - r, cx, cy - r * 0.02, [(0, "#ffffff", 0.82), (1, "#ffffff", 0.08)])
    low = d.grad(cx, cy + r * 0.2, cx, cy + r, [(0, "#ffffff", 0), (1, "#ffffff", 0.34)])
    lens = ('<rect x="%s" y="%s" width="%s" height="%s" rx="%s" fill="%s"/>' % (n(cx - r * 0.8), n(cy - r * 0.93), n(r * 1.6), n(r * 0.86), n(r * 0.3), top) if tile
            else '<ellipse cx="%s" cy="%s" rx="%s" ry="%s" fill="%s"/>' % (n(cx), n(cy - r * 0.47), n(r * 0.74), n(r * 0.45), top))
    return [group(['<rect x="%s" y="%s" width="%s" height="%s" fill="%s"/>' % (n(cx - r), n(cy), n(2 * r), n(r), low), lens], clip=clip)]

def badge(d, mark, edge, cx=35, cy=34.5, r=24.5, t=4.2, edges=None):
    """A thick disc standing, facing forward, `t` thick: its edge shows to the left and above.
    `edge`: the edge's colour; `edges(d, bx, by)`: more of the edge's colours, drawn over it."""
    bx, by = cx - t, cy - t
    (ax, ay), (ex, ey) = pt(cx, cy, r, -45), pt(cx, cy, r, 135)
    sil = "M%s %sL%s %sA%s %s 0 0 0 %s %sL%s %sA%s %s 0 0 0 %s %sZ" % (n(ax), n(ay), n(ax - t), n(ay - t), n(r), n(r), n(ex - t), n(ey - t), n(ex), n(ey), n(r), n(r), n(ax), n(ay))
    face = d.clip(circle(cx, cy, r, "#000"))
    d.shadow(cx - 1, 60.3, r * 0.94)
    d.add(path(sil, d.rim(edge), "o"))
    if edges or d.st["grad"]:
        inside = d.clip(path(sil, "#000"))
        more = list(edges(d, bx, by) if edges else [])
        if d.st["grad"]:
            # (The edge turns from the light above to the dark at the left.)
            g = d.grad(bx - r, by + r * 0.6, bx + r * 0.5, by - r, [(0, "#000000", 0.26), (0.55, "#000000", 0), (1, "#ffffff", 0.2)])
            more.append('<rect x="0" y="0" width="64" height="64" fill="%s"/>' % g)
        d.add(group(more, clip=inside))
    d.add(mark(d, cx, cy, r))
    d.add(shade(d, cx, cy, r, face), gel(d, cx, cy, r, face))
    if d.st["grad"]:
        (hx, hy), (kx, ky) = pt(cx, cy, r - 0.6, 172), pt(cx, cy, r - 0.6, 282)
        d.add('<path class="from48" fill="none" stroke="#fff" stroke-opacity=".6" stroke-width=".6" stroke-linecap="round" d="M%s %sA%s %s 0 0 1 %s %s"/>' % (n(hx), n(hy), n(r - 0.6), n(r - 0.6), n(kx), n(ky)))

def rrect(x, y, w, h, r):
    return "M%s %sH%sA%s %s 0 0 1 %s %sV%sA%s %s 0 0 1 %s %sH%sA%s %s 0 0 1 %s %sV%sA%s %s 0 0 1 %s %sZ" % (
        n(x + r), n(y), n(x + w - r), n(r), n(r), n(x + w), n(y + r), n(y + h - r), n(r), n(r), n(x + w - r), n(y + h), n(x + r), n(r), n(r), n(x), n(y + h - r), n(y + r), n(r), n(r), n(x + r), n(y))
def slab(d, top, side, face, x=11.5, y=10.5, s=47, r=10, t=4.2):
    "A thick rounded tile standing, facing forward: its top and its left side show. Returns its face's clip."
    k = r * (1 - math.sqrt(0.5))
    x2, y2 = x + s, y + s
    sil = ("M%s %sA%s %s 0 0 1 %s %sV%sA%s %s 0 0 1 %s %sH%sA%s %s 0 0 1 %s %sL%s %sA%s %s 0 0 1 %s %sV%sA%s %s 0 0 1 %s %sH%sA%s %s 0 0 1 %s %sZ" % (
        n(x2 - k), n(y + k), n(r), n(r), n(x2), n(y + r), n(y2 - r), n(r), n(r), n(x2 - r), n(y2), n(x + r), n(r), n(r), n(x + k), n(y2 - k),
        n(x + k - t), n(y2 - k - t), n(r), n(r), n(x - t), n(y2 - r - t), n(y + r - t), n(r), n(r), n(x + r - t), n(y - t), n(x2 - r - t), n(r), n(r), n(x2 - k - t), n(y + k - t)))
    d.shadow(x + s / 2 - 1, 60.3, s * 0.52)
    d.add(path(sil, d.col(side), "o"))
    inside = d.clip(path(sil, "#000"))
    # Its top and its left side meet along the corner that comes forward (the diagonal through the
    # face's top left corner): soft sets turn from one to the other there, flat ones change there.
    if d.st["grad"]:
        g = d.grad(x - t, y2 - t, x2 - t, y - t, [(0, d.col(dark(side, 0.22)), 1), (0.44, d.col(side), 1), (0.53, d.col(top), 1), (1, d.col(light(top, 0.45)), 1)])
        d.add(group(['<rect x="0" y="0" width="64" height="64" fill="%s"/>' % g], clip=inside))
    else:
        d.add(group([path("M%s %sL%s %sH80V-20Z" % (n(x - 30), n(y - 30), n(x + 30), n(y + 30)), d.col(top))], clip=inside))
    fp = rrect(x, y, s, s, r)
    paint = d.grad(x, y, x2, y2, [(0, d.col(light(face, 0.5)), 1), (1, d.col(dark(face, 0.07)), 1)]) if d.st["grad"] else d.col(face)
    d.add(path(fp, paint))
    if d.st["grad"]:
        d.add('<path class="from48" fill="none" stroke="#fff" stroke-opacity=".8" stroke-width=".6" stroke-linecap="round" d="M%s %sV%sA%s %s 0 0 1 %s %sH%s"/>' % (
            n(x + 0.4), n(y2 - r), n(y + r), n(r - 0.4), n(r - 0.4), n(x + r), n(y + 0.4), n(x2 - r)))
    return d.clip(path(fp, "#000"))

# --- the drawings ---------------------------------------------------------------
def chrome_edges(d, bx, by):
    # The green arm's edge, below where it meets the red at the rim (210 degrees): the rest is red's.
    r = 24.5
    px, py = pt(bx + 4.2, by + 4.2, r, 210)
    u = math.sqrt(0.5)
    a, b = (px - 40 * u, py - 40 * u), (px + 40 * u, py + 40 * u)
    return [path("M%s %sL%s %sL%s %sL%s %sZ" % (n(a[0]), n(a[1]), n(b[0]), n(b[1]), n(b[0] - 80 * u), n(b[1] + 80 * u), n(a[0] - 80 * u), n(a[1] + 80 * u)), d.rim("#34a853", 0.38))]

def draw(set_id, name):
    st = SETS[set_id]
    if name == "google-chrome":
        d = D(st, "Google Chrome: its mark, facing forward")
        if st["solid"]: badge(d, chrome, "#ea4335", edges=chrome_edges)
        else: d.add(chrome(d, 32, 32, 27))
    elif name == "telegram":
        d = D(st, "Telegram: its paper plane on its blue, facing forward")
        if st["solid"]: badge(d, telegram, "#2aabee")
        else: d.add(telegram(d, 32, 32, 27))
    elif name == "github":
        d = D(st, "GitHub: its cat in its dark disc, facing forward")
        if st["solid"]: badge(d, github, "#d0d7de")
        else: d.add(github(d, 32, 32, 27))
    elif name == "slack":
        d = D(st, "Slack: its mark on a white tile, facing forward")
        if st["solid"]:
            face = slab(d, "#e9e8e4", "#a9a8ab", "#f2f1ee")
            cx, cy = 11.5 + 23.5, 10.5 + 23.5
            # (Aqua's shine is the tile's, under what's printed on it.)
            d.add(gel(d, cx, cy, 23.5, face, tile=True))
            d.add(slack(d, cx, cy, 31, "from48"), slack(d, cx, cy, 35, "upto32"))
        else:
            d.add('<rect x="6" y="6" width="52" height="52" rx="13" fill="#ffffff" stroke="#c3ccd9" stroke-width="2"/>')
            d.add(slack(d, 32, 32, 33, "from48"), slack(d, 32, 32, 37, "upto32"))
    manner = {"classic": "the classic set", "platinum": "Platinum: its ink and lilac greys", "beos": "BeOS: flat faces on its palette", "amiga": "AmigaOS: flat faces in a Workbench screen's pens",
              "aqua": "Aqua: gel", "flat": "Flat: seen straight on, plain faces"}[set_id]
    return d.svg(manner)

NAMES = {"classic": ["google-chrome", "slack", "telegram", "github"], "platinum": ["google-chrome", "slack", "telegram", "github"], "beos": ["google-chrome", "slack", "telegram", "github"],
         "amiga": ["google-chrome", "slack", "telegram", "github"], "aqua": ["google-chrome", "slack", "telegram", "github"], "flat": ["google-chrome", "slack", "telegram", "github"]}

def write(root):
    count = 0
    for set_id, names in NAMES.items():
        folder = os.path.join(root, SETS[set_id]["folder"])
        os.makedirs(folder, exist_ok=True)
        for name in names:
            with open(os.path.join(folder, name + ".svg"), "w") as f:
                f.write(draw(set_id, name))
            count += 1
    return count

def sheet(out, only=None):
    from PIL import Image
    tmp = os.path.join(os.path.dirname(os.path.abspath(out)), "marks-sheet")
    write(tmp)
    svgicon = os.path.join(ROOT, "target", "release", "examples", "svgicon")
    sizes = [(128, 1), (48, 1), (40, 2), (32, 2), (20, 3), (16, 3)]
    sets = [s for s in NAMES if not only or s in only]
    names = ["google-chrome", "slack", "telegram", "github"]
    cw = sum(s * k + 4 for s, k in sizes) + 12
    im = Image.new("RGB", (cw * 2, (len(sets) * len(names)) * 132), "#eef1f5")
    row = 0
    for set_id in sets:
        for name in names:
            src = os.path.join(tmp, SETS[set_id]["folder"], name + ".svg")
            if not os.path.exists(src): continue
            png = os.path.join(tmp, "%s-%s.png" % (set_id, name))
            r = subprocess.run([svgicon, src, png] + [str(s) for s, _ in sizes], capture_output=True, text=True)
            if r.returncode:
                print("can't draw", set_id, name, r.stderr[:300]); continue
            for half, ground in enumerate(("#eef1f5", "#20262e")):
                x = half * cw
                Image.Image.paste(im, Image.new("RGB", (cw, 132), ground), (x, row * 132))
                x += 6
                for s, k in sizes:
                    p = Image.open(png.replace(".png", "-%d.png" % s)).convert("RGBA").resize((s * k, s * k), Image.NEAREST)
                    im.paste(p, (x, row * 132 + (132 - s * k) // 2), p); x += s * k + 4
            row += 1
    im = im.crop((0, 0, im.width, row * 132))
    im.save(out); print(out, im.size)

if __name__ == "__main__":
    a = sys.argv[1:]
    if a[:1] == ["write"]: print(write(ROOT), "drawings written")
    elif a[:1] == ["sheet"] and len(a) >= 2: sheet(a[1], a[2:])
    else: sys.exit(__doc__)
