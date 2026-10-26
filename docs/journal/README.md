# Screenshot journal

Tabula's visual evolution, newest first. Added with `tools/journal.py`
(images are stored with Git LFS; run `git lfs pull` after cloning).

## 2026-10-25

### Tracker Status: background file operations

*20:43* — Copy, move, duplicate, Trash and Empty Trash run on worker threads (copy_file_range in chunks). Jobs longer than 0.4 s appear here with bytes, items, current file and time left; Stop cancels and removes the partial copy.

![Tracker Status: background file operations](2026-10-25/204337-tracker-status-background-file-operations.png)

### Tracker: Find searches contents with ripgrep

*20:38* — Find is built on ripgrep's crates (ignore + grep-searcher): parallel walk, smart case, .gitignore-aware, binary files skipped. Contents matches show the line number and text; live progress, Stop/Escape cancel.

![Tracker: Find searches contents with ripgrep](2026-10-25/203834-tracker-find-searches-contents-with-ripgrep.png)

### Menus: shortcut glyphs on the baseline

*20:29* — ⌘, ⇧ and arrows come from a fallback font; each is now laid out separately so it shares the item's baseline. The Trash is titled Trash.

![Menus: shortcut glyphs on the baseline](2026-10-25/202952-menus-shortcut-glyphs-on-the-baseline.png)

### Tracker: Find

*20:29* — Find takes focus on open, closes with ⌘W, shows home-relative locations and a baseline-aligned status.

![Tracker: Find](2026-10-25/202952-tracker-find.png)

### Tracker: column view, dark

*20:20*

![Tracker: column view, dark](2026-10-25/202049-tracker-column-view-dark.png)

### Tracker: column view

*20:20* — macOS-style column view (⌘4): one column per folder level, auto-hiding scroll bars, and a preview pane with an async thumbnail and file facts.

![Tracker: column view](2026-10-25/202047-tracker-column-view.png)

### Tracker: editing a name in place

*20:14* — BeOS-style inline rename replaces the Rename dialog: the field sits on the label's own baseline and lines up with the selection highlight; the name without its extension starts selected.

![Tracker: editing a name in place](2026-10-25/201411-tracker-editing-a-name-in-place.png)

### Tracker mini icon view

*19:58*

![Tracker mini icon view](2026-10-25/195831-tracker-mini-icon-view.png)

### Tracker list view

*19:58* — Sortable ColumnListView with 16 px icons; count and free space in the bar beside the scroll bar.

![Tracker list view](2026-10-25/195830-tracker-list-view.png)

### Tracker desktop, folder window and Get Info (light)

*19:58* — Selected labels inverted as on BeOS, highlight balanced on cap height and baseline; links in italics; executable scripts get the executable icon.

![Tracker desktop, folder window and Get Info (light)](2026-10-25/195830-tracker-desktop-folder-window-and-get-info-light.png)

### Tracker and Get Info in dark mode

*19:58* — Get Info rows on an exact LABEL_ROW + TIGHT grid, window sized to its content.

![Tracker and Get Info in dark mode](2026-10-25/195829-tracker-and-get-info-in-dark-mode.png)

### First Tracker desktop

*19:58* — Volumes and Home beside the Deskbar, Trash in the corner. Labels were off-centre (centred galleys double-offset) and ~/Desktop items were missing.

![First Tracker desktop](2026-10-25/195828-first-tracker-desktop.png)

### Icon set after the style guide

*19:58* — Generated against the reference set (docs/ICONS.md): the outliers are now three-quarter view like the rest.

![Icon set after the style guide](2026-10-25/195828-icon-set-after-the-style-guide.png)

### Icon set before the style guide

*19:58* — Text-only prompts: about, bluetooth, screenshot, display and trash drift to flat or straight-on views.

![Icon set before the style guide](2026-10-25/195828-icon-set-before-the-style-guide.png)
