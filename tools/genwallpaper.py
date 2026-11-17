#!/usr/bin/env python3
"""Generate a theme's wallpaper with OpenAI's sunburst image model.

    genwallpaper.py THEME [--variants N]          draw candidates
    genwallpaper.py THEME --scene TEXT            try another scene (SCENES keeps the chosen one)
    genwallpaper.py THEME --dark-from SRC.png     the same picture for dark mode
    genwallpaper.py THEME --promote SRC.png [--dark] [--crop-y PX] [--collection]
    genwallpaper.py olo-orbit …                   Olo's own abstract pictures, likewise

Each theme (Classic, and Platinum in data/themes) has a wallpaper of its own in the
pack, named after it (see "Themes" in docs/ICONS.md). It is drawn by the
same model at quality "high" and full 4K (3840x2160): a quiet picture
that's a nod to the system the theme is after (BeOS, Mac OS 9), never
its logo or a word (Ben, 2026-11-14: "not too in your face, subtle beos
(for classic) and macos9 (for platinum) throwbacks/nostalgia"). --refs
attaches the theme's reference icons and its Olo mark as style references
(for a scene made of the theme's own objects; the quiet scenes are
drawn from words alone). Candidates go to data/wallpapers/src/THEME-N.png,
each with a .json sidecar; --dark-from redraws a chosen one for dark mode
(THEME-dark-N.png).

--promote makes the pack's files from a chosen candidate: 4k/THEME.webp,
ultrawide/THEME.webp (the 3840x1648 band, from --crop-y, default the
middle; none with --collection) and thumbs/THEME.png (320x180); with
--dark, THEME-dark. The pictures are lossless WebP. Restore a model's
picture first (`restorewallpaper.py restore`: its decoder's texture and
8-bit bands out; docs/ICONS.md, "Restored, and lossless").

Reads OPENAI_API_KEY from the environment or ~ben/.env. Never print it.
"""
import argparse, base64, json, os, sys, time, urllib.error, urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import genicon  # noqa: E402  (its key and multipart, and the model)

MODEL, QUALITY = genicon.MODEL, genicon.QUALITY
SIZE = (3840, 2160)
ULTRAWIDE_H = 1648
THUMB = (320, 180)

DATA = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data")
WALLPAPERS = os.path.join(DATA, "wallpapers")
SAMPLES = ["application", "disk", "folder", "home", "terminal"]

# Every wallpaper: how it sits on the desktop, whatever the theme.
COMMON = (
    "A desktop wallpaper for a computer: one finished picture filling the whole 16:9 frame edge to edge, "
    "at very high resolution, with clean, beautifully finished detail and smooth, banding-free gradients. "
    "It is made to sit quietly behind windows and desktop icons: calm, uncluttered and low in contrast, "
    "with nothing loud or attention-seeking; the right third especially quiet (the desktop's icons sit "
    "there) and the middle free of strong detail (windows cover it). Keep anything of interest inside the "
    "middle three quarters of the height: the picture is also cropped to 21:9 for ultrawide screens. "
    "Light from the upper left. No text, no letters or letter-like shapes, no numbers, no logos, no "
    "watermark, no user interface, no people, no animals. "
)

# Olo's own abstract pictures (the "Olo" collection), plays on the brand's
# colours and its ring (docs/BRAND.md): no words, no logo (Ben,
# 2026-11-16: "new olo high-res 4k backgrounds that are abstract (no
# words) with cool plays on the olo colors"). Named for the picture
# (`genwallpaper.py olo-orbit`), promoted with --jpeg.
BRAND = (
    "The palette is Olo's: Olo blue (#0D6EFD), a clear, saturated azure; Olo yellow (#FFC700), a warm golden "
    "yellow; Olo charcoal (#0B0D14), a blue-black; desktop grey (#6B7280); and soft cool whites (#F3F6FB). "
    "Abstract: no recognisable objects, and nothing that reads as a logo, a letter or a word. "
)
OLO_SCENES = {
    "olo-orbit": (
        "Scene: a few large, smooth, glossy rings, each a fat torus with a rich blue top face and thick "
        "golden-yellow sides, floating at different depths in a bright, airy pale blue-white space that "
        "brightens toward the upper left. The nearest ring is very large and runs off the lower-left edge, "
        "softly focused; two smaller rings float further back toward the upper middle, softer still, one "
        "turned edge-on so only a blue and gold ellipse shows. Gentle, soft reflections and faint coloured "
        "bounce light on the pale ground, very soft contact-free shadows. A premium, calm studio-render "
        "finish with shallow depth of field; the right third open pale space."
    ),
    "olo-ribbons": (
        "Scene: two broad, smooth, silky ribbons of colour flowing across a deep charcoal-navy ground "
        "(#0B0D14 at the edges, a faint #14204A glow behind the middle) in long, slow curves, one a deep "
        "luminous Olo blue, the other a warm golden yellow, rising from the lower left, twisting once softly "
        "around each other and thinning out toward the upper right, their folds catching soft light from the "
        "upper left. Smooth, satin sheen; no sparkles, no particles. Dark, rich and calm."
    ),
    "olo-halo": (
        "Scene: one immense, soft ring of light seen at a slight angle, so large that only its sweeping "
        "lower-left arc crosses the picture, from the bottom edge up through the left third and out of the "
        "top edge. Its body glows a deep Olo blue and its inner rim burns a thin warm golden yellow, like the "
        "edge of an eclipse, the light falling off gently into a smooth night-blue to charcoal ground. A "
        "faint, wide atmospheric haze of blue around the arc; the rest of the frame calm, dark and empty."
    ),
    "olo-shapes": (
        "Scene: a modern, flat, geometric composition, as if cut from fine matte paper: large overlapping "
        "circles, rings, half circles and quarter arcs in Olo blue, Olo yellow, charcoal and desktop grey, "
        "gathered toward the lower left and the bottom edge, some cropped by the frame, on an off-white "
        "ground (#F3F6FB) with an extremely fine paper texture and soft, subtle shadows where the shapes "
        "overlap. Balanced, playful and calm in the manner of a Bauhaus print; the upper right two thirds "
        "mostly open off-white."
    ),
    "olo-glass": (
        "Scene: thick, rounded rings and discs of frosted glass standing in soft light, their edges "
        "refracting the colours behind them into smooth bands of Olo blue and golden yellow, on a pale "
        "cool-grey to white ground. Blurred and dreamy, as if seen through a lens at very shallow depth of "
        "field: soft caustics, glowing rims, no hard detail. The coloured light gathered on the left and "
        "lower middle, fading into airy white toward the right."
    ),
    "olo-dunes": (
        "Scene: smooth, rolling dunes of colour, a landscape of soft layered waves reduced to pure gradient "
        "forms, the near waves a deep Olo blue, rising and lightening through brighter azure to a warm "
        "golden-yellow glow along a low horizon about a third up, under a pale, clean blue-white sky. Soft "
        "light from the upper left, gentle shading on every curve, no texture, no sun disc; serene and wide."
    ),
}

# Each theme's scene: a quiet nod to the system it's after.
SCENES = {
    "classic": (
        "Scene: 'Be blue hour', a calm, nearly abstract landscape seen from far away in the clear blue light "
        "of a late afternoon, the whole picture in the family of BeOS's desktop blue, a dusty steel blue "
        "(#336698), lifted for daylight toward soft dusty steel-blue tints, never teal, cyan or turquoise. A "
        "broad, smooth sky graded from steel blue at the top to a pale blue-grey haze at a low horizon about a "
        "quarter up from the bottom. One long, gentle hill silhouette in deeper steel blue runs low across "
        "the picture, and before it a still, subdued stretch of water that softly mirrors the sky, without "
        "sparkle or glare. Far away in the left quarter, standing on the hill line, one tiny, slender, plain "
        "dark tower with two short side-by-side columns of small, muted amber pinpoint lights (#FFCB00 "
        "dimmed), like a distant beacon: so small it's discovered rather than noticed, with no glow or bloom. "
        "Nothing else. A refined painterly-photographic finish, soft atmospheric depth, restrained and "
        "spacious; the right third almost empty sky and water."
    ),
    "platinum": (
        "Scene: a quiet sky of soft clouds, a gentle nod to the cloud desktop pictures of 1990s computers: a "
        "calm sky in platinum grey with lavender undertones (cool neutral greys with a faint lilac tint, "
        "never a saturated purple), a few broad, softly airbrushed cumulus clouds gathered toward the upper "
        "left, modelled by gentle light from the upper left with soft lilac-grey shadows, dissolving toward "
        "the right into open grey-lilac space. A barely perceptible horizon near the bottom; no sunset bands. "
        "The gradients carry an extremely fine, low-amplitude ordered dither that hints at a 1990s screen's "
        "colours, not film grain and not a distressed filter. Modest colour depth, cool, serene and generous "
        "in its emptiness; the right third almost featureless."
    ),
}

DARK = (
    "The attached image is a desktop wallpaper. Draw exactly the same picture again, for a computer's dark "
    "mode: the same scene, composition, forms, style and framing, but in the dark, so the whole picture is "
    "deep and low in overall brightness, with rich, deepened colour and the forms still gently separated, "
    "nothing in it brighter than it needs to be. {extra}No text, no letters, no numbers, no logos, no "
    "watermark, no user interface."
)
DARK_EXTRA = {
    "classic": "A deep steel-blue and navy twilight, not black; the hill still faintly set apart from the sky, "
    "the water calm; the beacon's amber pinpoints dimmed with the scene, not its brightest thing. ",
    "platinum": "Smoky violet-grey clouds over a charcoal-platinum sky, still soft; no stars, no moon, no "
    "luminous cloud edges. ",
}


SCENES.update({name: BRAND + scene for name, scene in OLO_SCENES.items()})
DARK_EXTRA.update({
    "olo-orbit": "A deep navy-charcoal space; the rings' blue still rich and their gold softly lit, not glowing. ",
    "olo-shapes": "A charcoal ground (#0B0D14 to #151A26); the shapes' blue and yellow deepened a little, the charcoal "
    "shapes now a lifted slate grey so they still read, the paper texture kept. ",
    "olo-glass": "Smoked glass on a deep charcoal-navy ground; the refracted blue and gold glowing softly. ",
    "olo-dunes": "A night: the dunes deep navy to blue, the horizon's glow a low ember of gold under a dark blue sky. ",
})


def refs(theme):
    """The theme's reference icons and its Olo mark."""
    icons = os.path.join(DATA, "icons") if theme == "classic" else os.path.join(DATA, "themes", theme)
    mark = os.path.join(icons, "256", "olo.png") if theme == "classic" else os.path.join(icons, "icons", "256", "olo.png")
    return [os.path.join(icons, "reference", n + ".png") for n in SAMPLES] + [mark]


def generate(prompt, images, n):
    fields = {"model": MODEL, "prompt": prompt, "size": f"{SIZE[0]}x{SIZE[1]}", "quality": QUALITY,
              "background": "opaque", "output_format": "png", "n": str(n)}
    if images:
        body, ctype = genicon.multipart(fields, [("image[]", p) for p in images])
        url = "https://api.openai.com/v1/images/edits"
    else:
        body, ctype = json.dumps({**fields, "n": n}).encode(), "application/json"
        url = "https://api.openai.com/v1/images/generations"
    req = urllib.request.Request(url, body, {"Authorization": "Bearer " + genicon.key(), "Content-Type": ctype})
    for attempt in range(4):
        try:
            return [base64.b64decode(d["b64_json"]) for d in json.load(urllib.request.urlopen(req, timeout=1800))["data"]]
        except urllib.error.HTTPError as e:
            msg = e.read().decode()[:500]
            if e.code < 500 and e.code != 429:
                sys.exit(f"HTTP {e.code}: {msg}")
            time.sleep(20 * (attempt + 1))
    sys.exit("image API kept failing")


def draw(theme, variants, dark_from, scene=None, with_refs=False):
    if dark_from:
        prompt, images, stem = DARK.format(extra=DARK_EXTRA.get(theme, "")), [dark_from], f"{theme}-dark"
    else:
        prompt, images, stem = COMMON + (scene or SCENES[theme]), refs(theme) if with_refs else [], theme
    out = os.path.join(WALLPAPERS, "src")
    os.makedirs(out, exist_ok=True)
    # Never over an earlier candidate: number on from the last.
    first = 1
    while os.path.exists(os.path.join(out, f"{stem}-{first}.png")):
        first += 1
    for i, data in enumerate(generate(prompt, images, variants), first):
        path = os.path.join(out, f"{stem}-{i}.png")
        open(path, "wb").write(data)
        json.dump({"model": MODEL, "quality": QUALITY, "size": f"{SIZE[0]}x{SIZE[1]}", "theme": theme, "prompt": prompt,
                   "references": [os.path.relpath(p, DATA) for p in images],
                   "created": time.strftime("%Y-%m-%dT%H:%M:%S")},
                  open(path[:-4] + ".json", "w"), indent=1)
        print(path)


def save_picture(img, shape, stem):
    """`img` as the pack keeps a picture: lossless WebP (the pixels a PNG
    would hold, a quarter smaller; Ben, 2026-11-16: "zero visual
    artifacts"), `shape`/STEM.webp, any PNG or JPEG of it from before
    taken away. Its path."""
    path = os.path.join(WALLPAPERS, shape, stem + ".webp")
    # (Method 4: the same size as the slowest setting, an eighth of the time.)
    img.save(path, lossless=True, quality=75, method=4, exact=True)
    for old in (".png", ".jpg"):
        if os.path.exists(os.path.join(WALLPAPERS, shape, stem + old)):
            os.remove(os.path.join(WALLPAPERS, shape, stem + old))
    return path


def promote(theme, src, dark, crop_y, collection=False):
    from PIL import Image
    img = Image.open(src).convert("RGB")
    if img.size != SIZE:
        sys.exit(f"{src} is {img.size[0]}x{img.size[1]}, not {SIZE[0]}x{SIZE[1]}")
    name = theme + ("-dark" if dark else "")
    y = (SIZE[1] - ULTRAWIDE_H) // 2 if crop_y is None else crop_y
    if not 0 <= y <= SIZE[1] - ULTRAWIDE_H:
        sys.exit(f"--crop-y runs off the picture (0 to {SIZE[1] - ULTRAWIDE_H})")
    shapes = [("4k", img), ("thumbs", img.resize(THUMB, Image.LANCZOS))]
    if not collection:
        # (A collection's picture has none: its 4K one is cropped to the screen.)
        shapes.append(("ultrawide", img.crop((0, y, SIZE[0], y + ULTRAWIDE_H))))
    for shape, im in shapes:
        if shape == "thumbs":
            path = os.path.join(WALLPAPERS, shape, name + ".png")
            im.save(path, optimize=True)
        else:
            path = save_picture(im, shape, name)
        print(path)


def main():
    if os.environ.get("OLO_IMAGE_MODEL") not in (None, MODEL):
        sys.exit(f"wallpapers must be generated with {MODEL} (see docs/ICONS.md)")
    ap = argparse.ArgumentParser()
    ap.add_argument("theme", choices=sorted(SCENES))
    ap.add_argument("--variants", type=int, default=1)
    ap.add_argument("--dark-from", help="redraw this candidate for dark mode")
    ap.add_argument("--scene", help="draw this scene instead of the theme's")
    ap.add_argument("--refs", action="store_true", help="attach the theme's reference icons and its Olo mark")
    ap.add_argument("--promote", metavar="SRC", help="make the pack's files from this candidate")
    ap.add_argument("--dark", action="store_true", help="with --promote: it is the dark-mode version")
    ap.add_argument("--crop-y", type=int, help="with --promote: the ultrawide band's top")
    ap.add_argument("--collection", "--jpeg", action="store_true", help="with --promote: a collection's picture (4K only, no ultrawide version)")
    a = ap.parse_args()
    if a.promote:
        promote(a.theme, a.promote, a.dark, a.crop_y, a.collection)
    else:
        draw(a.theme, a.variants, a.dark_from, a.scene, a.refs)


if __name__ == "__main__":
    main()
