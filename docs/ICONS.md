# Icons: Olo's drawings

Every Olo icon belongs to one set that looks as if one artist drew it in
one sitting. That is the bar. An icon that is pretty on its own but sits
differently from its neighbours is wrong.

![The set's forms](img/icon-reference.png)

*Some of the set (the folder, the application, home, the archive, the
calculator, the installer, the disk and Terminal) on the light, dark
and night panel colours, at 64 px with the 16 px rendering beside each.*

Every icon is a drawing, an SVG written by hand (`data/icons/svg`), that
Olo renders for the exact pixels it's shown at. This guide is the
style, how a drawing is made and judged, how a theme colours the set,
and how code shows an icon.

## Style

The look is classic BeOS (R5's Tracker and Deskbar icons), redrawn for
high-resolution displays.

| Aspect | Rule |
|---|---|
| View | Isometric, from slightly above, as if the object sits on a desk: one projection for every icon ("The projection", below). **Never straight-on.** Badges and symbols (About, Bluetooth, Restart) are thick discs standing, not flat. |
| Outline | One crisp dark outline around each form's silhouette, of the same weight on every icon (the renderer sets it for the size). No outline on interior detail. |
| Colour | Bold, saturated, friendly: folder manila, Be blue (the application cube), BeOS tab yellow, signal green (the installer's arrow), alert red (home's roof), and neutral greys from silver to charcoal. Don't add a new dominant hue without reason. |
| Shading | Flat-shaded faces, each with a soft gradient. One light, from the top left: the top faces lightest, the front next, the side darkest. |
| Shadow | A soft contact shadow beneath the object, large sizes only. No long drop shadows. |
| Proportion | Slightly chunky and tactile. Detail must still read at 32 px, and the silhouette at 16 px: a drawing says what shows at which size. |
| Content | No text, letters or numerals, except when the glyph *is* the symbol (the "i" of About, StyledEdit's B). No brand logos except the marks Olo draws its own take on (below). |
| Themes | One drawing in every theme; the theme gives the colours ("A theme's colours"). Check every icon on the light, dark and night panel colours. Dark-grey objects (the trash, System, a black phone) need their outline and highlights to separate them from dark panels. |

Composite icons (a document with a tick, a folder with a magnifier) put
the primary object at the back left and the modifier in front at the
lower right, overlapping it by about a third.

Things that come as a set are drawn from one set of pieces, so they
agree: the document kinds (one page), the skies (one sun, moon, cloud,
drop, flake), the phones (one phone seen from behind, each model its
colour and its cameras), the toolbar's letters and alignments.

Other programs (Chrome, Slack, Telegram) and the services in
Preferences ▸ Integrations (GitHub) get Olo's own take on their mark:
the logo made an Olo object (a thick badge or tile standing on the
desk), drawn like any other icon, in every theme. One named as a
program's desktop entry (`google-chrome`, `slack`) replaces the icon
the program ships wherever Olo shows it (`olo_xdg::has_own_icon`, in
`desktop_entry::parse`), even one the entry names by path.

## Drawings (SVG)

An icon is a drawing: `data/icons/svg/NAME.svg` (a theme's:
`data/themes/ID/icons/svg/`), written by hand, one icon at a time, from
looking at the icon (Ben, 2026-11-17: "convert the large high quality
one to an SVG ... add SVG support to olo for all icons/graphics"; "i
want you to just manually look at each icon and then write out a picture
perfect SVG for each one"; "improve every icon to perfection as part of
this -- they must all be consistently angled, sized, styled etc. and fit
the respective theme"). Olo renders it for the size it's shown at
(`olo_xdg::svg`, resvg, in process): 32 pixels are drawn as 32 pixels,
not a 256-pixel picture shrunk, and 20 and 28 pixels (16 points at
1.25× and 1.75×) are as exact as 16 and 32. Until November 2026 the
icons were pictures from an image model, shrunk to eight sizes
(`<px>/NAME.png`): fine large, a blur small. What was tried first and
turned down, at true size: the picture shrunk better, a second picture
drawn for small sizes, flat drawings, pixel art. The pictures and their
tools are gone (they're in the repository's history; the 181 drawings
were each written from looking at its picture).

**The grid.** `viewBox="0 0 64 64"`. The object's paths keep within 3
and 61 both ways, so that with its outline it's within 2 and 62 (the
test measures it: what reaches the edge is cut off), and it fills that
square as its neighbours do. Its main
verticals and corners are on whole units, even ones where it can be: a
unit is half a pixel at 32 pixels and a pixel at 64.

**The projection**, the same for every icon:

- The front face is on the right and faces the lower right; the left
  side shows on the left; the top shows above. Never mirrored.
- Front edges (left to right along the front) rise **1 in 4**. Side
  edges (front to back) rise **1 in 2** going left. Verticals are
  vertical. Both slopes fall cleanly on pixels.
- What's on the front face is drawn in the face's own plane, upright,
  inside `<g transform="matrix(1 -.25 0 1 X Y)">` (X, Y the face's top
  left corner): a rectangle there is the face.
- A level circle (a lid, a coin lying flat) is an ellipse 0.36 as tall
  as it is wide.
- A sheet (a page, a card, a folder's flap) stands in the front plane.
  Every document kind is the same page (36 by 47, its top right corner
  folded), what tells its kind on it or standing before it.
- A flat shape with depth (an arrow, a star, a gear, a bar of a chart)
  is its front face pushed back along the side direction
  (`icontrace.py extrude`): the faces that look up lighter, those that
  look sideways darker.
- A flat thing whose face is its top (a keyboard, a touchpad, a
  calculator, a chip, a hard drive, a fingerprint reader) is seen from
  higher up, or its top would be a sliver: its face is drawn in
  `<g transform="matrix(1 -.25 .5 1 X Y)">` (its near edge rising 1 in
  4 like any front edge, its sides going back 2 up for 1 across), its
  thickness straight down from the near and left edges. What stands on
  it (the line on Activity Monitor's chip, AI's star) stands in the
  front plane.
- A thick disc standing (a badge: About, Bluetooth; a clock, a
  loudspeaker, a ring of shares) is a circle in the front plane, its
  edge showing to the left and above.
- A drum or a bin standing is upright, its ends level ellipses.
- A thing whose own shape tapers or leans (a CRT's housing, a pen, a
  key) keeps its shape, set on the same desk.
- The same thing is the same drawing wherever it appears: the CRT
  (Terminal, the desktop, Agent Screens), the page, the open folder
  (the folder, Tracker), the hard drive (Disk, Backup, Drive Setup),
  the chip, the magnifying glass, the spanner.
- Light from the top left: the top lightest, the front next, the side
  darkest; each face a soft gradient, lighter toward the top left.

**Forms and outlines.** Each solid part is a form: a path of class `o`
(its silhouette, filled with the form's darkest face, its outline
`#1b1b1f`), then its faces over it, clipped to the silhouette
(`<clipPath>` of the same path) so a face can be a plain polygon and
the silhouette's rounded corners are everyone's. `o` strokes are
painted under their fill (`paint-order:stroke`), so only the half
outside shows and the fill's edge stays where it's drawn. Forms are
drawn back to front; the nearer form's outline is the line between
them. No lines between a form's own faces: their difference in
lightness is the edge (and a thin light stroke along a lit edge, large
only).

**Drawn for its size.** The renderer injects a style sheet for the size
(`svg::sheet`); a drawing uses its classes:

| Class | What |
|---|---|
| `o` | A form's outline: a device pixel wide up to 80 pixels, then an eightieth of the size. |
| `l` | A line of the outline's weight on its own (a plain stroke: the edge of a hole seen through). |
| `i` | A line inside a form: three fifths of that. |
| `fromN` | Shown at N pixels and up: detail (`from48`: highlights, vents, rims, the contact shadow; `from96`: the finest). |
| `uptoN` | Shown at N pixels and down: what stands in small (`upto32`: the mark alone and larger, a bolder prompt). |

N is 16, 24, 32, 48, 64, 96, 128, 256 or 512. At 32 pixels and under an
icon is its forms, their faces and the one thing that says what it is,
drawn large; everything else is `from48`. The drawing's own `<style>`
gives `o` a weight for programs that don't know the classes (a browser,
GTK through Olo's icon theme), where it shows whole.

**Kept once rendered.** Rendering costs 0.1 to 0.4 ms at 32 pixels and
2 to 3 ms at 256 (measured: `svgicon --time`), so each size is rendered
once and its pixels kept in `~/.cache/olo/icons` (`svg::own`): read
back in 2 µs (7 at 256), less than decoding the PNG took (10 and 520).

**An icon set's colours.** The classic set is the drawings; another
icon set (`olo_theme::looks::ICON_SETS`: Platinum's, BeOS's) without a
drawing of its own for an icon (`data/themes/ID/icons/svg/NAME.svg`)
shows the classic one its own way, by the rules it keeps beside its
icons, `data/themes/ID/icons/recolour.txt` (`olo_xdg::svg::recolour`,
`themes::recolour`): so an icon has the same angle, size and detail in
every set, and each set its palette. Platinum's: the ink black, greys
a lilac grey, the folder lilac blue, blues toward periwinkle,
everything a little softer. BeOS's: as R5 drew its icons, each face
one flat colour from its 8-bit palette (a cube of six levels a
channel and 32 greys), a black outline, no soft shadow. Both keep the
brand's ring and other people's marks as they are.

```text
ink 0b0b16                     # the classic ink (1b1b1f) becomes this
grey 240 0.24                  # greys take this hue and saturation
turn 25-56 to 232 0.72 +0.12 in folder folder-open tracker
                               # hues 25 to 56: to hue 232, 0.72 of the saturation, 0.12 lighter, in those icons
turn 196-232 by 10 0.84 +0.07  # ... turned by 10 degrees, in every icon
flat                           # each face one colour: a gradient's stops take their mean
levels 0 51 102 152 203 255    # a colour's red, green and blue each take the nearest of these
greys 8                        # a grey is a multiple of this
no shadow                      # the soft shadow under a thing isn't drawn
keep olo slack github          # these stay as they are
```

A colour is a grey when its channels differ by less than 0.12; any
other takes the first `turn` that holds it, so the particular come
first; `flat`, `levels` and `greys` come after them (a colour is
turned, then set on the levels). Look at a set with `svgicon --theme ID
IN.svg OUT.png SIZE`. The rendering is kept per set like any other. A
set draws an icon of its own only where its rules can't say it (a
different thing, not a different colour).

**Writing one.**

The forms most drawings are made of (a box, a sheet standing, a flat
thing seen from above, a thick disc, the page, the CRT, a chip, a
drive, a magnifying glass, a padlock, the sky's pieces, a phone) are in
`tools/dev/iconforms.py`, each giving the SVG for itself: start a
drawing from them (its doc comment shows how), then it's the drawing
that's kept and edited.

```sh
T=tools/dev/icontrace.py
cargo build --release -p olo-xdg --example svgicon   # what the tools render with
$T review /var/tmp/…/r.png NAME NAME …               # each drawing at 128 and at every small size, light and dark
$T round 6,12.5,1.5 36,5,1.5 53,13.5,1.5 …           # a polygon's path, corners rounded
$T extrude -3,-1.5 5,0 13,0 13,11 …                  # a flat shape given depth: its solid, its faces
# Drawing from a picture of the thing (a photograph, a sketch, any square PNG):
$T grid PICTURE.png /var/tmp/…/g.png                 # look at it on the grid
$T sheet /var/tmp/…/s.png A.png B.png C.png          # several on the grid at once
$T outline PICTURE.png                               # its silhouette, in grid units
$T region PICTURE.png 30,20                          # one part's polygon and colour
$T pick PICTURE.png 30,20 12,40                      # colours
$T compare PICTURE.png data/icons/svg/NAME.svg /var/tmp/…/c.png
```

`review` is where a drawing is judged: at true size, on the light and
the dark panel, beside its neighbours (name a few of them too): it's
the small sizes that have to be right. `compare` shows a picture and
the drawing at 256 pixels, their difference, and both small. Then look
at it in the apps, in a test session at 1×, 1.5× and 2×, light and
dark. Then
`cargo test -p olo-xdg svg` (every drawing is on the grid, renders, and
stays on the canvas).
Improve as you draw: one projection, one outline, the same optical size
as its neighbours, nothing that only reads large left to carry the
small sizes.

## Review checklist

On `icontrace.py review`'s sheet and then in the apps, at 64 px and 16
px, on all three panel colours:

1. The one projection: the front on the right, the top showing, nothing
   mirrored; a flat thing seen from above like its kin.
2. The same outline weight as its neighbours; forms' faces told apart by
   lightness, not lines.
3. Lit from the top left, a contact shadow, no long shadow.
4. The same optical size as its neighbours, within the canvas (the test
   measures that: 2 to 62 of the grid).
5. Legible at 16 px: the silhouette alone identifies it, and what only
   reads large is `from48`.
6. The same thing drawn the same way as wherever else it appears.

## Themes

A theme (Preferences ▸ Themes; `olo_theme::looks::THEMES`: Classic,
Platinum, BeOS and the colour schemes from elsewhere) has an icon set,
and the person can give it another (Customize… ▸ Icons;
`Appearance::icons`). Every app follows at once.

| Id | Name | Its icons | Whose |
|---|---|---|---|
| `classic` | Classic (the default) | After BeOS, glossy and bright: the drawings as they're written, `data/icons/svg`. | Classic's, and every scheme's. |
| `platinum` | Platinum | After Mac OS 9: the same drawings with a black outline, lilac greys, a lilac-blue folder, periwinkle blues and softer accents. | Platinum's. |
| `beos` | BeOS | As BeOS R5 drew its icons: each face one flat colour of its 8-bit palette, black outlines, no soft shadow. | BeOS's. |

An icon set other than the classic one lives in `data/themes/<id>/`:

| Path | What |
|---|---|
| `icons/recolour.txt` | Its way with the classic set's drawings ("An icon set's colours", above). |
| `icons/svg/<name>.svg` | A drawing of its own for an icon, where a different colour doesn't say it (none yet). |

Rules:

* **A set has every icon**: the classic drawing its way, or its own.
  Nothing to generate for a new icon: draw it once, look at it in each
  set (`svgicon --theme ID`, and the apps).
* **No wordmark of its own**: the wordmark is the brand's lettering, the
  same in every theme ([BRAND.md](BRAND.md)), and so is the Olo ring
  (`keep olo` in a theme's rules).
* **Weather's 18 skies** (`weather-*.svg`) are a language of their own:
  one sun, one cloud, the same drops, so they read as a set. Check them
  at 16 px (the places list) and 48 px (the forecast) next to each
  other. A new one is made from the same pieces (`iconforms.Sky`).
* **A paired phone's own picture** is one of twenty (`phone-*.svg`: the
  ten commonest iPhones and the ten commonest Android phones of 2026,
  `phone-iphone-17-pro`, `phone-galaxy-s26-ultra`, `phone-pixel-10`,
  …), the nearest to what the phone says it is
  (`olo_protocol::phone::picture`; `phone` for one that matches none).
  Each is the phone seen from behind, where one differs from another
  (its colour, its cameras), with no maker's mark; a new model is
  `iconforms.phone(...)`. Check them at 16 px (Preferences ▸ Phone's
  list) and 48 px (its panel) in a row: the same pose, the same size,
  told apart by colour and cameras.
* Install and upgrade copy the drawings and each theme's `icons/` to
  `~/.local/share/olo/` (`upgrade.rs`, `sync_data`), and take away the
  pictures an older Olo kept there.
* **Other programs** (GTK, Qt) get Olo's icons as a freedesktop icon
  theme app_server writes (`icon_theme.rs`): each icon rendered by Olo
  at each size in the theme's colours, as PNGs. Never hand another
  program a drawing itself: its renderer doesn't know the size classes
  and would show the large detail and the small stand-ins at once.

### The wordmark

Themes had wordmarks of their own (Olo's o, l, o in each theme's
material, drawn by the model) until November 2026. The wordmark is now
the brand's, "Olo" set in Sora by `tools/mkbrand.py`, the same in every
theme: see [BRAND.md](BRAND.md). So is the Olo mark: one drawing,
`olo.svg`, which every theme's rules keep as it is.

### Theme wallpapers and the retro collection

The pack's pictures are the retro systems' own, made 4K (Ben, 2026-11-15:
"upscale all the macos 9 ones and use them for both themes"; then "the
latest amigaos, nextstep, and other classic desktops … for a complete
retro os collection"; "don't skip the tiled ones … add tiling support to
olo"; "they have visible pixellation, bluriness in places … make them
all perfect and stunning"). Each collection is a system's set, chosen
in Backgrounds by Collection:

| Collection | What | From |
|---|---|---|
| Mac OS 9 | The twenty it installed, but Mac OS Background (the Finder's face: too on the nose) | a Mac OS 9 disc image (HFS, read with `machfs`); Blue Dalmatian and Flower Power (only in 9.1's installer) from 512 Pixels' copies |
| Mac OS 9 Extras, Photos, Nostalgia | The disc's Additional Desktop Pictures (3D, Convergence, Ensemble; photographs; Nostalgia, older pictures) | the 9.2.2 disc's CD Extras |
| Mac OS 9 Patterns, Large Patterns | Its Additional Desktop Patterns (PICT resources: pixel patterns, decoded) and Large Patterns (JPEG tiles) | the 9.2.2 disc |
| Mac OS X | Each release's Aqua Blue and Aqua Graphite, redrawn each time (Cheetah's, Jaguar's, Panther's, Tiger's) | the 10.0–10.4 install discs (HFS+; the pictures in `Essentials.pkg`'s or `MediaFiles.pkg`'s `Archive.pax.gz`) |
| Mac OS X Abstract, Nature, Black & White, Patterns | What each release added: the Abstracts, Flows, Lines and Tiles; Cheetah's and Jaguar's photographs and Tiger's Nature and Plants; Tiger's Black & White; the Ripples tiles (the 128-pixel solid colours left out: Desktop Colour is that) | the same; one of each, a wide-screen original over its 5:4 twin |
| AmigaOS 3.9 | Its 30 backdrops (1024×768) | the AmigaOS 3.9 CD (ISO 9660) |
| AmigaOS 3.9 Patterns | Its Workbench patterns (JPEG and IFF ILBM tiles; Pixel64's twins of Pixel128's left out) | the same |
| Windows 3.1 | Its eleven tiles (Arcade, Argyle, Castle, Egypt, Honey, Red Brick, Rivets, Squares, Thatch, Zigzag, 256 Color; its logo left out) | Windows 3.11's floppies (FAT; MS COMPRESS's KWAJ files, expanded with libmspack) |
| Windows 95 | Clouds, and its tiles (Bubbles, Circles, Houndstooth, Pinstripe, Red Blocks, Blue Rivets, Black Thatch, Gold Weave, Straw Mat, Triangles, 3D Blocks, Forest, Sandstone) | Windows 98 SE's CD (its cabinets carry 95's set) |
| Windows 98 | Its fifteen desktop pictures but those that are its logo (Setup, More Windows, Windows 98) | the same |
| Windows 2000 | Its eleven tiles (Blue Lace 16, Coffee Bean, Feather Texture, Gone Fishing, Greenstone, Prairie Wind, Rhododendron, River Sumida, Santa Fe Stucco, Soap Bubbles, Zapotec; its NT logos left out) | Windows 2000 Professional's CD (`I386/*.BM_`, single-file cabinets) |
| OS/2 Warp 4 | Its eight pictures (Garden, Garden 2, Ocean, Ocean 2, Space, Space 2, Warp 03, Warp 04; its logos left out: the Warp desktop, WARP02, OS2LOGO, OS2WARP, FTP-PM's backdrop) | OS/2 Warp 4's CD (`OS2IMAGE/DISK_*/BUNDLE`: PACK2 archives, unpacked with deark's `os2pack2`; three are OS/2 bitmap arrays, its richest bitmap taken) |
| OS/2 Warp 4 Patterns | Its 35 textures (`\OS2\BITMAP`: Big Blue, Burlap, Gold, Green Marble, Pool, Rice Mat, Wicker, TXTR_000–011 as Texture 1–12, …) | the same |
| CDE | Its 24 backdrops (Sun's logo, the plain fills and "no backdrop" left out), in the Default palette as CDE drew them: KnitLight, WaterDrops, RicePaper and Pebbles in their workspaces' colours (sets 3, 5, 6, 7), the rest in workspace One's | CDE 2.5.2's source (`programs/backdrops`, `programs/palettes`), rendered by `tools/cdebackdrops.py` |

BeOS shipped no wallpapers worth having (two Be logos, a TV overlay, two
photos of its people) and NeXTSTEP none (its desktop was a plain grey);
IRIX drew its backgrounds as it ran. CDE's backdrops are pictures in
symbolic colours the window manager filled from the palette, so
`tools/cdebackdrops.py` does what CDE did, each step read from its
source rather than guessed: a palette colour's shadows and select
colour by Motif's CalculateColorsRGB (lib/Xm/Color.c, integer arithmetic
as C truncates it); workspace n's colour set {3, 5, 6, 7}[n mod 4] and a
backdrop's foreground the set's background, its background the set's
bottom shadow (dtwm, WmResource.c); an XBM's set bits the foreground and
the rest the background; an XPM's `background`, `foreground`,
`topShadowColor`, `bottomShadowColor` and `selectColor` the set's own
(Motif's ImageCache.c finds the set that holds the two colours it was
given) and its `iconGray`/`iconColor` symbols the file's colours, its
strings read literally as libXpm does (the skies use `\` as a pixel).
Pixel-art tiles (Mac OS 9's
8 and 16 pixel ones, Windows 3.1's and 95's) are made larger by nearest
neighbour, 4×: their pixels are the design. Dithered tiles (OS/2's, 10
to 120 colours, the dither visible) are blurred 0.9 pixels as a mosaic
first (`finishwallpaper.py tile IN OUT 0.9`): 4x-UltraSharp sharpened
the dither into a cross-hatch, and 0.9 merges it and keeps the texture.
Each theme names its default (`Theme::wallpaper`): Classic Blueberry
Oxygen (the nearest to Be's blue), Platinum Quantum Foam; those two have
a dark version and an ultrawide crop; the rest are 4K only, cropped to
the screen as shown. Every picture and tile is lossless WebP (see
"Restored, and lossless" below).
Choosing a theme in Preferences ▸ Themes switches to its picture
(`olo_xdg::themes::wallpaper`). A setting naming a picture that went
shows its theme's (`pack:platinum`, the old name) or the default's.

How a picture is made, in two steps, both kept:

1. `tools/upscalewallpaper.py SRC NAME`: the original attached to the
   sunburst model at quality "high", exactly that picture again at
   3840×2160, widened to 16:9 by continuing its background, lettering
   kept as it is (`--dark`: the same picture for dark mode). A tile
   (`--tile`): its mosaic drawn again at 4× or more, the middle tile
   cut out and its edges cross-faded with its neighbours, so it repeats
   without a seam. 3×3, or more tiles along a strip's short side until
   it's within 2:1 (the model refuses past 3:1: a 16×256 strip's 3×3 was
   192×3072), at a smaller scale where the mosaic would be over the
   model's pixel budget (about 8 megapixels: a 256-point Large Pattern's
   3×3 at 4× was refused at 3072 square; at 2784 its tile is still 928
   pixels, more than 2× needs); tiles of 32 pixels or less are too small for the
   model (it doubled a stripe pattern) and go to step 2 instead.
2. `tools/finishwallpaper.py` (its own Python with PyTorch and spandrel,
   on the NVIDIA GPU; see its header): 4x-UltraSharp, supersampled. The
   model draws the whole 4K frame at once, and copies the low-resolution
   originals' stair-stepped edges, a pixel or three tall. A render is
   brought down to 1280 wide (the steps under a pixel), made 4× by the
   model and brought back to 4K, and the model's kept only where there
   are edges (its own faint canvas texture in flat areas is not wanted:
   there the picture's gradients stay as they were); a photograph from
   1920 wide, the model's everywhere; a small tile from its 3×3 mosaic;
   8- and 16-pixel Mac pixel patterns by nearest-neighbour (their
   pixels are the design). Quantised with a fine dither, so deep
   gradients don't band.

Which route a picture takes depends on what it is:

- A wide original (16:10 or wider, 1600 pixels and more: Tiger's 2560 ×
  1600, the "wide screen" versions) is cropped to 16:9 and finished
  (step 2) from its own pixels: nothing drawn that wasn't there.
- A 4:3 or 5:4 original whose composition a crop would cut (an Aqua, an
  Abstract) is widened by step 1, then finished, then its colours set
  back to the original's (`finishwallpaper.py match ORIGINAL IN OUT`:
  each YCbCr channel's mean and spread): the model brightened them and
  spread their colours 10–26%. A dithered original's dither doubles its
  colours' spread, so where the redraw's is half the original's, only
  their means go back (`match … --keep-spread`: scaled to the dither,
  OS/2's Warp 03's white clouds went yellow).
- A regular pattern or soft field (Mac OS X's Lines and Tiles) is never
  drawn again by the model, which turned a grid of fine lines wobbly: it
  is cropped to 16:9 and finished from its native width
  (`finishwallpaper.py photo|picture IN OUT WIDTH`).

Then `restorewallpaper.py restore` (below), `--promote NAME` (`--jpeg` for a
collection's picture, 4K only; `--tile` for a tile: `tiles/NAME.webp` and
a tiled thumbnail), a line in `pack.txt`
(`name|label|collection`, `|tile:N` for a tile N points wide), and the HDR
maps: `cargo run --release -p olo-images --bin wallpaper-hdr` makes
`NAME.gain.png` beside each picture in `4k/` and `ultrawide/` (WebP, PNG
or JPEG; not the tiles) that's newer than its map (`olo_images::gain`;
`--preview DIR` tints what shines). `cargo test -p olo-xdg` checks every
theme has its wallpaper, light and dark, at each shape. Candidates and
their `.json` sidecars (model, prompt, source) go to
`data/wallpapers/src/` (gitignored). The API key is read from
`OPENAI_API_KEY` or `~ben/.env`; never print, log or commit it.

### Olo's own abstract collection

Six pictures in the "Olo" collection are plays on the brand (Ben,
2026-11-16: "new olo high-res 4k backgrounds that are abstract (no
words) with cool plays on the olo colors"): Orbit (glossy rings in
space), Halo (an eclipse's arc), Ribbons (two silk ribbons), Shapes (a
Bauhaus print of circles and rings), Glass (frosted rings refracting
blue and gold) and Dunes (layered waves under a gold horizon). Orbit,
Shapes, Glass and Dunes have dark versions; Halo and Ribbons are dark
already. Each was drawn at 4K by the model from words alone
(`genwallpaper.py olo-NAME --variants 2`: `BRAND` and `OLO_SCENES` hold
the palette and each scene; `--dark-from` the dark version), the best of
two restored (`restorewallpaper.py`, below) and promoted with `--promote`
(`--dark` for the dark one).

### Restored, and lossless

Every picture in the pack has been restored, and is kept lossless (Ben,
2026-11-16: "i can still see like color banding and stuff"; "fix and
restore any visual issues you find in all of the wallpapers with
whatever approaches work best"; "zero visual artifacts"). What was
wrong, measured by `restorewallpaper.py check` on each picture's smooth
areas (where flaws show; `flaws()` says how):

- **The image model's own flaws.** It draws 8 bits a channel with no
  dither, so a slow gradient is a staircase of flat bands (Halo's dark
  sky: 9 levels of red across 1,800 pixels), and its decoder leaves a
  faint cross-hatch and colour blotches a level or two deep, seen as
  mottling in dark gradients. Every redrawn picture had both.
- **JPEG's.** The retro collections were kept as JPEG at quality 95
  (4:4:4): its 8×8 blocks showed in their gradients (the step across a
  block's edge 1.8 to 3.2 times the step inside one), and it rang on
  the smooth ground beside sharp edges.

`tools/restorewallpaper.py` takes each out, in one pass on the GPU:

1. **Where the picture is smooth** (`smooth_mask`: luma's spread in a
   7×7 window under 1.5 levels restored, over 3.5 kept as it is,
   feathered between), and only there: a photograph's grain and a
   stone's sparkle are the picture's, not flaws, and stay to the pixel.
2. A JPEG: **FBCNN** (blind JPEG restoration, its colour model; spandrel
   loads it from `data/wallpapers/src/models/fbcnn_color.pth`, from the
   authors' v1.0 release) on the smooth ground within 12 pixels of a
   sharp edge (`edge_zone`), where JPEG rings, its result **held to
   what the file says the original was** (`project`: each 8×8 block's
   DCT coefficients within the interval the file's quantised value
   stands for, by its own tables), so it can take out ringing and never
   invent or lose what the file holds (unheld, it took a tenth of a
   granite photograph's sparkle). Only there, because that is all it
   changes: beside edges it leaves a quarter less ringing than the
   polish alone (luma's fine noise 0.30 → 0.24 levels, colour's 0.42 →
   0.29); everywhere else the two measured the same (0.005 against
   0.004). The network is 45 TFLOP for a 4K picture, 4.2 s of this
   laptop's GPU flat out (11.2 TFLOP/s at its 60 W limit: its float32
   ceiling), so it runs on the 256-pixel tiles that hold such ground (59
   of a 4K picture's 135 at the median of the pack's 206 JPEGs; none of
   a plain gradient's), not on the picture.
3. **The polish** (`polished`): He's guided filter, five passes from 2
   to 100 pixels (`POLISH`), smoothing only windows that vary less than
   the noise (an edge or a texture keeps its pixels), no pixel moved
   more than 4 levels: JPEG's blocks, the model's cross-hatch and
   blotches out, and its bands back into the ramps they stand for.
4. A **dither** (`dither`): each pixel rounded up or down by chance in
   proportion to its fraction, the same chance for its three channels,
   so a ramp between two levels is a mix of them and not a step, and a
   pixel that is a whole level (all that wasn't changed) stays exactly
   as it was.

A tile is worked on as a mosaic of itself wide enough that its middle
sees only its own repeats, and the middle taken, so it still repeats
without a seam; one of 256 colours or fewer is pixel art (its pixels
are the design) and kept exactly. The result is saved as **lossless
WebP** (`genwallpaper.save_picture`; the pixels exactly, checked by
reading it back; a quarter smaller than PNG; libwebp's method 4, the
same size as its slowest in an eighth of the time). Decoding costs more
than PNG's (app_server's `wallpaper_cost`, a 4K picture for a 3200 ×
2000 screen on one core: PNG 100 ms, WebP 181 ms, JPEG 89 ms), once, on
its wallpaper thread, when a picture is chosen.

```sh
# The GPU's Python (torch, spandrel: finishwallpaper.py's header has the venv)
/var/tmp/olo-sr/bin/python tools/restorewallpaper.py check data/wallpapers/4k/*.webp
/var/tmp/olo-sr/bin/python tools/restorewallpaper.py restore IN OUT.webp [--tile]
/var/tmp/olo-sr/bin/python tools/restorewallpaper.py pack     # every PNG or JPEG still in the pack
```

`pack` keeps the GPU busy and nothing else waiting: pictures are decoded
a few ahead on threads, each is restored without leaving the GPU (0.5 s
for one with no JPEG edges, 3.8 s at the median of the JPEGs, 7 s where
every tile has them), and the WebP encoding (3 s of a core for a 4K
picture) runs on every core behind it: the pack's 559 pictures took 870
s, 864 of them the GPU's. A new picture is restored before it is
promoted. The pack is 1.6 GB (it was 0.85 GB as PNG and JPEG): lossless
photographs are three times their JPEGs.

## Using icons in code

`olo_ui::icons::paint(window, bounds, "name")`. Names are the drawings'
(`data/icons/svg/NAME.svg`); freedesktop icon names and absolute paths
also work. The kit has the icon drawn for exactly the rect's device
pixels, in the current theme (`olo_xdg::themes::current()`); to show
another theme, as Preferences does, use `icons::paint_in(window,
bounds, theme, name)` (the wordmark, `brand::wordmark_centred`, is the
same in every theme). Use these sizes: 16 for lists, menus and the
Deskbar; 32 for Tracker mini icons and dialogs; 48 to 64 for the
Tracker icon view and desktop; 128 for About boxes. Don't tint or
recolour an icon in code: colours are the drawing's and the theme's.
Outside the kit: `olo_xdg::load_icon(name, px)` (kept once rendered),
`olo_xdg::render_own_icon(theme, name, px)` for what's written
elsewhere (a PNG: `Image::to_png`).
