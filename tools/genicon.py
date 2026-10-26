#!/usr/bin/env python3
"""Generate a Tabula icon with OpenAI's image model.

    genicon.py NAME "subject description" [OUT_DIR]

Reads OPENAI_API_KEY from the environment or ~/.env. Writes NAME.png
(1024², transparent) to OUT_DIR (default: data/icons/src).
"""
import base64, json, os, sys, urllib.request

STYLE = (
    "A desktop application icon in the classic BeOS style, refined for a modern high-resolution display. "
    "The object is drawn in a three-quarter isometric view from slightly above, as if sitting on a desk, "
    "with a crisp, confident dark outline around the silhouette, bold saturated colours, clean flat-shaded "
    "faces with a soft gradient on each face, a single light source from the top left, and a subtle soft "
    "contact shadow beneath it. Friendly, tactile, slightly chunky proportions; no text, no letters, no "
    "watermark. Centred with generous transparent padding, completely transparent background, nothing "
    "else in the frame. Subject: "
)

def key():
    k = os.environ.get("OPENAI_API_KEY")
    if k:
        return k
    for line in open(os.path.expanduser("~ben/.env")):
        if line.startswith("OPENAI_API_KEY="):
            return line.split("=", 1)[1].strip().strip('"').strip("'")
    sys.exit("no OPENAI_API_KEY")

def main():
    name, subject = sys.argv[1], sys.argv[2]
    out = sys.argv[3] if len(sys.argv) > 3 else os.path.join(os.path.dirname(__file__), "..", "data", "icons", "src")
    os.makedirs(out, exist_ok=True)
    body = json.dumps({
        "model": os.environ.get("TABULA_IMAGE_MODEL", "gpt-image-2.5-sunburst"),
        "prompt": STYLE + subject,
        "size": "1024x1024",
        "quality": "high",
        "background": "transparent",
        "output_format": "png",
        "n": 1,
    }).encode()
    req = urllib.request.Request("https://api.openai.com/v1/images/generations", body, {
        "Authorization": "Bearer " + key(), "Content-Type": "application/json"})
    try:
        resp = json.load(urllib.request.urlopen(req, timeout=600))
    except urllib.error.HTTPError as e:
        sys.exit(f"{name}: HTTP {e.code}: {e.read().decode()[:500]}")
    data = base64.b64decode(resp["data"][0]["b64_json"])
    path = os.path.join(out, name + ".png")
    open(path, "wb").write(data)
    print(path)

if __name__ == "__main__":
    main()
