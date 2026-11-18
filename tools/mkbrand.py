#!/usr/bin/env python3
"""Make Olo's brand pictures from the mark and the Sora typeface.

    mkbrand.py                 # everything below
    mkbrand.py --sheet OUT.png # and a contact sheet of it all, on light and dark

The brand (docs/BRAND.md): the Olo mark, a glossy blue and yellow ring
(a drawing, data/icons/svg/olo.svg: Olo's own `olo` icon, rendered here
as Olo renders it, for each size it's wanted at; it was written from
the icon model's picture of the mark, docs/brand/olo-mark.png), and the
wordmark, "Olo" set in Sora
ExtraBold with "DESKTOP" under it in Sora SemiBold, tracked wide. The
lettering is set here from the font (data/fonts/Sora[wght].ttf), never
drawn by a model: a typeface's letters, exact at any size.

Writes:
  data/brand/olo-wordmark.png            "Olo", charcoal: Olo's UI wordmark on light
  data/brand/olo-wordmark-on-dark.png    "Olo", near-white, on dark
  data/brand/sizes/olo-logo-on-dark-W.png  the horizontal logo, on dark, W px wide
                                         (256 to 512: the upgrade's veil and the
                                         boot splash at each scale, pixel for pixel)
  website/static/brand/…                 the brand kit: the mark, the horizontal and
                                         stacked logos, the wordmark and "Olo" alone
                                         (light and on dark), favicons and the social
                                         picture
  docs/img/olo-social.png                GitHub's social preview

Run with /usr/bin/python3 (it has PIL), after
`cargo build --release -p olo-xdg --example svgicon` (what renders the
drawing).
"""
import argparse, os, subprocess, sys

from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
FONT = os.path.join(ROOT, "data", "fonts", "Sora[wght].ttf")
DRAWING = os.path.join(ROOT, "data", "icons", "svg", "olo.svg")
SVGICON = os.path.join(ROOT, "target", "release", "examples", "svgicon")
SCRATCH = os.path.join("/var/tmp", f"mkbrand-{os.getuid()}")
KIT = os.path.join(ROOT, "website", "static", "brand")

# The palette (docs/BRAND.md).
BLUE = (13, 110, 253)
YELLOW = (255, 199, 0)
CHARCOAL = (11, 13, 20)
GRAY = (107, 114, 128)
# On dark: the wordmark near-white, DESKTOP a lighter grey (the brand's
# grey on Olo's dark is 3.6:1; this is 7:1).
WHITE = (244, 246, 250)
GRAY_ON_DARK = (161, 167, 179)
# Olo's dark (app_server's veil) and the brand sheet's pale ground.
DARK_GROUND = (30, 30, 34)
LIGHT_GROUND = (238, 242, 249)

# Type: "Olo" in ExtraBold; "DESKTOP" in SemiBold, tracked 0.32 em, as
# wide as "Olo".
OLO_WEIGHT, DESKTOP_WEIGHT, TRACKING = 800, 600, 0.32
# "Olo" a little tighter than Sora sets it, as on the sheet.
OLO_TRACK = -0.03


def font(size, weight):
    f = ImageFont.truetype(FONT, size)
    f.set_variation_by_axes([weight])
    return f


def ink(im):
    """The box of what `im` draws."""
    return im.getchannel("A").point(lambda a: 255 if a > 4 else 0).getbbox()


def trimmed(im):
    return im.crop(ink(im))


def olo(cap, color):
    """ "Olo", its capital `cap` pixels tall, trimmed to its ink.

    As the brand sheet sets it: Sora ExtraBold, a little tight (OLO_TRACK),
    and its l a plain bar, Sora's own stem without the flag at its top
    (neither Sora's l nor its alternate, which has a foot, is the sheet's).
    Set four times larger and averaged down, so the bar's edges are
    smoothed as the letters' are."""
    k = 4
    size = round(cap / 0.70) * k  # Sora's cap height is 0.70 em
    f = font(size, OLO_WEIGHT)
    base = size  # the baseline's y in the canvas
    im = Image.new("RGBA", (size * 4, size * 2), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    # The l's stem: its columns along the bottom row of the glyph, and the
    # glyph's top (the flag's top: the ascender).
    probe = Image.new("L", (size * 2, size * 2), 0)
    ImageDraw.Draw(probe).text((size // 2, base), "l", font=f, fill=255, anchor="ls")
    x0, top, _, bottom = probe.getbbox()
    row = [probe.getpixel((x, bottom - 2)) > 127 for x in range(probe.width)]
    s0 = row.index(True)
    s1 = len(row) - row[::-1].index(True)
    lsb = s0 - size // 2  # the stem's left from the pen
    x = size // 4
    d.text((x, base), "O", font=f, fill=color + (255,), anchor="ls")
    x += f.getlength("O") + OLO_TRACK * size
    d.rectangle((x + lsb, top, x + lsb + (s1 - s0) - 1, bottom - 1), fill=color + (255,))
    x += f.getlength("l") + OLO_TRACK * size
    d.text((x, base), "o", font=f, fill=color + (255,), anchor="ls")
    im = trimmed(im)
    return im.convert("RGBa").resize((round(im.width / k), round(im.height / k)), Image.LANCZOS).convert("RGBA")


def desktop(width, color):
    """ "DESKTOP", tracked wide, `width` pixels from its D to its P."""

    def set_at(size):
        f = font(size, DESKTOP_WEIGHT)
        track = TRACKING * size
        w = sum(f.getlength(c) for c in "DESKTOP") + track * 6
        im = Image.new("RGBA", (int(w + size), size * 2), (0, 0, 0, 0))
        d, x = ImageDraw.Draw(im), size / 2
        for c in "DESKTOP":
            d.text((x, size / 2), c, font=f, fill=color + (255,))
            x += f.getlength(c) + track
        return trimmed(im)

    size = max(4, round(width / 6.5))
    for _ in range(6):  # Fit its ink to the width.
        im = set_at(size)
        if abs(im.width - width) <= 1:
            break
        size = max(4, round(size * width / im.width))
    return im


def wordmark(cap, dark, sub=True):
    """The wordmark: "Olo", and under it "DESKTOP" when `sub`."""
    top = olo(cap, WHITE if dark else CHARCOAL)
    if not sub:
        return top
    under = desktop(round(top.width * 0.98), GRAY_ON_DARK if dark else GRAY)
    gap = round(cap * 0.30)
    im = Image.new("RGBA", (max(top.width, under.width), top.height + gap + under.height), (0, 0, 0, 0))
    im.alpha_composite(top, ((im.width - top.width) // 2, 0))
    im.alpha_composite(under, ((im.width - under.width) // 2, top.height + gap))
    return im


def drawn(px):
    """The mark's drawing rendered on a canvas `px` pixels a side, as Olo
    renders its icons (drawn for the size: its outline a pixel at least),
    without its contact shadow (a logo stands on nothing)."""
    os.makedirs(SCRATCH, exist_ok=True)
    svg = open(DRAWING).read()
    a = svg.index('<ellipse class="from48"')
    b = svg.index("/>", a) + 2
    assert "url(#shadow)" in svg[a:b], "the drawing's first large-only ellipse is its shadow"
    plain, out = os.path.join(SCRATCH, "olo.svg"), os.path.join(SCRATCH, f"olo-{px}.png")
    open(plain, "w").write(svg[:a] + svg[b:])
    subprocess.run([SVGICON, plain, out, str(px)], check=True, capture_output=True)
    return Image.open(out).convert("RGBA")


_FILLS = []


def fills():
    """How much of its canvas the drawing's ink takes: (across, down)."""
    if not _FILLS:
        x0, y0, x1, y1 = ink(drawn(1024))
        _FILLS.append(((x1 - x0) / 1024, (y1 - y0) / 1024))
    return _FILLS[0]


def mark_in(w, h):
    """The mark as large as fits `w` by `h` pixels, drawn for that size.
    (The square marks and the favicons are used as drawn; a logo is
    composed large, lettering and mark together, and scaled whole.)"""
    fw, fh = fills()
    px = int(min(w / fw, h / fh))
    m = trimmed(drawn(px))
    while m.width > w or m.height > h:  # (a pixel over, by rounding)
        px -= 1
        m = trimmed(drawn(px))
    return m


def mark(height):
    return mark_in(10 * height, height)


def mark_square(side, frac):
    """The mark centred in a square `side` pixels, `frac` of it clear all round."""
    inner = side - 2 * round(side * frac)
    m = mark_in(inner, inner)
    out = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    out.alpha_composite(m, ((side - m.width) // 2, (side - m.height) // 2))
    return out


def horizontal(cap, dark):
    """The primary logo: the mark, a rule, the wordmark."""
    word = wordmark(cap, dark)
    m = mark(round(word.height * 1.4))
    gap, rule_w = round(cap * 0.42), max(1, round(cap * 0.035))
    rule_h = round(word.height * 1.2)
    w = m.width + gap + rule_w + gap + word.width
    h = max(m.height, rule_h, word.height)
    im = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    im.alpha_composite(m, (0, (h - m.height) // 2))
    rule = Image.new("RGBA", (rule_w, rule_h), (GRAY_ON_DARK if dark else GRAY) + (150,))
    im.alpha_composite(rule, (m.width + gap, (h - rule_h) // 2))
    im.alpha_composite(word, (m.width + 2 * gap + rule_w, (h - word.height) // 2))
    return im


def stacked(cap, dark):
    """The mark over the wordmark."""
    word = wordmark(cap, dark)
    m = mark(round(word.width * 1.25))
    gap = round(cap * 0.35)
    im = Image.new("RGBA", (max(m.width, word.width), m.height + gap + word.height), (0, 0, 0, 0))
    im.alpha_composite(m, ((im.width - m.width) // 2, 0))
    im.alpha_composite(word, ((im.width - word.width) // 2, m.height + gap))
    return im


def padded(im, frac=0.03):
    pad = round(max(im.size) * frac)
    out = Image.new("RGBA", (im.width + 2 * pad, im.height + 2 * pad), (0, 0, 0, 0))
    out.alpha_composite(im, (pad, pad))
    return out


def to_width(im, w):
    """Averaged down to `w` pixels wide (premultiplied, so edges don't darken)."""
    h = round(im.height * w / im.width)
    return im.convert("RGBa").resize((w, h), Image.LANCZOS).convert("RGBA")


def square(im, side, frac=0.06):
    """`im` centred on a transparent square, `frac` of it margin."""
    inner = round(side * (1 - 2 * frac))
    s = inner / max(im.size)
    small = im.convert("RGBa").resize((max(1, round(im.width * s)), max(1, round(im.height * s))), Image.LANCZOS).convert("RGBA")
    out = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    out.alpha_composite(small, ((side - small.width) // 2, (side - small.height) // 2))
    return out


def ground(size, top, bottom):
    """A soft vertical gradient, the sheet's pale ground or a dark one."""
    w, h = size
    g = Image.new("RGB", (1, h))
    for y in range(h):
        t = y / max(1, h - 1)
        g.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)))
    return g.resize((w, h)).convert("RGBA")


def social(path):
    """1280×640: the horizontal logo on the sheet's pale ground and a line."""
    im = ground((1280, 640), (245, 247, 252), (226, 233, 246))
    logo = to_width(horizontal(320, False), 820)
    im.alpha_composite(logo, ((1280 - logo.width) // 2, 200 - logo.height // 2 + 40))
    f = font(34, 500)
    line = "A desktop for Linux in the spirit of BeOS"
    d = ImageDraw.Draw(im)
    d.text(((1280 - d.textlength(line, font=f)) / 2, 470), line, font=f, fill=GRAY + (255,))
    im.convert("RGB").save(path, optimize=True)
    return path


def save(im, path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    im.save(path, optimize=True)
    print(os.path.relpath(path, ROOT), f"{im.width}×{im.height}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sheet", help="also draw a contact sheet of it all here")
    a = ap.parse_args()
    if not os.path.exists(SVGICON):
        sys.exit("build what renders the drawing first: cargo build --release -p olo-xdg --example svgicon")

    brand = os.path.join(ROOT, "data", "brand")
    # Olo's UI wordmark: "Olo" alone (DESKTOP can't be read at the
    # Deskbar's 18 points), drawn by `olo_kit::brand` at any height.
    for dark, name in [(False, "olo-wordmark.png"), (True, "olo-wordmark-on-dark.png")]:
        save(padded(wordmark(560, dark, sub=False)), os.path.join(brand, name))
    # The veil's and boot splash's logo, pixel for pixel at each scale.
    big = horizontal(480, True)
    for w in (256, 320, 384, 448, 512):
        save(to_width(big, w), os.path.join(brand, "sizes", f"olo-logo-on-dark-{w}.png"))

    # The brand kit (the website's /brand and the README).
    kit = {}
    for dark in (False, True):
        sfx = "-on-dark" if dark else ""
        kit[f"olo-logo{sfx}.png"] = to_width(padded(horizontal(240, dark)), 1600)
        kit[f"olo-logo-stacked{sfx}.png"] = to_width(padded(stacked(240, dark)), 1000)
        kit[f"olo-wordmark{sfx}.png"] = to_width(padded(wordmark(400, dark)), 1200)
        # "Olo" alone, for small places (the website's header).
        kit[f"olo-name{sfx}.png"] = to_width(wordmark(400, dark, sub=False), 480)
    kit["olo-mark.png"] = mark_square(1024, 0.04)
    for name, im in kit.items():
        save(im, os.path.join(KIT, name))
    for side, name in [(32, "favicon-32.png"), (180, "apple-touch-icon.png"), (192, "icon-192.png"), (512, "icon-512.png")]:
        im = mark_square(side, 0.04 if side < 64 else 0.08)
        if name == "apple-touch-icon.png":  # iOS draws it on black: give it the pale ground.
            bg = ground((side, side), (245, 247, 252), (226, 233, 246))
            bg.alpha_composite(im)
            im = bg.convert("RGB")
        save(im, os.path.join(KIT, name))
    # favicon.ico: 16, 32 and 48 in one, each drawn for its size.
    ico = [mark_square(side, 0.02) for side in (48, 32, 16)]
    ico[0].save(os.path.join(KIT, "favicon.ico"), sizes=[(16, 16), (32, 32), (48, 48)], append_images=ico[1:])
    print("website/static/brand/favicon.ico")
    print(os.path.relpath(social(os.path.join(ROOT, "docs", "img", "olo-social.png")), ROOT))
    og = os.path.join(KIT, "olo-social.png")
    social(og)
    print(os.path.relpath(og, ROOT))

    if a.sheet:
        parts = [kit["olo-logo.png"], kit["olo-logo-stacked.png"], kit["olo-wordmark.png"],
                 Image.open(os.path.join(brand, "olo-wordmark.png")).convert("RGBA")]
        dparts = [kit["olo-logo-on-dark.png"], kit["olo-logo-stacked-on-dark.png"], kit["olo-wordmark-on-dark.png"],
                  Image.open(os.path.join(brand, "olo-wordmark-on-dark.png")).convert("RGBA")]
        H = 360
        row = lambda ps: [p.resize((round(p.width * H / p.height), H), Image.LANCZOS) for p in ps]
        lt, dk = row(parts), row(dparts)
        W = sum(p.width for p in lt) + 40 * (len(lt) + 1)
        sheet = Image.new("RGBA", (W, 2 * H + 120), LIGHT_GROUND + (255,))
        sheet.alpha_composite(Image.new("RGBA", (W, H + 60), DARK_GROUND + (255,)), (0, H + 60))
        for y, ps in [(30, lt), (H + 90, dk)]:
            x = 40
            for p in ps:
                sheet.alpha_composite(p, (x, y))
                x += p.width + 40
        sheet.convert("RGB").save(a.sheet)
        print(a.sheet)


if __name__ == "__main__":
    main()
