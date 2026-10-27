#!/usr/bin/env python3
"""Generate an Olo icon with OpenAI's sunburst image model.

    genicon.py NAME "subject" [--variants N] [--no-ref] [--out DIR]

See docs/ICONS.md for the style guide. Always uses gpt-image-2.5-sunburst
at quality "high" (the maximum); this is a project requirement and not
configurable. The canonical icons in data/icons/reference/ are sent as
style references (images/edits endpoint) so every icon matches the set;
--no-ref falls back to text-only generation (only for bootstrapping a new
reference set).

With --variants N, writes N candidates to data/icons/src/candidates/
NAME-1.png … for review with tools/iconsheet.py; promote the chosen one
with `mv data/icons/src/candidates/NAME-2.png data/icons/src/NAME.png`.
Otherwise writes data/icons/src/NAME.png. Each output gets a .json
sidecar recording model, quality, prompt and references.

Reads OPENAI_API_KEY from the environment or ~ben/.env. Never print it.
"""
import argparse, base64, glob, json, os, sys, time, urllib.error, urllib.request, uuid

MODEL = "gpt-image-2.5-sunburst"
QUALITY = "high"
SIZE = "1024x1024"

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "icons")

# The house style. Keep in sync with docs/ICONS.md ("Style").
STYLE = (
    "A desktop application icon in the classic BeOS style, refined for a modern high-resolution display. "
    "The object is drawn in a three-quarter isometric view from slightly above, as if sitting on a desk, "
    "with a crisp, confident dark outline around the silhouette, bold saturated colours, clean flat-shaded "
    "faces with a soft gradient on each face, a single light source from the top left, and a subtle soft "
    "contact shadow beneath it. Friendly, tactile, slightly chunky proportions; no text, no letters, no "
    "watermark. Centred with generous transparent padding, completely transparent background, nothing "
    "else in the frame. "
)
REFERENCE = (
    "The attached images are reference icons from the same icon set. Match their style exactly: the same "
    "viewing angle, outline weight, shading, saturation, lighting direction, level of detail and padding. "
    "Do not copy their subjects or include them in the image; draw only the new subject. "
)


def key():
    k = os.environ.get("OPENAI_API_KEY")
    if k:
        return k
    for line in open(os.path.expanduser("~ben/.env")):
        if line.startswith("OPENAI_API_KEY="):
            return line.split("=", 1)[1].strip().strip('"').strip("'")
    sys.exit("no OPENAI_API_KEY")


def multipart(fields, files):
    boundary = uuid.uuid4().hex
    parts = []
    for k, v in fields.items():
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode())
    for k, path in files:
        data = open(path, "rb").read()
        head = (f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"; '
                f'filename="{os.path.basename(path)}"\r\nContent-Type: image/png\r\n\r\n')
        parts.append(head.encode() + data + b"\r\n")
    parts.append(f"--{boundary}--\r\n".encode())
    return b"".join(parts), f"multipart/form-data; boundary={boundary}"


def generate(prompt, refs, n):
    common = {"model": MODEL, "prompt": prompt, "size": SIZE, "quality": QUALITY,
              "background": "transparent", "output_format": "png", "n": n}
    if refs:
        body, ctype = multipart({k: str(v) for k, v in common.items()}, [("image[]", r) for r in refs])
        url = "https://api.openai.com/v1/images/edits"
    else:
        body, ctype = json.dumps(common).encode(), "application/json"
        url = "https://api.openai.com/v1/images/generations"
    req = urllib.request.Request(url, body, {"Authorization": "Bearer " + key(), "Content-Type": ctype})
    for attempt in range(3):
        try:
            return [base64.b64decode(d["b64_json"]) for d in json.load(urllib.request.urlopen(req, timeout=900))["data"]]
        except urllib.error.HTTPError as e:
            msg = e.read().decode()[:500]
            if e.code < 500 and e.code != 429:
                sys.exit(f"HTTP {e.code}: {msg}")
            time.sleep(10 * (attempt + 1))
    sys.exit("image API kept failing")


def main():
    if os.environ.get("OLO_IMAGE_MODEL") not in (None, MODEL):
        sys.exit(f"icons must be generated with {MODEL} (see docs/ICONS.md)")
    ap = argparse.ArgumentParser()
    ap.add_argument("name")
    ap.add_argument("subject")
    ap.add_argument("--variants", type=int, default=1)
    ap.add_argument("--no-ref", action="store_true")
    ap.add_argument("--out", default=os.path.join(ROOT, "src"))
    a = ap.parse_args()

    refs = [] if a.no_ref else sorted(glob.glob(os.path.join(ROOT, "reference", "*.png")))
    refs = [r for r in refs if os.path.basename(r)[:-4] != a.name]  # never reference itself
    prompt = STYLE + (REFERENCE if refs else "") + "Subject: " + a.subject
    images = generate(prompt, refs, a.variants)

    out = os.path.join(a.out, "candidates") if a.variants > 1 else a.out
    os.makedirs(out, exist_ok=True)
    for i, data in enumerate(images, 1):
        stem = f"{a.name}-{i}" if a.variants > 1 else a.name
        path = os.path.join(out, stem + ".png")
        open(path, "wb").write(data)
        json.dump({"model": MODEL, "quality": QUALITY, "size": SIZE, "subject": a.subject, "prompt": prompt,
                   "references": [os.path.basename(r) for r in refs],
                   "created": time.strftime("%Y-%m-%dT%H:%M:%S")},
                  open(os.path.join(out, stem + ".json"), "w"), indent=1)
        print(path)


if __name__ == "__main__":
    main()
