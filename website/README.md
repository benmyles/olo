# olo-desktop.com

Olo's website: a static site, built by a small Rust program of its own
into `website/dist`, to host anywhere (plain files, pretty URLs as
`NAME/index.html`, a `404.html`, a `CNAME` for GitHub Pages).

```sh
cd website
cargo run --release                  # build into dist/
cargo run --release -- serve [PORT]  # build, then serve dist/ at http://localhost:8000
```

The site is hosted at Cloudflare (the project `olo-desktop`):
`tools/deploy-website.sh` builds it and deploys it there, asking before
it fetches Cloudflare's `wrangler` or signs in (`--preview` for a preview
URL, `--build` to build only, `--help` for the rest). `dist/` is plain
files and would serve from any static host (GitHub Pages, Netlify, S3, a
web server's folder). It isn't part of Olo's workspace:
Olo's build never waits for it, and it has its own `target/`.

## What's where

| Path | What |
|---|---|
| `pages/*.html` | The pages: front matter (`title`, `description`) between `---` lines, then the page's HTML. `index.html` is `/`, `404.html` is `/404.html`, `NAME.html` is `/NAME/`. |
| `templates/layout.html` | Every page's head, header and footer. `{{title}}`, `{{description}}`, `{{url}}`, `{{content}}`, `{{page}}` and `{{current:NAME}}` (the navigation's current page) are filled in. |
| `static/` | Copied as it is: `css/site.css`, `js/site.js` (the copy buttons, and a screenshot enlarged on a click; nothing runs by itself), `fonts/` (Sora, Noto Sans and Noto Sans Mono, Latin subsets as WOFF2, from `data/fonts`), `brand/` (the brand kit, made by `tools/mkbrand.py`), `site.webmanifest`. |
| `src/main.rs` | The builder. |

## Shortcodes

In a page:

- `{{shot NAME | ALT [| eager]}}`: a screenshot from `docs/img/NAME.png`,
  made at 960, 1600, 2400 and 3200 pixels wide (no wider than it is) for
  a `srcset`, as lossless WebP; when `docs/img/NAME-dark.png` is there,
  it is shown in dark mode. `eager` for the one at the top of a page.
  It is a link to itself at its largest: a click shows it as large as
  the window has room for, over the page (a `<dialog>`: Escape, a click
  or Close puts it away); with no script the link opens the picture.
- `{{icon NAME [PX]}}`: an icon from `data/icons/svg`, drawn for PX CSS pixels (64),
  sharp at 1× and 2×.
- `{{wallpaper NAME | LABEL}}`: a picture from the wallpaper pack
  (`data/wallpapers/4k`), with its dark version when it has one.

Pictures are made again only when their source is newer than what's in
`dist/`. The screenshots are the README's: `tools/dev/readme-shots`
takes them in 4K on an Agent Screen.

## Look and feel

The brand ([docs/BRAND.md](../docs/BRAND.md)): Olo blue, Olo yellow,
charcoal and desktop grey; Sora for headings, Noto Sans for text. Light
and dark follow the system, as Olo does, so the screenshots always match
the page. Like Olo, it is light: no framework, no web fonts from
elsewhere, no trackers, nothing that animates on its own.
