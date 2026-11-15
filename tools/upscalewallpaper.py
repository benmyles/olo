#!/usr/bin/env python3
"""Make a classic desktop picture a 4K one with OpenAI's sunburst image
model at its maximum quality: the pack's Mac OS 9 pictures (Ben,
2026-11-15: "upscale all the macos 9 ones and use them for both
themes").

    upscalewallpaper.py SRC NAME            draw data/wallpapers/src/NAME.png from SRC
    upscalewallpaper.py SRC NAME --dark     the same picture for dark mode (NAME-dark.png)
    upscalewallpaper.py --promote NAME [--dark] [--crop-y PX]

The original (4:3, 1024x768 for most of Mac OS 9's) goes to the edits
endpoint with a prompt to make exactly that picture again at 3840x2160,
widened to 16:9 by continuing its background, adding nothing. --dark
redraws a 4K one for dark mode. Each candidate has a .json sidecar (the
model, the prompt, its source). --promote makes the pack's files from
one, as genwallpaper.py's does: 4k/NAME.png, the 3840x1648 ultrawide band
(from --crop-y, default the middle) and the 320x180 thumbnail. Then give
it its HDR map (`cargo run --release -p olo-images --bin wallpaper-hdr`)
and a line in pack.txt.

Reads OPENAI_API_KEY from the environment or ~ben/.env. Never print it.
"""
import argparse, base64, json, os, sys, time, urllib.error, urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import genicon  # noqa: E402  (the key, multipart and the model)
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
    "No text, no logos, no watermark, no frame or border."
)
DARK = (
    "The attached image is a 4K desktop picture. Draw exactly the same picture again for a computer's dark "
    "mode: the same composition, forms, rendering and style, every element where it is, but deep and low in "
    "overall brightness, with rich, deepened colour and the forms still gently lit and separated. "
    "No text, no logos, no watermark."
)


def draw(src, name, dark):
    from PIL import Image
    os.makedirs(SRC, exist_ok=True)
    out = os.path.join(SRC, name + ("-dark" if dark else "") + ".png")
    tmp = out + ".in.png"
    Image.open(src).convert("RGB").save(tmp)
    prompt = DARK if dark else UPSCALE
    fields = {"model": genicon.MODEL, "prompt": prompt, "size": "3840x2160", "quality": genicon.QUALITY,
              "background": "opaque", "output_format": "png", "n": "1"}
    body, ctype = genicon.multipart(fields, [("image[]", tmp)])
    req = urllib.request.Request("https://api.openai.com/v1/images/edits", body,
                                 {"Authorization": "Bearer " + genicon.key(), "Content-Type": ctype})
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
    json.dump({"model": genicon.MODEL, "quality": genicon.QUALITY, "size": "3840x2160", "source": os.path.basename(src),
               "prompt": prompt, "created": time.strftime("%Y-%m-%dT%H:%M:%S")}, open(out[:-4] + ".json", "w"), indent=1)
    print(out)


def main():
    if os.environ.get("OLO_IMAGE_MODEL") not in (None, genicon.MODEL):
        sys.exit(f"wallpapers must be made with {genicon.MODEL} (see docs/ICONS.md)")
    ap = argparse.ArgumentParser()
    ap.add_argument("src", nargs="?")
    ap.add_argument("name", nargs="?")
    ap.add_argument("--dark", action="store_true")
    ap.add_argument("--promote", metavar="NAME")
    ap.add_argument("--crop-y", type=int)
    a = ap.parse_args()
    if a.promote:
        src = os.path.join(SRC, a.promote + ("-dark" if a.dark else "") + ".png")
        genwallpaper.promote(a.promote, src, a.dark, a.crop_y)
    elif a.src and a.name:
        draw(a.src, a.name, a.dark)
    else:
        ap.error("SRC NAME, or --promote NAME")


if __name__ == "__main__":
    main()
