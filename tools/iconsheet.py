#!/usr/bin/env python3
"""Contact sheet of the icon set, for reviewing consistency.

    iconsheet.py OUT.png [--size PX] [--theme ID] [names or candidate .png paths...]

Draws every icon (or the named ones) at PX device pixels (default 64)
on light, dark and night panel colours, in a grid with a 16 px reference
row (how they look in lists and the Deskbar). Arguments ending in .png are
raw 1024² images (e.g. data/icons/src/candidates/NAME-1.png), processed
exactly as tools/mkicons.py would, so candidates can be judged in place
next to the shipped set. Review it before committing
new icons: every icon should have the same weight, angle, outline and
lighting as its neighbours. --theme ID draws theme ID's set
(data/themes/ID/icons) instead.
"""
import os, sys
from PIL import Image, ImageDraw

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mkicons import square

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "icons")
# Panel backgrounds of the three themes (crates/theme).
BACKS = [(216, 216, 216), (43, 43, 46), (36, 30, 26)]

def load(name, size, root=ROOT):
    if name.endswith(".png"):
        return square(Image.open(name)).convert("RGBa").resize((size, size), Image.LANCZOS).convert("RGBA")
    p = os.path.join(root, str(size), name + ".png")
    return Image.open(p).convert("RGBA") if os.path.exists(p) else None

def main():
    args = sys.argv[1:]
    out = args.pop(0)
    px = 64
    if "--size" in args:
        i = args.index("--size")
        px = int(args[i + 1])
        del args[i:i + 2]
    root = ROOT
    if "--theme" in args:
        i = args.index("--theme")
        root = os.path.join(os.path.dirname(ROOT), "themes", args[i + 1], "icons")
        del args[i:i + 2]
    names = args or sorted(f[:-4] for f in os.listdir(os.path.join(root, str(px))) if f.endswith(".png"))
    cols = 10
    rows = (len(names) + cols - 1) // cols
    cell = px + px // 2
    band = rows * (cell + 24) + 8
    sheet = Image.new("RGBA", (cols * cell + 16, band * len(BACKS)), (0, 0, 0, 255))
    d = ImageDraw.Draw(sheet)
    for b, back in enumerate(BACKS):
        y0 = b * band
        d.rectangle((0, y0, sheet.width, y0 + band), fill=back + (255,))
        fg = (0, 0, 0) if b == 0 else (220, 220, 220)
        for n, name in enumerate(names):
            x = 8 + (n % cols) * cell + (cell - px) // 2
            y = y0 + 8 + (n // cols) * (cell + 24)
            for size, (ix, iy) in ((px, (x, y)), (16, (x + px - 16, y + px + 4))):
                im = load(name, size, root)
                if im:
                    sheet.alpha_composite(im, (ix, iy))
            label = os.path.basename(name)[:-4] if name.endswith(".png") else name
            d.text((x - (cell - px) // 2 + 2, y + px + 6), label[:12], fill=fg)
    sheet.save(out)
    print(out, sheet.size)

if __name__ == "__main__":
    main()
