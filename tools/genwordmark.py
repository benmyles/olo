#!/usr/bin/env python3
"""Draw a theme's Olo wordmark with the icons' image model.

    genwordmark.py THEME [--variants N] [--note TEXT]   # candidates
    genwordmark.py THEME --promote PNG         # → data/themes/THEME/brand/

The wordmark is Olo's lowercase o, l, o: the o's Be blue, the l BeOS tab
yellow, drawn in the theme's material (see "Themes" in docs/ICONS.md). The
Classic wordmark (data/brand/olo-wordmark.png) is attached for its shape and
colours, and the theme's reference icons for its look. Always
gpt-image-2.5-sunburst at quality "high", 1536×1024, as genicon.py.
Candidates go to data/themes/THEME/src/wordmark/ (gitignored), each with a
.json sidecar; --note adds a line on how the theme makes letters (the
one chosen goes in NOTES). --promote trims one to its visible pixels, pads it as the
other themes' are and writes it 768 px wide to brand/olo-wordmark.png.

Reads OPENAI_API_KEY from the environment or ~ben/.env. Never print it.
"""
import argparse, glob, json, os, re, sys, time

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import genicon

DATA = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data")
WIDTH = 768
MARGIN = 0.03

LEAD = (
    "Olo's wordmark: the word olo as three chunky lowercase letters, o, l and o, standing side by side as "
    "solid objects, as in the first attached image, which is the same wordmark in another look: keep its "
    "letters, their proportions and their colours (both o's a clear Be blue, the l a BeOS tab yellow and a "
    "little taller than the o's), and draw it in the look of the other attached images, icons of one set: "
    "the same material, rendering, outline, palette, viewing angle, lighting and cast shadow. "
    "Do not draw the icons themselves; nothing but the three letters. "
)


# How a theme makes its letters, where its icon style alone gave the
# Classic wordmark's gloss.
NOTES = {
    "cube": "Here the letters are made as NeXTSTEP made its objects: solid bodies of matte black plastic with crisp "
    "bevelled edges and a fine highlight along the lit ones, only their front faces in the blue and the yellow, "
    "upright, seen nearly straight on. ",
}


def style(theme):
    """The theme's icon style, made to describe lettering, not an icon."""
    s = open(os.path.join(DATA, "themes", theme, "style.txt")).read().strip()
    s = re.sub(r"^An? (desktop |application )*icon", "Lettering", s)
    s = re.sub(r"No text, no letters, no logos, no watermark\.", "No other text, no logos, no watermark.", s)
    s = re.sub(r"Centred on a fully transparent background[^.]*\.", "", s)
    return s.strip()


def draw(theme, n, note=None):
    refs = [os.path.join(DATA, "brand", "olo-wordmark.png")] + sorted(glob.glob(os.path.join(DATA, "themes", theme, "reference", "*.png")))
    prompt = LEAD + (note if note is not None else NOTES.get(theme, "")) + "The look: " + style(theme) + " Wide, centred on a fully transparent background with generous padding; nothing else in the frame."
    genicon.SIZE = "1536x1024"
    out = os.path.join(DATA, "themes", theme, "src", "wordmark")
    os.makedirs(out, exist_ok=True)
    for i, data in enumerate(genicon.generate(prompt, refs, n), 1):
        path = os.path.join(out, f"olo-wordmark-{len(glob.glob(os.path.join(out, '*.png'))) + 1}.png")
        open(path, "wb").write(data)
        json.dump({"model": genicon.MODEL, "quality": genicon.QUALITY, "size": genicon.SIZE, "theme": theme, "prompt": prompt,
                   "references": [os.path.basename(r) for r in refs], "created": time.strftime("%Y-%m-%dT%H:%M:%S")},
                  open(path[:-4] + ".json", "w"), indent=1)
        print(path)


def promote(theme, src):
    im = Image.open(src).convert("RGBA")
    box = im.getchannel("A").point(lambda a: 255 if a > 8 else 0).getbbox()
    im = im.crop(box)
    pad = int(max(im.size) * MARGIN)
    framed = Image.new("RGBA", (im.width + 2 * pad, im.height + 2 * pad), (0, 0, 0, 0))
    framed.alpha_composite(im, (pad, pad))
    h = round(framed.height * WIDTH / framed.width)
    # Premultiplied, so the edges don't darken.
    out = framed.convert("RGBa").resize((WIDTH, h), Image.LANCZOS).convert("RGBA")
    dest = os.path.join(DATA, "themes", theme, "brand")
    os.makedirs(dest, exist_ok=True)
    out.save(os.path.join(dest, "olo-wordmark.png"), optimize=True)
    print(os.path.join(dest, "olo-wordmark.png"), out.size)


def main():
    if os.environ.get("OLO_IMAGE_MODEL") not in (None, genicon.MODEL):
        sys.exit(f"wordmarks must be drawn with {genicon.MODEL} (see docs/ICONS.md)")
    ap = argparse.ArgumentParser()
    ap.add_argument("theme")
    ap.add_argument("--variants", type=int, default=2)
    ap.add_argument("--note")
    ap.add_argument("--promote")
    a = ap.parse_args()
    if not os.path.isdir(os.path.join(DATA, "themes", a.theme)):
        sys.exit(f"no theme {a.theme!r} in data/themes")
    if a.promote:
        promote(a.theme, a.promote)
    else:
        draw(a.theme, a.variants, a.note)


if __name__ == "__main__":
    main()
