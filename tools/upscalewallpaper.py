#!/usr/bin/env python3
"""Make a classic desktop picture a 4K one with OpenAI's sunburst image
model at its maximum quality: the pack's Mac OS 9 pictures (Ben,
2026-11-15: "upscale all the macos 9 ones and use them for both
themes").

    upscalewallpaper.py SRC NAME            draw data/wallpapers/src/NAME.png from SRC
    upscalewallpaper.py SRC NAME --dark     the same picture for dark mode (NAME-dark.png)
    upscalewallpaper.py --promote NAME [--dark] [--crop-y PX]
    upscalewallpaper.py --promote NAME --jpeg       a retro collection's: 4k/NAME.webp, no ultrawide
    upscalewallpaper.py SRC NAME --tile             a desktop pattern's tile, made larger and seamless
    upscalewallpaper.py --promote NAME --tile       tiles/NAME.webp and its tiled thumbnail

The original (4:3, 1024x768 for most of Mac OS 9's) goes to the edits
endpoint with a prompt to make exactly that picture again at 3840x2160,
widened to 16:9 by continuing its background, adding nothing. --dark
redraws a 4K one for dark mode. Each candidate has a .json sidecar (the
model, the prompt, its source). --promote makes the pack's files from
one, as genwallpaper.py's does: 4k/NAME.webp, the 3840x1648 ultrawide band
(from --crop-y, default the middle) and the 320x180 thumbnail. Then give
it its HDR map (`cargo run --release -p olo-images --bin wallpaper-hdr`)
and a line in pack.txt.

Reads OPENAI_API_KEY from the environment or ~ben/.env. Never print it.
"""
import argparse, base64, json, os, sys, time, urllib.error, urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import imagemodel  # noqa: E402  (the key, multipart and the model)
import genwallpaper  # noqa: E402  (promote)

SRC = os.path.join(genwallpaper.WALLPAPERS, "src")

UPSCALE = (
    "The attached image is a desktop picture from a 1999 computer, at its original low resolution. Make it again "
    "as a 4K desktop picture, 3840x2160: exactly the same picture, the same composition, forms, colours, "
    "lighting, rendering and mood, every element where it is, faithful to the original in every way, only at "
    "full 4K resolution with crisp, clean, beautifully finished detail and smooth gradients, no JPEG artefacts, "
    "no noise and no banding. The original is 4:3 and this is 16:9: keep the original picture whole in the "
    "middle at its size, and extend it at the left and right by continuing its background, gradients and "
    "patterns seamlessly, as the original's artist would have, adding no new objects or focal points. "
    "Any lettering, logos or words in it stay exactly as they are: the same words, spelling, typeface, size "
    "and place, crisp; add none. No watermark, no frame or border."
)
DARK = (
    "The attached image is a 4K desktop picture. Draw exactly the same picture again for a computer's dark "
    "mode: the same composition, forms, rendering and style, every element where it is, but deep and low in "
    "overall brightness, with rich, deepened colour and the forms still gently lit and separated. "
    "No text, no logos, no watermark."
)


TILE = (
    "The attached image is a seamless repeating desktop pattern from a 1990s computer at its original low "
    "resolution, shown as a grid of {nx} columns and {ny} rows of the same tile. Make it again at {w}x{h}: exactly "
    "the same pattern, the same grid of identical tiles, the same motifs, colours, shading and texture, every tile "
    "identical to the others so the pattern still repeats seamlessly across their edges, only at a high "
    "resolution with crisp, clean, finely finished detail, no JPEG artefacts and no noise. Any lettering "
    "stays exactly as it is. Add, remove or rearrange nothing; no border, no gaps between the tiles."
)

# A tile's upscale: at least 4x (sharp at 4x the scale it had), its 3x3
# mosaic at least 1024 pixels (the model's least), the tile at most 1024
# (a big one, a scene, only a little larger).
def tile_scale(w, h):
    side = max(w, h)
    return min(max(4.0, 1024 / (3 * side)), 1024 / side)


# Tiles this small (pixels on their long side) are made larger with a
# filter, not the model.
SMALL_TILE = 32


def r16(v):
    return max(16, int(round(v / 16.0)) * 16)


# What the image model takes: at most this many pixels, this long a side,
# and 3:1 (a strip of a tile in a 3 by 3 grid was refused: 192x3072).
BUDGET = 8_000_000
MOST_SIDE = 3840


def grid(w, h):
    """How many tiles across and down the mosaic shows: 3 by 3 at least,
    more along a strip's short side, until it's within 2:1."""
    nx = ny = 3
    while nx * w * 2 < ny * h:
        nx += 1
    while ny * h * 2 < nx * w:
        ny += 1
    return nx, ny


def edit(prompt, image, size):
    fields = {"model": imagemodel.MODEL, "prompt": prompt, "size": size, "quality": imagemodel.QUALITY,
              "background": "opaque", "output_format": "png", "n": "1"}
    body, ctype = imagemodel.multipart(fields, [("image[]", image)])
    req = urllib.request.Request("https://api.openai.com/v1/images/edits", body,
                                 {"Authorization": "Bearer " + imagemodel.key(), "Content-Type": ctype})
    for attempt in range(4):
        try:
            return base64.b64decode(json.load(urllib.request.urlopen(req, timeout=1800))["data"][0]["b64_json"])
        except urllib.error.HTTPError as e:
            msg = e.read().decode()[:500]
            if e.code < 500 and e.code != 429:
                sys.exit(f"{image}: HTTP {e.code}: {msg}")
            time.sleep(20 * (attempt + 1))
    sys.exit(f"{image}: the image API kept failing")


def draw_tile(src, name):
    """A tile made larger: a mosaic of it (3 by 3, more along a strip's
    short side: `grid`) drawn again, its middle
    tile cut out and its edges cross-faded with the tiles beside it, so
    it repeats without a seam."""
    from PIL import Image
    os.makedirs(SRC, exist_ok=True)
    t = Image.open(src).convert("RGB")
    w, h = t.size
    if max(w, h) <= SMALL_TILE:
        # (Too few pixels for the model to keep: stripes doubled, pebbles
        # made up. Made larger with a fine filter, as it is.)
        k = max(4, 128 / max(w, h))
        out = os.path.join(SRC, name + ".png")
        t.resize((round(w * k), round(h * k)), Image.LANCZOS).save(out)
        json.dump({"tile": [w, h], "source": os.path.basename(src), "filter": "lanczos", "created": time.strftime("%Y-%m-%dT%H:%M:%S")},
                  open(out[:-4] + ".json", "w"), indent=1)
        print(out, f"tile:{w}")
        return
    nx, ny = grid(w, h)
    # (The mosaic within the model's budget: a 256-point tile 4x in a 3 by
    # 3 grid, 3072 square, was refused; at 960 it's still more than a 2x
    # display needs.)
    k = min(tile_scale(w, h), (BUDGET / (nx * w * ny * h)) ** 0.5, MOST_SIDE / (nx * w), MOST_SIDE / (ny * h))
    tw, th = max(16, int(w * k) // 16 * 16), max(16, int(h * k) // 16 * 16)
    mosaic = Image.new("RGB", (nx * w, ny * h))
    for y in range(ny):
        for x in range(nx):
            mosaic.paste(t, (x * w, y * h))
    out = os.path.join(SRC, name + ".png")
    tmp = out + ".in.png"
    mosaic.save(tmp)
    size = f"{nx * tw}x{ny * th}"
    prompt = TILE.format(w=nx * tw, h=ny * th, nx=nx, ny=ny)
    try:
        data = edit(prompt, tmp, size)
    finally:
        os.remove(tmp)
    import io
    big = Image.open(io.BytesIO(data)).convert("RGB").resize((nx * tw, ny * th), Image.LANCZOS)
    cell = lambda cx, cy: big.crop((cx * tw, cy * th, (cx + 1) * tw, (cy + 1) * th))
    # The middle one (tiles all round it), and those right of it and under it.
    mx, my = nx // 2, ny // 2
    tile = cell(mx, my)
    # Its left band from the tile on its right (which continues its right
    # edge in the mosaic), fading to its own; its top band likewise from
    # the tile under it.
    band = max(4, tw // 8)
    right, below = cell(mx + 1, my), cell(mx, my + 1)
    px, rp = tile.load(), right.load()
    for x in range(band):
        a = x / band
        for y in range(th):
            p, q = px[x, y], rp[x, y]
            px[x, y] = tuple(round(q[i] * (1 - a) + p[i] * a) for i in range(3))
    band = max(4, th // 8)
    bp = below.load()
    for y in range(band):
        a = y / band
        for x in range(tw):
            p, q = px[x, y], bp[x, y]
            px[x, y] = tuple(round(q[i] * (1 - a) + p[i] * a) for i in range(3))
    tile.save(out)
    json.dump({"model": imagemodel.MODEL, "quality": imagemodel.QUALITY, "size": size, "source": os.path.basename(src),
               "tile": [w, h], "prompt": prompt, "created": time.strftime("%Y-%m-%dT%H:%M:%S")},
              open(out[:-4] + ".json", "w"), indent=1)
    print(out, f"tile:{w}")


def promote_jpeg(name, dark=False):
    """A retro collection's picture: 4k/NAME.webp (lossless) and its
    thumbnail; no ultrawide version (the 4K one is cropped to the
    screen). (Named for the JPEG it was until November 2026.)"""
    from PIL import Image
    stem = name + ("-dark" if dark else "")
    img = Image.open(os.path.join(SRC, stem + ".png")).convert("RGB")
    if img.size != genwallpaper.SIZE:
        sys.exit(f"{stem}: {img.size}, not {genwallpaper.SIZE}")
    path = genwallpaper.save_picture(img, "4k", stem)
    img.resize(genwallpaper.THUMB, Image.LANCZOS).save(os.path.join(genwallpaper.WALLPAPERS, "thumbs", stem + ".png"), optimize=True)
    print(path)


def promote_tile(name):
    """tiles/NAME.webp and its preview in thumbs/ (the pattern repeated at
    a quarter of its points: as a 1280-point screen shrunk to 320)."""
    from PIL import Image
    src = Image.open(os.path.join(SRC, name + ".png")).convert("RGB")
    meta = json.load(open(os.path.join(SRC, name + ".json")))
    w, h = meta["tile"]
    os.makedirs(os.path.join(genwallpaper.WALLPAPERS, "tiles"), exist_ok=True)
    genwallpaper.save_picture(src, "tiles", name)
    pw = max(8, round(w * 0.25 * 2))
    ph = max(8, round(pw * src.height / src.width))
    small = src.resize((pw, ph), Image.LANCZOS)
    thumb = Image.new("RGB", genwallpaper.THUMB)
    for y in range(0, thumb.height, ph):
        for x in range(0, thumb.width, pw):
            thumb.paste(small, (x, y))
    thumb.save(os.path.join(genwallpaper.WALLPAPERS, "thumbs", name + ".png"), optimize=True)
    print(name, f"tile:{w}")


def draw(src, name, dark):
    from PIL import Image
    os.makedirs(SRC, exist_ok=True)
    out = os.path.join(SRC, name + ("-dark" if dark else "") + ".png")
    tmp = out + ".in.png"
    Image.open(src).convert("RGB").save(tmp)
    prompt = DARK if dark else UPSCALE
    fields = {"model": imagemodel.MODEL, "prompt": prompt, "size": "3840x2160", "quality": imagemodel.QUALITY,
              "background": "opaque", "output_format": "png", "n": "1"}
    body, ctype = imagemodel.multipart(fields, [("image[]", tmp)])
    req = urllib.request.Request("https://api.openai.com/v1/images/edits", body,
                                 {"Authorization": "Bearer " + imagemodel.key(), "Content-Type": ctype})
    try:
        for attempt in range(4):
            try:
                data = json.load(urllib.request.urlopen(req, timeout=1800))["data"][0]["b64_json"]
                break
            except urllib.error.HTTPError as e:
                msg = e.read().decode()[:500]
                if e.code < 500 and e.code != 429:
                    sys.exit(f"{src}: HTTP {e.code}: {msg}")
                time.sleep(20 * (attempt + 1))
        else:
            sys.exit(f"{src}: the image API kept failing")
    finally:
        os.remove(tmp)
    open(out, "wb").write(base64.b64decode(data))
    json.dump({"model": imagemodel.MODEL, "quality": imagemodel.QUALITY, "size": "3840x2160", "source": os.path.basename(src),
               "prompt": prompt, "created": time.strftime("%Y-%m-%dT%H:%M:%S")}, open(out[:-4] + ".json", "w"), indent=1)
    print(out)


def main():
    if os.environ.get("OLO_IMAGE_MODEL") not in (None, imagemodel.MODEL):
        sys.exit(f"wallpapers must be made with {imagemodel.MODEL} (see docs/ICONS.md)")
    ap = argparse.ArgumentParser()
    ap.add_argument("src", nargs="?")
    ap.add_argument("name", nargs="?")
    ap.add_argument("--dark", action="store_true")
    ap.add_argument("--tile", action="store_true", help="SRC is a desktop pattern's tile")
    ap.add_argument("--jpeg", action="store_true", help="with --promote: a retro collection's picture, 4K only (lossless WebP)")
    ap.add_argument("--promote", metavar="NAME")
    ap.add_argument("--crop-y", type=int)
    a = ap.parse_args()
    if a.promote and a.tile:
        promote_tile(a.promote)
    elif a.promote and a.jpeg:
        promote_jpeg(a.promote, a.dark)
    elif a.promote:
        src = os.path.join(SRC, a.promote + ("-dark" if a.dark else "") + ".png")
        genwallpaper.promote(a.promote, src, a.dark, a.crop_y)
    elif a.src and a.name and a.tile:
        draw_tile(a.src, a.name)
    elif a.src and a.name:
        draw(a.src, a.name, a.dark)
    else:
        ap.error("SRC NAME, or --promote NAME")


if __name__ == "__main__":
    main()
