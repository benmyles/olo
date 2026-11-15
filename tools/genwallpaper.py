#!/usr/bin/env python3
"""Generate a theme's wallpaper with OpenAI's sunburst image model.

    genwallpaper.py THEME [--variants N]          draw candidates
    genwallpaper.py THEME --scene TEXT            try another scene (SCENES keeps the chosen one)
    genwallpaper.py THEME --dark-from SRC.png     the same picture for dark mode
    genwallpaper.py THEME --promote SRC.png [--dark] [--crop-y PX]

Each theme (Classic, and Platinum in data/themes) has a wallpaper of its own in the
pack, named after it (see "Themes" in docs/ICONS.md). It is drawn by the
same model at quality "high" and full 4K (3840x2160), with the theme's
reference icons and wordmark as style references, so it is in the
theme's material. Candidates go to data/wallpapers/src/THEME-N.png, each
with a .json sidecar; --dark-from redraws a chosen one for dark mode
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
    "at very high resolution, with crisp, clean, beautifully finished detail everywhere. "
    "It is drawn in exactly the look of the attached images, which are icons and the wordmark of one set: "
    "the same material, rendering, palette, edges and lighting, so it plainly belongs with them. "
    "Do not draw the attached icons themselves or the whole wordmark; draw only the scene below. "
    "Calm and uncluttered, made to sit behind windows and desktop icons: one hero element left of centre, "
    "the rest restful space with gentle detail, and the right third quieter still, for the desktop's icons. "
    "Keep everything important inside the middle three quarters of the height: the picture is also cropped "
    "to 21:9 for ultrawide screens. Light from the upper left. "
    "No text, no letters, no numbers, no logos, no watermark, no user interface, no people. "
)

# Each theme's scene, in its own material. The hero is the wordmark's o,
# on its own, as an object.
SCENES = {
    "classic": (
        "Scene: Olo's first look, glossy, bright and saturated: a big glossy Be blue ring like the o of the "
        "attached wordmark (a ring-shaped object, not a letter), with a glossy BeOS-yellow bar beside it, "
        "stands left of centre on a softly reflective pale blue floor that fades into a bright, luminous "
        "sky-blue backdrop; glossy plastic with crisp highlights and soft reflections, a bold dark outline as "
        "in the icons, cheerful bright light from the upper left."
    ),
    "platinum": (
        "Scene: a friendly, clean late-1990s desktop picture: soft rounded hills in pale lilac-grey and "
        "platinum grey rolling under a clear, pale sky that fades from soft periwinkle at the top to white at "
        "the horizon, a few small puffy white clouds; on a hill left of centre, a big chunky Be blue ring like "
        "the o of the attached wordmark (a ring-shaped object, not a letter) beside a sunny BeOS-yellow bar, "
        "seen in a gentle view from slightly above. Crisp even black outlines as in the icons, flat colour "
        "with gentle smooth shading and one soft highlight on each form, a small soft grey shadow to the "
        "lower right. Cheerful, tidy and restful."
    ),
}

DARK = (
    "The attached image is a desktop wallpaper. Draw exactly the same picture again, for a computer's dark "
    "mode: the same scene, composition, objects, materials, style and framing, but at night in a dim room "
    "or under a night sky, so the whole picture is deep and dark with low overall brightness and rich, "
    "deepened colour, and the hero element is still clearly readable, picked out by a soft rim light from "
    "the upper left. {extra}No text, no letters, no numbers, no logos, no watermark, no user interface."
)
DARK_EXTRA = {
    "platinum": "Still black outlines and flat, gently shaded colour: a deep periwinkle dusk, hills in dark lilac-grey. ",
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
    body, ctype = genicon.multipart(fields, [("image[]", p) for p in images])
    req = urllib.request.Request("https://api.openai.com/v1/images/edits", body,
                                 {"Authorization": "Bearer " + genicon.key(), "Content-Type": ctype})
    for attempt in range(4):
        try:
            return [base64.b64decode(d["b64_json"]) for d in json.load(urllib.request.urlopen(req, timeout=1800))["data"]]
        except urllib.error.HTTPError as e:
            msg = e.read().decode()[:500]
            if e.code < 500 and e.code != 429:
                sys.exit(f"HTTP {e.code}: {msg}")
            time.sleep(20 * (attempt + 1))
    sys.exit("image API kept failing")


def draw(theme, variants, dark_from, scene=None):
    if dark_from:
        prompt, images, stem = DARK.format(extra=DARK_EXTRA.get(theme, "")), [dark_from], f"{theme}-dark"
    else:
        prompt, images, stem = COMMON + (scene or SCENES[theme]), refs(theme), theme
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
    ap.add_argument("--promote", metavar="SRC", help="make the pack's files from this candidate")
    ap.add_argument("--dark", action="store_true", help="with --promote: it is the dark-mode version")
    ap.add_argument("--crop-y", type=int, help="with --promote: the ultrawide band's top")
    a = ap.parse_args()
    if a.promote:
        promote(a.theme, a.promote, a.dark, a.crop_y)
    else:
        draw(a.theme, a.variants, a.dark_from, a.scene)


if __name__ == "__main__":
    main()
