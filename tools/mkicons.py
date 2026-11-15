#!/usr/bin/env python3
"""Turn generated 1024² icons (data/icons/src) into the shipped sizes.

Each icon is trimmed to its visible pixels, centred in a square with a
consistent margin (so every icon has the same optical weight), and
resampled with Lanczos to data/icons/<px>/<name>.png. Sizes are device
pixels: the kit picks the smallest one at least as large as it needs.

    mkicons.py [--theme ID] [names...]
    mkicons.py --small [--theme ID] [names...]

With --theme ID, does the same for a theme: data/themes/ID/src to
data/themes/ID/icons/<px>/ (see "Themes" in docs/ICONS.md).

16 and 24 pixels are drawn for their size, as BeOS R5's mini icons were
(`small`): sharpened a touch after resampling, and a contour a pixel
wide round the silhouette, dark and part see-through, so a light thing
(a page, a keyboard, a white case) reads on a light panel instead of
fading into it; on a dark panel it costs nothing. --small redraws just
those two sizes from each icon's 256-pixel picture (every icon, or the
named ones), for sets whose 1024² masters aren't at hand.
"""
import os, sys
from PIL import Image, ImageChops, ImageFilter

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "icons")
THEMES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "themes")
SIZES = [16, 24, 32, 48, 64, 96, 128, 256]
MARGIN = 0.03
# The contour's strength at the small sizes (none above): R5's mini icons
# had a solid black one; at 16 px 0.6 reads as clearly and sits lighter.
CONTOUR = {16: 0.6, 24: 0.4}

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

def small(im, px):
    """`im` (RGBA, square) at `px`, drawn for its size where it's small."""
    # Premultiplied resampling avoids dark fringes at the edges.
    s = im.convert("RGBa").resize((px, px), Image.LANCZOS).convert("RGBA")
    strength = CONTOUR.get(px)
    if strength is None:
        return s
    rgb = s.convert("RGB").filter(ImageFilter.UnsharpMask(radius=0.6, percent=60, threshold=0))
    s = Image.merge("RGBA", (*rgb.split(), s.getchannel("A")))
    a = s.getchannel("A")
    ring = ImageChops.subtract(a.filter(ImageFilter.MaxFilter(3)), a).point(lambda v: int(v * strength))
    out = Image.new("RGBA", s.size, (0, 0, 0, 0))
    out.putalpha(ring)
    out.alpha_composite(s)
    return out


def process(src, name, root=ROOT):
    sq = square(Image.open(src))
    for px in SIZES:
        out = os.path.join(root, str(px))
        os.makedirs(out, exist_ok=True)
        small(sq, px).save(os.path.join(out, name + ".png"), optimize=True)


def redraw_small(name, root=ROOT):
    """16 and 24 px again from the 256-pixel picture."""
    big = Image.open(os.path.join(root, "256", name + ".png")).convert("RGBA")
    for px in CONTOUR:
        small(big, px).save(os.path.join(root, str(px), name + ".png"), optimize=True)

def main():
    args = sys.argv[1:]
    only_small = args[:1] == ["--small"]
    if only_small:
        args = args[1:]
    src, root = os.path.join(ROOT, "src"), ROOT
    if args[:1] == ["--theme"]:
        theme = os.path.join(THEMES, args[1])
        src, root = os.path.join(theme, "src"), os.path.join(theme, "icons")
        args = args[2:]
    if only_small:
        names = args or [f[:-4] for f in os.listdir(os.path.join(root, "256")) if f.endswith(".png")]
        for n in sorted(names):
            redraw_small(n, root)
        print(f"{len(names)} icons' 16 and 24 px drawn again")
        return
    names = args or [f[:-4] for f in os.listdir(src) if f.endswith(".png")]
    for n in sorted(names):
        process(os.path.join(src, n + ".png"), n, root)
        print(n)

if __name__ == "__main__":
    main()
