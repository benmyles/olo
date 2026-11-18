#!/usr/bin/python3
"""Forms to draw Olo's icons with: the pieces the classic set's drawings are made of.

A drawing (docs/ICONS.md, "Drawings") is an SVG written by hand, from
looking at the thing. Most are made of the same few forms on the one
projection (a box, a sheet standing, a flat thing seen from above, a
thick disc standing, a drum), so the forms are here, each giving the SVG
for itself: its silhouette with its outline (class `o`), its faces
clipped to it, its highlights (`from48`). What's particular to an icon
is written beside them, by hand.

    import sys; sys.path.insert(0, "tools/dev")
    from iconforms import *
    sd, se = shadow(32, 60, 26)                       # the contact shadow: (defs, body)
    d, b = box("crate", 27, 34, 30, 22, 20, "#b97a34", "#f4c98a", "#dda258")
    write("crate", "Crate: a wooden crate.", sd + d, se + b)   # data/icons/svg/crate.svg

then look at it at every size (`icontrace.py review OUT.png crate`) and
run `cargo test -p olo-xdg svg`. The drawing is what's kept, and is
edited by hand from then on: these only start one.

The planes (the `transform` of a group whose content is drawn flat):
FRONT(x, y), the front face, rising 1 in 4; SIDE(x, y), the left side;
TOP(x, y), a level plane on a box's top; PLATE(x, y), a flat thing seen
from above (a keyboard, a drive, a sheet lying).
"""
import os, math, re, subprocess
REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = REPO + "/data/icons/svg"
STYLE = '<style>.o{paint-order:stroke;stroke:#1b1b1f;stroke-width:1.2;stroke-linejoin:round}.h{fill:none;stroke:#fff;stroke-linecap:round}</style>'
INK = "#1b1b1f"
def f(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s
def write(name, about, defs, body, out=OUT):
    open(os.path.join(out, name + ".svg"), "w").write(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n<!-- {about} -->\n{STYLE}\n<defs>\n{defs.strip()}\n</defs>\n{body.strip()}\n</svg>\n')
def shadow(cx, cy, rx, op=.32):
    return (f'<radialGradient id="shadow" cx="{f(cx)}" cy="{f(cy)}" r="{f(rx)}" gradientUnits="userSpaceOnUse" gradientTransform="translate(0 {f(cy*0.87)}) scale(1 .13)"><stop offset="0" stop-opacity="{op}"/><stop offset="1" stop-opacity="0"/></radialGradient>\n',
            f'<ellipse class="from48" cx="{f(cx)}" cy="{f(cy)}" rx="{f(rx)}" ry="3.4" fill="url(#shadow)"/>\n')
def lin(id, x1, y1, x2, y2, *stops, units=True):
    n = len(stops)
    ss = "".join(f'<stop offset="{f(i/(n-1))}" stop-color="{c}"/>' if isinstance(c, str) else f'<stop offset="{f(c[0])}" stop-color="{c[1]}"/>' for i, c in enumerate(stops))
    u = ' gradientUnits="userSpaceOnUse"' if units else ''
    return f'<linearGradient id="{id}" x1="{f(x1)}" y1="{f(y1)}" x2="{f(x2)}" y2="{f(y2)}"{u}>{ss}</linearGradient>\n'
def rad(id, cx, cy, r, *stops):
    n = len(stops)
    ss = "".join(f'<stop offset="{f(i/(n-1))}" stop-color="{c}"/>' for i, c in enumerate(stops))
    return f'<radialGradient id="{id}" cx="{f(cx)}" cy="{f(cy)}" r="{f(r)}" gradientUnits="userSpaceOnUse">{ss}</radialGradient>\n'
pt = lambda r, a: (math.cos(math.radians(a)) * r, math.sin(math.radians(a)) * r)   # a point at radius r, angle a
def poly(ps):
    return "M" + "L".join(f"{f(x)} {f(y)}" for x, y in ps) + "Z"
def rounded(pts, r):
    n = len(pts); out = []
    for i, (x, y) in enumerate(pts):
        def toward(ax, ay):
            L = math.hypot(ax - x, ay - y); k = min(r, L / 2) / L
            return x + (ax - x) * k, y + (ay - y) * k
        a = toward(*pts[i - 1]); b = toward(*pts[(i + 1) % n])
        out.append(("L" if i else "M") + f"{f(a[0])} {f(a[1])}Q{f(x)} {f(y)} {f(b[0])} {f(b[1])}")
    return "".join(out) + "Z"
def extrude(d, pts):
    out = subprocess.run(["/usr/bin/python3", REPO + "/tools/dev/icontrace.py", "extrude", f"{d[0]},{d[1]}"] + [f"{x:.3f},{y:.3f}" for x, y in pts], capture_output=True, text=True).stdout
    got = dict(l.split(":", 1) for l in out.strip().splitlines())
    return got["whole"].strip(), got["top"].strip(), got["side"].strip()
FRONT = lambda x, y: f'matrix(1 -.25 0 1 {f(x)} {f(y)})'      # the front plane
SIDE = lambda x, y: f'matrix(1 .5 0 1 {f(x)} {f(y)})'          # the left side's plane
TOP = lambda x, y: f'matrix(1 -.25 -.6 -.3 {f(x)} {f(y)})'     # a level plane: u along the front, v going back
def top_pt(x0, y0, u, v):
    return x0 + u - .6 * v, y0 - .25 * u - .3 * v
def box(id, x0, y0, W, H, D, side, top, front, base=None, r=1.5, inner="", ontop="", hl=.5, cls="o"):
    """A box: (x0, y0) its front face's top left corner; W, H the front; D the side's width on screen."""
    E = (x0 - D, y0 - D / 2); F = (x0 + W - D, y0 - W / 4 - D / 2); B = (x0 + W, y0 - W / 4)
    C = (x0 + W, y0 - W / 4 + H); Dd = (x0, y0 + H); G = (x0 - D, y0 + H - D / 2)
    sil = rounded([E, F, B, C, Dd, G], r)
    defs = f'<clipPath id="{id}"><path d="{sil}"/></clipPath>\n'
    lights = ""
    if hl:
        lights = (f'<path class="h from48" stroke-opacity="{hl}" stroke-width=".55" d="M{f(x0+.6)} {f(y0+.1)}L{f(B[0]-.8)} {f(B[1]+.45)}M{f(x0+.3)} {f(y0+1)}V{f(y0+H-1)}"/>\n'
                  f'<path class="h from48" stroke-opacity="{f(hl*.6)}" stroke-width=".5" d="M{f(E[0]+1.6)} {f(E[1]+.9)}L{f(x0-.6)} {f(y0-.1)}"/>\n')
    body = (f'<path class="{cls}" fill="{base or side}" d="{sil}"/>\n<g clip-path="url(#{id})">\n'
            f'<path fill="{side}" d="{poly([(E[0]-3, E[1]-1.5), (x0, y0), (x0, y0+H+3), (E[0]-3, y0+H+3)])}"/>\n'
            f'<path fill="{top}" d="{poly([(E[0]-3, E[1]-1.5), (x0, y0), (B[0]+3, B[1]-.75), (B[0]+3, F[1]-6), (E[0]-3, F[1]-6)])}"/>\n'
            f'<g transform="{FRONT(x0, y0)}"><path fill="{front}" d="M0 0H{f(W+1)}V{f(H+1)}H0Z"/>{inner}</g>\n'
            f'{ontop}{lights}</g>\n')
    return defs, body
def gear(R, r, teeth, top=11.0, base=17.0, sy=1.0):
    pts = []
    for k in range(teeth):
        a = 360.0 / teeth * k
        for da, rr in ((-base, r), (-top, R), (top, R), (base, r)):
            t = math.radians(a + da)
            pts.append((rr * math.cos(t), rr * math.sin(t) * sy))
    return pts
def thick(points, w, head=None):
    """A polyline given width (mitred); head=(half width, length) ends it in an arrow head."""
    n = len(points); L = []; R = []
    def unit(a, b):
        dx, dy = b[0]-a[0], b[1]-a[1]; l = math.hypot(dx, dy); return dx/l, dy/l
    for i, p in enumerate(points):
        if i == 0: d = unit(points[0], points[1]); nx, ny = -d[1], d[0]; k = 1
        elif i == n-1: d = unit(points[-2], points[-1]); nx, ny = -d[1], d[0]; k = 1
        else:
            d1 = unit(points[i-1], p); d2 = unit(p, points[i+1])
            nx, ny = -(d1[1]+d2[1]), d1[0]+d2[0]; l = math.hypot(nx, ny); nx, ny = nx/l, ny/l
            k = 1 / max(.35, nx * -d1[1] + ny * d1[0])
        L.append((p[0]+nx*w/2*k, p[1]+ny*w/2*k)); R.append((p[0]-nx*w/2*k, p[1]-ny*w/2*k))
    mid = []
    if head:
        hw, hl = head; d = unit(points[-2], points[-1]); nx, ny = -d[1], d[0]; p = points[-1]
        mid = [(p[0]+nx*hw, p[1]+ny*hw), (p[0]+d[0]*hl, p[1]+d[1]*hl), (p[0]-nx*hw, p[1]-ny*hw)]
    return L + mid + R[::-1]
def arc_pts(cx, cy, r, a0, a1, step=12):
    n = max(2, int(abs(a1 - a0) / step) + 1)
    return [(cx + r * math.cos(math.radians(a0 + (a1-a0)*i/(n-1))), cy + r * math.sin(math.radians(a0 + (a1-a0)*i/(n-1)))) for i in range(n)]
def arc(r, a0, a1, cx, cy):
    x0, y0 = cx + r * math.cos(math.radians(a0)), cy + r * math.sin(math.radians(a0))
    x1, y1 = cx + r * math.cos(math.radians(a1)), cy + r * math.sin(math.radians(a1))
    return f"M{f(x0)} {f(y0)}A{r} {r} 0 {1 if abs(a1-a0) > 180 else 0} {1 if a1 > a0 else 0} {f(x1)} {f(y1)}"
def badge(R, cx, cy, face, side, depth=(-5.0, -3.75), hl=.55):
    """A thick disc standing in the front plane; leaves its group open for what's on its face."""
    px, py = 0.6 * R, -0.8 * R; dx, dy = depth
    path = f"M{f(px)} {f(py)}A{R} {R} 0 0 1 {f(-px)} {f(-py)}L{f(-px + dx)} {f(-py + dy)}A{R} {R} 0 0 1 {f(px + dx)} {f(py + dy)}Z"
    return (f'<g transform="{FRONT(cx, cy)}">\n<path class="o" fill="{side}" d="{path}"/>\n<circle cx="0" cy="0" r="{R}" fill="{face}"/>\n'
            f'<path class="h from48" stroke-opacity="{hl}" stroke-width=".55" d="M{f(-R * 0.94)} {f(R * 0.2)}A{f(R - 0.6)} {f(R - 0.6)} 0 0 1 {f(R * 0.25)} {f(-R * 0.93)}"/>\n')
STEEL = ("#f6f7f9", "#c3c6cf", "#8b8f9b")
GOLD = ("#ffe66e", "#fcc522", "#e99a08")
GREEN = ("#7beb70", "#3fc640", "#1d9a29")
BLUE = ("#62b1ff", "#2380f4", "#1259d0")
RED = ("#ff6a5c", "#f03a30", "#c81e1e")
def crt(glass_stops, screen):
    """The CRT monitor Terminal and Desktop are, with another screen: (defs, body)."""
    s = open(OUT + "/desktop.svg").read()
    defs = s[s.index("<defs>") + 6:s.index("</defs>")]
    g0 = defs.index('<linearGradient id="glass"'); g1 = defs.index("</linearGradient>", g0) + len("</linearGradient>")
    defs = defs[:g0] + lin("glass", 5, 6, 32, 30, *glass_stops).strip() + defs[g1:]
    body = s[s.index("</defs>") + 7:s.rindex("</svg>")]
    a = body.index('<rect x="4.8" y="6.5"'); a = body.index("\n", a) + 1
    b = body.index("<!-- The power light. -->")
    return defs, body[:a] + screen.strip() + "\n" + body[b:]
PAGE_DEFS = (lin("paper", 2, 2, 34, 47, "#ffffff", (.55, "#f4f5f9"), "#dfe2ec") + lin("fold", 26, 10, 32.5, 3.5, "#f7f8fb", "#c4c8d4"))
def page(x=14, y=13.5, on="", h=47):
    """The page every document kind is: 36 by 47 in the front plane, its corner folded."""
    return (f'<g transform="{FRONT(x, y)}">\n<path class="o" fill="url(#paper)" d="M1.2 0H26L36 10V{f(h-1.2)}Q36 {f(h)} 34.8 {f(h)}H1.2Q0 {f(h)} 0 {f(h-1.2)}V1.2Q0 0 1.2 0Z"/>\n'
            '<path class="from48" fill="#1b1b1f" fill-opacity=".14" d="M26 10.6H36V13.2L27 11.6Z"/>\n<path fill="url(#fold)" d="M26 0L36 10H27.2Q26 10 26 8.8Z"/>\n'
            '<path class="from32" fill="none" stroke="#1b1b1f" stroke-opacity=".55" stroke-width=".5" stroke-linejoin="round" d="M26 0V8.8Q26 10 27.2 10H36"/>\n' + on + '</g>\n')
def magnifier_defs(n="1", grip0="#4f9bff", grip1="#1552c4"):
    return (lin(f"ring{n}", 0, 0, 1, 1, "#fafbfc", "#c3c6cf", "#8b8f9b", units=False) + lin(f"lens{n}", 0, 0, 1, 1, "#d9f0ff", "#8fcbff", "#4f9df0", units=False)
            + lin(f"grip{n}", 0, -3.4, 0, 3.4, grip0, grip1))
def magnifier(cx, cy, r, angle, n="1", grip=13.6):
    """A magnifying glass: its ring and lens at (cx, cy), its handle going off along `angle`."""
    return f'''<g transform="translate({cx} {cy}) rotate({angle})">
<rect class="o" x="{f(r - 0.6)}" y="-1.7" width="6.4" height="3.4" fill="#b9bcc6"/>
<rect class="o" x="{f(r + 4.4)}" y="-3.4" width="{grip}" height="6.8" rx="3.2" fill="url(#grip{n})"/>
<path class="h from48" stroke-opacity=".55" stroke-width=".6" d="M{f(r + 6.5)} -2H{f(r + grip + 2)}"/>
</g>
<circle class="o" cx="{cx}" cy="{cy}" r="{r}" fill="url(#ring{n})"/>
<circle cx="{cx}" cy="{cy}" r="{f(r - 2.6)}" fill="#1b1b1f" fill-opacity=".75"/>
<circle cx="{cx}" cy="{cy}" r="{f(r - 3.1)}" fill="url(#lens{n})"/>
<path class="from48" fill="#fff" fill-opacity=".65" d="M{f(cx - (r - 4.6) * 0.82)} {f(cy - (r - 4.6) * 0.3)}A{f(r - 4.6)} {f(r - 4.6)} 0 0 1 {f(cx - (r - 4.6) * 0.1)} {f(cy - (r - 4.6) * 0.92)}A{f(r - 3)} {f(r - 3)} 0 0 0 {f(cx - (r - 4.6) * 0.82)} {f(cy - (r - 4.6) * 0.3)}Z"/>
'''
def wrench(transform, fill="url(#steel)", a=-22, b=20):
    """A spanner along its own x axis: a ring at one end, an open jaw at the other."""
    ring = f"M{a-9.5} 0A6.5 6.5 0 1 0 {a+3.5} 0A6.5 6.5 0 1 0 {a-9.5} 0ZM{a-5.8} 0A2.8 2.8 0 1 1 {a-.2} 0A2.8 2.8 0 1 1 {a-5.8} 0Z"
    shaft = f"M{a} -2.8H{b}V2.8H{a}Z"
    jaw = f"M{b+10.9} -3.2A7.6 7.6 0 1 0 {b+10.9} 3.2H{b+6}L{b+3.6} 0L{b+6} -3.2Z"
    shapes = f'<path fill-rule="evenodd" d="{ring}"/><path d="{shaft}"/><path d="{jaw}"/>'
    return (f'<g transform="{transform}"><g class="o" fill="{fill}">{shapes}</g><g fill="{fill}">{shapes}</g>'
            f'<path class="h from48" stroke-opacity=".7" stroke-width=".6" d="M{a+2} -1.4H{b-2}"/></g>\n')
def sector(R, a0, a1, r=0):
    p = lambda rr, a: f"{f(rr*math.cos(math.radians(a)))} {f(rr*math.sin(math.radians(a)))}"
    big = 1 if (a1 - a0) % 360 > 180 else 0
    if r: return f"M{p(r, a0)}L{p(R, a0)}A{R} {R} 0 {big} 1 {p(R, a1)}L{p(r, a1)}A{r} {r} 0 {big} 0 {p(r, a0)}Z"
    return f"M0 0L{p(R, a0)}A{R} {R} 0 {big} 1 {p(R, a1)}Z"
PLATE = lambda x, y: f'matrix(1 -.25 .5 1 {f(x)} {f(y)})'    # a plate tipped up toward us
def plate(id, x0, y0, W, L, T, face, front, side, r=2, on="", hl=.8, under=""):
    """A flat thing lying, seen from above (the tipped plate): W across, L deep, T thick: (defs, body, point-on-it)."""
    TL = (x0, y0); TR = (x0 + W, y0 - W / 4); BL = (x0 + L / 2, y0 + L); BR = (x0 + W + L / 2, y0 - W / 4 + L)
    sil = rounded([TL, TR, BR, (BR[0], BR[1] + T), (BL[0], BL[1] + T), (TL[0], TL[1] + T)], r)
    defs = f'<clipPath id="{id}"><path d="{sil}"/></clipPath>\n'
    light = f'<path class="h from48" stroke-opacity="{hl}" stroke-width=".45" d="M.8 {f(L-r)}V{f(r)}Q.8 .8 {f(r)} .8H{f(W-r)}"/>' if hl else ""
    body = (f'{under}<path class="o" fill="{front}" d="{sil}"/>\n<g clip-path="url(#{id})"><path fill="{side}" d="{poly([(TL[0]-2, TL[1]-4), (BL[0], BL[1]), (BL[0], BL[1]+T+2), (TL[0]-2, TL[1]+T+2)])}"/></g>\n'
            f'<g transform="{PLATE(x0, y0)}"><rect x="0" y="0" width="{f(W)}" height="{f(L)}" rx="{r}" fill="{face}"/>{on}{light}</g>\n')
    return defs, body, (lambda px, py: (x0 + px + py / 2, y0 - px / 4 + py))
def chip(x0, y0, W, L, board=("url(#pcb)", "#1f7d48", "#125c33"), lid=("url(#lidg)", "#8d919c"), T=3.4, inset=5, lidh=1.8, npf=9, nps=8, pw=1.9):
    """A chip lying, seen from above: a board, gold legs along its near edges, a raised steel lid."""
    BL = (x0 + L / 2, y0 + L)
    leg = lambda x, y, w, h: f'<rect class="o" x="{f(x)}" y="{f(y)}" width="{f(w)}" height="{f(h)}" fill="#f7c531"/>'
    legs = (f'<g transform="{PLATE(x0, y0 + T - .3)}">'
            + "".join(leg(-2.7, 2.6 + i * (L - 5.2 - pw) / (nps - 1), 3.4, pw) for i in range(nps))
            + "".join(leg(2.6 + i * (W - 5.2 - pw) / (npf - 1), L - .7, pw, 3.4) for i in range(npf)) + '</g>\n')
    w, l = W - 2 * inset, L - 2 * inset
    on = (f'<rect x="{inset}" y="{inset}" width="{f(w)}" height="{f(l)}" rx="1.6" fill="{lid[1]}" stroke="{INK}" stroke-width=".7"/>')
    d, b, at = plate("chip", x0, y0, W, L, T, board[0], board[1], board[2], r=1.4, on=on, under=legs)
    top = (f'<g transform="{PLATE(x0, y0 - lidh)}"><rect x="{inset}" y="{inset}" width="{f(w)}" height="{f(l)}" rx="1.6" fill="{lid[0]}" stroke="{INK}" stroke-width=".5"/>'
           f'<path class="h from48" stroke-opacity=".9" stroke-width=".45" d="M{inset+.9} {f(inset+l-1.6)}V{inset+2}Q{inset+.9} {inset+.9} {inset+2} {inset+.9}H{f(inset+w-1.6)}"/></g>\n')
    defs = d + lin("pcb", 0, 0, W, L, "#58c684", "#2f9e5c") + lin("lidg", inset, inset, inset + w, inset + l, "#fbfbfd", "#d5d8df", "#aeb2bc")
    c = at(W / 2, L / 2)
    return defs, b + top, (c[0], c[1] - lidh)
def drive(on, x0=4.5, y0=14.5, W=37, L=36, T=7):
    """A hard drive lying, seen from above: (defs, body, point-on-it)."""
    screws = "".join(f'<circle cx="{x}" cy="{y}" r="1" fill="{INK}"/><circle class="from48" cx="{x}" cy="{y}" r=".45" fill="#9a9ea9"/>' for x, y in ((2.8, 2.8), (W-2.8, 2.8), (W-2.8, L-2.8), (2.8, L-2.8)))
    d, b, at = plate("drive", x0, y0, W, L, T, "url(#dtop)", "url(#dfront)", "#1d1e23", r=2, on=on + f'<g class="from32">{screws}</g>')
    BL = at(0, L)
    led = (f'<g transform="{FRONT(BL[0], BL[1])}"><rect class="from48" x="10" y="2" width="16" height="3.2" rx=".6" fill="#17181c"/><circle cx="4.6" cy="3.6" r="1.7" fill="{INK}"/><circle cx="4.6" cy="3.6" r="1.15" fill="#46d84a"/></g>\n')
    defs = d + lin("dtop", 0, 0, W, L, "#fbfbfd", "#d5d8df", "#b4b7c1") + lin("dfront", x0, y0 + L, x0, y0 + L + T, "#4b4e58", "#2a2c33") + lin("platter", -13, -13, 13, 13, "#fbfbfd", "#c3c6cf", "#eceef2")
    return defs, b + led, at
def platter(cx, cy, r=14):
    return (f'<g transform="translate({cx} {cy})"><circle r="{r+.9}" fill="{INK}"/><circle r="{r}" fill="url(#platter)"/><circle r="4.8" fill="{INK}"/><circle r="4" fill="#d5d8df"/>'
            f'<circle class="from48" r="1.6" fill="#9a9ea9"/><path class="from48" fill="#fff" fill-opacity=".6" d="{sector(r-.8, 200, 232, 5.6)}{sector(r-.8, 20, 52, 5.6)}"/></g>')
def slab(pts, x0, y0, T, r, top, side, steps=4, holes=(), hr=1.0):
    """A flat shape lying, seen from above, T thick: its outline (and holes through it) in plate coordinates -> its body."""
    m = lambda ps: [(x0 + px + py / 2, y0 - px / 4 + py) for px, py in ps]
    scr = m(pts); hs = [m(h) for h in holes]
    d = lambda dy: rounded([(x, y + dy) for x, y in scr], r) + "".join(rounded([(x, y + dy) for x, y in h], hr) for h in hs)
    ds = [d(T * k / steps) for k in range(steps, 0, -1)]
    rule = ' fill-rule="evenodd"' if holes else ''
    out = "".join(f'<path class="o"{rule} fill="{side}" d="{x}"/>\n' for x in ds) + "".join(f'<path{rule} fill="{side}" d="{x}"/>\n' for x in ds)
    return out + f'<path class="o"{rule} fill="{top}" d="{d(0)}"/>\n'
def turn(pts, deg, dx=0, dy=0):
    c, s_ = math.cos(math.radians(deg)), math.sin(math.radians(deg))
    return [(x * c - y * s_ + dx, x * s_ + y * c + dy) for x, y in pts]
CARD = "M3 0H43Q46 0 46 3V27Q46 30 43 30H3Q0 30 0 27V3Q0 0 3 0Z"
def card(x, y, face, edge, on):
    """A card standing in the front plane, 46 by 30, its edge showing to the left."""
    return (f'<g transform="{FRONT(x, y)}">\n<path class="o" fill="{edge}" d="{CARD}" transform="translate(-2.6 -1.95)"/>\n<path class="o" fill="{face}" d="{CARD}"/>\n{on}'
            f'<path class="h from48" stroke-opacity=".6" stroke-width=".5" d="M.9 27V3Q.9 .9 3 .9H43"/>\n</g>\n')
SHEET_SHADOW = ('<linearGradient id="shadow" x1="30" y1="54" x2="31" y2="58.4" gradientUnits="userSpaceOnUse"><stop offset="0" stop-opacity=".2"/><stop offset="1" stop-opacity="0"/></linearGradient>\n',
                '<path class="from48" fill="url(#shadow)" d="M9 55.5L57 43.5L60 46.5L13 60Z"/>\n')

# ---- A padlock (Security, Authentication, the secure note). ----
def padlock(id, x0, y0, W, H, D, cols, hole=True, leg=9.4):
    """A padlock: a box, a steel shackle through its top: (defs, body). cols: top, front (2), side (2), base."""
    cx = W / 2
    k = W / 28
    inner = (f'<circle cx="{f(cx)}" cy="{f(H*.4)}" r="{f(3.4*k)}" fill="{INK}"/><path fill="{INK}" d="M{f(cx-1.5*k)} {f(H*.4+1.4*k)}L{f(cx-2.6*k)} {f(H*.4+8.6*k)}H{f(cx+2.6*k)}L{f(cx+1.5*k)} {f(H*.4+1.4*k)}Z"/>') if hole else ""
    d, b = box(id, x0, y0, W, H, D, f"url(#{id}s)", f"url(#{id}t)", f"url(#{id}f)", base=cols[5], r=1.8, inner=inner)
    defs = d + lin(f"{id}t", x0 - D, y0 - D / 2 - W / 4, x0 + W, y0, cols[0], cols[1]) + lin(f"{id}f", 0, 0, W, H, cols[1], cols[2]) + lin(f"{id}s", x0 - D, y0 - D / 2, x0, y0 + H, cols[3], cols[4]) + lin(f"{id}k", cx - 11 * k, -19 * k, cx + 11 * k, 0, *STEEL)
    ro, ri = 10.6 * k, 5.4 * k
    sh = (f'<g transform="{FRONT(x0 - D / 2, y0 - D / 4)}">\n<path class="o" fill="url(#{id}k)" d="M{f(cx-ro)} 0V{f(-leg*k)}A{f(ro)} {f(ro)} 0 0 1 {f(cx+ro)} {f(-leg*k)}V0H{f(cx+ri)}V{f(-leg*k)}A{f(ri)} {f(ri)} 0 0 0 {f(cx-ri)} {f(-leg*k)}V0Z"/>\n'
          f'<path class="h from48" stroke-opacity=".8" stroke-width=".6" d="M{f(cx-ro+1.4)} -2V{f(-leg*k)}A{f(ro-1.4)} {f(ro-1.4)} 0 0 1 {f(cx)} {f(-leg*k-ro+1.4)}"/>\n</g>\n')
    return defs, b + sh
BRASS = ("#ffe98a", "#fbc93a", "#e9a116", "#e3a11a", "#b97a07", "#b97a07")
IRON = ("#fbfbfd", "#d5d8df", "#a3a7b1", "#9a9ea9", "#6d7280", "#6d7280")


# ---- The sky (Weather and its skies): one sun, moon, star, cloud, drop, flake, pellet, bolt, bar of fog. ----
def tpath(d, fx, fy):
    out = []
    for cmd, args in re.findall(r"([MLCQZH])([^MLCQZH]*)", d):
        n = [float(v) for v in args.replace(",", " ").split()]
        out.append(cmd + " ".join(f"{f(fx(n[i]))} {f(fy(n[i+1]))}" for i in range(0, len(n), 2)))
    return "".join(out)
TONES = {"white": ("#ffffff", "#f1f3f8", "#c9d0de"), "light": ("#f8f9fb", "#dadee7", "#a4abba"), "grey": ("#e6e9f0", "#b4b9c6", "#7a8194"), "dark": ("#9096a6", "#575c6c", "#2e313c")}
CLOUD = "M9.6 26Q1 26 1 18.4Q1 12 7.4 10.8Q8.4 5 14 4.2Q16.6 4 18.4 5.2Q21.6 0 28 0.2Q35.4 0.8 36.8 8.2Q43.6 9 43.6 16.6Q43.6 26 34.8 26Z"
class Sky:
    def __init__(self): self.defs = ""; self.body = ""; self.n = 0
    def sun(self, cx, cy, r):
        self.defs += rad("sun", cx - r * .35, cy - r * .4, r * 1.5, "#fff6b0", "#ffd21a", "#f2a000") + lin("ray", cx - r * 1.7, cy - r * 1.7, cx + r * 1.7, cy + r * 1.7, "#ffe98a", "#f7b21a")
        rays = ""
        for k in range(8):
            a = 45 * k - 90
            b0 = pt(r * .9, a - 17); b1 = pt(r * .9, a + 17); tip = pt(r * 1.74, a); mid = pt(r * .9, a)
            P = lambda p: f"{f(cx + p[0])} {f(cy + p[1])}"
            rays += f'<path class="o" fill="url(#ray)" d="M{P(b0)}L{P(tip)}L{P(b1)}Z"/><path class="from32" fill="#d98a06" fill-opacity=".55" d="M{P(mid)}L{P(tip)}L{P(b1)}Z"/>'
        self.body += (f'{rays}\n<circle class="o" cx="{cx}" cy="{cy}" r="{r}" fill="url(#sun)"/>\n'
                      f'<path class="from48" fill="#fff" fill-opacity=".6" d="M{f(cx-r*.74)} {f(cy-r*.2)}A{f(r*.8)} {f(r*.8)} 0 0 1 {f(cx-r*.1)} {f(cy-r*.78)}A{f(r)} {f(r)} 0 0 0 {f(cx-r*.74)} {f(cy-r*.2)}Z"/>\n')
    def moon(self, cx, cy, r):
        # The crescent: a disc less another, up and to the right of it.
        ox, oy, r2 = .46 * r, -.4 * r, .86 * r
        d = math.hypot(ox, oy); a = (r * r - r2 * r2 + d * d) / (2 * d); h = math.sqrt(r * r - a * a)
        mx, my = a * ox / d, a * oy / d
        p1 = (mx - h * oy / d, my + h * ox / d); p2 = (mx + h * oy / d, my - h * ox / d)
        P = lambda p: f"{f(cx + p[0])} {f(cy + p[1])}"
        self.defs += lin("moon", cx - r, cy - r, cx + r * .6, cy + r, "#fff3a0", "#fcc522", "#e99a08")
        self.body += (f'<path class="o" fill="url(#moon)" d="M{P(p2)}A{f(r)} {f(r)} 0 1 0 {P(p1)}A{f(r2)} {f(r2)} 0 0 1 {P(p2)}Z"/>\n'
                      f'<path class="h from48" stroke="#fffbe0" stroke-opacity=".9" stroke-width=".8" d="M{f(cx-r*.86)} {f(cy+r*.2)}A{f(r*.9)} {f(r*.9)} 0 0 1 {f(cx-r*.2)} {f(cy-r*.86)}"/>\n')
    def star(self, cx, cy, r):
        if "starg" not in self.defs: self.defs += lin("starg", 0, 0, 1, 1, "#fff3a0", "#f7b21a", units=False)
        ps = [pt(r if k % 2 == 0 else r * .46, -90 + 36 * k) for k in range(10)]
        self.body += f'<path class="o" fill="url(#starg)" d="{poly([(cx + x, cy + y) for x, y in ps])}"/>\n'
    def cloud(self, x, y, k, tone):
        self.n += 1; id = f"cloud{self.n}"; t = TONES[tone]
        fx = lambda v: x + k * v; fy = lambda v: y + k * v
        self.defs += lin(id, x, y, x + 6 * k, y + 26 * k, t[0], t[1], t[2])
        self.body += (f'<path class="o" fill="url(#{id})" d="{tpath(CLOUD, fx, fy)}"/>\n'
                      f'<path class="from48" fill="{t[2]}" fill-opacity=".45" d="{tpath("M3 19Q11 23.6 22 23.2Q34 22.8 42.8 17.6Q44 26 34.8 26L9.6 26Q1.6 26 3 19Z", fx, fy)}"/>\n'
                      f'<path class="h from48" stroke-opacity=".9" stroke-width="{f(max(.5, .8*k))}" d="{tpath("M20.4 6.6Q23 2.6 27.6 2.4", fx, fy)}{tpath("M3.4 16.6Q3.6 13.6 7 12.6", fx, fy)}{tpath("M9.8 9.4Q10.6 6.6 13.6 6.2", fx, fy)}"/>\n')
    def drop(self, cx, cy, k=1.0, tilt=0, pale=False):
        id = "pale" if pale else "drop"
        if f'id="{id}"' not in self.defs: self.defs += lin(id, 0, 0, 1, 1, *(("#f1f8ff", "#9cc8f5") if pale else ("#8fd0ff", "#1f6fe8")), units=False)
        spike = f'<path class="o" fill="url(#{id})" d="M{f(-1.6*k)} {f(6.6*k)}L0 {f(12.6*k)}L{f(1.6*k)} {f(6.6*k)}Z"/>' if pale else ""
        self.body += (f'<g transform="translate({cx} {cy}) rotate({tilt})">{spike}<path class="o" fill="url(#{id})" d="M0 {f(-6.4*k)}Q{f(4.6*k)} {f(.2*k)} {f(4.6*k)} {f(3*k)}A{f(4.6*k)} {f(4.6*k)} 0 0 1 {f(-4.6*k)} {f(3*k)}Q{f(-4.6*k)} {f(.2*k)} 0 {f(-6.4*k)}Z"/>'
                      f'<path class="from48" fill="#fff" fill-opacity=".7" d="M{f(-2.6*k)} {f(3*k)}Q{f(-2.6*k)} {f(.6*k)} {f(-.8*k)} {f(-2.2*k)}Q{f(-1.4*k)} {f(1*k)} {f(-2.6*k)} {f(3*k)}Z"/></g>\n')
    def flake(self, cx, cy, R):
        arms = "".join(f"M{f(cx - pt(R, a)[0])} {f(cy - pt(R, a)[1])}L{f(cx + pt(R, a)[0])} {f(cy + pt(R, a)[1])}" for a in (30, 90, 150))
        w = max(1.3, R * .34)
        self.body += (f'<g fill="none" stroke-linecap="round"><path stroke="{INK}" stroke-width="{f(w + 1.5)}" d="{arms}"/><path stroke="#f4f8ff" stroke-width="{f(w)}" d="{arms}"/>'
                      f'<path class="from48" stroke="#a9b6cc" stroke-width="{f(w * .3)}" d="{arms}"/></g>\n')
    def pellet(self, cx, cy, r=2.3):
        self.body += f'<circle class="o" cx="{cx}" cy="{cy}" r="{r}" fill="#f4f7fc"/><path class="from48" fill="#a9b6cc" fill-opacity=".7" d="M{cx} {cy+r}A{r} {r} 0 0 0 {cx+r} {cy}A{f(r*1.3)} {f(r*1.3)} 0 0 1 {cx} {cy+r}Z"/>\n'
    def bolt(self, cx, cy, k=1.0):
        self.defs += lin("bolt", cx - 6 * k, cy, cx + 8 * k, cy + 24 * k, "#fff3a0", "#ffd21a", "#f2a000")
        p = [(6.4, 0), (-4.4, 12), (1, 12), (-3.4, 24), (9.6, 8.4), (3.8, 8.4), (8.6, 0)]
        self.body += (f'<path class="o" fill="url(#bolt)" d="{poly([(cx + x * k, cy + y * k) for x, y in p])}"/>\n'
                      f'<path class="from48" fill="#d98a06" fill-opacity=".5" d="{poly([(cx + x * k, cy + y * k) for x, y in ((1, 12), (-3.4, 24), (9.6, 8.4), (3.8, 8.4))])}"/>\n')
    def fog(self, bars):
        self.defs += lin("fog", 0, 0, 0, 1, "#f1f3f8", "#aeb4c2", units=False)
        for x, y, w in bars:
            self.body += f'<rect class="o" x="{x}" y="{y}" width="{w}" height="5.6" rx="2.8" fill="url(#fog)"/><path class="h from48" stroke-opacity=".9" stroke-width=".6" d="M{x+3} {y+1.5}H{x+w-3}"/>\n'
    def write(self, name, about, out=OUT): write(name, about, self.defs, self.body, out=out)


# ---- A phone seen from behind (the phones): its colour, its cameras. ----
PHONE = "M3.6 0H20.4Q24 0 24 3.6V40.4Q24 44 20.4 44H3.6Q0 44 0 40.4V3.6Q0 0 3.6 0Z"
def lens(x, y, r):
    return (f'<circle cx="{x}" cy="{y}" r="{f(r+.7)}" fill="{INK}"/><circle cx="{x}" cy="{y}" r="{r}" fill="#22242b"/><circle cx="{x}" cy="{y}" r="{f(r*.68)}" fill="url(#glass)"/>'
            f'<circle class="from48" cx="{f(x-r*.26)}" cy="{f(y-r*.3)}" r="{f(r*.2)}" fill="#fff" fill-opacity=".8"/>')
def pad(x, y, w, h, r, fill):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{INK}" fill-opacity=".8"/><rect x="{f(x+.5)}" y="{f(y+.5)}" width="{f(w-1)}" height="{f(h-1)}" rx="{f(r-.5)}" fill="{fill}"/>'
SILVER, DARK = "url(#silver)", "#25272d"
def cams(kind, tone):
    if kind == "pill2": return pad(2.8, 3, 8, 14.4, 4, tone) + lens(6.8, 7.2, 2.4) + lens(6.8, 13.2, 2.4)
    if kind == "pill3": return pad(2.8, 3, 8, 20.4, 4, tone) + lens(6.8, 7.2, 2.4) + lens(6.8, 13.2, 2.4) + lens(6.8, 19.2, 2.4)
    if kind == "ultra": return lens(6.8, 7, 2.9) + lens(6.8, 14, 2.9) + lens(6.8, 21, 2.9) + lens(13.2, 8.6, 1.5) + lens(13.2, 14.6, 1.5)
    if kind == "pillultra": return pad(2.8, 3, 8.4, 21.6, 4.2, tone) + lens(7, 7.4, 2.5) + lens(7, 13.8, 2.5) + lens(7, 20.2, 2.5) + lens(13.6, 8.6, 1.5) + lens(13.6, 14.6, 1.5)
    if kind == "square2": return pad(2.8, 3, 12.6, 12.6, 3.6, tone) + lens(6.8, 7, 2.5) + lens(11.4, 11.6, 2.5)
    if kind == "square3": return pad(2.8, 3, 13, 13, 3.6, tone) + lens(6.6, 6.6, 2.4) + lens(6.6, 12.4, 2.4) + lens(12, 9.5, 2.4)
    if kind == "single": return lens(7, 7.2, 3)
    if kind == "plateau": return pad(1.6, 2.2, 20.8, 12.8, 3, tone) + lens(6, 6, 2.3) + lens(6, 11.4, 2.3) + lens(10.8, 8.7, 2.3)
    if kind == "bar1": return pad(2.6, 3.6, 18.8, 7.6, 3.8, tone) + lens(6.6, 7.4, 2.4)
    if kind == "bar3": return pad(2, 5, 20, 8, 4, tone) + lens(7, 9, 2.2) + lens(12, 9, 2.2) + lens(17, 9, 2.2)
    if kind == "round": return f'<circle cx="12" cy="11" r="8.6" fill="{INK}" fill-opacity=".8"/><circle cx="12" cy="11" r="8" fill="{tone}"/>' + lens(8.8, 11, 2.6) + lens(15.2, 11, 2.6)
    if kind == "square4": return pad(4.4, 3.4, 15.2, 15.2, 4, tone) + lens(8.6, 7.6, 2.3) + lens(15.4, 7.6, 2.3) + lens(8.6, 14.4, 2.3) + lens(15.4, 14.4, 2.3)
def phone(name, model, colour, c, frame, kind, tone, out=OUT):
    """A phone's picture: `c` its back's three colours, `frame` its edge, `kind` its cameras (see `cams`), `tone` their plate (body, silver, dark)."""
    sd = ('<linearGradient id="shadow" x1="32" y1="55.6" x2="33" y2="60" gradientUnits="userSpaceOnUse"><stop offset="0" stop-opacity=".2"/><stop offset="1" stop-opacity="0"/></linearGradient>\n')
    tone = {"body": "url(#bump)", "silver": SILVER, "dark": DARK}[tone]
    write(name, f"{model}: the phone from behind, {colour}, its cameras as that model has them.",
        sd + lin("back", 0, 0, 24, 44, *c) + lin("bump", 2, 2, 22, 16, c[0], c[1]) + lin("silver", 0, 0, 1, 1, "#fbfbfd", "#b9bcc6", units=False) + rad("glass", -1, -1, 4, "#7f8fff", "#1b2a8a", "#070a22").replace('gradientUnits="userSpaceOnUse"', 'gradientUnits="objectBoundingBox"').replace('cx="-1" cy="-1" r="4"', 'cx=".35" cy=".3" r=".75"'),
        '<path class="from48" fill="url(#shadow)" d="M17 58.4L44 51.6L48 55L22 62Z"/>\n' + f'''<g transform="{FRONT(20, 13)}">
<path class="o" fill="{frame}" d="{PHONE}" transform="translate(-2.2 -1.65)"/>
<path class="o" fill="url(#back)" d="{PHONE}"/>
<path class="from48" fill="#fff" fill-opacity=".09" d="M14 .9H20.4Q23.1 .9 23.1 3.6V30L6 43.1H3.6Q2 43.1 1.4 42Z"/>
{cams(kind, tone)}
<path class="h from48" stroke-opacity=".5" stroke-width=".5" d="M.9 40V3.6Q.9 .9 3.6 .9H20"/>
</g>''', out=out)
