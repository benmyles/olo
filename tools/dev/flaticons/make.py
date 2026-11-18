#!/usr/bin/python3
"""The Flat icon set's drawings, made from its forms.

    make.py write             write every drawing to data/themes/flat/icons/svg
    make.py sheet OUT.png [NAME…]
                              render them (svgicon, as Olo draws them) at 64, 32
                              and 16 pixels on a light and a dark ground, to look at

The set is Olo's icons seen straight on, in plain flat faces (docs/ICONS.md,
"The Flat set"): `forms.py` has what they are made of (a page, a folder, a
screen, a cloud, a gear, a padlock…), `set1.py` to `set5.py` each drawing,
a few lines of those forms (a later set's drawing replaces an earlier one's).
A drawing changed by hand is kept by changing it here: `write` writes them
all. Build `svgicon` first (cargo build --release -p olo-xdg --example svgicon).
"""
import importlib, os, subprocess, sys
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
sys.path.insert(0, HERE)
from forms import svg

def drawings():
    out = {}
    for k in range(1, 6):
        for name, (title, fn) in importlib.import_module("set%d" % k).ICONS.items():
            out[name] = svg(title, fn())
    return out

def write(folder):
    os.makedirs(folder, exist_ok=True)
    made = drawings()
    for name, text in made.items():
        with open(os.path.join(folder, name + ".svg"), "w") as f:
            f.write(text)
    return made

def sheet(out, names):
    from PIL import Image, ImageDraw
    folder = os.path.join(os.path.dirname(os.path.abspath(out)), "flat-sheet", "icons", "svg")
    made = write(folder)
    names = names or sorted(made)
    svgicon = os.path.join(ROOT, "target", "release", "examples", "svgicon")
    sizes, cell, per = [64, 32, 16], 74, 12
    rows = (len(names) + per - 1) // per
    rowh = 64 + 8 + 64 + 8 + 48 + 14 + 12
    im = Image.new("RGB", (per * cell + 8, rows * rowh * 2), "#f1f3f6")
    g = ImageDraw.Draw(im)
    g.rectangle((0, rows * rowh, im.width, im.height), fill="#20262e")
    for i, name in enumerate(names):
        png = os.path.join(folder, name + ".png")
        r = subprocess.run([svgicon, os.path.join(folder, name + ".svg"), png] + [str(s) for s in sizes], capture_output=True, text=True)
        if r.returncode:
            print("can't draw", name, r.stderr[:200])
            continue
        pics = {s: Image.open(png.replace(".png", "-%d.png" % s)).convert("RGBA") for s in sizes}
        for half in (0, 1):
            x, y = 6 + (i % per) * cell, (i // per) * rowh + half * rows * rowh + 4
            im.paste(pics[64], (x, y), pics[64])
            a = pics[32].resize((64, 64), Image.NEAREST)
            im.paste(a, (x, y + 72), a)
            b = pics[16].resize((48, 48), Image.NEAREST)
            im.paste(b, (x + 8, y + 144), b)
            g.text((x, y + 194), name[:12], fill="#aaa" if half else "#555")
    im.save(out)
    print(out, im.size, len(names), "drawings")

if __name__ == "__main__":
    a = sys.argv[1:]
    if a[:1] == ["write"]:
        print(len(write(os.path.join(ROOT, "data", "themes", "flat", "icons", "svg"))), "drawings written")
    elif a[:1] == ["sheet"] and len(a) >= 2:
        sheet(a[1], a[2:])
    else:
        sys.exit(__doc__)
