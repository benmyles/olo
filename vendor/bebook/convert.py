#!/usr/bin/env python3
"""Turn the Be Book's HTML (HaikuPorts' be_book package source) into the
Markdown kept here, for reading and grepping beside the code.

    curl -LO https://s3.us-west-1.wasabisys.com/haiku-inc/old_beos/bebook_20081026.zip
    sha256sum bebook_20081026.zip   # 8bd48367…7a28, as in the recipe
    unzip bebook_20081026.zip
    uv run --with html2text vendor/bebook/convert.py bebook vendor/bebook

Each page loses its navigation header and footer; links between pages
point at the .md files; images are copied alongside.
"""

import pathlib
import re
import shutil
import sys

import html2text


def page(html: str) -> tuple[str, str]:
    title = re.search(r"<title>(.*?)</title>", html, re.S)
    title = re.sub(r"^\s*The Be Book\s*-\s*", "", title.group(1)).strip() if title else ""
    start = html.find('<div id="header"')
    if start >= 0:
        end = html.find("<hr /></div>", start)
        html = html[end + len("<hr /></div>"):] if end >= 0 else html[start:]
    end = html.find('<div id="footer"')
    if end >= 0:
        html = html[:end]
    return title, html


def main() -> None:
    src, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    conv = html2text.HTML2Text()
    conv.body_width = 0
    conv.unicode_snob = True
    conv.mark_code = False
    conv.single_line_break = False
    index = []
    for f in sorted(src.glob("*.html")):
        title, body = page(f.read_text(encoding="utf-8", errors="replace"))
        # Links between pages go to the Markdown copies.
        body = re.sub(r'href="([A-Za-z0-9_.-]+)\.html(#[^"]*)?"', lambda m: f'href="{m.group(1)}.md{m.group(2) or ""}"', body)
        md = conv.handle(body).strip() + "\n"
        md = re.sub(r"\n{3,}", "\n\n", md)
        (out / (f.stem + ".md")).write_text(f"<!-- {title} -->\n\n{md}", encoding="utf-8")
        index.append((f.stem, title))
    for img in src.rglob("*.png"):
        if "navigation" in img.parts:
            continue
        dest = out / img.relative_to(src)
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(img, dest)
    lines = [f"- [{t or stem}]({stem}.md)" for stem, t in sorted(index, key=lambda x: x[1].lower())]
    (out / "INDEX.md").write_text("# The Be Book: every page\n\n" + "\n".join(lines) + "\n", encoding="utf-8")
    print(f"{len(index)} pages")


if __name__ == "__main__":
    main()
