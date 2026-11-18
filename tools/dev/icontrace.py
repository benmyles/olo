#!/usr/bin/python3
"""Help for writing an icon's SVG by hand (docs/ICONS.md, "Drawings"):
looking at its picture on the drawing's 64-unit grid, measuring it, and
comparing the drawing with it.

    icontrace.py grid PICTURE OUT.png [x0,y0,x1,y1]
        The picture with the 64 by 64 grid over it (a line every 4 units,
        numbered every 8), or only that part of it (grid units), enlarged.
    icontrace.py sheet OUT.png PICTURE [PICTURE...]
        Several pictures side by side (three a row), each with the grid
        over it (a line every 8 units, numbered every 16): to look at a
        few at once.
    icontrace.py outline PICTURE [--tol UNITS]
        Its silhouette as a polygon, in grid units (corners found, edges
        straightened to within --tol, 0.35 by default).
    icontrace.py region PICTURE X,Y [--like N] [--tol UNITS]
        The area of one colour around grid point X,Y (colours within
        --like, 28 by default, of it) as a polygon; and its mean colour.
    icontrace.py pick PICTURE X,Y [X,Y...]
        The colours at those grid points.
    icontrace.py round X,Y[,R] X,Y[,R] ...
        The path data of that polygon with each corner rounded to its
        radius R (grid units; none: sharp), to paste into a drawing.
    icontrace.py extrude DX,DY X,Y X,Y ...
        A flat shape given depth: that polygon pushed back by DX,DY (the
        side direction: -2,-1 times its depth). Prints the whole solid's
        path (for its `o` form), and the paths of the faces that show:
        those looking up (lighter) and those looking sideways (darker).
        The polygon itself is the front face.
    icontrace.py review OUT.png NAME [NAME...] [--set DIR]
        A row for each icon NAME of the set in DIR (data/icons by
        default): its picture and its drawing at 128 pixels, then the
        drawing at 64, 48, 32, 24, 20 and 16 at true size on the light
        and on the dark panel, the picture's 32 and 16 after each.
    icontrace.py compare PICTURE DRAWING.svg OUT.png [--theme-dir DIR]
        The picture beside the drawing at 256 pixels, their difference,
        and both at 16 to 64 pixels at true size on the light and the dark
        panel. (Renders with target/release/examples/svgicon.)

A picture is its 256-pixel size (or any square one); coordinates are the
drawing's: 0 to 64, x right, y down.
"""
import os, subprocess, sys
from collections import deque
from PIL import Image, ImageChops, ImageDraw, ImageFont

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
FONT = os.path.join(ROOT, "data", "fonts", "NotoSans[wght].ttf")
LIGHT, DARK = (219, 227, 238), (24, 43, 54)
GRID = 64.0


def load(path, side=256):
    im = Image.open(path).convert("RGBA")
    if im.size != (side, side):
        im = im.resize((side, side), Image.LANCZOS)
    return im


def on(im, bg):
    out = Image.new("RGBA", im.size, bg + (255,))
    out.alpha_composite(im)
    return out.convert("RGB")


def cmd_grid(path, out, part=None):
    x0, y0, x1, y1 = part or (0, 0, 64, 64)
    src = Image.open(path).convert("RGBA")
    k = src.width / GRID
    crop = src.crop((int(x0 * k), int(y0 * k), int(x1 * k), int(y1 * k)))
    side = 1280
    scale = side / max(x1 - x0, y1 - y0)
    crop = crop.resize((int((x1 - x0) * scale), int((y1 - y0) * scale)), Image.LANCZOS)
    pad = 36
    sheet = Image.new("RGB", (crop.width + pad, crop.height + pad), (245, 245, 245))
    sheet.paste(on(crop, (255, 255, 255)), (pad, pad))
    d = ImageDraw.Draw(sheet, "RGBA")
    font = ImageFont.truetype(FONT, 18)
    step = 1 if (x1 - x0) <= 16 else 2 if (x1 - x0) <= 32 else 4
    u = int(x0 // step * step)
    while u <= x1:
        if u >= x0:
            x = pad + (u - x0) * scale
            strong = u % (step * 2) == 0
            d.line([(x, pad), (x, sheet.height)], fill=(255, 0, 160, 150 if strong else 60), width=1)
            if strong:
                d.text((x - 8, 8), str(u), fill=(160, 0, 100, 255), font=font)
        u += step
    u = int(y0 // step * step)
    while u <= y1:
        if u >= y0:
            y = pad + (u - y0) * scale
            strong = u % (step * 2) == 0
            d.line([(pad, y), (sheet.width, y)], fill=(255, 0, 160, 150 if strong else 60), width=1)
            if strong:
                d.text((4, y - 12), str(u), fill=(160, 0, 100, 255), font=font)
        u += step
    sheet.save(out)
    print(out, sheet.size)


def cmd_sheet(out, paths):
    side, pad, cols = 448, 18, 3
    rows = (len(paths) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * (side + pad) + pad, rows * (side + pad + 16) + pad), (245, 245, 245))
    d = ImageDraw.Draw(sheet, "RGBA")
    font = ImageFont.truetype(FONT, 13)
    for i, path in enumerate(paths):
        x0, y0 = pad + (i % cols) * (side + pad), pad + (i // cols) * (side + pad + 16) + 16
        im = Image.open(path).convert("RGBA").resize((side, side), Image.LANCZOS)
        sheet.paste(on(im, (255, 255, 255)), (x0, y0))
        d.text((x0, y0 - 16), os.path.basename(path)[:-4], fill=(40, 40, 40, 255), font=font)
        for u in range(0, 65, 8):
            v = u * side / GRID
            strong = u % 16 == 0
            d.line([(x0 + v, y0), (x0 + v, y0 + side)], fill=(255, 0, 160, 120 if strong else 50), width=1)
            d.line([(x0, y0 + v), (x0 + side, y0 + v)], fill=(255, 0, 160, 120 if strong else 50), width=1)
            if strong and u < 64:
                d.text((x0 + v + 2, y0 + 1), str(u), fill=(160, 0, 100, 255), font=font)
                d.text((x0 + 2, y0 + v + 1), str(u), fill=(160, 0, 100, 255), font=font)
    sheet.save(out)
    print(out, sheet.size)


def contour(mask, w, h):
    """The outer boundary of `mask` (a set of (x, y)) as a closed list of
    corner points, walked along pixel edges (clockwise)."""
    start = min(mask, key=lambda p: (p[1], p[0]))
    # Edges walked on the pixel-corner lattice, the area on the right.
    x, y = start
    px, py = x, y  # lattice point: the pixel's top-left corner
    dx, dy = 1, 0
    pts = [(px, py)]
    inside = lambda a, b: (a, b) in mask
    for _ in range(8 * (w + h) * 8):
        # At lattice point (px, py) heading (dx, dy): the pixels ahead to the left and right.
        if (dx, dy) == (1, 0):
            left, right = (px, py - 1), (px, py)
        elif (dx, dy) == (0, 1):
            left, right = (px, py), (px - 1, py)
        elif (dx, dy) == (-1, 0):
            left, right = (px - 1, py), (px - 1, py - 1)
        else:
            left, right = (px - 1, py - 1), (px, py - 1)
        if inside(*left):
            dx, dy = dy, -dx  # turn left
        elif not inside(*right):
            dx, dy = -dy, dx  # turn right
        else:
            px, py = px + dx, py + dy
            if (px, py) == pts[0]:
                break
            pts.append((px, py))
            continue
        # (Turned: try again from here.)
    return pts


def simplify(pts, tol):
    """Douglas-Peucker on a closed polygon."""
    if len(pts) < 4:
        return pts

    def dp(seg):
        if len(seg) < 3:
            return [seg[0]]
        (x0, y0), (x1, y1) = seg[0], seg[-1]
        dx, dy = x1 - x0, y1 - y0
        n = (dx * dx + dy * dy) ** 0.5 or 1e-9
        worst, at = 0.0, 0
        for i in range(1, len(seg) - 1):
            d = abs((seg[i][0] - x0) * dy - (seg[i][1] - y0) * dx) / n
            if d > worst:
                worst, at = d, i
        if worst <= tol:
            return [seg[0]]
        return dp(seg[: at + 1]) + dp(seg[at:])

    # Split at the two points farthest apart, so neither half is closed.
    a = 0
    b = max(range(len(pts)), key=lambda i: (pts[i][0] - pts[a][0]) ** 2 + (pts[i][1] - pts[a][1]) ** 2)
    a = max(range(len(pts)), key=lambda i: (pts[i][0] - pts[b][0]) ** 2 + (pts[i][1] - pts[b][1]) ** 2)
    if a > b:
        a, b = b, a
    return dp(pts[a : b + 1]) + dp(pts[b:] + pts[: a + 1])


def say_polygon(pts, k):
    print(" ".join(f"{x / k:.1f},{y / k:.1f}" for x, y in pts))


def cmd_outline(path, tol):
    im = load(path)
    a = im.getchannel("A").load()
    w, h = im.size
    mask = {(x, y) for y in range(h) for x in range(w) if a[x, y] >= 128}
    k = w / GRID
    say_polygon(simplify(contour(mask, w, h), tol * k), k)


def cmd_region(path, at, like, tol):
    im = load(path)
    px = im.load()
    w, h = im.size
    k = w / GRID
    sx, sy = int(at[0] * k), int(at[1] * k)
    seed = px[sx, sy]
    near = lambda c: c[3] >= 128 and abs(c[0] - seed[0]) + abs(c[1] - seed[1]) + abs(c[2] - seed[2]) <= like * 3
    mask, todo = {(sx, sy)}, deque([(sx, sy)])
    tot = [0, 0, 0]
    while todo:
        x, y = todo.popleft()
        c = px[x, y]
        for i in range(3):
            tot[i] += c[i]
        for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
            if 0 <= nx < w and 0 <= ny < h and (nx, ny) not in mask and near(px[nx, ny]):
                mask.add((nx, ny))
                todo.append((nx, ny))
    n = len(mask)
    print(f"seed #{seed[0]:02x}{seed[1]:02x}{seed[2]:02x}  mean #{tot[0] // n:02x}{tot[1] // n:02x}{tot[2] // n:02x}  area {n / k / k:.1f} sq units")
    say_polygon(simplify(contour(mask, w, h), tol * k), k)


def cmd_pick(path, points):
    im = load(path)
    px = im.load()
    k = im.width / GRID
    for x, y in points:
        c = px[min(im.width - 1, int(x * k)), min(im.height - 1, int(y * k))]
        print(f"{x},{y}: #{c[0]:02x}{c[1]:02x}{c[2]:02x} a{c[3]}")


def cmd_round(points):
    """A closed polygon's path, its corners rounded (a quadratic curve
    through each corner's cut ends)."""
    n = len(points)
    f = lambda v: f"{v:.2f}".rstrip("0").rstrip(".")
    out = []
    for i, (x, y, r) in enumerate(points):
        (ax, ay, _), (bx, by, _) = points[i - 1], points[(i + 1) % n]
        if r <= 0:
            out.append(("L", x, y))
            continue
        la = ((ax - x) ** 2 + (ay - y) ** 2) ** 0.5
        lb = ((bx - x) ** 2 + (by - y) ** 2) ** 0.5
        ra, rb = min(r, la / 2), min(r, lb / 2)
        out.append(("L", x + (ax - x) / la * ra, y + (ay - y) / la * ra))
        out.append(("Q", x, y, x + (bx - x) / lb * rb, y + (by - y) / lb * rb))
    d = []
    for k, seg in enumerate(out):
        if seg[0] == "L":
            d.append(("M" if k == 0 else "L") + f"{f(seg[1])} {f(seg[2])}")
        else:
            d.append(f"Q{f(seg[1])} {f(seg[2])} {f(seg[3])} {f(seg[4])}")
    print("".join(d) + "Z")


def cmd_extrude(d, pts):
    f = lambda v: f"{v:.2f}".rstrip("0").rstrip(".")
    poly = lambda ps: "M" + "L".join(f"{f(x)} {f(y)}" for x, y in ps) + "Z"
    n = len(pts)
    area = sum(pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1] for i in range(n))
    if area < 0:  # (Clockwise on the screen, y down: positive.)
        pts = pts[::-1]
    back = [(x + d[0], y + d[1]) for x, y in pts]
    whole, top, side = [poly(pts), poly(back)], [], []
    for i in range(n):
        (x0, y0), (x1, y1) = pts[i], pts[(i + 1) % n]
        ex, ey = x1 - x0, y1 - y0
        # Outward normal of a clockwise polygon's edge (y down): (ey, -ex).
        nx, ny = ey, -ex
        quad = [(x0, y0), (x1, y1), (x1 + d[0], y1 + d[1]), (x0 + d[0], y0 + d[1])]
        qa = sum(quad[k][0] * quad[(k + 1) % 4][1] - quad[(k + 1) % 4][0] * quad[k][1] for k in range(4))
        if qa < 0:
            quad = quad[::-1]
        whole.append(poly(quad))
        if nx * d[0] + ny * d[1] > 1e-9:
            (top if -ny >= abs(nx) * 0.5 else side).append(poly(quad))
    print("whole:", "".join(whole))
    print("top:  ", "".join(top))
    print("side: ", "".join(side))


def render(svg, px, tmp):
    exe = os.path.join(ROOT, "target", "release", "examples", "svgicon")
    out = os.path.join(tmp, f"r{px}.png")
    subprocess.run([exe, svg, out, str(px)], check=True)
    return Image.open(out).convert("RGBA")


def cmd_compare(path, svg, out):
    tmp = os.path.join("/var/tmp", f"icontrace-{os.getuid()}")
    os.makedirs(tmp, exist_ok=True)
    name = os.path.basename(svg)[:-4]
    pic_dir = os.path.dirname(os.path.dirname(path)) if os.path.basename(os.path.dirname(path)).isdigit() else None
    big_old, big_new = load(path), render(svg, 256, tmp)
    sizes = [16, 20, 24, 32, 48, 64]
    font = ImageFont.truetype(FONT, 13)
    W = 20 + 3 * 266 + 20
    row_h = 74
    H = 20 + 256 + 20 + 2 * (2 * row_h + 10) + 10
    sheet = Image.new("RGB", (W, H), LIGHT)
    d = ImageDraw.Draw(sheet)
    sheet.paste(on(big_old, LIGHT), (20, 20))
    sheet.paste(on(big_new, LIGHT), (20 + 266, 20))
    diff = ImageChops.difference(on(big_old, LIGHT), on(big_new, LIGHT))
    sheet.paste(diff, (20 + 2 * 266, 20))
    y = 20 + 256 + 20
    for bg in (LIGHT, DARK):
        d.rectangle([0, y - 5, W, y + 2 * row_h + 5], fill=bg)
        ink = (20, 26, 38) if bg == LIGHT else (231, 237, 247)
        for r, (label, get) in enumerate((("picture", lambda px: old_at(path, pic_dir, name, px)), ("drawing", lambda px: render(svg, px, tmp)))):
            x = 20
            d.text((x, y + r * row_h + 24), label, fill=ink, font=font)
            x += 70
            for px in sizes:
                im = get(px)
                sheet.paste(on(im, bg), (x, y + r * row_h + (64 - px) // 2))
                x += px + 22
            # And as Deskbar shows one: 32 pixels with its name beside it.
            im = get(32)
            sheet.paste(on(im, bg), (x + 20, y + r * row_h + 16))
            d.text((x + 64, y + r * row_h + 12), name.replace("-", " ").title(), fill=ink, font=ImageFont.truetype(FONT, 26))
        y += 2 * row_h + 10
    sheet.save(out)
    print(out, sheet.size)


def cmd_review(out, names, root):
    tmp = os.path.join("/var/tmp", f"icontrace-{os.getuid()}")
    os.makedirs(tmp, exist_ok=True)
    sizes = [64, 48, 32, 24, 20, 16]
    row_h = 136
    W = 16 + 2 * 136 + 2 * (sum(sizes) + len(sizes) * 10 + 32 + 16 + 40)
    sheet = Image.new("RGB", (W, len(names) * row_h + 8), LIGHT)
    d = ImageDraw.Draw(sheet)
    font = ImageFont.truetype(FONT, 11)
    for r, name in enumerate(names):
        y = 8 + r * row_h
        svg = os.path.join(root, "svg", name + ".svg")
        pic = lambda px: Image.open(os.path.join(root, str(px), name + ".png")).convert("RGBA") if os.path.exists(os.path.join(root, str(px), name + ".png")) else Image.new("RGBA", (px, px))
        sheet.paste(on(pic(128), LIGHT), (16, y))
        d.text((16, y), name, fill=(60, 60, 60), font=font)
        if not os.path.exists(svg):
            continue
        sheet.paste(on(render(svg, 128, tmp), LIGHT), (16 + 136, y))
        x = 16 + 2 * 136
        for bg in (LIGHT, DARK):
            width = sum(sizes) + len(sizes) * 10 + 32 + 16 + 40
            d.rectangle([x - 4, y - 4, x + width - 12, y + row_h - 8], fill=bg)
            cx = x + 4
            for px in sizes:
                sheet.paste(on(render(svg, px, tmp), bg), (cx, y + (64 - px) // 2 + 28))
                cx += px + 10
            # (The picture as it is today, at 32 and 16, to compare.)
            cx += 12
            for px in (32, 16):
                sheet.paste(on(pic(px), bg), (cx, y + (64 - px) // 2 + 28))
                cx += px + 10
            x += width
    sheet.save(out)
    print(out, sheet.size)


def old_at(path, pic_dir, name, px):
    """The picture at `px`: its own pre-scaled size if there is one, else shrunk."""
    if pic_dir:
        p = os.path.join(pic_dir, str(px), name + ".png")
        if os.path.exists(p):
            return Image.open(p).convert("RGBA")
    return Image.open(path).convert("RGBA").resize((px, px), Image.LANCZOS)


def main():
    a = sys.argv[1:]
    opt = lambda flag, default: float(a[a.index(flag) + 1]) if flag in a else default
    point = lambda s: tuple(float(v) for v in s.split(","))
    if a[:1] == ["grid"]:
        cmd_grid(a[1], a[2], point(a[3]) if len(a) > 3 else None)
    elif a[:1] == ["sheet"]:
        cmd_sheet(a[1], a[2:])
    elif a[:1] == ["outline"]:
        cmd_outline(a[1], opt("--tol", 0.35))
    elif a[:1] == ["region"]:
        cmd_region(a[1], point(a[2]), opt("--like", 28), opt("--tol", 0.35))
    elif a[:1] == ["pick"]:
        cmd_pick(a[1], [point(p) for p in a[2:]])
    elif a[:1] == ["round"]:
        cmd_round([(p + (0.0,))[:3] for p in (point(v) for v in a[1:])])
    elif a[:1] == ["extrude"]:
        cmd_extrude(point(a[1]), [point(v) for v in a[2:]])
    elif a[:1] == ["review"]:
        root = a[a.index("--set") + 1] if "--set" in a else os.path.join(ROOT, "data", "icons")
        cmd_review(a[1], [n for n in a[2:] if n != "--set" and n != root], root)
    elif a[:1] == ["compare"]:
        cmd_compare(a[1], a[2], a[3])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
