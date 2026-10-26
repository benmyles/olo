#!/usr/bin/env python3
"""Turn generated 1024² icons (data/icons/src) into the shipped sizes.

Each icon is trimmed to its visible pixels, centred in a square with a
consistent margin (so every icon has the same optical weight), and
resampled with Lanczos to data/icons/<px>/<name>.png. Sizes are device
pixels: the kit picks the smallest one at least as large as it needs.
"""
import os, sys
from PIL import Image

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "icons")
SIZES = [16, 24, 32, 48, 64, 96, 128, 256]
MARGIN = 0.03

def process(src, name):
    im = Image.open(src).convert("RGBA")
    alpha = im.getchannel("A").point(lambda a: 255 if a > 8 else 0)
    box = alpha.getbbox()
    if box:
        im = im.crop(box)
    side = int(max(im.size) * (1 + 2 * MARGIN))
    sq = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    # Optically centred: sit the object slightly low, like a desk icon.
    sq.alpha_composite(im, ((side - im.width) // 2, (side - im.height) // 2 + int(side * 0.005)))
    for px in SIZES:
        out = os.path.join(ROOT, str(px))
        os.makedirs(out, exist_ok=True)
        # Premultiplied resampling avoids dark fringes at the edges.
        small = sq.convert("RGBa").resize((px, px), Image.LANCZOS).convert("RGBA")
        small.save(os.path.join(out, name + ".png"), optimize=True)

def main():
    names = sys.argv[1:] or [f[:-4] for f in os.listdir(os.path.join(ROOT, "src")) if f.endswith(".png")]
    for n in sorted(names):
        process(os.path.join(ROOT, "src", n + ".png"), n)
        print(n)

if __name__ == "__main__":
    main()
