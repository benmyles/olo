# olo-desktop.com

Olo's website: one page that is itself an Olo desktop, to be read two
ways (by a person, and by an agent). It is served as it is: no build step
for the page, plain files with relative paths. `tools/deploy-website.sh`
deploys it to Cloudflare (below).

- **The hero is the desktop** with nothing open yet: Olo's own Dunes
  wallpaper (Classic's, its 21:9 band, light and dark), the mark and
  "Olo" top left (set from Sora as `tools/mkbrand.py` sets it, as Olo's
  About box shows them), four desktop icons under Deskbar (the User's
  Guide first), and the
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
- **The User's Guide** (below) opens over the desktop as a book, from its
  icon on the desktop, the hero's button, Deskbar and the About window.

## The User's Guide

*User's Guide to Olo*, a manual in the manner of the boxed software of the
1990s (IBM's *User's Guide to OS/2 Warp*, the BeOS User's Guide): thirty
printed pages, two covers and fourteen spreads, in six chapters with their
edge tabs, running heads and folios, figures and captions, and four kinds of
box: *Try This*, *Tip*, *Note* and *Shortcuts*.

- `assets/manual.html` is its pages, and a page of its own that any reader
  reads one page after another: set to read on any screen, and as printed
  pages (500 points wide or more, with even margins and hard edges) where
  scripts run and its column, measured as it is, has 532 points or more.
  What its script does (Read as a Book, page 24's night link) shows only
  where it runs; without it, page 24 points to Olo's own Night Mode. Its
  addresses are relative to `assets/`; a picture cut to its point is a link
  to the whole. `#book` says how many pages the edition has (`data-pages`),
  and the back cover ends with a mark (`data-end`, its words its own).
- `assets/manual.js` (with the page) opens it: the first time an entry to it
  is pointed at or chosen, it fetches `manual.html` and `manual.css` and
  sets the book in `<dialog id="manual">`, whose window (its bar, arrows,
  plate and alert) is `site.css`'s, so it opens at once. Fetching the two
  is one attempt with one deadline (20 s): at the deadline, at Close or
  Cancel, or on any failure (an answer past 1 MB, read in pieces; one that
  isn't the whole edition: every page in order, as many as `data-pages`
  says, to the back cover's end mark) it is dropped whole, at once, its
  fetch aborted and a stylesheet still coming taken out, and nothing is
  built after; Try Again asks the server again, past the browser's cache.
  Meanwhile an Olo alert window says it's opening, Cancel its default (and,
  after 2.5 s, offers Read as a Web Page beside it); failed, it offers Try
  Again and Read as a Web Page, the default. Full Screen can't be chosen
  until there is a book. `#manual`, `#manual/p-N` and `#manual/ch-N` open
  it at a page or a chapter; turning a page replaces the address, as what
  turned it says (never a turn that follows Back or Forward, which leaves
  the address as it found it); closing it goes back from the entry it
  pushed (marked in `history.state`) or, for an address that came another
  way, takes the guide's part off it, so it never leaves the site. Closing
  is tidied at once, so it may be opened again in the same moment (the
  dialog's own close event, a task later, then does nothing). Full Screen
  fills the screen with the book (its frame: a `<dialog>` can't itself be
  the fullscreen element), and Close leaves full screen too, even one still
  being granted.
- `assets/manual.md` is the guide for agents, linked from `index.md` and
  `llms.txt`; Agent view names both.

Two pages open at once where both fit at a size to read (a page 500 points
wide or more, 24 points under the bar and above the screen's foot, with,
either side, the page edges, the edge tabs, 24 points clear and a 36 × 72
arrow 16 from the screen's edge), else one; a page narrower than that (a
phone) is set again at a reading size, as long as it is, in a stage that
scrolls between two bars of 44-point controls. A page is a printed page:
everything on it is in em of a fortieth of its width, so it lays out the
same at every size, and every page was measured to fit at 500 (a 1235 × 900
window), 520 (1280 × 800), 594 (1440 × 900) and 680 points. Its pictures
are the page's own (`assets/img`), at their own size, flush left on the
measure, each cut to the part its caption is about, on the window's own
edges between its rows and panes, and to a narrower part on a phone where
the first doesn't fit (with its own caption where it differs). A figure is
as wide as its picture (280 points at least), its caption balanced to it
and, at the caption's end, on the picture's right edge, a mark that opens
the picture whole: fitted to the screen (never enlarged), framed, its
figure's number in a bar with Full Screen and Close where the book's are;
a click on it, or Return, shows it at its own size, from the part the page
showed (a phone's cut, as far as it was moved) or the place clicked, to
move round by finger, wheel or arrow keys. The desktop on page 7 is the
one picture reduced on the page, and its caption says so (the plate's
doesn't promise its numbered callouts). Bold is kept for menu paths and
an app or feature at its first mention on a spread. Pages turn as leaves
do, on the spine, for 0.68 s, lifting no more than 12 points and shading
the page under them toward the spine (cross-fading in a moment where
motion is reduced, and on a phone); the bars, tabs and arrows say where
the book is when the leaf lands. Nothing runs while the book lies open:
the Deskbar's clock waits while a window is open over the page, and 65 s
with the guide open, across a minute's turn, ran no timer or frame
callback, worked out no style and laid nothing out (timers counted by
wrapping them before the page's scripts run; Chrome's performance
counters, headless: `--long`, below).

The book is in the top layer, over the page's night light, so at night it
warms its own pixels, its pictures with the rest (`#mn-warm`, the night
light's multiply of `#FFB46B` at 80% as a colour matrix), fading as the
night light does. Its backdrop is solid, the desk out of sight. Its bar has
Light, Dark and Night of its own, and its buttons are Olo's dark ones in
every look.

Change its words in both `manual.html` and `manual.md`, as `index.html` and
`index.md` are kept, and look at every page you change in a spread at the
smallest size (a 1235 × 900 window), at 1280 × 800 and 1440 × 900 and on a
phone: a page that runs over its foot is cut. Its copy was written with an
Antigravity agent (Gemini 4 Neon) from an outline of facts checked in Olo's
code, and set from that.

Its behaviour is checked in a headless Chrome, which the check starts
itself (`$CHROME`, else Playwright's `chrome-headless-shell`, else a
Chromium on the PATH), serving `website/` on a free port of 127.0.0.1:

```sh
node website/tools/check-manual.mjs          # 94 checks, about four minutes
node website/tools/check-manual.mjs --long   # and a minute at rest with it open
node website/tools/check-manual.mjs --only back-during-turn,plate-keyboard
```

It opens a guide whose answer has no book in it, or only part of one, or
one cut off at the back cover, says the wrong number of pages or none, is
past 1 MB, or is a failure the browser cached; whose stylesheet fails or
never comes; cancels one while it opens; goes Back while the pages come,
while a page is turning (its animation held) and to a chapter's address;
opens again from the close event and in the same moment as Close (open,
and opening); reloads and closes; follows deep links in and out; turns
fast both ways; Ctrl-clicks a page in the Contents (a new tab); opens a
picture whole, fitted, and moves round it at its own size from the
keyboard; swipes on a phone, with a second finger on the bar and across a
listing; chooses a look; presses the default button in the dark; goes to
and from Agent view (a row the narrower window keeps, Escape with Deskbar
folded); reads the guide's own page with and without scripts and at 531,
532 and 547 points and with a scroll bar's room taken; and fills the
screen, and closes while full screen is being granted. A name it hasn't
(`--only`) is an error. Its Chrome runs in a process group of its own, in
a profile of its own in `/var/tmp`, removed once all of it has stopped.

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
`data/icons/svg` (each with Olo's rule for its sizes in its own style
sheet, as `crates/xdg/src/svg.rs` draws them: a browser draws every layer
of a drawing at once, so `fromN` is hidden under N points and `uptoN` from
the step after N, by the width the drawing is shown at), the mark without
its contact shadow, "Olo" as a
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

`tools/deploy-website.sh` deploys it to Cloudflare Pages, and sets up
what isn't there yet. It needs Cloudflare's `cf` and `wrangler`, each
signed in (`cf auth login`, `wrangler login`): `cf` does the setting up,
and `wrangler` the upload (`cf` has no command that uploads a Pages
deployment's files).

```sh
tools/deploy-website.sh --check    # what is set up, what a deploy would do; changes nothing
tools/deploy-website.sh            # check, set up, stage, deploy (asks before each change)
tools/deploy-website.sh --preview  # to a preview URL instead of the site
tools/deploy-website.sh --yes      # don't ask
```

At its start it checks that the commands are there and that both are
signed in to the site's account: the one whose id is
`CLOUDFLARE_ACCOUNT_ID`, else the id kept in `website/.cloudflare-account`
(not in git; the first run asks before keeping it). Then, each only if
it isn't so already: the Pages project `olo-desktop` (production branch
`main`); `website/dist` staged from what is served (`index.html`,
`index.md`, `llms.txt`, `assets/`; `--build` does only this); the
deployment; and each domain (`olo-desktop.com`, `www.olo-desktop.com`)
attached to the project, with a proxied CNAME to the project's
`pages.dev` name where its zone is in the account (a record that is
there already is never changed). Run again, it changes nothing but the
deployment.

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
