#!/usr/bin/env python3
"""Generate a theme's wallpaper with OpenAI's sunburst image model.

    genwallpaper.py THEME [--variants N]          draw candidates
    genwallpaper.py THEME --scene TEXT            try another scene (SCENES keeps the chosen one)
    genwallpaper.py THEME --dark-from SRC.png     the same picture for dark mode
    genwallpaper.py THEME --promote SRC.png [--dark] [--crop-y PX]

Each theme (Classic, and Platinum in data/themes) has a wallpaper of its own in the
pack, named after it (see "Themes" in docs/ICONS.md). It is drawn by the
same model at quality "high" and full 4K (3840x2160): a quiet picture
that's a nod to the system the theme is after (BeOS, Mac OS 9), never
its logo or a word (Ben, 2026-11-14: "not too in your face, subtle beos
(for classic) and macos9 (for platinum) throwbacks/nostalgia"). --refs
attaches the theme's reference icons and wordmark as style references
(for a scene made of the theme's own objects; the quiet scenes are
drawn from words alone). Candidates go to data/wallpapers/src/THEME-N.png,
each with a .json sidecar; --dark-from redraws a chosen one for dark mode
(THEME-dark-N.png).

--promote makes the pack's files from a chosen candidate: 4k/THEME.png,
ultrawide/THEME.png (the 3840x1648 band, from --crop-y, default the
middle) and thumbs/THEME.png (320x180); with --dark, THEME-dark.png.

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


def refs(theme):
    """The theme's reference icons and its wordmark."""
    if theme == "classic":
        icons, brand = os.path.join(DATA, "icons", "reference"), os.path.join(DATA, "brand")
    else:
        icons, brand = os.path.join(DATA, "themes", theme, "reference"), os.path.join(DATA, "themes", theme, "brand")
    return [os.path.join(icons, n + ".png") for n in SAMPLES] + [os.path.join(brand, "olo-wordmark.png")]


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


def promote(theme, src, dark, crop_y):
    from PIL import Image
    img = Image.open(src).convert("RGB")
    if img.size != SIZE:
        sys.exit(f"{src} is {img.size[0]}x{img.size[1]}, not {SIZE[0]}x{SIZE[1]}")
    name = theme + ("-dark" if dark else "")
    y = (SIZE[1] - ULTRAWIDE_H) // 2 if crop_y is None else crop_y
    if not 0 <= y <= SIZE[1] - ULTRAWIDE_H:
        sys.exit(f"--crop-y runs off the picture (0 to {SIZE[1] - ULTRAWIDE_H})")
    for shape, im in [("4k", img), ("ultrawide", img.crop((0, y, SIZE[0], y + ULTRAWIDE_H))),
                      ("thumbs", img.resize(THUMB, Image.LANCZOS))]:
        path = os.path.join(WALLPAPERS, shape, name + ".png")
        im.save(path, optimize=True)
        print(path)


def main():
    if os.environ.get("OLO_IMAGE_MODEL") not in (None, MODEL):
        sys.exit(f"wallpapers must be generated with {MODEL} (see docs/ICONS.md)")
    ap = argparse.ArgumentParser()
    ap.add_argument("theme", choices=sorted(SCENES))
    ap.add_argument("--variants", type=int, default=1)
    ap.add_argument("--dark-from", help="redraw this candidate for dark mode")
    ap.add_argument("--scene", help="draw this scene instead of the theme's")
    ap.add_argument("--refs", action="store_true", help="attach the theme's reference icons and wordmark")
    ap.add_argument("--promote", metavar="SRC", help="make the pack's files from this candidate")
    ap.add_argument("--dark", action="store_true", help="with --promote: it is the dark-mode version")
    ap.add_argument("--crop-y", type=int, help="with --promote: the ultrawide band's top")
    a = ap.parse_args()
    if a.promote:
        promote(a.theme, a.promote, a.dark, a.crop_y)
    else:
        draw(a.theme, a.variants, a.dark_from, a.scene, a.refs)


if __name__ == "__main__":
    main()
