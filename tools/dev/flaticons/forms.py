# Flat, front-facing forms for Olo's icons, on the 64 grid: plain faces, no outlines, no gradients.
import math
BLUE, BLUE_D, BLUE_L, SKY = "#3b82f6", "#2563eb", "#93c5fd", "#60a5fa"
BLUE_P = "#dbeafe"
RED, RED_D, RED_L = "#ef4444", "#dc2626", "#fca5a5"
ORANGE, AMBER, YELLOW, YELLOW_L = "#f97316", "#f59e0b", "#fbbf24", "#fde68a"
GREEN, GREEN_D, GREEN_L, TEAL = "#22c55e", "#16a34a", "#86efac", "#14b8a6"
PURPLE, PURPLE_D, PINK = "#8b5cf6", "#7c3aed", "#ec4899"
G50, G100, G200, G300, G400, G500, G600, G700, G800 = "#f8fafc", "#f1f5f9", "#e2e8f0", "#cbd5e1", "#94a3b8", "#64748b", "#475569", "#334155", "#1e293b"
WHITE, PAPER, RIM = "#ffffff", "#f8fafc", "#c3ccd9"
BROWN, BROWN_D, BROWN_L, CREAM = "#b98155", "#94623d", "#d9a877", "#fdf1d8"

def _c(c): return ' class="%s"' % c if c else ""
def n(v):
    s = ("%.2f" % v).rstrip("0").rstrip(".")
    return s if s else "0"
def R(x, y, w, h, r=0, f=G500, c=None, extra=""):
    return '<rect%s x="%s" y="%s" width="%s" height="%s"%s fill="%s"%s/>' % (_c(c), n(x), n(y), n(w), n(h), (' rx="%s"' % n(r)) if r else "", f, extra)
def C(cx, cy, r, f, c=None, extra=""):
    return '<circle%s cx="%s" cy="%s" r="%s" fill="%s"%s/>' % (_c(c), n(cx), n(cy), n(r), f, extra)
def E(cx, cy, rx, ry, f, c=None):
    return '<ellipse%s cx="%s" cy="%s" rx="%s" ry="%s" fill="%s"/>' % (_c(c), n(cx), n(cy), n(rx), n(ry), f)
def P(d, f, c=None, extra=""):
    return '<path%s fill="%s" d="%s"%s/>' % (_c(c), f, d, extra)
def S(d, stroke, w, c=None, cap="round", join="round"):
    return '<path%s fill="none" stroke="%s" stroke-width="%s" stroke-linecap="%s" stroke-linejoin="%s" d="%s"/>' % (_c(c), stroke, n(w), cap, join, d)
def L(x1, y1, x2, y2, stroke, w, c=None, cap="round"):
    return S("M%s %sL%s %s" % (n(x1), n(y1), n(x2), n(y2)), stroke, w, c, cap)
def poly(pts, f, c=None):
    return P("M" + "L".join("%s %s" % (n(x), n(y)) for x, y in pts) + "Z", f, c)
def ring(cx, cy, r, w, stroke, c=None):
    return '<circle%s cx="%s" cy="%s" r="%s" fill="none" stroke="%s" stroke-width="%s"/>' % (_c(c), n(cx), n(cy), n(r), stroke, n(w))
def arc(cx, cy, r, a0, a1, stroke, w, c=None, cap="round"):
    "An arc from angle a0 to a1 (degrees, clockwise from 12 o'clock)."
    p = lambda a: (cx + r * math.sin(math.radians(a)), cy - r * math.cos(math.radians(a)))
    (x0, y0), (x1, y1) = p(a0), p(a1)
    large = 1 if (a1 - a0) % 360 > 180 else 0
    return S("M%s %sA%s %s 0 %d 1 %s %s" % (n(x0), n(y0), n(r), n(r), large, n(x1), n(y1)), stroke, w, c, cap)
def G(items, transform):
    return '<g transform="%s">%s</g>' % (transform, "".join(items))

def svg(title, items):
    return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n<!-- %s (Flat: seen straight on, plain faces). -->\n'
            '<style>.o{paint-order:stroke;stroke:none}</style>\n%s\n</svg>\n') % (title, "\n".join(items))

# --- forms ---------------------------------------------------------------
def page(x=13, y=5, w=38, h=54, fold=11, paper=PAPER):
    "A sheet with its corner folded, a rim so it shows on white."
    r = 3
    d = "M%s %sH%sL%s %sV%sQ%s %s %s %sH%sQ%s %s %s %sV%sQ%s %s %s %sZ" % (
        n(x + r), n(y), n(x + w - fold), n(x + w), n(y + fold), n(y + h - r), n(x + w), n(y + h), n(x + w - r), n(y + h),
        n(x + r), n(x), n(y + h), n(x), n(y + h - r), n(y + r), n(x), n(y), n(x + r), n(y))
    return [S(d, RIM, 2), P(d, paper), P("M%s %sV%sQ%s %s %s %sH%sZ" % (n(x + w - fold), n(y), n(y + fold - 2), n(x + w - fold), n(y + fold), n(x + w - fold + 2), n(y + fold), n(x + w)), G300)]
def lines(x, y, w, count, gap=7, color=G400, weight=3, c="from24", short_last=True):
    out = []
    for i in range(count):
        ww = w * (0.62 if short_last and i == count - 1 else 1)
        out.append(L(x, y + i * gap, x + ww, y + i * gap, color, weight, c))
    return out
def badge(cx, cy, r, color):
    "A round badge at an icon's corner, a rim of the ground's light around it."
    return [C(cx, cy, r + 2, WHITE), C(cx, cy, r, color)]
def check(cx, cy, s, color=WHITE, w=3.4):
    return S("M%s %sL%s %sL%s %s" % (n(cx - s), n(cy), n(cx - s * 0.3), n(cy + s * 0.7), n(cx + s), n(cy - s * 0.7)), color, w)
def plus(cx, cy, s, color=WHITE, w=3.4):
    return S("M%s %sH%sM%s %sV%s" % (n(cx - s), n(cy), n(cx + s), n(cx), n(cy - s), n(cy + s)), color, w)
def folder(back=AMBER, front=YELLOW, y=10):
    return [P("M6 %s Q6 %s 10 %s H24 Q26 %s 27.5 %s L30 %s Q31 %s 33 %s H54 Q58 %s 58 %s V50 Q58 54 54 54 H10 Q6 54 6 50Z" % (n(y + 4), n(y), n(y), n(y), n(y + 1.5), n(y + 4.5), n(y + 6), n(y + 6), n(y + 6), n(y + 10)), back),
            R(6, y + 12, 52, 44 - y, 4, front)]
def monitor(screen=BLUE, bezel=G700, stand=G400):
    return [R(27, 46, 10, 8, 0, stand), R(19, 53, 26, 4, 2, stand), R(5, 9, 54, 39, 4, bezel), R(8, 12, 48, 33, 2, screen)]
def gear(cx, cy, r, color, teeth=8, hole=None, c=None):
    out = []
    tw = r * 0.42
    for i in range(teeth):
        a = 360 / teeth * i
        out.append('<rect%s x="%s" y="%s" width="%s" height="%s" rx="%s" fill="%s" transform="rotate(%s %s %s)"/>' % (_c(c), n(cx - tw / 2), n(cy - r * 1.28), n(tw), n(r * 0.6), n(tw * 0.25), color, n(a), n(cx), n(cy)))
    out.append(C(cx, cy, r, color, c))
    if hole: out.append(C(cx, cy, r * 0.42, hole, c))
    return out
def cloud(cx, cy, s=1.0, f=G200, c=None):
    "A cloud about 36 wide and 20 high at s=1, its middle at cx, cy. A light one has a rim, to show on white."
    d = "M%s %s a%s %s 0 0 1 %s %s a%s %s 0 0 1 %s %s a%s %s 0 0 1 %s %s H%s a%s %s 0 0 1 %s %sZ" % (
        n(cx - 12 * s), n(cy + 10 * s), n(8 * s), n(8 * s), n(1.2 * s), n(-15.9 * s), n(11 * s), n(11 * s), n(20.5 * s), n(-3.4 * s),
        n(8.6 * s), n(8.6 * s), n(4.3 * s), n(19.3 * s), n(cx - 12 * s), 0, 0, 0, 0)
    light = f in (PAPER, WHITE, G100, G50)
    return ([S(d, RIM, 2, c)] if light else []) + [P(d, f, c)]
def sun(cx, cy, r, f=YELLOW, rays=AMBER, c=None):
    out = []
    for i in range(8):
        a = math.radians(45 * i)
        out.append(L(cx + math.sin(a) * r * 1.35, cy - math.cos(a) * r * 1.35, cx + math.sin(a) * r * 1.8, cy - math.cos(a) * r * 1.8, rays, max(2.4, r * 0.28), c))
    out.append(C(cx, cy, r, f))
    return out
def moon(cx, cy, r, f=YELLOW_L):
    return [P("M%s %s A%s %s 0 1 0 %s %s A%s %s 0 0 1 %s %sZ" % (n(cx + r * 0.25), n(cy - r), n(r), n(r), n(cx + r), n(cy + r * 0.35), n(r * 0.82), n(r * 0.82), n(cx + r * 0.25), n(cy - r)), f)]
def drops(xs, y, color=BLUE, h=7, c=None):
    return [L(x, y, x - 2.2, y + h, color, 3, c) for x in xs]
def flakes(xs, y, color=SKY, c=None):
    out = []
    for x in xs:
        out += [L(x - 3, y, x + 3, y, color, 2, c), L(x - 1.5, y - 2.6, x + 1.5, y + 2.6, color, 2, c), L(x + 1.5, y - 2.6, x - 1.5, y + 2.6, color, 2, c)]
    return out
def bolt(cx, cy, s=1.0, f=YELLOW):
    return [poly([(cx + 2 * s, cy - 9 * s), (cx - 5 * s, cy + 1 * s), (cx - 0.5 * s, cy + 1 * s), (cx - 2.5 * s, cy + 9 * s), (cx + 5 * s, cy - 2 * s), (cx + 0.5 * s, cy - 2 * s)], f)]
def padlock(cx, cy, s=1.0, body=AMBER, shackle=G400, hole=G800):
    w, h = 22 * s, 17 * s
    return [S("M%s %sV%sA%s %s 0 0 1 %s %sV%s" % (n(cx - 6.5 * s), n(cy - h / 2 + 1), n(cy - h / 2 - 5 * s), n(6.5 * s), n(6.5 * s), n(cx + 6.5 * s), n(cy - h / 2 - 5 * s), n(cy - h / 2 + 1)), shackle, 4 * s, cap="butt"),
            R(cx - w / 2, cy - h / 2, w, h, 3 * s, body), C(cx, cy - 1 * s, 2.6 * s, hole), R(cx - 1.1 * s, cy, 2.2 * s, 5 * s, 1, hole)]
def magnifier(cx, cy, r, glass=BLUE_P, rim=G600, handle=G600):
    a = r * 0.72
    return [L(cx + a, cy + a, cx + a + r * 0.95, cy + a + r * 0.95, handle, max(3.4, r * 0.42)), C(cx, cy, r, rim), C(cx, cy, r - max(2.2, r * 0.24), glass)]
def tile(color, r=13, x=6, y=6, s=52):
    return [R(x, y, s, s, r, color)]
def disc(color, r=26):
    return [C(32, 32, r, color)]
def arrow(x1, y1, x2, y2, color, w=4, head=6, c=None):
    a = math.atan2(y2 - y1, x2 - x1)
    hx = lambda d: (x2 + head * math.cos(a + d), y2 + head * math.sin(a + d))
    (ax, ay), (bx, by) = hx(math.radians(145)), hx(math.radians(-145))
    return [L(x1, y1, x2, y2, color, w, c), S("M%s %sL%s %sL%s %s" % (n(ax), n(ay), n(x2), n(y2), n(bx), n(by)), color, w, c)]
