#!/usr/bin/env python3
"""Restore the pack's wallpapers and keep them lossless.

    restorewallpaper.py pack [--report FILE]     every pack picture still PNG or JPEG (4k/,
                                                 ultrawide/, tiles/): restored, saved beside it as
                                                 lossless WebP, the old file taken away
    restorewallpaper.py restore IN OUT [--tile]  one picture (OUT.webp or .png)
    restorewallpaper.py check PICTURE…           each picture's flaws: smooth share, bands,
                                                 texture, JPEG blocks (see `flaws`)

Ben, 2026-11-16: "i can still see like color banding and stuff"; "fix and
restore any visual issues you find in all of the wallpapers with whatever
approaches work best"; "zero visual artifacts". What it takes out, and
only where a picture is smooth (texture is the picture's own, kept to the
pixel): an image model's 8-bit bands and its decoder's cross-hatch and
blotches, and JPEG's blocks and ringing. How: docs/ICONS.md, "Restored,
and lossless".

One pass on the GPU for each picture, nothing brought back to the
processor between steps: the smooth mask, FBCNN (blind JPEG restoration)
on the 256-pixel tiles that have a sharp edge beside smooth ground (where
JPEG rings: a hundredth of a picture's pixels, in under half its tiles;
the network is 45 TFLOP for a 4K picture, four seconds of this laptop's
GPU flat out, and away from edges its result measured the same as the
polish alone), its result held to
the file's own DCT coefficient intervals, the polish (guided filters, 2
to 100 pixels), a dither, and the flaws before and after. Decoding runs
ahead on threads and the lossless WebP encoding (three seconds of a core
for a 4K picture) on every core, so the GPU never
waits for either.

Runs in the GPU's own Python (finishwallpaper.py's: torch, spandrel,
numpy, pillow); FBCNN's colour model is data/wallpapers/src/models/
fbcnn_color.pth, from github.com/jiaxi-jiang/FBCNN's v1.0 release.
"""
import glob, math, multiprocessing, os, sys, threading, time
from concurrent.futures import ThreadPoolExecutor

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
PACK = os.path.normpath(os.path.join(HERE, "..", "data", "wallpapers"))
JPEG_MODEL = os.environ.get("OLO_JPEG_MODEL", os.path.join(PACK, "src", "models", "fbcnn_color.pth"))

# The polish's passes, finest first: (radius in pixels, eps in levels²).
# The fine ones take out an image model's decoder texture (a cross-hatch
# and colour blotches a level or two deep) and JPEG's blocks; the wide
# ones turn 8-bit steps in slow gradients (bands 50 to 300 pixels wide in
# a dark sky) back into the ramps they stand for.
POLISH = ((2, 6.0), (6, 4.0), (16, 3.0), (40, 2.0), (100, 1.5))
# No pixel moves further than this (levels): noise and steps go, never a
# picture's own detail.
POLISH_LIMIT = 4.0
# Where a picture is smooth enough to restore: luma's spread in a 7×7
# window, under SMOOTH_LO levels all restored, over SMOOTH_HI kept as it
# is (a photograph's grain, a stone's sparkle), blended between.
SMOOTH_LO, SMOOTH_HI = 1.5, 3.5
# A sharp edge: a channel changing this much across two pixels. JPEG
# rings in the 8×8 blocks an edge crosses: this far from it, and a margin.
EDGE, RING = 32.0, 12
# The network's tiles: this much of each kept, this much context around
# it, this many at a time; and a tile is worth the network when this
# share of it is smooth ground by an edge (a photograph's every tile has
# a speck of it: 127 of a granite's 135 tiles, for a five-hundredth of
# its pixels).
TILE, PAD, BATCH = 256, 32, 8
WORTH = 0.01
# This few colours is pixel art (a pattern's pixels are the design): kept
# exactly.
PALETTE = 256
# How far the polish reaches (each pass's two box means), for a tile's
# mosaic: its middle sees neighbours this far.
REACH = 2 * sum(r for r, _ in POLISH) + 16

_torch = None
_dev = None
_net = None


def T():
    """torch, loaded when first needed (the encoding processes never do)."""
    global _torch, _dev
    if _torch is None:
        import torch

        _torch = torch
        _dev = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    return _torch


def box(x, r, dim):
    """A box mean of radius r along `dim` (edges repeated), by running
    sums. In float64: a row's running sum of squares reaches 10⁸, and the
    filter measures spreads of a level (float32 keeps 7 digits)."""
    torch = T()
    n = x.shape[dim]
    sizes = list(x.shape)
    sizes[dim] = r + 1
    head = x.narrow(dim, 0, 1).expand(*sizes)
    sizes[dim] = r
    tail = x.narrow(dim, n - 1, 1).expand(*sizes)
    c = torch.cumsum(torch.cat([head, x, tail], dim), dim)
    return (c.narrow(dim, 2 * r + 1, n) - c.narrow(dim, 0, n)) / (2 * r + 1)


def mean(x, r):
    return box(box(x, r, 0), r, 1)


def guided(a, r, eps):
    """He's guided filter of each channel by itself: where a window
    varies less than eps (noise, a step between levels) its mean; where
    it varies much more (an edge, a texture) the pixel as it is."""
    m = mean(a, r)
    var = (mean(a * a, r) - m * m).clamp_min(0.0)
    k = var / (var + eps)
    return mean(k, r) * a + mean(m - k * m, r)


def polished(x):
    torch = T()
    y = x
    for r, eps in POLISH:
        y = guided(y, r, eps)
    return torch.minimum(torch.maximum(y, x - POLISH_LIMIT), x + POLISH_LIMIT)


def luma(a):
    return a @ T().tensor([0.2126, 0.7152, 0.0722], dtype=a.dtype, device=a.device)


def smooth_mask(a):
    """How much of each pixel the restoration may change (0 to 1): flaws
    show only where a picture is smooth, and texture elsewhere is its own
    detail. A small window, so it reaches to a few pixels from an edge;
    feathered, so where it starts and stops can't be seen."""
    y = luma(a)
    m = mean(y, 3)
    spread = (mean(y * y, 3) - m * m).clamp_min(0).sqrt()
    k = ((SMOOTH_HI - spread) / (SMOOTH_HI - SMOOTH_LO)).clamp(0, 1)
    return mean(mean(k, 1), 1)


def edge_zone(a):
    """Where JPEG rings (0 to 1): all of it within RING pixels of a sharp
    edge, none 16 pixels further."""
    torch = T()
    F = torch.nn.functional
    h, w, _ = a.shape
    d = torch.zeros(h, w, dtype=a.dtype, device=a.device)
    d[:, 1:-1] = (a[:, 2:] - a[:, :-2]).abs().amax(-1)
    d[1:-1, :] = torch.maximum(d[1:-1, :], (a[2:] - a[:-2]).abs().amax(-1))
    z = F.max_pool2d((d > EDGE).to(a.dtype)[None, None], 2 * (RING + 8) + 1, 1, RING + 8)
    for _ in range(2):
        z = F.avg_pool2d(z, 9, 1, 4, count_include_pad=False)
    return z[0, 0]


def network():
    global _net
    if _net is None:
        from spandrel import ModelLoader

        _net = ModelLoader().load_from_file(JPEG_MODEL).to(_dev).eval()
    return _net


def fbcnn(a, need):
    """`a` (H×W×3 float32, 0 to 255) with the network's restoration in
    the tiles that hold enough of `need` (WORTH); how many tiles that
    was."""
    torch = T()
    F = torch.nn.functional
    h, w, _ = a.shape
    hc, wc = math.ceil(h / TILE) * TILE, math.ceil(w / TILE) * TILE
    grid = F.avg_pool2d(F.pad(need.float()[None, None], (0, wc - w, 0, hc - h)), TILE, TILE)[0, 0] >= WORTH
    wanted = grid.nonzero().tolist()
    if not wanted:
        return a, 0
    net = network()
    x = F.pad((a / 255).permute(2, 0, 1)[None], (PAD, PAD + wc - w, PAD, PAD + hc - h), mode="reflect")[0]
    out = a.clone()
    side = TILE + 2 * PAD
    with torch.inference_mode():
        for s in range(0, len(wanted), BATCH):
            part = wanted[s:s + BATCH]
            tiles = torch.stack([x[:, i * TILE:i * TILE + side, j * TILE:j * TILE + side] for i, j in part])
            core = net(tiles)[:, :, PAD:PAD + TILE, PAD:PAD + TILE].clamp(0, 1) * 255
            for (i, j), c in zip(part, core):
                y0, x0 = i * TILE, j * TILE
                th, tw = min(TILE, h - y0), min(TILE, w - x0)
                out[y0:y0 + th, x0:x0 + tw] = c[:, :th, :tw].permute(1, 2, 0)
    return out, len(wanted)


# JFIF's colour transform (full range), as libjpeg makes and reads it.
TO_YCC = ((0.299, 0.587, 0.114), (-0.168736, -0.331264, 0.5), (0.5, -0.418688, -0.081312))


def dct8():
    """The orthonormal 8-point DCT-II: JPEG's 2D transform is D·f·Dᵀ."""
    return [[(math.sqrt(1 / 8) if k == 0 else math.sqrt(2 / 8)) * math.cos(math.pi * (2 * n + 1) * k / 16) for n in range(8)] for k in range(8)]


def project(net, a, tables):
    """`net` made what the JPEG allows: each 8×8 block's DCT coefficients
    held within the interval the file's quantised value stands for
    (q·(c ± ½), c from the decoded picture `a`), so a restoration can
    take out what the compression did and never invent or lose what the
    file holds. `tables`: its luma and chroma tables, natural order."""
    torch = T()
    h, w, _ = a.shape
    kw = {"dtype": torch.float64, "device": a.device}
    d = torch.tensor(dct8(), **kw)
    m = torch.tensor(TO_YCC, **kw)
    q = [torch.tensor(tables[0], **kw).reshape(8, 8), torch.tensor(tables[1 if 1 in tables else 0], **kw).reshape(8, 8)]

    def coefficients(rgb):
        ycc = rgb.double() @ m.T
        ycc[..., 0] -= 128.0  # (JPEG's level shift; Cb and Cr are centred already.)
        return [d @ ycc[..., ch].reshape(h // 8, 8, w // 8, 8).permute(0, 2, 1, 3) @ d.T for ch in range(3)]

    planes = []
    for ch, (ca, cn) in enumerate(zip(coefficients(a), coefficients(net))):
        qt = q[0 if ch == 0 else 1]
        c = torch.round(ca / qt)
        held = torch.minimum(torch.maximum(cn, (c - 0.5) * qt), (c + 0.5) * qt)
        planes.append((d.T @ held @ d).permute(0, 2, 1, 3).reshape(h, w))
    ycc = torch.stack(planes, -1)
    ycc[..., 0] += 128.0
    return (ycc @ torch.linalg.inv(m).T).clamp(0, 255)


def dither(x, seed=1):
    """`x` (0 to 255) to 8 bits, each pixel rounded up or down by chance
    in proportion to its fraction (the same chance for its three
    channels: no colour noise): a ramp between two levels becomes a mix
    of them, not a step, and a whole level stays exactly as it is."""
    torch = T()
    g = torch.Generator(device=x.device).manual_seed(seed)
    d = torch.rand(x.shape[:2], generator=g, device=x.device, dtype=x.dtype)[..., None] - 0.5
    return (x + d).round().clamp(0, 255).to(torch.uint8)


def flaws(u8):
    """How flawed a picture's smooth areas are (H×W×3, 0 to 255), as
    numbers: `smooth`, the share of it that is smooth (luma's spread in
    13×13 under 2.5 levels); `bands`, how stepped its gradients are (luma
    low-passed, its second differences, mean, ×1000); `texture`, the fine
    noise on them (luma less its 5×5 mean, twice: its spread; a dither
    alone is 0.25); `blocks`, JPEG's 8×8 grid (the step across a block's
    edge over the step inside one: 1 is none)."""
    torch = T()
    y = luma(u8.double())

    def lp(x, r, n):
        for _ in range(n):
            x = mean(x, r)
        return x

    m = lp(y, 6, 1)
    smooth = (lp(y * y, 6, 1) - m * m).clamp_min(0).sqrt() < 2.5
    out = {"smooth": float(smooth.double().mean()), "bands": 0.0, "texture": 0.0, "blocks": 1.0}
    if y.shape[0] < 16 or y.shape[1] < 16 or not bool(smooth.any()):
        return out
    inner = smooth[2:-2, 2:-2]
    n = max(int(inner.sum()), 1)
    low = lp(y, 3, 3)
    d2 = (low[2:-2, 4:] - 2 * low[2:-2, 2:-2] + low[2:-2, :-4]).abs() + (low[4:, 2:-2] - 2 * low[2:-2, 2:-2] + low[:-4, 2:-2]).abs()
    hf = (y - lp(y, 1, 2))[2:-2, 2:-2]
    out["bands"] = float((d2 * inner).sum()) / n * 1000
    out["texture"] = (float((hf * hf * inner).sum()) / n) ** 0.5
    dx = (y[:, 1:] - y[:, :-1]).abs()
    both = smooth[:, 1:] & smooth[:, :-1]
    edge = (torch.arange(dx.shape[1], device=y.device) % 8 == 7)[None, :]
    across, inside = dx[both & edge], dx[both & ~edge]
    if across.numel() and inside.numel() and float(inside.mean()) > 0:
        out["blocks"] = float(across.mean() / inside.mean())
    return out


def finish(u8, tables=None, tile=False):
    """A picture restored, on the GPU: `u8` (H×W×3 uint8, there) → the
    same, and what was done. `tables`: a JPEG's quantisation tables (its
    blocks and ringing are restored too). A `tile` is worked on as a
    mosaic of itself wide enough that its middle sees only its own
    repeats, and the middle taken: it still repeats without a seam."""
    torch = T()
    h, w, _ = u8.shape
    info = {"tiles": 0, "palette": False}
    packed = (u8.long() * torch.tensor([65536, 256, 1], device=u8.device)).sum(-1)
    if torch.unique(packed).numel() <= PALETTE:
        info["palette"] = True
        return u8, info
    work, ry, rx = u8, 0, 0
    if tile:
        ry, rx = math.ceil(REACH / h), math.ceil(REACH / w)
        work = u8.repeat(2 * ry + 1, 2 * rx + 1, 1)
    a = work.double()
    k = smooth_mask(a)
    x = a
    if tables is not None and h % 8 == 0 and w % 8 == 0:
        a32 = work.float()
        zone = edge_zone(a32)
        net, info["tiles"] = fbcnn(a32, (zone > 0.5) & (k > 0.5))
        if info["tiles"]:
            z = zone.double()[..., None]
            x = z * project(net, a32, tables) + (1 - z) * a
    k = k[..., None]
    out = dither(k * polished(x) + (1 - k) * a)
    if tile:
        out = out[ry * h:(ry + 1) * h, rx * w:(rx + 1) * w]
    return out, info


# ---- files


def load(path):
    """A picture's pixels (H×W×3 uint8) and, for a JPEG, its tables."""
    with Image.open(path) as im:
        tables = None
        if im.format == "JPEG" and im.mode == "RGB" and getattr(im, "layer", None) and all(l[1] == 1 and l[2] == 1 for l in im.layer):
            # (4:4:4 only: a subsampled one's chroma isn't on the 8×8 grid.)
            tables = {i: list(t) for i, t in im.quantization.items()}
        return np.array(im.convert("RGB")), tables


def encode(dest, src, shape, data):
    """`data` (RGB bytes) as lossless WebP at `dest`, checked by reading
    it back; `src` (the PNG or JPEG it was) then taken away. Its size, or
    -1 if what came back differed (nothing replaced)."""
    img = Image.frombuffer("RGB", (shape[1], shape[0]), data, "raw", "RGB", 0, 1)
    tmp = dest + ".tmp"
    if dest.endswith(".webp"):
        # (Method 4: the same size as the slowest, 6 at quality 100, in an
        # eighth of the time: Halo 3.67 MB either way, 2.9 s against 22.8.)
        img.save(tmp, "WEBP", lossless=True, quality=75, method=4, exact=True)
    else:
        img.save(tmp, "PNG", optimize=True)
    with Image.open(tmp) as back:
        same = back.convert("RGB").tobytes() == data
    if not same:
        os.remove(tmp)
        return dest, -1
    os.replace(tmp, dest)
    if src and src != dest and os.path.exists(src):
        os.remove(src)
    return dest, os.path.getsize(dest)


def restore(src, dest, tile=False):
    torch = T()
    a, tables = load(src)
    out, info = finish(torch.from_numpy(a).to(_dev), tables, tile)
    print(encode(dest, None, a.shape, out.cpu().numpy().tobytes()), info)


def pack(report=None):
    files = sorted(f for d in ("4k", "ultrawide", "tiles") for f in glob.glob(os.path.join(PACK, d, "*")) if f.endswith((".png", ".jpg")) and not f.endswith(".gain.png"))
    if not files:
        print("every picture in the pack is WebP already")
        return
    # The encoders first (forked before the GPU is touched), all but two cores.
    workers = max(1, (os.cpu_count() or 4) - 2)
    pool = multiprocessing.get_context("fork").Pool(workers)
    torch = T()
    pending = threading.Semaphore(2 * workers)
    done, failed = [], []

    def encoded(result):
        (failed if result[1] < 0 else done).append(result)
        pending.release()

    def errored(e):
        failed.append((str(e), -1))
        pending.release()

    log = open(report, "w") if report else None
    started, gpu_time = time.time(), 0.0
    with ThreadPoolExecutor(4) as readers:
        # Decoded a few ahead of the GPU.
        ahead = [readers.submit(load, f) for f in files[:6]]
        for i, f in enumerate(files):
            a, tables = ahead.pop(0).result()
            if i + 6 < len(files):
                ahead.append(readers.submit(load, files[i + 6]))
            rel = os.path.relpath(f, PACK)
            t = time.time()
            u8 = torch.from_numpy(a).to(_dev)
            out, info = finish(u8, tables, tile=rel.startswith("tiles/"))
            before, after = flaws(u8), flaws(out)
            changed = float((out != u8).any(-1).double().mean())
            most = int((out.int() - u8.int()).abs().max())
            data = out.cpu().numpy().tobytes()
            took = time.time() - t
            gpu_time += took
            line = "\t".join([rel, "palette" if info["palette"] else str(info["tiles"]), f"{changed:.3f}", str(most)] + [f"{before[key]:.3f}\t{after[key]:.3f}" for key in ("smooth", "bands", "texture", "blocks")] + [f"{took:.2f}"])
            if log:
                log.write(line + "\n")
                log.flush()
            pending.acquire()
            pool.apply_async(encode, (f.rsplit(".", 1)[0] + ".webp", f, a.shape, data), callback=encoded, error_callback=errored)
            if (i + 1) % 20 == 0 or i + 1 == len(files):
                print(f"{i + 1}/{len(files)} restored ({gpu_time:.0f} s on {_dev}), {len(done)} saved, {time.time() - started:.0f} s", flush=True)
    pool.close()
    pool.join()
    size = sum(n for _, n in done)
    print(f"{len(done)} pictures saved as lossless WebP ({size / 1e6:.0f} MB) in {time.time() - started:.0f} s; the GPU's share {gpu_time:.0f} s")
    for name, _ in failed:
        print("FAILED:", name)
    if failed:
        sys.exit(1)


def main():
    args = sys.argv[1:]
    if args[:1] == ["pack"]:
        pack(args[args.index("--report") + 1] if "--report" in args else None)
    elif args[:1] == ["restore"] and len(args) >= 3:
        restore(args[1], args[2], tile="--tile" in args)
    elif args[:1] == ["check"]:
        torch = T()
        for src in args[1:]:
            f = flaws(torch.from_numpy(load(src)[0]).to(_dev))
            print(f"{src}\t{f['smooth']:.3f}\t{f['bands']:.2f}\t{f['texture']:.3f}\t{f['blocks']:.3f}", flush=True)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
