# olo-desktop.com

Olo's website: one page that is itself an Olo desktop, to be read two
ways (by a person, and by an agent). It is served as it is: no build step
for the page, plain files with relative paths. `tools/deploy-website.sh`
deploys it to Cloudflare (below).

- **The hero is the desktop** with nothing open yet: Olo's own Dunes
  wallpaper (Classic's, its 21:9 band, light and dark), the mark and
  "Olo" top left (set from Sora as `tools/mkbrand.py` sets it, as Olo's
  About box shows them), three desktop icons under Deskbar, and the
  headline on the windows' grid, in the sky, its foot on the dunes'
  horizon. The wallpaper is never enlarged and never banded: lossless,
  each width resized in floating point and dithered again (below).
- **Deskbar**, top right as Olo's, is the navigation: the visitor's own
  clock (turned once a minute, never while the page is hidden), Light,
  Dark and Night (Night warms the whole screen as Olo's night light
  moves the white point, fading in over 1.5 s), **Agent view**, and the
  parts of the page as its window list. (The page's weight as it loaded,
  from the Performance API, is in the About window.) As soon as the page scrolls
  it folds to its corner button (Olo's ⌃⌘D), naming the window in front;
  on a phone, a slim bar across the top.
- **Every part of the page is an Olo window** on the desktop: Olo's own
  frame (measured from a real DeskCalc: outer line, bevel, face, inner
  bevel), a tab of Olo's size on its top edge, at its left corner. The
  window you're reading is the one in front: its
  tab is the one yellow on the screen; at the top of the page that is the
  first window, whose tab shows on the first screen. Every window has a
  menu bar whose menus work (its own: Copy Link, Agent View; View, where
  it has choices; Go: Previous Window, Next Window). Inside every window,
  one three-column grid. The footer is Olo's About window.
- **Every picture of Olo is at its own size**: one picture pixel to a
  device pixel at 1× and 2× (`srcset` with density descriptors; a
  narrower column shows less of a picture, from its top left, never a
  smaller one). The desktop's plate is the one exception, and says so.
- **Agent view** turns the whole page into the tree `olo-screen tree`
  gives a web page: each thing's role, name and place in pixels, measured
  from the page as it is, its links working. `index.md` and `llms.txt`
  are the page for agents that fetch rather than look.
- **Two readers** shows a real DeskCalc and its real tree (taken from an
  Agent Screen, `tools/deskcalc-tree.txt`); pointing at one lights the
  other.
- **The desktop's notes**: a number pointed at lights its line in the
  legend; chosen, its words open in a balloon where its line ends, and
  the page stays where it is.
- **Apps** is a Tracker icon view: an app pointed at says what it does on
  the status line; chosen, its Get Info window opens in front (a
  `<dialog>`, the page's tabs grey behind it), with the app's own window
  at its own size, on made-up data (below), its icon, and more about
  it; Previous and Next (and the arrow keys) go through all of them.
- **Looks** has each look's picture and, under it, its icons: sixteen of
  them as Olo draws them in each of the six icon sets, Flat too; a look
  chosen brings its own set.

No framework and no build step for the page itself: `index.html`,
`assets/site.css` and `assets/site.js`, every handler event-driven:
nothing polls, nothing animates at rest. Measured on an Agent Screen's
Chrome after a minute at rest: 0.03% of a core and 7.5 wakes a second
for Chrome's whole process tree, against 0.03% and 6.9 for about:blank
in the same Chrome.

## Assets

`tools/build-assets.py` makes `assets/` from Olo's own sources: parts of
`docs/img`'s 2× pictures at their own size (`NAME-2x.webp` and its half,
`NAME-1x.webp`; Rex, Agent Screens and Token Pool made by
`tools/dev/readme-shots` at the column's own size, `OLO_SHOTS_WINDOW=688x452`,
in both modes, and cut at their frames below their tabs: `olo-site-*.png`,
below), the desktop's plate at the widths the page asks for,
Dunes, DeskCalc's two pictures, the icon sets, each app's Get Info
picture, the app icons from
`data/icons/svg`, the mark without its contact shadow, "Olo" as a
vector in the brand's two inks, and the fonts subset to what the page
writes:

```sh
python3 -m venv /var/tmp/fonttools && /var/tmp/fonttools/bin/pip install fonttools brotli
FONTTOOLS_PY=/var/tmp/fonttools/bin/python /usr/bin/python3 website/tools/build-assets.py
```

Only what's missing or older than its source is made again, and what
was made from a source that isn't on disk is kept as it is: the pictures
made for this page alone aren't in the repository (they were made into a
folder of their own and cut from there; `WEBSITE_SHOTS=DIR` gives the
builder that folder when they are made again, and `WEBSITE_APP_SHOTS=DIR`
the Get Info pictures').

- **Dunes** (`data/wallpapers/ultrawide/olo-dunes*.webp`) at 1280, 1920,
  2880 and 3840 wide, lossless. Its gradients are dithered; a lossy
  encoder smooths the dither away and the sky's 8-bit bands come back
  (WebP at 100 and AVIF at 95 both did), so each width is resized in
  floating point and dithered to 8 bits again. The page asks for the
  width it draws (`image-set`, by density, and by width past 1920):
  2.6 MB on a 2× screen (at 1440 × 900 the band is drawn 1,700 points
  wide), 0.7 MB at 1×, 0.3 MB on a phone.
- **The icon sets**: `SVGICON=…` or `target/debug/examples/svgicon`
  (`jato build -p olo-xdg --example svgicon`) draws each icon as Olo
  does, in each set's colours, one strip a set at 64 points, 1× and 2×.
- **Each app's Get Info picture**: `OLO_SHOTS_OUT=DIR tools/dev/readme-shots
  website-apps` (`OLO_SHOTS_APPS=rex,todo` for some) pictures each app
  alone, on its demo data, its window below its tab, light and dark; then
  `WEBSITE_APP_SHOTS=DIR` for this script. Their sizes in points are the
  buttons' `data-w` and `data-h` in `index.html` (840 x 479 for most;
  Activity Monitor 880 x 499). Activity Monitor, the Log Viewer, OS
  Updates, DriveSetup and Passwords have no demo data in Olo itself (they
  show the computer's own): their pictures were made on demo modes written
  for them on the branch this site was made on (`webv2-before-cleanup`,
  commit `5068fd427`), not in Olo, and the scene leaves them out rather
  than picture this computer's own.

Where `docs/img` had a picture in one mode only, its other mode was made
by `tools/dev/readme-shots` with `OLO_SHOTS_FLIP=1` (each scene in its
other mode, into a folder of its own: `olo-palette-dark.png` and the like
are in `docs/img`; the others, `-light` and `-dark`, are given with
`WEBSITE_SHOTS`). The six looks are the Pictures window's corner of
readme-shots' themes scene, cut from its 4K grabs (`olo-look-NAME.png`,
and `-dark`). DeskCalc's pictures and tree come from an Agent
Screen at 2× (`olo-screen start NAME --size 1280x800 --scale 2`, `run
olo-deskcalc`, `set --name Expression 1000/120`, `click --name =`,
`tree`, `look`), its desktop made transparent round the window; dark
through the screen's app_server (`get_settings`, `set_settings`, as
`tools/dev/tsession theme` does).

## Looking at it

Serve the folder (any static server: the page is relative paths only;
while you work, one that tells the browser not to keep what it serves)
and picture it on an Agent Screen, never on the person's display:

```sh
python3 -m http.server 8817 --bind 127.0.0.1 --directory website
olo-screen start web --size 1440x900 --scale 2
olo-screen run web olo-chrome --start-fullscreen "http://127.0.0.1:8817/?theme=dark"
olo-screen look web /var/tmp/me/web.png --no-pointer
```

`?theme=light|dark|night` picks the look for a picture without keeping
it. Check phone width (390), 1280 × 800, 1440 and wide, light, dark and
night, Agent view, and the states (a key or a tree line pointed at, an
app's status line, Deskbar open on a phone). A page's tree on the
screen (`olo-screen tree`) needs Olo's Chrome with the screen's own
profile (below).

### Pictures of the page on a host's screen

What making its pictures on a dev server's own screens (`olo screen --on
host`) taught (2026-11-27):

- **Olo's Chrome with a `--user-data-dir` of its own gave `tree` an empty
  page** (`Document … at 0,0 1440x0`) until `olo-chrome` with the
  screen's own profile was started on the same screen. Use the screen's
  own profile where you need the page's tree or `click --name`.
- **Serve with `Cache-Control: no-store` while you work**: `python3 -m
  http.server` lets Chrome keep what it serves, and pictures showed the
  stylesheet of before.
- **One Chrome window a screen, and check the page in front**: `olo
  screen run NAME olo-chrome URL` (not `--quick`: with it no tab opened)
  hands the address to the running Chrome as a new tab; with two Chrome
  windows, Olo asks which in an "Open Link" window, and the picture is of
  that. Give the address a token of its own (`?r123&theme=dark`) and look
  for it in the tree's `Document` before taking a picture.
- **A screen's look** is set through its app_server (`get_settings`,
  `appearance.theme`, `set_settings` on the screen's `OLO_SOCKET`, as
  `tools/dev/tsession theme` does); a screen can be 1440 × 4000 (not
  7800), and `scroll --at X,Y` must be inside the screen.
- **Chrome's "press Esc to exit full screen"** stands over the page for
  some four seconds after `--start-fullscreen`: wait it out.
- **A host's screens stop when its release is replaced** (a
  `host-upgrade`): start yours again.
- **The release with a few apps of the tree's**: a folder of links to
  every program in `~/.local/share/olo/desktop/bin`, the tree's
  `target/debug/olo-NAME` linked over the ones changed, given to
  readme-shots as `OLO_SHOTS_BUILD`.
- **Olo's wallpapers are dithered; keep them lossless** (above): a
  contrast-stretched crop of the sky shows a lossy encoder's bands at once.
- **A `<dialog>` opened with `showModal()` is above everything**, the
  page's night light too: the Get Info window warms itself (a multiply
  layer over its window and its tab only).

## Deploying

`tools/deploy-website.sh` copies what is served (`index.html`,
`index.md`, `llms.txt`, `assets/`) into `website/dist` (not in git) and
deploys that to the Cloudflare project `olo-desktop`, asking before it
fetches Cloudflare's `wrangler` or signs in (`--build` to stage only,
`--preview` for a preview URL, `--help` for the rest). Nothing is
published yet.

## Left to do

In the words of the design reviews of 2026-11-27 (Opus 5.5 and Gemini 4
Neon), what their third round left:

- "Phone scenes for every made-to-size picture … render phone scenes at
  the phone column (about 335pt wide) as whole windows" (phones show Rex,
  Agent Screens and Token Pool at their own size, panned sideways).
- "Rex's toolbar shows 'Olo w…'. Rex's tree shows 'Schema…' and 'Running
  visual…' … so nothing in Rex's toolbar or tree is shortened. End Token
  Pool's list on a whole row." (Rex's and Token Pool's demo data.)
- "Agent Screens: the sidebar is a single row in a tall, empty pane. Add
  two or three running screens so it reads as a list."
- "Two readers on phones: the tree's 20 Button lines take about 370pt of
  scrolling. Collapse the middle of the run behind a disclosure
  triangle."
- "Leaving Agent view: … let the Terminal tab's close box (plus a
  Terminal menu item) take you back to the page." (The menu item is
  there; the close box isn't live.)
- "Surface The Olo Book, Releases, and Source links in the phone
  Deskbar" (on phones they are only in the About window).
