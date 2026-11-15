"""Finish the pack's upscaled wallpapers with an ESRGAN-family model
(4x-UltraSharp), on the NVIDIA GPU when there is one (fp32: the CPU's
arithmetic, faster). Ben, 2026-11-15: "they have visible pixellation,
bluriness in places … figure out how to make them all perfect".

  finishwallpaper.py picture IN.png OUT.png   a render: edges from the model (from a 1280-wide
                                              base: stair-steps under a pixel), flat areas as they are
  finishwallpaper.py photo IN.png OUT.png     a photograph: the model's from a 1920-wide base
  finishwallpaper.py tile IN.png OUT.png      a small tile: its 3x3 mosaic 4x, the middle cut out
  finishwallpaper.py batch LIST               lines "picture|photo|tile<TAB>IN<TAB>OUT", one model load

Each is supersampled (made at 4x, brought down) and quantised to 8 bits
with a fine dither, so deep gradients don't band. Runs in its own Python
(see "Theme wallpapers" in docs/ICONS.md):

  uv venv --python 3.12 /var/tmp/olo-sr
  uv pip install --python /var/tmp/olo-sr/bin/python --index-url https://download.pytorch.org/whl/cu128 torch torchvision
  uv pip install --python /var/tmp/olo-sr/bin/python spandrel numpy pillow
  OLO_SR_MODEL=…/4x-UltraSharp.pth /var/tmp/olo-sr/bin/python tools/finishwallpaper.py batch LIST
"""
import os, sys, time

import numpy as np
import torch
from PIL import Image
from spandrel import ModelLoader

MODEL = os.environ.get("OLO_SR_MODEL", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "wallpapers", "src", "models", "4x-UltraSharp.pth"))
DEV = torch.device("cuda" if torch.cuda.is_available() else "cpu")
_model = None


def model():
    global _model
    if _model is None:
        # (fp32 on the GPU too: the CPU's arithmetic, only faster.)
        _model = ModelLoader().load_from_file(MODEL).model.eval().to(DEV)
    return _model


def sr(img, tile=512, pad=24):
    """4x of an RGB uint8 array, in padded tiles."""
    m = model()
    a = img.astype(np.float32) / 255.0
    h, w, _ = a.shape
    out = np.zeros((h * 4, w * 4, 3), dtype=np.float32)
    with torch.inference_mode():
        for y in range(0, h, tile):
            for x in range(0, w, tile):
                y0, x0, y1, x1 = max(0, y - pad), max(0, x - pad), min(h, y + tile + pad), min(w, x + tile + pad)
                t = torch.from_numpy(a[y0:y1, x0:x1].transpose(2, 0, 1).copy()).unsqueeze(0).to(DEV)
                up = m(t).float().clamp(0, 1)[0].cpu().numpy().transpose(1, 2, 0)
                ty, tx = (y - y0) * 4, (x - x0) * 4
                th, tw = (min(y + tile, h) - y) * 4, (min(x + tile, w) - x) * 4
                out[y * 4:y * 4 + th, x * 4:x * 4 + tw] = up[ty:ty + th, tx:tx + tw]
    return out * 255.0


def quantise(a, seed=1):
    rng = np.random.default_rng(seed)
    d = (rng.random(a.shape[:2]) - rng.random(a.shape[:2]))[..., None] * 0.5
    return np.clip(np.round(a + d), 0, 255).astype(np.uint8)


def box(a, r, axis):
    if r <= 0:
        return a
    pad = [(0, 0)] * a.ndim
    pad[axis] = (r + 1, r)
    c = np.cumsum(np.pad(a, pad, mode="edge"), axis=axis, dtype=np.float64)
    n = a.shape[axis]
    return ((np.take(c, np.arange(2 * r + 1, 2 * r + 1 + n), axis=axis) - np.take(c, np.arange(n), axis=axis)) / (2 * r + 1)).astype(np.float32)


def blur(a, sigma):
    r = max(1, int(round((np.sqrt(4 * sigma * sigma + 1) - 1) / 2)))
    for axis in (0, 1):
        for _ in range(3):
            a = box(a, r, axis)
    return a


def grow(m, r):
    """The largest value within r pixels (a square), by shifts."""
    out = m.copy()
    for axis in (0, 1):
        acc = out.copy()
        for d in range(1, r + 1):
            acc = np.maximum(acc, np.roll(out, d, axis=axis))
            acc = np.maximum(acc, np.roll(out, -d, axis=axis))
        out = acc
    return out


def picture(src, out, base_w=None, photo=False):
    """A 4K picture finished. A render (smooth forms, gradients, lettering):
    the model's from a 1280-wide base (its stair-steps under a pixel, so
    edges come out clean) where there are edges, the picture's own smooth
    gradients elsewhere (the model draws a faint canvas into flat areas).
    A photograph: the model's from a 1920-wide base everywhere."""
    im = Image.open(src).convert("RGB")
    W, H = im.size
    bw = base_w or (W // 2 if photo else 1280)
    base = np.asarray(im.resize((bw, round(H * bw / W)), Image.LANCZOS))
    big = sr(base)
    big_im = Image.fromarray(np.clip(np.round(big), 0, 255).astype(np.uint8))
    model_out = np.asarray(big_im.resize((W, H), Image.LANCZOS), dtype=np.float32)
    if photo:
        final = model_out
    else:
        orig = np.asarray(im, dtype=np.float32)
        luma = blur(model_out.mean(axis=2), 1.0)
        gy, gx = np.gradient(luma)
        g = np.hypot(gx, gy)
        # (Edges: more than 2.5 levels a pixel; the canvas is under that.)
        m = np.clip((g - 1.5) / 2.5, 0, 1)
        m = blur(grow(m, 6), 4.0)[..., None]
        final = orig + m * (model_out - orig)
    Image.fromarray(quantise(final)).save(out)


def tile(src, out):
    t = Image.open(src).convert("RGB")
    w, h = t.size
    mosaic = Image.new("RGB", (3 * w, 3 * h))
    for y in range(3):
        for x in range(3):
            mosaic.paste(t, (x * w, y * h))
    big = sr(np.asarray(mosaic))
    mid = big[4 * h:8 * h, 4 * w:8 * w]
    Image.fromarray(quantise(mid)).save(out)


def main():
    t0 = time.time()
    if sys.argv[1] == "batch":
        for line in open(sys.argv[2]):
            if not line.strip():
                continue
            kind, src, out = line.rstrip("\n").split("\t")
            if os.path.exists(out) and os.path.getmtime(out) > os.path.getmtime(src):
                continue
            s = time.time()
            if kind == "tile":
                tile(src, out)
            else:
                picture(src, out, photo=kind == "photo")
            print(f"{out} {time.time() - s:.1f} s", flush=True)
    else:
        if sys.argv[1] in ("picture", "photo"):
            picture(sys.argv[2], sys.argv[3], int(sys.argv[4]) if len(sys.argv) > 4 else None, photo=sys.argv[1] == "photo")
        else:
            tile(sys.argv[2], sys.argv[3])
        print(sys.argv[3])
    print(f"done on {DEV} in {time.time() - t0:.0f} s")


if __name__ == "__main__":
    main()
