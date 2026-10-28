#!/usr/bin/env python3
"""Generate a theme's wallpaper with OpenAI's sunburst image model.

    genwallpaper.py THEME [--variants N]          draw candidates
    genwallpaper.py THEME --scene TEXT            try another scene (SCENES keeps the chosen one)
    genwallpaper.py THEME --dark-from SRC.png     the same picture for dark mode
    genwallpaper.py THEME --promote SRC.png [--dark] [--crop-y PX]

Each theme (data/themes, and classic) has a wallpaper of its own in the
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
    "soft-be": (
        "Scene: a dreamy miniature diorama made of soft matte modelling clay and soft-touch vinyl: gently "
        "rolling, plump, rounded hills in softened Be blue and mint green, a few puffy rounded clay clouds in "
        "a pale sky that fades from soft blue to warm white, and, resting on a hill left of centre, a big "
        "plump Be blue clay ring like the o of the attached wordmark (a ring-shaped object, not a letter), "
        "with a short plump BeOS-yellow pill-shaped pillar beside it. Everything rounded, puffy and friendly "
        "with no sharp edges, a faint satin sheen, soft ambient occlusion and a soft diffuse shadow under "
        "each form. Pastel, airy and restful."
    ),
    "bebox": (
        "Scene: a vast, calm, minimalist landscape in soft morning light: a smooth pale warm-grey "
        "plain stretching to a low, distant horizon under a wide, clean sky that fades from pale "
        "blue-grey overhead to warm light at the horizon. Left of centre, set back so they are "
        "modest in the frame with a long soft shadow each across the plain, stand two monumental "
        "sculptures in the BeBox's materials: a huge upright ring like the o of the attached "
        "wordmark (a ring-shaped object, not a letter), matte graphite with a Be blue face and "
        "crisp bevelled edges, and beside it a tall graphite pillar with a single vertical column "
        "of small square LEDs glowing BeOS tab yellow. Far off on the horizon, a few low graphite "
        "blocks. Matte graphite plastic, crisp bevels, smooth gradients, the icons' clean outlines;"
        " no clutter. Serene, spacious and precise, like a classic 1990s computer's desktop picture"
        " made with today's care."
    ),
    "toybox": (
        "Scene: a cheerful still life of hand-made wooden toys on a pale beech-wood tabletop, against a soft "
        "out-of-focus warm cream wall: a big Be blue painted wooden ring like the o of the attached wordmark "
        "(a ring-shaped object, not a letter) stands upright left of centre beside a BeOS-yellow painted "
        "wooden peg; a few small toy blocks, a tomato-red cylinder and a leaf-green disc further back, softly "
        "out of focus. Smooth beech wood in soft satin enamel paint, well-rounded edges, natural end grain "
        "where the wood is unpainted, soft window light from the upper left, a gentle shallow depth of field."
    ),
    "papercraft": (
        "Scene: a landscape folded from matte coloured card: faceted low-poly hills and distant mountains in "
        "Be blue, white card and warm folder tan, a sky of large flat pale blue card panels with a few folded "
        "paper clouds; left of centre, a big faceted Be blue card ring like the o of the attached wordmark (a "
        "ring-shaped object, not a letter) stands on a hill beside a folded BeOS-yellow card pillar. Every "
        "face one flat tone set only by which way it faces the light, like the faceted shading of BeOS R5 "
        "icons; crisp straight folds, a faint paper texture, slightly softened fold edges, soft shadows. "
        "Quiet and orderly."
    ),
    "enamel": (
        "Scene: a single large hard enamel pin shaped like the o of the attached wordmark (a ring-shaped "
        "pin, not a letter), its raised outlines polished gold and its faces smooth, slightly glossy Be blue "
        "enamel, lies left of centre on a deep navy-blue felt cloth with a soft, fine fabric texture, beside "
        "a small BeOS-yellow enamel bar pin; far off to the right, a few tiny enamel pins (a yellow folder, a "
        "red-roofed house) softly out of focus. Soft studio light from the upper left, a gentle sheen on the "
        "gold, deep rich colour, lots of calm navy space."
    ),
    "halftone": (
        "Scene: a bold 1990s computer-magazine cover illustration printed in flat ink: a big Be blue ring "
        "like the o of the attached wordmark (a ring-shaped object, not a letter) with a BeOS-yellow bar "
        "beside it, both with heavy black outlines and halftone-dot shading, stand on a flat horizon left of "
        "centre; behind them a wide sky of flat Be blue with broad radiating sunburst rays in a slightly "
        "lighter blue and a pale yellow halftone glow along the horizon; the cast shadows are coarse "
        "halftone dots. Flat solid inks, a slightly coarse dot screen, no gradients except printed dots, a "
        "crisp printed look."
    ),
    "classic": (
        "Scene: Olo's first look, glossy, bright and saturated: a big glossy Be blue ring like the o of the "
        "attached wordmark (a ring-shaped object, not a letter), with a glossy BeOS-yellow bar beside it, "
        "stands left of centre on a softly reflective pale blue floor that fades into a bright, luminous "
        "sky-blue backdrop; glossy plastic with crisp highlights and soft reflections, a bold dark outline as "
        "in the icons, cheerful bright light from the upper left."
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
    "bebox": "The pillar's LEDs glow brightly in yellow and cast a faint warm light on the plain. ",
    "halftone": "Still flat printed inks and halftone dots: a deep navy night sky, the rays a little lighter navy. ",
    "papercraft": "Still flat card faces, one tone each: a deep navy card sky with a few small folded paper stars. ",
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
