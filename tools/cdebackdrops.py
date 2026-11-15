#!/usr/bin/env python3
"""CDE's backdrops rendered as CDE showed them (dtwm + Motif + dtsession):

- a palette colour's shadows and select colour by Motif's
  CalculateColorsRGB (lib/Xm/Color.c: Brightness, the light/medium/dark
  branches, thresholds 93/20/70 % of 65535 in steps of 655), integer
  arithmetic truncating as C does;
- workspace n's colour set {3, 5, 6, 7}[n % 4] (dtwm WmResource.c
  DefaultWsColorSetId), backdrop fg = set.bg, bg = set.bs;
- an XBM: 1 bits the fg, 0 bits the bg; an XPM: its symbolic background,
  foreground, topShadowColor, bottomShadowColor and selectColor the set's
  own (Motif ImageCache.c CompleteUnspecColors finds the set holding fg
  and bg and uses all of it), other symbols (iconGrayN, iconColorN) the
  file's own colours.

    cdebackdrops.py PALETTE.dp BACKDROP OUT.png SET

PALETTE.dp and BACKDROP from CDE's source (programs/palettes,
programs/backdrops; LGPL). Then nearest neighbour 4× into
data/wallpapers/src and `upscalewallpaper.py --promote NAME --tile`
(docs/ICONS.md, "Theme wallpapers and the retro collection").
"""
import re, sys
from PIL import Image

MAX = 65535
PCT = MAX // 100
LITE, DARK, FG = 93 * PCT, 20 * PCT, 70 * PCT

def cdiv(a, b):
    q = abs(a) // abs(b)
    return q if (a >= 0) == (b > 0) else -q

def brightness(c):
    r, g, b = c
    intensity = (r + g + b) // 3
    lum = int(0.30 * r + 0.59 * g + 0.11 * b)
    light = (min(c) + max(c)) // 2
    return (intensity * 75 + light * 0 + lum * 25) // 100

def colors(bg):
    """(fg, select, top shadow, bottom shadow) for background bg (16-bit)."""
    b = brightness(bg)
    fg = (0, 0, 0) if b > FG else (MAX, MAX, MAX)
    down = lambda c, f: tuple(v - cdiv(v * f, 100) for v in c)
    up = lambda c, f: tuple(v + cdiv(f * (MAX - v), 100) for v in c)
    if b < DARK:
        return fg, up(bg, 15), up(bg, 50), up(bg, 30)
    if b > LITE:
        return fg, down(bg, 15), down(bg, 20), down(bg, 40)
    f = lambda lo, hi: lo + cdiv(b * (hi - lo), MAX)
    return fg, down(bg, f(15, 15)), up(bg, f(50, 60)), down(bg, f(60, 40))

def parse_color(s):
    s = s.strip()
    if s.startswith("#"):
        h = s[1:]
        n = len(h) // 3
        return tuple(int(h[i * n:(i + 1) * n], 16) * MAX // (16 ** n - 1) for i in range(3))
    return {"black": (0, 0, 0), "white": (MAX, MAX, MAX)}.get(s.lower())

def palette(path):
    sets = []
    for line in open(path):
        line = line.strip()
        if line.startswith("#"):
            bg = parse_color(line)
            fg, sc, ts, bs = colors(bg)
            sets.append(dict(bg=bg, fg=fg, sc=sc, ts=ts, bs=bs))
    return sets

def to8(c):
    return tuple(round(v * 255 / MAX) for v in c)

def xbm(path, fg, bg):
    t = open(path).read()
    w = int(re.search(r"_width\s+(\d+)", t).group(1))
    h = int(re.search(r"_height\s+(\d+)", t).group(1))
    data = [int(x, 16) for x in re.findall(r"0x[0-9a-fA-F]{2}", t.split("{", 1)[1])]
    stride = (w + 7) // 8
    im = Image.new("RGB", (w, h))
    px = im.load()
    for y in range(h):
        for x in range(w):
            bit = data[y * stride + x // 8] >> (x % 8) & 1
            px[x, y] = to8(fg if bit else bg)
    return im

def xpm(path, cs):
    strings = re.findall(r'"([^"]*)"', open(path).read())
    w, h, n, cpp = (int(v) for v in strings[0].split()[:4])
    sym = {"background": cs["bg"], "foreground": cs["fg"], "topShadowColor": cs["ts"], "bottomShadowColor": cs["bs"], "selectColor": cs["sc"]}
    table = {}
    for line in strings[1:1 + n]:
        key, rest = line[:cpp], line[cpp:].split()
        spec = dict(zip(rest[0::2], rest[1::2]))
        if spec.get("s") in sym:
            table[key] = sym[spec["s"]]
        elif spec.get("c", "").lower() == "none":
            table[key] = cs["bg"]
        else:
            table[key] = parse_color(spec.get("c", "black")) or (0, 0, 0)
    im = Image.new("RGB", (w, h))
    px = im.load()
    for y, row in enumerate(strings[1 + n:1 + n + h]):
        for x in range(w):
            px[x, y] = to8(table[row[x * cpp:(x + 1) * cpp]])
    return im

if __name__ == "__main__":
    sets = palette(sys.argv[1])
    cs = sets[int(sys.argv[4]) - 1]
    src = sys.argv[2]
    im = xbm(src, cs["bg"], cs["bs"]) if src.endswith(".bm") else xpm(src, cs)
    im.save(sys.argv[3])
    print(sys.argv[3], im.size)
