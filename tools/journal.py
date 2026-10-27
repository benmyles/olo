#!/usr/bin/env python3
"""Screenshot journal: Olo's visual history, kept in docs/journal/.

    journal.py add SOURCE "Title" ["What changed and why"] [--crop X Y W H]
    journal.py redo                                   rebuild README.md and journal.html from entries.json

SOURCE is a tsession name (screenshot taken now) or an image path. --crop
is in logical pixels, as for `tsession zoom`. Images are stored at device
resolution in docs/journal/<date>/<time>-<slug>.png (tracked with Git LFS)
and listed newest first in docs/journal/README.md and journal.html (one
self-contained page with every image embedded, for sharing; see
journal_html.py).

Add an entry whenever something visible changes: a new app or window, a
look-and-feel fix (with before/after), a theme. Commit it with the change.
"""
import datetime, json, os, re, shutil, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
DIR = os.path.join(ROOT, "docs", "journal")
DB = os.path.join(DIR, "entries.json")
TSESSION = os.path.join(ROOT, "tools", "dev", "tsession")


def load():
    try:
        return json.load(open(DB))
    except FileNotFoundError:
        return []


def write_pages(entries):
    write_readme(entries)
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import journal_html
    journal_html.build(os.path.join(DIR, "journal.html"))


def write_readme(entries):
    out = ["# Screenshot journal", "",
           "Olo's visual evolution, newest first. Added with `tools/journal.py`",
           "(images are stored with Git LFS; run `git lfs pull` after cloning).", ""]
    day = None
    order = sorted(enumerate(entries), key=lambda ie: (ie[1]["time"], ie[0]), reverse=True)
    for _, e in order:
        d = e["time"][:10]
        if d != day:
            out += [f"## {d}", ""]
            day = d
        out += [f"### {e['title']}", "", f"*{e['time'][11:16]}*" + (f" — {e['caption']}" if e.get("caption") else ""), ""]
        out += [f"![{e['title']}]({e['image']})", ""]
    open(os.path.join(DIR, "README.md"), "w").write("\n".join(out))


def add(source, title, caption, crop):
    now = datetime.datetime.now()
    slug = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")[:48]
    rel = os.path.join(now.strftime("%Y-%m-%d"), f"{now:%H%M%S}-{slug}.png")
    dest = os.path.join(DIR, rel)
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    if os.path.exists(source):
        from PIL import Image
        im = Image.open(source)
        if crop:
            x, y, w, h = (int(float(v) * 2) for v in crop)
            im = im.crop((x, y, x + w, y + h))
        im.save(dest, optimize=True)
    else:
        args = [TSESSION, "shot", source, dest] + (list(crop) if crop else [])
        subprocess.run(args, check=True, stdout=subprocess.DEVNULL)
    entries = load()
    entries.append({"time": now.isoformat(timespec="seconds"), "title": title, "caption": caption, "image": rel})
    json.dump(entries, open(DB, "w"), indent=1)
    write_pages(entries)
    print(dest)


def main():
    a = sys.argv[1:]
    if not a or a[0] in ("-h", "--help"):
        print(__doc__)
        return
    if a[0] == "redo":
        write_pages(load())
        return
    if a[0] != "add" or len(a) < 3:
        sys.exit(__doc__)
    crop = None
    if "--crop" in a:
        i = a.index("--crop")
        crop = a[i + 1:i + 5]
        del a[i:i + 5]
    add(a[1], a[2], a[3] if len(a) > 3 else "", crop)


if __name__ == "__main__":
    main()
