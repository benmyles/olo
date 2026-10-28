#!/usr/bin/env python3
"""Turn generated 1024² icons (data/icons/src) into the shipped sizes.

Each icon is trimmed to its visible pixels, centred in a square with a
consistent margin (so every icon has the same optical weight), and
resampled with Lanczos to data/icons/<px>/<name>.png. Sizes are device
pixels: the kit picks the smallest one at least as large as it needs.

    mkicons.py [--theme ID] [names...]

With --theme ID, does the same for a theme: data/themes/ID/src to
data/themes/ID/icons/<px>/ (see "Themes" in docs/ICONS.md).
"""
import os, sys
from PIL import Image

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "icons")
THEMES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "themes")
SIZES = [16, 24, 32, 48, 64, 96, 128, 256]
MARGIN = 0.03

def square(im):
    """Trim to the visible pixels and centre in a square with MARGIN."""
    im = im.convert("RGBA")
    alpha = im.getchannel("A").point(lambda a: 255 if a > 8 else 0)
    box = alpha.getbbox()
    if box:
        im = im.crop(box)
    side = int(max(im.size) * (1 + 2 * MARGIN))
    sq = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    # Optically centred: sit the object slightly low, like a desk icon.
    sq.alpha_composite(im, ((side - im.width) // 2, (side - im.height) // 2 + int(side * 0.005)))
    return sq

def process(src, name, root=ROOT):
    sq = square(Image.open(src))
    for px in SIZES:
        out = os.path.join(root, str(px))
        os.makedirs(out, exist_ok=True)
        # Premultiplied resampling avoids dark fringes at the edges.
        small = sq.convert("RGBa").resize((px, px), Image.LANCZOS).convert("RGBA")
        small.save(os.path.join(out, name + ".png"), optimize=True)

def main():
    args = sys.argv[1:]
    src, root = os.path.join(ROOT, "src"), ROOT
    if args[:1] == ["--theme"]:
        theme = os.path.join(THEMES, args[1])
        src, root = os.path.join(theme, "src"), os.path.join(theme, "icons")
        args = args[2:]
    names = args or [f[:-4] for f in os.listdir(src) if f.endswith(".png")]
    for n in sorted(names):
        process(os.path.join(src, n + ".png"), n, root)
        print(n)

if __name__ == "__main__":
    main()
