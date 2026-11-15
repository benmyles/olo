# Icons: style guide and generation

Every Olo icon belongs to one set that looks as if one artist drew it in
one sitting. That is the bar. An icon that is pretty on its own but sits
differently from its neighbours is wrong.

![Reference set](img/icon-reference.png)

*The canonical reference icons (`data/icons/reference/`) on the light,
dark and night panel colours, at 64 px with the 16 px rendering beside
each.*

## Style

The look is classic BeOS (R5's Tracker and Deskbar icons), redrawn for
high-resolution displays.

| Aspect | Rule |
|---|---|
| View | Three-quarter isometric from slightly above, as if the object sits on a desk. **Never straight-on.** Badges and symbols (about, bluetooth) are thick coins or tiles standing in 3/4 view, not flat discs. |
| Outline | One crisp dark outline around the silhouette, of the same visual weight on every icon. No outline on interior detail. |
| Colour | Bold, saturated, friendly, taken from the reference set: folder tan, Be blue (application cube), BeOS tab yellow, signal green (installer arrow), alert red (home roof), and neutral greys from silver to charcoal. Don't add a new dominant hue without reason. |
| Shading | Flat-shaded faces, each with a soft gradient. One light source from the top left. The top faces are lightest and the right faces darkest. |
| Shadow | A subtle soft contact shadow beneath the object. No long drop shadows. |
| Proportion | Slightly chunky and tactile. Detail must still read at 32 px, and the silhouette must still read at 16 px. |
| Content | No text, letters or numerals, except when the glyph *is* the symbol (the "i" of About). No brand logos except protocol marks (Bluetooth). No watermarks. |
| Canvas | 1024² with a transparent background and generous padding. `mkicons.py` trims and re-pads every icon to the same 3% margin, so how much empty space the model leaves doesn't matter. |
| Themes | Icons are the same in light, dark and night themes. Check every one on all three panel colours. Dark-grey objects (trash, system, disk) need the outline and their highlights to separate them from dark panels. |

Composite icons (a document with a pencil, a folder with a magnifier) put
the primary object at the back left and the modifier in front at the lower
right, overlapping it by about a third.

Other programs (Chrome, Slack) and the services in Preferences ▸
Integrations (GitHub) get Olo's own take on their mark: the logo made an
Olo object (a chunky coin or tile standing on the desk), drawn like any
other icon, in every theme. One named as a program's desktop entry
(`google-chrome`, `slack`) replaces the icon the program ships wherever
Olo shows it (`olo_xdg::has_own_icon`, in `desktop_entry::parse`), even
one the entry names by path.

## Requirements

* **Model: `gpt-image-2.5-sunburst`, quality `high` (the maximum), 1024²,
  transparent background.** `tools/genicon.py` hard-codes these. Don't
  generate icons any other way. Don't hand-draw, use another model, or
  lower the quality to save time.
* **Always generate against the reference set.** `genicon.py` sends every
  image in `data/icons/reference/` to the edits endpoint as a style
  reference. This is what makes the set consistent. A text prompt alone
  drifts (that is how the old flat `about` and `bluetooth` badges came
  about). `--no-ref` exists only for bootstrapping a new reference set.
* **Generate candidates and pick one.** For any new or replacement icon,
  use `--variants 2` (or 3). Compare the candidates against their
  neighbours on a contact sheet and promote the best. Never ship an icon
  you haven't looked at next to the rest of the set.
* The API key is read from `OPENAI_API_KEY` or `~ben/.env`. Never print,
  log or commit it.

## Writing a subject (template)

`data/icons/manifest.txt` has one line per icon: `name|subject`. The
subject describes **only the object**. `genicon.py` supplies the style.
Template:

```
<name>|<article> <object>, <material and colour from the reference hues>, <one or two distinguishing details>[, <pose if not obvious: "seen in three-quarter view…">]
```

Good:

```
calculator|a chunky desk calculator with a small green LCD, grey body and coloured buttons
trash-full|a dark grey metal waste bin with a ribbed side, seen in three-quarter view from slightly above, overflowing with crumpled white paper
```

Avoid:

* Style words ("icon", "isometric", "glossy 3D render", "BeOS"). They are
  already in the style prompt, and repeating them skews the result.
* Several unrelated objects. One object, plus at most one modifier.
* Scenes, people or backgrounds.
* Flat symbols without a pose ("a blue circle with…"). Give the symbol a
  body: a coin, a tile, a badge standing on the desk.

## Workflow

```sh
# 1. Add or edit the line in data/icons/manifest.txt, then generate candidates
python3 tools/genicon.py NAME "subject from the manifest" --variants 2
#    (tools/genicons.sh NAME… regenerates manifest entries in one go; it
#     produces one image each and skips existing ones unless named)

# 2. Compare candidates in place, next to the reference set and neighbours
python3 tools/iconsheet.py /tmp/sheet.png NAME data/icons/src/candidates/NAME-*.png \
    folder application home archive calculator disk

# 3. Promote the winner and render the shipped sizes
mv -f data/icons/src/candidates/NAME-2.png  data/icons/src/NAME.png
mv -f data/icons/src/candidates/NAME-2.json data/icons/src/NAME.json
rm -f data/icons/src/candidates/NAME-*
python3 tools/mkicons.py NAME          # → data/icons/{16,24,…,256}/NAME.png

# 4. Review the whole set once more, then commit data/icons/<px>/ and the manifest
python3 tools/iconsheet.py /tmp/all.png
```

Review checklist (on the contact sheet, at 64 px and 16 px, on all three
backgrounds):

1. Same angle as the references: you can see the top face.
2. Same outline weight. There is no outline-less "photo" look.
3. Lit from the top left, with a contact shadow and no long shadow.
4. Similar optical size to its neighbours. If it looks big or small, the
   silhouette is wrong; fix the subject wording rather than the padding.
5. Legible at 16 px. The silhouette alone identifies it.
6. No stray text, artefacts, or halo around the edges.

Raw images (`data/icons/src/`) are gitignored. Their `.json` sidecars
record the model, quality, full prompt and references, so any icon can be
reproduced. The processed sizes and the manifest are committed.

## The reference set

`data/icons/reference/` holds 512 px copies of the icons that define the
style: folder, application, home, archive, calculator, installer, disk and
terminal. They cover a warm material, a cool primary-coloured solid, a
small scene, a cardboard texture, a device with buttons, a composite, a
metallic object and a screen. Change the reference set only on purpose,
because doing so changes every icon generated afterwards. To swap a
reference, replace the file and regenerate `docs/img/icon-reference.png`:

```sh
python3 tools/iconsheet.py docs/img/icon-reference.png \
    $(ls data/icons/reference | sed 's/\.png$//')
```

## Themes

The whole set, the `olo` mark and the wordmark come in two looks, chosen
in Preferences ▸ Themes (`Appearance::icon_theme`; new users get
Classic). Every app follows the choice at once.

| Id | Name | Medium |
|---|---|---|
| `classic` | Classic (the default) | After BeOS: this guide's glossy set, `data/icons` and `data/brand` themselves. |
| `platinum` | Platinum | After Mac OS 9: friendly black-outlined icons, lilac folders, gentle isometric devices. |

Platinum draws the same subjects in its own medium and keeps the Be
grammar: a three-quarter view from the upper left, flat things lying on
the desk, chunky shapes that read at 16 px, a soft cast shadow to the
lower right, and Be blue and BeOS tab yellow as the brand colours.
(There were ten themes until November 2026; the other eight went, and a
setting naming one draws Classic.)

A theme lives in `data/themes/<id>/`:

| Path | What |
|---|---|
| `style.txt` | Its style prompt, used in place of the house style. |
| `subjects.txt` | Optional: `name|subject` lines, a subject of its own for an icon where the manifest's names what it draws otherwise (Platinum's lilac folder). |
| `reference/` | Its reference icons, 512 px: folder, application, disk, terminal and home. |
| `src/` | Raw 1024² renders and their sidecars (gitignored). |
| `icons/<px>/<name>.png` | The shipped sizes, laid out as in `data/icons`. |
| `brand/olo-wordmark.png` | Its wordmark, one picture for light and dark (an `olo-wordmark-on-dark.png` beside it is used on dark when present). |

Rules:

* **A theme draws every icon in the manifest**, `olo` and `themes`
  included, at every size, plus its wordmark. `cargo test -p olo-xdg`
  checks this (`themes::tests::every_theme_is_complete`). At run time a
  missing icon falls back to the Classic one, but never ship a theme with
  gaps.
* **A new manifest icon is generated for every theme**, against that
  theme's references. `genicon.py --theme ID` uses the theme's
  `style.txt` and `reference/`, drops "glossy" from the subject (the
  manifest is written for Classic) and writes to `data/themes/ID/src/`.
* **Pictures that aren't a thing's icon stay out of the manifest.**
  Weather's 18 skies (`data/icons/weather.txt`, `name|subject` lines as
  in the manifest) are drawn for Classic with the same tools
  (`genicon.py NAME "subject"`, `mkicons.py NAME`) and shown in every
  theme, so the theme test doesn't ask for them: a forecast's symbols
  are a language of their own. Check them at 16 px (the places list)
  and 48 px (the forecast) next to each other: one sun, one cloud, the
  same drops, so they read as a set.
* The theme sets were first drawn one image per icon (about 500), not as
  two candidates each, and then reviewed on a contact sheet per theme.
  Redraw any icon that drifts with `--variants 2` as usual.
* The PNGs are in Git LFS (`.gitattributes`). Install and upgrade copy
  each theme's `icons/` and `brand/` to `~/.local/share/olo/themes/`.

```sh
# A new icon NAME, in every theme (one image each; skips ones already drawn)
tools/genicons.sh --theme platinum NAME
# …or candidates
python3 tools/genicon.py --theme platinum NAME "subject" --variants 2
# Review, then render the sizes
python3 tools/mkicons.py --theme platinum NAME
python3 tools/iconsheet.py /var/tmp/sheets/platinum.png --theme platinum
```

### Theme wordmarks

Each theme's wordmark is Olo's o, l, o (the o's Be blue, the l BeOS tab
yellow) made in its material. `tools/genwordmark.py` draws it with the
icons' model at quality "high", 1536×1024, with the Classic wordmark
attached for its letters and colours and the theme's reference icons for
its look; `NOTES` holds a theme's line on how it makes letters, where its
icon style alone would give the Classic gloss. Candidates
go to `data/themes/<id>/src/wordmark/`; `--promote` trims one and writes
it 768 px wide to `brand/olo-wordmark.png`. Look at each on light and dark
panels before promoting.

```sh
python3 tools/genwordmark.py platinum --variants 2
python3 tools/genwordmark.py platinum --promote data/themes/platinum/src/wordmark/olo-wordmark-3.png
```

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
| AmigaOS 3.9 | Its 30 backdrops (1024×768) | the AmigaOS 3.9 CD (ISO 9660) |
| AmigaOS 3.9 Patterns | Its Workbench patterns (JPEG and IFF ILBM tiles; Pixel64's twins of Pixel128's left out) | the same |

BeOS shipped no wallpapers worth having (two Be logos, a TV overlay, two
photos of its people) and NeXTSTEP none (its desktop was a plain grey).
Each theme names its default (`Theme::wallpaper`): Classic Blueberry
Oxygen (the nearest to Be's blue), Platinum Quantum Foam; those two have
a dark version and an ultrawide crop, as PNG; the rest are 4K JPEGs
(quality 95, no chroma subsampling) cropped to the screen as shown.
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

Then `--promote NAME` (`--jpeg` for a collection's picture, `--tile` for
a tile: `tiles/NAME.png` and a tiled thumbnail), a line in `pack.txt`
(`name|label|collection`, `|tile:N` for a tile N points wide), and the HDR
maps: `cargo run --release -p olo-images --bin wallpaper-hdr` makes
`NAME.gain.png` beside each picture in `4k/` and `ultrawide/` (PNG or
JPEG; not the tiles) that's newer than its map (`olo_images::gain`;
`--preview DIR` tints what shines). `cargo test -p olo-xdg` checks every
theme has its wallpaper, light and dark, at each shape. Candidates and
their `.json` sidecars (model, prompt, source) go to
`data/wallpapers/src/` (gitignored). The API key is read from
`OPENAI_API_KEY` or `~ben/.env`; never print, log or commit it.

## Using icons in code

`olo_ui::icons::paint(window, bounds, "name")`. Names come from the
manifest; freedesktop icon names and absolute paths also work. The kit
picks the smallest pre-scaled size that covers the rect in device pixels,
in the current theme (`olo_xdg::themes::current()`); to show another
theme, as Preferences does, use `icons::paint_in(window, bounds, theme,
name)` and `brand::wordmark_centred_in`. Use these sizes: 16 for lists, menus and
the Deskbar; 32 for Tracker mini icons and dialogs; 48 to 64 for the
Tracker icon view and desktop; 128 for About boxes. Don't tint, recolour
or scale icons up beyond 256 px.
